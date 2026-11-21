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

// ---------------------------------------------------------------------------
// Cross-client analytics event schema registry (Issue #249)
// ---------------------------------------------------------------------------

/** Producer that owns the meaning of an event. */
export type EventOwner = "frontend" | "server" | "shared";

/**
 * Sensitive attribute classes that must never appear in analytics payloads.
 * - `auth_token`: session tokens, JWTs, refresh tokens, API keys
 * - `payment_detail`: card numbers, CVV, bank account / IBAN, billing address
 * - `raw_form_content`: free-text form input, passwords, emails, chat bodies
 * - `private_key`: wallet private keys, seed phrases, signing material
 */
export type ProhibitedFieldClass =
  | "auth_token"
  | "payment_detail"
  | "raw_form_content"
  | "private_key";

/**
 * Versioned contract for a single analytics event.
 *
 * `version` follows semver-ish `MAJOR.MINOR` semantics: adding optional fields
 * is a MINOR bump; renaming/removing fields or tightening required fields is a
 * MAJOR bump and requires a migration plan (see docs/analytics/event-review.md).
 */
export interface EventSchemaDefinition {
  /** Event name this contract describes. */
  name: AnalyticsEventName;
  /** Producer that owns the event's meaning. */
  owner: EventOwner;
  /** Human-readable reason the event is collected. */
  purpose: string;
  /** Schema version, e.g. "1.0". Breaking changes bump the major. */
  version: string;
  /** Fields that must be present on every payload. */
  requiredFields: readonly string[];
  /** Fields that may be present but are not required. */
  optionalFields: readonly string[];
  /** Attribute classes that must never be sent with this event. */
  prohibitedFields: readonly ProhibitedFieldClass[];
}

const BASE_REQUIRED = ["event", "timestamp", "sessionId"] as const;
const BASE_OPTIONAL = ["userId"] as const;
const BASE_PROHIBITED: readonly ProhibitedFieldClass[] = [
  "auth_token",
  "payment_detail",
  "raw_form_content",
  "private_key",
];

function schema(
  name: AnalyticsEventName,
  owner: EventOwner,
  purpose: string,
  required: readonly string[],
  optional: readonly string[] = [],
  prohibited: readonly ProhibitedFieldClass[] = BASE_PROHIBITED,
): EventSchemaDefinition {
  return {
    name,
    owner,
    purpose,
    version: "1.0",
    requiredFields: [...BASE_REQUIRED, ...required],
    optionalFields: [...BASE_OPTIONAL, ...optional],
    prohibitedFields: prohibited,
  };
}

/**
 * Canonical registry of analytics event contracts. Every event emitted by a
 * browser or server producer must have an entry here; validation helpers in
 * `lib/analytics/schema.ts` enforce it in CI and at runtime.
 */
export const EVENT_SCHEMA_REGISTRY: Readonly<
  Record<AnalyticsEventName, EventSchemaDefinition>
> = {
  page_view: schema("page_view", "frontend", "Measure page navigation and entry points.", ["path"], ["referrer"]),
  game_start: schema("game_start", "frontend", "Measure game engagement and mode popularity.", ["gameMode"], ["tournamentId"]),
  game_end: schema("game_end", "frontend", "Measure session length and outcome distribution.", ["gameMode", "durationMs", "outcome"]),
  game_mode_selected: schema("game_mode_selected", "frontend", "Measure mode selection funnel.", ["gameMode"]),
  matchmaking_queued: schema("matchmaking_queued", "server", "Measure matchmaking demand and queue health.", ["gameMode"]),
  matchmaking_matched: schema("matchmaking_matched", "server", "Measure matchmaking success and wait times.", ["gameMode"], ["waitTimeMs"]),
  matchmaking_cancelled: schema("matchmaking_cancelled", "server", "Measure abandoned matchmaking attempts.", ["gameMode"], ["waitTimeMs"]),
  tournament_viewed: schema("tournament_viewed", "frontend", "Measure tournament discovery.", ["tournamentId"], ["entryFee"]),
  tournament_joined: schema("tournament_joined", "server", "Measure tournament participation.", ["tournamentId"], ["entryFee"]),
  tournament_left: schema("tournament_left", "server", "Measure tournament drop-off.", ["tournamentId"], ["entryFee"]),
  tournament_win_shared: schema("tournament_win_shared", "frontend", "Measure social sharing of wins.", ["tournamentId", "platform"], ["winnerId"]),
  match_score_reported: schema("match_score_reported", "server", "Measure score reporting reliability.", ["matchId", "gameMode"]),
  match_disputed: schema("match_disputed", "server", "Measure dispute frequency.", ["matchId", "gameMode"]),
  purchase_initiated: schema("purchase_initiated", "server", "Measure checkout funnel entry.", ["amount", "currency"], ["method"], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  purchase_completed: schema("purchase_completed", "server", "Measure completed purchases.", ["amount", "currency"], ["method"], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  purchase_failed: schema("purchase_failed", "server", "Measure checkout failure reasons.", ["amount", "currency"], ["method"], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  wallet_connected: schema("wallet_connected", "frontend", "Measure wallet onboarding.", [], [], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  wallet_deposited: schema("wallet_deposited", "server", "Measure deposit volume.", ["amount", "currency"], [], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  wallet_withdrawn: schema("wallet_withdrawn", "server", "Measure withdrawal volume.", ["amount", "currency"], [], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  auth_signup: schema("auth_signup", "server", "Measure account creation funnel.", ["method"], [], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  auth_login: schema("auth_login", "server", "Measure login funnel.", ["method"], [], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  auth_logout: schema("auth_logout", "frontend", "Measure session termination.", [], [], ["auth_token", "payment_detail", "raw_form_content", "private_key"]),
  profile_viewed: schema("profile_viewed", "frontend", "Measure profile discovery.", ["profileId"]),
  profile_edited: schema("profile_edited", "server", "Measure profile update activity.", ["fieldsChanged"]),
  achievement_unlocked: schema("achievement_unlocked", "server", "Measure achievement progression.", ["achievementId"]),
  ab_test_assigned: schema("ab_test_assigned", "frontend", "Measure experiment assignment.", ["experimentId", "variant"]),
  funnel_step: schema("funnel_step", "frontend", "Measure funnel progression.", ["funnelName", "stepName", "stepIndex"]),
  pwa_prompt_shown: schema("pwa_prompt_shown", "frontend", "Measure PWA install prompt exposure."),
  pwa_prompt_dismissed: schema("pwa_prompt_dismissed", "frontend", "Measure PWA install prompt dismissal."),
  pwa_install_accepted: schema("pwa_install_accepted", "frontend", "Measure PWA install acceptance."),
  pwa_install_declined: schema("pwa_install_declined", "frontend", "Measure PWA install decline."),
  pwa_installed: schema("pwa_installed", "frontend", "Measure successful PWA installs."),
};

// ---------------------------------------------------------------------------
// Consent record (Issue #250)
// ---------------------------------------------------------------------------

/**
 * Authoritative, versioned consent record. The `version` is bumped on every
 * state transition so browser, server, and queued-event processors can detect
 * stale records and reject events produced under an older decision.
 */
export interface ConsentRecord {
  /** Monotonic version, incremented on every consent transition. */
  version: number;
  /** Current consent decision. */
  status: ConsentStatus;
  /** Epoch millis of the last transition. */
  updatedAt: number;
  /** Subject the consent applies to (user id or anonymous id). */
  subjectId: string;
}
