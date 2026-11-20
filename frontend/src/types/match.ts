import type { BracketMatch, ScoreReport } from "./bracket";

/**
 * Locale-aware formatting hints attached to match data (#245).
 *
 * The server remains authoritative for the raw numeric values; these fields
 * only describe *how* the client should present them so that money,
 * percentages, durations and dates follow the active locale's rules.
 */
export interface MatchLocaleFormatting {
  /** BCP 47 locale tag the values should be formatted for (e.g. "ar-EG"). */
  locale: string;
  /** IANA time zone used for date/time formatting. */
  timeZone?: string;
  /** ISO 4217 currency code for prize/payout amounts. */
  currency?: string;
  /** Whether the locale renders right-to-left. */
  direction?: "ltr" | "rtl";
}

/** A duration expressed in seconds, formatted per the active locale. */
export interface LocalizedDuration {
  /** Raw duration in seconds (authoritative numeric value). */
  seconds: number;
  /** Locale-formatted display string (e.g. "1:05", "١:٠٥"). */
  formatted: string;
}

/** A score pair formatted per the active locale, preserving semantic order. */
export interface LocalizedScore {
  player1: number;
  player2: number;
  /** Locale-formatted display string for each side. */
  formattedPlayer1: string;
  formattedPlayer2: string;
}

// Match-related types
export interface Match {
  id: string;
  tournamentId?: string;
  player1Id: string;
  player2Id: string;
  gameType: string;
  status: MatchStatus;
  winnerId?: string;
  scorePlayer1?: number;
  scorePlayer2?: number;
  startedAt?: string;
  completedAt?: string;
  createdAt: string;
  /**
   * Monotonic live-stream sequence number for this match (#33). Present when
   * the server has published at least one realtime event; the client adopts it
   * as the WebSocket baseline so the snapshot and the stream converge.
   */
  version?: number;
}

export type MatchStatus =
  | 'pending'
  | 'in_progress'
  | 'completed'
  | 'disputed'
  | 'cancelled';

export interface MatchWithPlayers extends Match {
  player1Username: string;
  player2Username: string;
  tournamentName?: string;
}

export interface MatchResult {
  matchId: string;
  winnerId: string;
  scorePlayer1: number;
  scorePlayer2: number;
}

export interface MatchFilters {
  tournamentId?: string;
  playerId?: string;
  userId?: string;
  status?: MatchStatus;
  gameType?: string;
  mine?: boolean;
  page?: number;
  limit?: number;
}

export interface ReportScoreRequest {
  score: number;
  opponentScore: number;
  proofUrl?: string;
  telemetryData?: Record<string, unknown>;
}

// Enhanced types for detailed match view
export interface MatchRound {
  roundNumber: number;
  scorePlayer1: number;
  scorePlayer2: number;
  winner?: 'player1' | 'player2' | 'draw';
  duration?: number; // in seconds
  /** Locale-formatted duration for display (#245). */
  localizedDuration?: LocalizedDuration;
  /** Locale-formatted score pair for display (#245). */
  localizedScore?: LocalizedScore;
  keyEvents?: string[];
}

export interface PlayerStats {
  playerId: string;
  username: string;
  kills?: number;
  deaths?: number;
  assists?: number;
  accuracy?: number;
  headshotRate?: number;
  economy?: number;
  utilityDamage?: number;
  firstBloods?: number;
  clutches?: number;
  plants?: number;
  defuses?: number;
}

export interface MatchDetail extends MatchWithPlayers {
  rounds?: MatchRound[];
  player1Stats?: PlayerStats;
  player2Stats?: PlayerStats;
  scoreProgression?: Array<{ round: number; scorePlayer1: number; scorePlayer2: number }>;
  rules?: string;
  format?: string;
  prizeDistribution?: {
    winner: number;
    loser: number;
    currency?: string;
  };
  tournamentBracketId?: string;
  replayUrl?: string;
  canDispute?: boolean;
  disputeDeadline?: string;
  /** Locale formatting hints for this match's money, dates and durations (#245). */
  localeFormatting?: MatchLocaleFormatting;
  /** Locale-formatted dispute deadline for display (#245). */
  localizedDisputeDeadline?: string;
}

export interface MatchHubEvent {
  id: string;
  type: "status" | "score" | "alert" | "report";
  message: string;
  createdAt: string;
}

export interface MatchHubPlayerSnapshot {
  id: string;
  username: string;
  avatar?: string;
  elo: number;
  region: string;
  seed: number;
  record: string;
  stats: Array<{ label: string; value: string }>;
}

export interface MatchHubDetails {
  id: string;
  tournamentId: string;
  tournamentName: string;
  gameType: string;
  bracketFormat: "single_elimination" | "double_elimination";
  roundLabel: string;
  arenaLabel: string;
  status: BracketMatch["status"];
  bestOf: number;
  scheduledTime: string;
  startedAt?: string;
  streamTitle?: string;
  prizePool: number;
  player1: MatchHubPlayerSnapshot;
  player2: MatchHubPlayerSnapshot;
  scorePlayer1: number;
  scorePlayer2: number;
  winnerId?: string;
  notes: string;
  reports: ScoreReport[];
  feed: MatchHubEvent[];
  canDisputeUntil: string;
  /** Spectator mode configuration */
  spectatorLimit?: number;
  replayUrl?: string;
  /** Live-stream sequence baseline adopted from the authoritative snapshot (#33) */
  version?: number;
  /** Locale formatting hints for this match's money, dates and durations (#245). */
  localeFormatting?: MatchLocaleFormatting;
  /** Locale-formatted score pair for display (#245). */
  localizedScore?: LocalizedScore;
  /** Locale-formatted scheduled time for display (#245). */
  localizedScheduledTime?: string;
  /** Locale-formatted prize pool for display (#245). */
  localizedPrizePool?: string;
  /** Locale-formatted dispute deadline for display (#245). */
  localizedCanDisputeUntil?: string;
}

// ─── Spectator Mode Types ──────────────────────────────────────────────────────

/** Which player's perspective the spectator is viewing */
export type SpectatorPerspective = "player1" | "player2" | "overview";

/** A single spectator chat message */
export interface SpectatorChatMessage {
  id: string;
  senderId: string;
  senderName: string;
  content: string;
  createdAt: number;
  /** True while the message is optimistically rendered before server confirmation */
  optimistic?: boolean;
}

/** Live spectator update received over WebSocket */
export interface SpectatorUpdate {
  matchId: string;
  spectatorCount: number;
  scorePlayer1?: number;
  scorePlayer2?: number;
  status?: BracketMatch["status"];
  winnerId?: string;
  /** Narrative event text (e.g. "ProGamer99 takes the lead!") */
  eventMessage?: string;
  timestamp: number;
}

/** State of the spectator WebSocket connection */
export type SpectatorConnectionStatus =
  | "idle"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "disconnected"
  | "error";