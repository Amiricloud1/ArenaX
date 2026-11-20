// Analytics types and event schema for ArenaX

export type ConsentStatus = "granted" | "denied" | "pending";

export interface ConsentState {
  analytics: ConsentStatus;
  updatedAt: number | null;
}

// Strongly-typed event names
export type AnalyticsEventName =
  | "page_view"
  | "game_start"
  | "game_end"
  | "game_mode_selected"
  | "matchmaking_queued"
  | "matchmaking_matched"
  | "matchmaking_cancelled"
  | "tournament_viewed"
  | "tournament_joined"
  | "tournament_left"
  | "tournament_win_shared"
  | "match_score_reported"
  | "match_disputed"
  | "purchase_initiated"
  | "purchase_completed"
  | "purchase_failed"
  | "wallet_connected"
  | "wallet_deposited"
  | "wallet_withdrawn"
  | "auth_signup"
  | "auth_login"
  | "auth_logout"
  | "profile_viewed"
  | "profile_edited"
  | "achievement_unlocked"
  | "ab_test_assigned"
  | "funnel_step"
  // PWA install funnel (Issue #858)
  | "pwa_prompt_shown"
  | "pwa_prompt_dismissed"
  | "pwa_install_accepted"
  | "pwa_install_declined"
  | "pwa_installed";

export interface SessionProperties {
  sessionId: string;
  userId?: string;
  deviceType: string;
  screenWidth: number;
  screenHeight: number;
  sessionStart: number;
}

export interface BaseEventPayload {
  event: AnalyticsEventName;
  timestamp: number;
  sessionId: string;
  userId?: string;
  /**
   * Stable, producer-assigned event identity (UUID v4). Used by the ingestion
   * boundary for durable deduplication: retries carrying the same eventId
   * converge to a single accepted event.
   */
  eventId: string;
  /** Schema version of the payload, e.g. "1.0". Enables schema-version rollout. */
  schemaVersion: string;
  /** Identifier of the producer instance that emitted the event. */
  producerId: string;
  /**
   * Pseudonymous subject identifier. This is the only identity reference
   * permitted in event payloads; raw auth identifiers must never appear here.
   */
  subjectId?: string;
}

// ---------------------------------------------------------------------------
// Identity stitching (Issue #252)
// ---------------------------------------------------------------------------

/** Purpose limitation for identity links. */
export type IdentityLinkPurpose =
  | "funnel_attribution"
  | "retention_cohort"
  | "experiment_assignment";

/** Consent scopes that gate identity stitching. */
export type ConsentScope =
  | "analytics"
  | "identity_stitching"
  | "marketing_attribution";

/**
 * A link between an anonymous subject and an authenticated subject.
 * Stored separately from event payloads; never contains raw auth identifiers.
 */
export interface IdentityLink {
  /** Pseudonymous anonymous subject identifier. */
  anonymousSubjectId: string;
  /** Pseudonymous authenticated subject identifier. */
  authenticatedSubjectId: string;
  purpose: IdentityLinkPurpose;
  consentScope: ConsentScope;
  createdAt: number;
  /** Set when the link has been reversed (logout, consent withdrawal). */
  unlinkedAt?: number | null;
  /** Deterministic merge marker for account merges. */
  mergedInto?: string | null;
}

// ---------------------------------------------------------------------------
// Ingestion outcomes (Issue #251)
// ---------------------------------------------------------------------------

export type IngestionRejectionReason =
  | "schema_invalid"
  | "unknown_event"
  | "duplicate"
  | "unauthorized"
  | "consent_denied"
  | "payload_too_large"
  | "internal_error";

export interface IngestionAccepted {
  status: "accepted";
  eventId: string;
  /** True when the event was a duplicate that converged to the accepted record. */
  deduplicated: boolean;
}

export interface IngestionRejected {
  status: "rejected";
  eventId: string;
  reason: IngestionRejectionReason;
  /** Human-readable diagnostic detail for the rejection path. */
  detail?: string;
}

export type IngestionOutcome = IngestionAccepted | IngestionRejected;

// ---------------------------------------------------------------------------
// Metric catalog & budgets (Issues #253, #254)
// ---------------------------------------------------------------------------

export type MetricPrivacyClass = "public" | "internal" | "restricted" | "sensitive";

export type MetricDeprecationState = "active" | "deprecated" | "retired";

export interface BudgetMetadata {
  /** Target p95 latency in milliseconds for the dashboard query. */
  latencyTargetMs: number;
  /** Maximum acceptable data staleness in seconds. */
  freshnessTargetSeconds: number;
  /** Bounded time range in days for the query window. */
  maxRangeDays: number;
  /** Maximum rows returned per page. */
  maxPageSize: number;
}

export interface MetricCatalogEntry {
  /** Unique metric name; duplicates are rejected by CI. */
  name: string;
  description: string;
  owner: string;
  /** Query definition (SQL or equivalent) backing the metric. */
  queryDefinition: string;
  /** Source events and/or tables feeding the metric. */
  sourceEvents: string[];
  sourceTables: string[];
  /** Freshness target in seconds. */
  freshnessSeconds: number;
  privacyClass: MetricPrivacyClass;
  deprecationState: MetricDeprecationState;
  budget: BudgetMetadata;
  /** Dashboards that reference this catalogued definition. */
  dashboards: string[];
}

// Per-event payload shapes
export interface PageViewPayload extends BaseEventPayload {
  event: "page_view";
  path: string;
  referrer?: string;
}

export interface GameModeSelectedPayload extends BaseEventPayload {
  event: "game_mode_selected";
  gameMode: string;
}

export interface GameStartPayload extends BaseEventPayload {
  event: "game_start";
  gameMode: string;
  tournamentId?: string;
}

export interface GameEndPayload extends BaseEventPayload {
  event: "game_end";
  gameMode: string;
  durationMs: number;
  outcome: "win" | "loss" | "draw";
}

export interface MatchmakingPayload extends BaseEventPayload {
  event: "matchmaking_queued" | "matchmaking_matched" | "matchmaking_cancelled";
  gameMode: string;
  waitTimeMs?: number;
}

export interface TournamentPayload extends BaseEventPayload {
  event: "tournament_viewed" | "tournament_joined" | "tournament_left";
  tournamentId: string;
  entryFee?: number;
}

export interface TournamentWinSharedPayload extends BaseEventPayload {
  event: "tournament_win_shared";
  tournamentId: string;
  /** "twitter" | "discord" | "link" | "native" */
  platform: string;
  winnerId?: string;
}

export interface PurchasePayload extends BaseEventPayload {
  event: "purchase_initiated" | "purchase_completed" | "purchase_failed";
  amount: number;
  currency: string;
  method?: string;
}

export interface ABTestPayload extends BaseEventPayload {
  event: "ab_test_assigned";
  experimentId: string;
  variant: string;
}

export interface FunnelStepPayload extends BaseEventPayload {
  event: "funnel_step";
  funnelName: string;
  stepName: string;
  stepIndex: number;
}

export interface GenericPayload extends BaseEventPayload {
  event: AnalyticsEventName;
  [key: string]: unknown;
}

export type AnalyticsPayload =
  | PageViewPayload
  | GameModeSelectedPayload
  | GameStartPayload
  | GameEndPayload
  | MatchmakingPayload
  | TournamentPayload
  | TournamentWinSharedPayload
  | PurchasePayload
  | ABTestPayload
  | FunnelStepPayload
  | GenericPayload;

// Adapter interface for third-party integrations
export interface AnalyticsAdapter {
  name: string;
  track(payload: AnalyticsPayload): void;
  identify?(userId: string, traits?: Record<string, unknown>): void;
  reset?(): void;
}

// A/B test types
export type ABVariant = "control" | "variant";

export interface ABExperiment {
  id: string;
  /** 0–1 fraction of users assigned to variant */
  splitRatio: number;
}

export interface ABAssignment {
  experimentId: string;
  variant: ABVariant;
  assignedAt: number;
}

export const ALLOWED_EVENT_NAMES: readonly AnalyticsEventName[] = [
  "page_view",
  "game_start",
  "game_end",
  "game_mode_selected",
  "matchmaking_queued",
  "matchmaking_matched",
  "matchmaking_cancelled",
  "tournament_viewed",
  "tournament_joined",
  "tournament_left",
  "tournament_win_shared",
  "match_score_reported",
  "match_disputed",
  "purchase_initiated",
  "purchase_completed",
  "purchase_failed",
  "wallet_connected",
  "wallet_deposited",
  "wallet_withdrawn",
  "auth_signup",
  "auth_login",
  "auth_logout",
  "profile_viewed",
  "profile_edited",
  "achievement_unlocked",
  "ab_test_assigned",
  "funnel_step",
] as const;
