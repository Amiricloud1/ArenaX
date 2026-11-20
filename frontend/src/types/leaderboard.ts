/** Compact player row used on the legacy /leaderboard page mock table. */
export interface LeaderboardPlayer {
  rank: number;
  userId: string;
  username: string;
  points: number;
  wins: number;
  winRate: number;
  game: string;
}

export interface LeaderboardEntry {
    id: string
    userId: string
    username: string
    avatarUrl?: string
    ranking: number
    eloRating: number
    matchesPlayed: number
    wins: number
    losses: number
    winRate: number
    period: string
    updatedAt: string
}

/**
 * Snapshot metadata attached to every leaderboard response.
 *
 * A snapshot is an immutable view of the rankings at a point in time. Clients
 * must pass `snapshotId` back on subsequent page requests so pagination stays
 * pinned to the same view and users never move between pages mid-scroll.
 */
export interface LeaderboardSnapshotMeta {
    /** Opaque identifier for the snapshot; echo back to pin pagination. */
    snapshotId: string
    /** Monotonically increasing version of the snapshot for this leaderboard. */
    snapshotVersion: number
    /** ISO timestamp of when the snapshot was created (data as-of time). */
    snapshotCreatedAt: string
    /** True when the snapshot is older than its freshness bound. */
    stale: boolean
    /** ISO timestamp after which the snapshot is considered stale. */
    staleAfter: string
}

export interface LeaderboardResponse extends LeaderboardSnapshotMeta {
    entries: LeaderboardEntry[]
    totalCount: number
    period: string
    category: string
}

export interface PlayerRankResponse {
    userId: string
    username: string
    avatarUrl?: string
    currentRank: number
    eloRating: number
    matchesPlayed: number
    wins: number
    losses: number
    winRate: number
    rankChange?: number | null
    updatedAt: string
}

export interface RankHistoryEntry {
    rank: number
    eloRating: number
    period: string
    timestamp: string
}

export interface RankHistory {
    userId: string
    username: string
    history: RankHistoryEntry[]
}

export interface SeasonalLeaderboard extends LeaderboardSnapshotMeta {
    seasonId: string
    seasonName: string
    startDate: string
    endDate: string
    entries: LeaderboardEntry[]
    totalParticipants: number
}

export interface LeaderboardStats extends LeaderboardSnapshotMeta {
    totalPlayers: number
    averageElo: number
    medianElo: number
    topPlayerElo: number
    lastUpdated: string
}

export type LeaderboardCategory = 'global' | 'tournaments' | 'casual' | 'ranked'
