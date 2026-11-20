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

  reset(): void {
    this.scrubSession();
    this.adapters.forEach((a) => a.reset?.());
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
