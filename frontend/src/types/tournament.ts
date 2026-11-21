// Tournament-related types

/**
 * Watermark describing the realtime stream position a snapshot was taken at.
 * Clients must apply subsequent websocket events only when their sequence is
 * exactly `sequence + 1`; anything else is a gap or a stale event and requires
 * a replacement snapshot.
 */
export interface TournamentStreamWatermark {
  /** Monotonic server-side sequence number for the tournament stream. */
  sequence: number;
  /** Server timestamp (ISO 8601) at which the snapshot was produced. */
  snapshotAt: string;
  /** Optional opaque stream identifier (e.g. per-tournament channel). */
  streamId?: string;
}

/**
 * Snapshot envelope returned by tournament read endpoints. `watermark` is the
 * stream position from which subsequent events can be applied.
 */
export interface TournamentSnapshot<T> {
  data: T;
  watermark: TournamentStreamWatermark;
}

/** How an optimistic mutation should be rolled back when it fails. */
export type RollbackStrategy =
  | 'none'
  | 'restore_previous'
  | 'refetch_resource'
  | 'invalidate_and_refetch';

/** How a mutation resolves conflicts with newer authoritative server state. */
export type ConflictStrategy =
  | 'server_wins'
  | 'reject_if_stale'
  | 'client_wins'
  | 'manual_reconcile';

/**
 * Metadata every tournament mutation declares so the client can centralize
 * rollback/reconciliation and decide whether it may be queued offline.
 */
export interface OptimisticMutationMeta {
  /** Whether the UI applies the change before server confirmation. */
  optimistic: boolean;
  /** Safe rollback/refetch strategy for each affected resource. */
  rollback: RollbackStrategy;
  /** Conflict resolution when server state is newer than the local view. */
  conflict: ConflictStrategy;
  /** Resources (cache keys) affected by the mutation. */
  affects: string[];
  /** Whether the mutation may be queued for offline replay. */
  offlineQueueable: boolean;
  /** Idempotency key required for queueable mutations. */
  idempotencyKey?: string;
  /** Expiry (ISO 8601) after which a queued command must not be replayed. */
  expiresAt?: string;
}

export interface Tournament {
  id: string;
  name: string;
  description?: string;
  banner?: string;
  gameType: string;
  tournamentType: string;
  entryFee: number;
  prizePool: number;
  maxParticipants: number;
  currentParticipants: number;
  status: TournamentStatus;
  visibility: TournamentVisibility;
  startTime: string;
  endTime?: string;
  createdBy: string;
  createdAt: string;
  updatedAt: string;
}

export type TournamentStatus =
  | 'draft'
  | 'registration_open'
  | 'registration_closed'
  | 'in_progress'
  | 'completed'
  | 'cancelled';

/** Simplified status buckets for the tournaments list UI (badges, colors, filters). */
export type TournamentPageStatus = 'ongoing' | 'upcoming' | 'completed';

export const TOURNAMENT_PAGE_STATUSES: TournamentPageStatus[] = [
  'ongoing',
  'upcoming',
  'completed',
];

export const TOURNAMENT_PAGE_STATUS_COLORS: Record<
  TournamentPageStatus,
  { badgeClass: string; label: string }
> = {
  ongoing: {
    badgeClass:
      'bg-blue-100 text-blue-800 dark:bg-blue-900/30 dark:text-blue-200',
    label: 'Ongoing',
  },
  upcoming: {
    badgeClass:
      'bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-200',
    label: 'Upcoming',
  },
  completed: {
    badgeClass:
      'bg-purple-100 text-purple-800 dark:bg-purple-900/30 dark:text-purple-200',
    label: 'Completed',
  },
};

export function toTournamentPageStatus(status: TournamentStatus): TournamentPageStatus {
  if (status === 'in_progress') {
    return 'ongoing';
  }
  if (status === 'completed' || status === 'cancelled') {
    return 'completed';
  }
  return 'upcoming';
}

export function isTournamentPageStatus(value: string): value is TournamentPageStatus {
  return (TOURNAMENT_PAGE_STATUSES as readonly string[]).includes(value);
}

export type TournamentVisibility =
  | 'public'
  | 'private'
  | 'invite_only';

export type TournamentType =
  | 'single_elimination'
  | 'double_elimination'
  | 'round_robin'
  | 'swiss';

export interface CreateTournamentRequest {
  name: string;
  description?: string;
  gameType: string;
  tournamentType: TournamentType;
  entryFee: number;
  maxParticipants: number;
  visibility: TournamentVisibility;
  startTime: string;
}

export type CreateTournamentWizardStep = 1 | 2 | 3 | 4 | "preview" | "success";

export interface CreateTournamentFormData {
  name: string;
  gameType: string;
  description: string;
  tournamentType: TournamentType;
  matchFormat: string;
  rules: string;
  entryFee: number;
  prizePool: number;
  prizeDistribution: string;
  visibility: TournamentVisibility;
  maxParticipants: number;
  registrationOpenDate: string;
  registrationCloseDate: string;
  startDate: string;
  endDate: string;
}

export interface TournamentFilters {
  gameType?: string;
  status?: TournamentStatus;
  /** Filter by simplified list status (ongoing / upcoming / completed). */
  pageStatus?: TournamentPageStatus;
  visibility?: TournamentVisibility;
  tournamentType?: TournamentType;
  minEntryFee?: number;
  maxEntryFee?: number;
  minPrizePool?: number;
  maxPrizePool?: number;
  search?: string;
  sortBy?: 'date' | 'prize_pool' | 'participants';
  sortOrder?: 'asc' | 'desc';
  page?: number;
  limit?: number;
}