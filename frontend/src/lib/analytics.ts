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
const EXPERIMENT_CACHE_KEY = "arenax:analytics:experiments";

export interface ExperimentAssignment {
  experimentId: string;
  variant: string;
  assignmentVersion: string;
}

export interface AssignExperimentOptions {
  /** Endpoint that resolves the assignment server-side. */
  endpoint?: string;
  /** Optional fetch implementation for tests / non-browser environments. */
  fetcher?: typeof fetch;
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

function loadExperimentCache(): Record<string, ExperimentAssignment> {
  if (typeof window === "undefined") return {};
  try {
    const raw = sessionStorage.getItem(EXPERIMENT_CACHE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, ExperimentAssignment>;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function saveExperimentCache(cache: Record<string, ExperimentAssignment>): void {
  if (typeof window === "undefined") return;
  try {
    sessionStorage.setItem(EXPERIMENT_CACHE_KEY, JSON.stringify(cache));
  } catch {
    // ignore
  }
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
  private experiments: Record<string, ExperimentAssignment>;

  constructor() {
    this.consent = loadConsent();
    this.session = loadOrCreateSession();
    this.experiments = loadExperimentCache();
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
    sessionStorage.removeItem(EXPERIMENT_CACHE_KEY);
    this.experiments = {};
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

  // ── Experiments ───────────────────────────────────────────────────────────

  /**
   * Returns the cached assignment for the current session, if any. Assignments
   * are pinned for the lifetime of the session so a participant is never
   * silently reassigned mid-session.
   */
  getExperimentAssignment(experimentId: string): ExperimentAssignment | null {
    const cached = this.experiments[experimentId];
    return cached ? { ...cached } : null;
  }

  /**
   * Resolves a deterministic assignment from the backend and caches it for the
   * session. Requires analytics consent; without consent no assignment is
   * requested or cached. A cached assignment always wins over a fresh request.
   */
  async assignExperiment(
    experimentId: string,
    options: AssignExperimentOptions = {},
  ): Promise<ExperimentAssignment | null> {
    if (this.consent.analytics !== "granted") return null;

    const cached = this.experiments[experimentId];
    if (cached) return { ...cached };

    const endpoint = options.endpoint ?? "/api/experiments/assign";
    const fetcher = options.fetcher ?? (typeof fetch !== "undefined" ? fetch : undefined);
    if (!fetcher) return null;

    try {
      const response = await fetcher(endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "same-origin",
        body: JSON.stringify({
          experimentId,
          sessionId: this.session.sessionId,
          userId: this.session.userId ?? null,
        }),
      });
      if (!response.ok) return null;

      const data = (await response.json()) as Partial<ExperimentAssignment> | null;
      if (
        !data ||
        typeof data.variant !== "string" ||
        typeof data.assignmentVersion !== "string"
      ) {
        return null;
      }

      const assignment: ExperimentAssignment = {
        experimentId,
        variant: data.variant,
        assignmentVersion: data.assignmentVersion,
      };
      this.experiments[experimentId] = assignment;
      saveExperimentCache(this.experiments);
      return { ...assignment };
    } catch {
      return null;
    }
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

  /**
   * Logs an experiment exposure. Must be called at presentation time (when the
   * participant actually sees the assigned experience), never at assignment
   * time. Respects analytics consent.
   */
  trackExposure(experimentId: string, variant: string, assignmentVersion: string): void {
    if (this.consent.analytics !== "granted") return;
    if (!experimentId || !variant || !assignmentVersion) return;

    this.track("experiment_exposure", {
      experimentId,
      variant,
      assignmentVersion,
    });
  }
}

// Singleton
let _instance: AnalyticsService | null = null;

export function getAnalyticsService(): AnalyticsService {
  if (!_instance) _instance = new AnalyticsService();
  return _instance;
}
