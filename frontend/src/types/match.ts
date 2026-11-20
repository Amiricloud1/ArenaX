import type { BracketMatch, ScoreReport } from "./bracket";

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
  /**
   * Authoritative stream sequence watermark (#239). Every snapshot response
   * carries the sequence of the last event already reflected in it; events with
   * `seq <= streamSeq` are stale and must be rejected, and events with
   * `seq > streamSeq + 1` indicate a gap that requires a replacement snapshot.
   */
  streamSeq?: number;
  /** Alias for `streamSeq` used by the snapshot/event merge contract (#239). */
  watermark?: number;
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
  /** Authoritative stream sequence watermark for this snapshot (#239) */
  streamSeq?: number;
  /** Alias for `streamSeq` used by the snapshot/event merge contract (#239) */
  watermark?: number;
}

// ─── Realtime Snapshot / Event Merge Contract (#239) ───────────────────────────

/**
 * Envelope for a single realtime event delivered over the WebSocket. `seq` is
 * the server-assigned monotonic sequence number for the match stream.
 */
export interface RealtimeEventEnvelope<T = unknown> {
  matchId: string;
  /** Monotonic per-match sequence number assigned by the server */
  seq: number;
  /** Server timestamp (ISO-8601) at which the event was produced */
  emittedAt: string;
  /** Event kind, e.g. "score", "status", "chat", "spectator" */
  type: string;
  payload: T;
}

/** Why the client decided to request a replacement snapshot */
export type RealtimeResyncReason =
  | "gap" // an event arrived with seq > watermark + 1
  | "stale" // an event arrived with seq <= watermark
  | "reconnect" // socket reconnected; baseline may be outdated
  | "retention_expired" // server no longer retains events after our watermark
  | "manual"; // explicit user/operator request

/** Request sent to the server to obtain a fresh authoritative snapshot */
export interface RealtimeSnapshotRequest {
  matchId: string;
  /** Watermark the client currently holds; server may answer with a delta or full snapshot */
  sinceSeq?: number;
  reason: RealtimeResyncReason;
  /** Client-generated id so duplicate requests can be coalesced */
  requestId: string;
}

/** Result of applying an event to the local state */
export type RealtimeApplyResult =
  | { status: "applied"; seq: number }
  | { status: "duplicate"; seq: number }
  | { status: "stale"; seq: number; watermark: number }
  | { status: "gap"; expectedSeq: number; receivedSeq: number }
  | { status: "buffered"; seq: number };

/**
 * Client-side buffer/merge state for a match stream. Events that arrive ahead
 * of the expected sequence are buffered until the gap is filled by a
 * replacement snapshot; stale events are dropped without touching state.
 */
export interface RealtimeMergeState {
  matchId: string;
  /** Highest sequence number successfully applied to local state */
  watermark: number;
  /** Events received out of order, keyed by seq, awaiting the gap to close */
  buffer: Map<number, RealtimeEventEnvelope>;
  /** True while a replacement snapshot request is in flight */
  resyncing: boolean;
  /** Last resync reason, for diagnostics and accessible error messaging */
  lastResyncReason?: RealtimeResyncReason;
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