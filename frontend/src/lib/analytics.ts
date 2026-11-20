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
const SUBJECT_KEY = "arenax:analytics:subject";
const OUTBOX_KEY = "arenax:analytics:outbox";
const SCHEMA_VERSION = 1;
const MAX_OUTBOX = 500;
const BATCH_SIZE = 20;

export interface QueuedEvent {
  eventId: string;
  schemaVersion: number;
  event: AnalyticsEventName;
  timestamp: number;
  sessionId: string;
  subjectId: string;
  props: Record<string, unknown>;
}

interface IdentityLink {
  subjectId: string;
  accountRef: string;
  purpose: "product-analytics";
  consentAt: number;
}

function generateId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`;
}

function generateUuid(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  // RFC4122 v4 fallback
  const bytes = new Uint8Array(16);
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") {
    crypto.getRandomValues(bytes);
  } else {
    for (let i = 0; i < 16; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function loadOrCreateSubjectId(): string {
  if (typeof window === "undefined") return generateUuid();
  try {
    const existing = localStorage.getItem(SUBJECT_KEY);
    if (existing) return existing;
    const created = generateUuid();
    localStorage.setItem(SUBJECT_KEY, created);
    return created;
  } catch {
    return generateUuid();
  }
}

function loadOutbox(): QueuedEvent[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = localStorage.getItem(OUTBOX_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as QueuedEvent[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function saveOutbox(events: QueuedEvent[]): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(OUTBOX_KEY, JSON.stringify(events.slice(-MAX_OUTBOX)));
  } catch {
    // ignore quota / privacy-mode failures
  }
}

function clearOutbox(): void {
  if (typeof window === "undefined") return;
  try {
    localStorage.removeItem(OUTBOX_KEY);
  } catch {
    // ignore
  }
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
  private subjectId: string;
  private outbox: QueuedEvent[];
  private flushing = false;

  constructor() {
    this.consent = loadConsent();
    this.session = loadOrCreateSession();
    this.subjectId = loadOrCreateSubjectId();
    this.outbox = loadOutbox();
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
      this.unlinkIdentity();
      this.outbox = [];
      clearOutbox();
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
    this.linkIdentity(userId);
    this.adapters.forEach((a) => a.identify?.(userId, traits));
  }

  reset(): void {
    this.unlinkIdentity();
    this.scrubSession();
    this.adapters.forEach((a) => a.reset?.());
  }

  // ── Identity stitching ────────────────────────────────────────────────────

  getSubjectId(): string {
    return this.subjectId;
  }

  private linkIdentity(accountRef: string): void {
    if (this.consent.analytics !== "granted") return;
    const link: IdentityLink = {
      subjectId: this.subjectId,
      accountRef,
      purpose: "product-analytics",
      consentAt: this.consent.updatedAt ?? Date.now(),
    };
    this.postIdentity("/api/analytics/identity/link", link);
  }

  private unlinkIdentity(): void {
    this.postIdentity("/api/analytics/identity/unlink", {
      subjectId: this.subjectId,
      purpose: "product-analytics",
    });
  }

  private postIdentity(path: string, body: Record<string, unknown>): void {
    if (typeof fetch === "undefined") return;
    try {
      void fetch(path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        keepalive: true,
      }).catch(() => {
        // Identity links are best-effort; server reconciles on next flush.
      });
    } catch {
      // ignore
    }
  }

  // ── Tracking ──────────────────────────────────────────────────────────────

  track(event: AnalyticsEventName, props?: Record<string, unknown>): void {
    if (this.consent.analytics !== "granted") return;

    const queued: QueuedEvent = {
      eventId: generateUuid(),
      schemaVersion: SCHEMA_VERSION,
      event,
      timestamp: Date.now(),
      sessionId: this.session.sessionId,
      subjectId: this.subjectId,
      props: { ...(props ?? {}) },
    };

    this.outbox.push(queued);
    if (this.outbox.length > MAX_OUTBOX) {
      this.outbox = this.outbox.slice(-MAX_OUTBOX);
    }
    saveOutbox(this.outbox);

    // Adapters receive the payload without raw auth identifiers.
    const payload: AnalyticsPayload = {
      event,
      timestamp: queued.timestamp,
      sessionId: queued.sessionId,
      ...(props ?? {}),
    } as AnalyticsPayload;
    this.adapters.forEach((a) => a.track(payload));

    void this.flush();
  }

  // ── Durable delivery ──────────────────────────────────────────────────────

  async flush(): Promise<void> {
    if (this.flushing) return;
    if (this.consent.analytics !== "granted") return;
    if (typeof fetch === "undefined") return;
    if (this.outbox.length === 0) return;

    this.flushing = true;
    try {
      while (this.outbox.length > 0) {
        const batch = this.outbox.slice(0, BATCH_SIZE);
        const accepted = await this.sendBatch(batch);
        if (!accepted) break;
        const acceptedIds = new Set(accepted);
        this.outbox = this.outbox.filter((e) => !acceptedIds.has(e.eventId));
        saveOutbox(this.outbox);
      }
    } finally {
      this.flushing = false;
    }
  }

  private async sendBatch(batch: QueuedEvent[]): Promise<string[] | null> {
    try {
      const res = await fetch("/api/analytics/events", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ schemaVersion: SCHEMA_VERSION, events: batch }),
      });
      if (!res.ok) return null;
      const data = (await res.json().catch(() => ({}))) as {
        accepted?: string[];
      };
      // Server is authoritative: only drop events it explicitly accepted.
      return Array.isArray(data.accepted)
        ? data.accepted
        : batch.map((e) => e.eventId);
    } catch {
      return null;
    }
  }
}

// Singleton
let _instance: AnalyticsService | null = null;

export function getAnalyticsService(): AnalyticsService {
  if (!_instance) _instance = new AnalyticsService();
  return _instance;
}
