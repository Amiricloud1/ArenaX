"use client";

import type {
  AnalyticsAdapter,
  AnalyticsEventName,
  AnalyticsPayload,
  ConsentState,
  SessionProperties,
} from "@/types/analytics";
import {
  getAnalyticsConsentCookie,
  setAnalyticsConsentCookie,
} from "@/lib/consentCookie";
import {
  CONSENT_VERSION,
  loadConsentRecord,
  saveConsentRecord,
  type ConsentRecord,
} from "@/lib/consentRecord";
import {
  validateEvent,
  type EventValidationResult,
} from "@/lib/analyticsSchema";

const SESSION_KEY = "arenax:analytics:session";

function generateId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`;
}

function loadConsent(): ConsentState {
  const record = loadConsentRecord();
  if (record) {
    return { analytics: record.analytics, updatedAt: record.updatedAt };
  }
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
  private queue: AnalyticsPayload[] = [];

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

  /**
   * Returns the authoritative, versioned consent record. The record is the
   * source of truth shared with the server and queued-event processing.
   */
  getConsentRecord(): ConsentRecord {
    const stored = loadConsentRecord();
    if (stored) return stored;
    return {
      analytics: this.consent.analytics,
      version: CONSENT_VERSION,
      updatedAt: this.consent.updatedAt,
    };
  }

  setConsent(analytics: "granted" | "denied"): void {
    this.consent = { analytics, updatedAt: Date.now() };
    saveConsent(this.consent);
    saveConsentRecord({
      analytics,
      version: CONSENT_VERSION,
      updatedAt: this.consent.updatedAt,
    });

    if (analytics === "denied") {
      this.withdrawConsent();
    }
  }

  /**
   * Withdraws analytics consent: halts future non-essential events, clears any
   * queued events, scrubs identifiers, and notifies adapters to reset.
   */
  withdrawConsent(): void {
    const updatedAt = Date.now();
    this.consent = { analytics: "denied", updatedAt };
    saveConsent(this.consent);
    saveConsentRecord({
      analytics: "denied",
      version: CONSENT_VERSION,
      updatedAt,
    });

    this.queue = [];
    this.scrubSession();
    this.adapters.forEach((a) => a.reset?.());
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

  /**
   * Re-reads the authoritative consent record at dispatch time so that
   * withdrawals made in another tab or by the server take effect immediately.
   */
  private isConsentGranted(): boolean {
    const record = loadConsentRecord();
    if (record) {
      this.consent = { analytics: record.analytics, updatedAt: record.updatedAt };
    } else {
      this.consent = {
        analytics: getAnalyticsConsentCookie(),
        updatedAt: this.consent.updatedAt,
      };
    }
    return this.consent.analytics === "granted";
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

  /**
   * Validates an event against the schema registry. Returns the validation
   * result without dispatching; useful for producers and tests.
   */
  validate(event: AnalyticsEventName, props?: Record<string, unknown>): EventValidationResult {
    return validateEvent(event, props);
  }

  track(event: AnalyticsEventName, props?: Record<string, unknown>): void {
    // Re-check authoritative consent at dispatch time.
    if (!this.isConsentGranted()) return;

    const validation = validateEvent(event, props);
    if (!validation.valid) {
      // Drop invalid events rather than leaking unvalidated/sensitive data.
      if (typeof console !== "undefined") {
        console.warn(
          `[analytics] dropped invalid event "${event}": ${validation.errors.join("; ")}`,
        );
      }
      return;
    }

    const payload: AnalyticsPayload = {
      event,
      timestamp: Date.now(),
      sessionId: this.session.sessionId,
      userId: this.session.userId,
      ...validation.sanitized,
    } as AnalyticsPayload;

    this.queue.push(payload);
    this.flush();
  }

  private flush(): void {
    // Re-check consent before delivering queued events; a withdrawal between
    // enqueue and delivery must suppress delivery.
    if (!this.isConsentGranted()) {
      this.queue = [];
      return;
    }
    const pending = this.queue;
    this.queue = [];
    pending.forEach((payload) => {
      this.adapters.forEach((a) => a.track(payload));
    });
  }
}

// Singleton
let _instance: AnalyticsService | null = null;

export function getAnalyticsService(): AnalyticsService {
  if (!_instance) _instance = new AnalyticsService();
  return _instance;
}
