"use client";

import type {
  AnalyticsAdapter,
  AnalyticsEventName,
  AnalyticsPayload,
  ConsentState,
  SessionProperties,
} from "@/types/analytics";
import { getAnalyticsConsentCookie, setAnalyticsConsentCookie } from "@/lib/consentCookie";

const SESSION_KEY = "arenax:analytics:session";

/**
 * Resource families whose caches are scoped by identity/permissions.
 * Only these identifiers are ever broadcast across tabs — never payloads.
 */
export type ResourceFamily =
  | "wallet"
  | "profile"
  | "membership"
  | "notifications"
  | "analytics";

const RESOURCE_FAMILIES: readonly ResourceFamily[] = [
  "wallet",
  "profile",
  "membership",
  "notifications",
  "analytics",
];

/** Minimum stable security scope for a cache key. */
export interface CacheScope {
  accountId: string | null;
  workspaceId: string | null;
  permissions: readonly string[];
}

/**
 * Build a stable, security-scoped query key. Authenticated keys always include
 * the account/workspace scope and a sorted permission fingerprint so a cached
 * response can never be served across identities or privilege changes.
 */
export function buildScopedQueryKey(
  family: ResourceFamily,
  scope: CacheScope,
  ...parts: readonly (string | number | boolean | null)[]
): readonly unknown[] {
  const permissions = [...scope.permissions].sort().join(",");
  return [
    family,
    scope.accountId ?? "anon",
    scope.workspaceId ?? "none",
    permissions,
    ...parts,
  ] as const;
}

/** True when the scope represents an authenticated, permission-bearing user. */
export function isAuthenticatedScope(scope: CacheScope): boolean {
  return Boolean(scope.accountId);
}

/** True when the scope is authorized to view a given resource family. */
export function canViewFamily(scope: CacheScope, family: ResourceFamily): boolean {
  if (!scope.accountId) return false;
  if (family === "analytics") return true;
  return scope.permissions.includes(family) || scope.permissions.includes("*");
}

// ── Cross-tab invalidation protocol ─────────────────────────────────────────

const CROSS_TAB_CHANNEL = "arenax:invalidation";
const CROSS_TAB_VERSION = 1;
const MAX_MESSAGE_BYTES = 512;
const MAX_FAMILIES_PER_MESSAGE = RESOURCE_FAMILIES.length;

interface InvalidationMessage {
  v: number;
  type: "invalidate";
  families: ResourceFamily[];
  ts: number;
}

function isResourceFamily(value: unknown): value is ResourceFamily {
  return typeof value === "string" && (RESOURCE_FAMILIES as readonly string[]).includes(value);
}

/** Parse an untrusted cross-tab message; returns null when malformed. */
export function parseInvalidationMessage(raw: unknown): InvalidationMessage | null {
  if (typeof raw !== "object" || raw === null) return null;
  const candidate = raw as Record<string, unknown>;
  if (candidate.v !== CROSS_TAB_VERSION) return null;
  if (candidate.type !== "invalidate") return null;
  if (typeof candidate.ts !== "number" || !Number.isFinite(candidate.ts)) return null;
  if (!Array.isArray(candidate.families)) return null;
  if (candidate.families.length === 0 || candidate.families.length > MAX_FAMILIES_PER_MESSAGE) {
    return null;
  }
  const families: ResourceFamily[] = [];
  for (const family of candidate.families) {
    if (!isResourceFamily(family)) return null;
    if (!families.includes(family)) families.push(family);
  }
  return { v: CROSS_TAB_VERSION, type: "invalidate", families, ts: candidate.ts };
}

type InvalidationHandler = (families: readonly ResourceFamily[]) => void;

/**
 * Versioned, bounded cross-tab invalidation bus. Uses BroadcastChannel when
 * available and falls back to a storage event. Messages carry only resource
 * family identifiers; receivers filter by their own authorization scope.
 */
export class CrossTabInvalidation {
  private channel: BroadcastChannel | null = null;
  private handlers = new Set<InvalidationHandler>();
  private scopeProvider: () => CacheScope;
  private storageKey: string;
  private onStorage: ((event: StorageEvent) => void) | null = null;
  private disposed = false;

  constructor(scopeProvider: () => CacheScope, namespace = "default") {
    this.scopeProvider = scopeProvider;
    this.storageKey = `${CROSS_TAB_CHANNEL}:${namespace}`;
    if (typeof window === "undefined") return;
    try {
      if (typeof BroadcastChannel !== "undefined") {
        this.channel = new BroadcastChannel(CROSS_TAB_CHANNEL);
        this.channel.onmessage = (event: MessageEvent) => this.receive(event.data);
        return;
      }
    } catch {
      this.channel = null;
    }
    // Fallback: storage events are delivered to other tabs but not the sender.
    this.onStorage = (event: StorageEvent) => {
      if (event.key !== this.storageKey || !event.newValue) return;
      try {
        this.receive(JSON.parse(event.newValue));
      } catch {
        // ignore malformed
      }
    };
    window.addEventListener("storage", this.onStorage);
  }

  subscribe(handler: InvalidationHandler): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  /** Broadcast invalidation for the given families (no private payloads). */
  publish(families: readonly ResourceFamily[]): void {
    if (this.disposed) return;
    const unique = families.filter((f, i) => families.indexOf(f) === i).slice(0, MAX_FAMILIES_PER_MESSAGE);
    if (unique.length === 0) return;
    const message: InvalidationMessage = {
      v: CROSS_TAB_VERSION,
      type: "invalidate",
      families: unique,
      ts: Date.now(),
    };
    const serialized = JSON.stringify(message);
    if (serialized.length > MAX_MESSAGE_BYTES) return;
    if (this.channel) {
      try {
        this.channel.postMessage(message);
        return;
      } catch {
        // fall through to storage
      }
    }
    if (typeof window === "undefined") return;
    try {
      window.localStorage.setItem(this.storageKey, serialized);
      window.localStorage.removeItem(this.storageKey);
    } catch {
      // storage unavailable; nothing else to do
    }
  }

  private receive(raw: unknown): void {
    if (this.disposed) return;
    const message = parseInvalidationMessage(raw);
    if (!message) return;
    const scope = this.scopeProvider();
    const authorized = message.families.filter((family) => canViewFamily(scope, family));
    if (authorized.length === 0) return;
    this.handlers.forEach((handler) => {
      try {
        handler(authorized);
      } catch {
        // isolate handler failures
      }
    });
  }

  dispose(): void {
    this.disposed = true;
    this.handlers.clear();
    if (this.channel) {
      try {
        this.channel.close();
      } catch {
        // ignore
      }
      this.channel = null;
    }
    if (this.onStorage && typeof window !== "undefined") {
      window.removeEventListener("storage", this.onStorage);
      this.onStorage = null;
    }
  }
}

function generateId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`;
}

function loadConsent(): ConsentState {
  return { analytics: getAnalyticsConsentCookie(), updatedAt: null };
}

function saveConsent(state: ConsentState): void {
  if (state.analytics === "pending") return;
  setAnalyticsConsentCookie(state.analytics);
}

function loadOrCreateSession(userId?: string): SessionProperties {
  if (typeof window === "undefined") {
    return {
      sessionId: generateId(),
      userId,
      deviceType: "unknown",
      screenWidth: 0,
      screenHeight: 0,
      sessionStart: Date.now(),
    };
  }
  try {
    const raw = sessionStorage.getItem(SESSION_KEY);
    if (raw) {
      const existing = JSON.parse(raw) as SessionProperties;
      if (userId && !existing.userId) {
        existing.userId = userId;
        sessionStorage.setItem(SESSION_KEY, JSON.stringify(existing));
      }
      return existing;
    }
  } catch {
    // ignore
  }
  const session: SessionProperties = {
    sessionId: generateId(),
    userId,
    deviceType: getDeviceType(),
    screenWidth: window.screen.width,
    screenHeight: window.screen.height,
    sessionStart: Date.now(),
  };
  sessionStorage.setItem(SESSION_KEY, JSON.stringify(session));
  return session;
}

function getDeviceType(): string {
  if (typeof navigator === "undefined") return "unknown";
  if (/Mobi|Android/i.test(navigator.userAgent)) return "mobile";
  if (/Tablet|iPad/i.test(navigator.userAgent)) return "tablet";
  return "desktop";
}

export class AnalyticsService {
  private adapters: AnalyticsAdapter[] = [];
  private consent: ConsentState;
  private session: SessionProperties;
  private scope: CacheScope = { accountId: null, workspaceId: null, permissions: [] };
  private invalidation: CrossTabInvalidation | null = null;
  private invalidationListeners = new Set<(families: readonly ResourceFamily[]) => void>();

  constructor() {
    this.consent = loadConsent();
    this.session = loadOrCreateSession();
  }

  // ── Adapter management ────────────────────────────────────────────────────

  registerAdapter(adapter: AnalyticsAdapter): void {
    if (!this.adapters.find((a) => a.name === adapter.name)) {
      this.adapters.push(adapter);
    }
  }

  // ── Consent ───────────────────────────────────────────────────────────────

  getConsent(): ConsentState {
    return { ...this.consent };
  }

  setConsent(analytics: "granted" | "denied"): void {
    this.consent = { analytics, updatedAt: Date.now() };
    saveConsent(this.consent);

    if (analytics === "denied") {
      this.scrubSession();
    }
  }

  private scrubSession(): void {
    if (typeof window === "undefined") return;
    // Remove ephemeral identifiers on opt-out
    sessionStorage.removeItem(SESSION_KEY);
    this.session = {
      sessionId: "anonymous",
      deviceType: "unknown",
      screenWidth: 0,
      screenHeight: 0,
      sessionStart: 0,
    };
  }

  // ── Session ───────────────────────────────────────────────────────────────

  getSession(): SessionProperties {
    return { ...this.session };
  }

  identify(userId: string, traits?: Record<string, unknown>): void {
    if (this.consent.analytics !== "granted") return;
    this.session.userId = userId;
    if (typeof window !== "undefined") {
      try {
        sessionStorage.setItem(SESSION_KEY, JSON.stringify(this.session));
      } catch {
        // ignore
      }
    }
    this.adapters.forEach((a) => a.identify?.(userId, traits));
  }

  // ── Cache scope & cross-tab invalidation ──────────────────────────────────

  getScope(): CacheScope {
    return { ...this.scope, permissions: [...this.scope.permissions] };
  }

  /** Scoped query key for a resource family under the current identity. */
  queryKey(
    family: ResourceFamily,
    ...parts: readonly (string | number | boolean | null)[]
  ): readonly unknown[] {
    return buildScopedQueryKey(family, this.scope, ...parts);
  }

  /**
   * Update the security scope. Any change to account, workspace, or
   * permissions invalidates all affected families and notifies other tabs.
   */
  setScope(next: CacheScope): void {
    const previous = this.scope;
    this.scope = {
      accountId: next.accountId,
      workspaceId: next.workspaceId,
      permissions: [...next.permissions],
    };
    const changed =
      previous.accountId !== this.scope.accountId ||
      previous.workspaceId !== this.scope.workspaceId ||
      [...previous.permissions].sort().join(",") !== [...this.scope.permissions].sort().join(",");
    if (!changed) return;
    const families = RESOURCE_FAMILIES.filter((family) => canViewFamily(this.scope, family));
    this.notifyInvalidation(families);
    this.invalidation?.publish(families);
  }

  /** Subscribe to cross-tab invalidations authorized for this tab's scope. */
  onInvalidate(handler: (families: readonly ResourceFamily[]) => void): () => void {
    this.invalidationListeners.add(handler);
    this.ensureInvalidationBus();
    return () => this.invalidationListeners.delete(handler);
  }

  /** Broadcast invalidation for families affected by a local mutation. */
  invalidate(families: readonly ResourceFamily[]): void {
    const authorized = families.filter((family) => canViewFamily(this.scope, family));
    if (authorized.length === 0) return;
    this.notifyInvalidation(authorized);
    this.invalidation?.publish(authorized);
  }

  private notifyInvalidation(families: readonly ResourceFamily[]): void {
    if (families.length === 0) return;
    this.invalidationListeners.forEach((handler) => {
      try {
        handler(families);
      } catch {
        // isolate handler failures
      }
    });
  }

  private ensureInvalidationBus(): void {
    if (this.invalidation || typeof window === "undefined") return;
    this.invalidation = new CrossTabInvalidation(() => this.getScope());
  }

  reset(): void {
    this.scrubSession();
    this.adapters.forEach((a) => a.reset?.());
    // Logout: drop scope and clear all caches in this and other tabs.
    const families = [...RESOURCE_FAMILIES];
    this.scope = { accountId: null, workspaceId: null, permissions: [] };
    this.notifyInvalidation(families);
    this.invalidation?.publish(families);
  }

  // ── Tracking ──────────────────────────────────────────────────────────────

  track(event: AnalyticsEventName, props?: Record<string, unknown>): void {
    if (this.consent.analytics !== "granted") return;

    const payload: AnalyticsPayload = {
      event,
      timestamp: Date.now(),
      sessionId: this.session.sessionId,
      userId: this.session.userId,
      ...(props ?? {}),
    } as AnalyticsPayload;

    this.adapters.forEach((a) => a.track(payload));
  }

  // ── Operational observability ─────────────────────────────────────────────
  // These helpers funnel client-side consistency flows (realtime resync,
  // offline queue, optimistic reconciliation, auth redirects) through the
  // same consent gate and adapter fan-out as regular tracking. They never
  // carry secrets (tokens, raw URLs, payload bodies) — only coarse metadata.

  /** A realtime snapshot/event gap was detected and a resync was requested. */
  trackRealtimeResync(props: {
    reason: "gap" | "stale_event" | "reconnect" | "retention_expired";
    streamId?: string;
    expectedSeq?: number;
    receivedSeq?: number;
    watermark?: number;
  }): void {
    this.track("realtime_resync", { ...props });
  }

  /** An offline command could not be replayed cleanly. */
  trackOfflineQueueConflict(props: {
    commandType: string;
    conflictStrategy: string;
    outcome: "conflict" | "expired" | "rejected" | "duplicate";
    resource?: string;
  }): void {
    this.track("offline_queue_conflict", { ...props });
  }

  /** An optimistic mutation was rolled back or reconciled with the server. */
  trackOptimisticRollback(props: {
    mutation: string;
    reason: "timeout" | "validation_error" | "conflict" | "reordered" | "superseded";
    resource?: string;
    hadIdempotencyKey?: boolean;
  }): void {
    this.track("optimistic_rollback", { ...props });
  }

  /** A validated post-auth return destination was restored (or rejected). */
  trackAuthRedirectRestore(props: {
    flow: "login" | "register" | "oauth_callback";
    outcome: "restored" | "rejected" | "expired" | "unavailable";
    locale?: string;
  }): void {
    this.track("auth_redirect_restore", { ...props });
  }
}

// Singleton
let _instance: AnalyticsService | null = null;

export function getAnalyticsService(): AnalyticsService {
  if (!_instance) _instance = new AnalyticsService();
  return _instance;
}
