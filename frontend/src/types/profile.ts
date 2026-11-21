export type AvatarProcessingState =
  | 'idle'
  | 'uploading'
  | 'scanning'
  | 'transforming'
  | 'ready'
  | 'failed';

/**
 * Bounded derivatives generated from a validated, metadata-stripped upload.
 * Only these URLs are ever rendered in public UI surfaces.
 */
export interface AvatarDerivatives {
  small: string;
  medium: string;
  large: string;
}

/**
 * Tracks the lifecycle of an avatar upload/transformation.
 * `previousAvatar` stays referenced (and rendered) until `state === 'ready'`,
 * so a failed scan or transformation never blanks out the current avatar.
 */
export interface AvatarProcessingStatus {
  state: AvatarProcessingState;
  /** Stable identifier for the in-flight or completed processing job. */
  jobId?: string;
  /** The avatar currently displayed; preserved across failures. */
  previousAvatar?: string;
  /** Derivatives for the newly processed avatar; only set when ready. */
  derivatives?: AvatarDerivatives;
  /** User-visible, recoverable error message when state is 'failed'. */
  error?: string;
  /** Whether the user can retry after a failure. */
  retryable?: boolean;
}

export interface PublicProfile {
  id: string;
  username: string;
  avatar?: string;
  avatarDerivatives?: AvatarDerivatives;
  avatarProcessing?: AvatarProcessingStatus;
  bio?: string;
  socialLinks?: { twitter?: string; discord?: string; twitch?: string; github?: string };
  elo: number;
  globalRank: number;
  createdAt: string;
  isOnline: boolean;
  customization: ProfileCustomization;
  privacySettings: PrivacySettings;
}

export interface PlayerStats {
  elo: number;
  globalRank: number;
  wins: number;
  losses: number;
  winRate: number;
  currentStreak: number;
  longestStreak?: number;
  averageMatchDuration?: number;
  favoriteGameType?: string;
  totalPlayTime?: number;
  recentPerformance?: {
    trend: 'up' | 'down' | 'stable';
    change: number;
  };
}

export interface Achievement {
  id: string;
  title: string;
  description: string;
  icon: string;
  progress: number;
  total: number;
  unlocked: boolean;
  unlockedAt?: string;
  category?: 'combat' | 'social' | 'progression' | 'special';
  rarity?: 'common' | 'rare' | 'epic' | 'legendary';
  points?: number;
}

export interface FriendEntry {
  id: string;
  username: string;
  avatar?: string;
  elo: number;
  status: 'online' | 'in-game' | 'offline';
  gamesPlayed?: number;
  lastSeen?: string;
  currentActivity?: string;
  mutualFriends?: number;
}

export type ActivityEventType =
  | 'match_completed'
  | 'achievement_unlocked'
  | 'tournament_joined'
  | 'tournament_completed'
  | 'friend_added'
  | 'rank_changed'
  | 'streak_achieved';

export interface ActivityEvent {
  id: string;
  type: ActivityEventType;
  timestamp: string;
  payload: Record<string, string | number | boolean>;
}

export interface ProfileCustomization {
  banner: string;
  colorTheme: string;
}

export type PrivacySetting = 'everyone' | 'friends' | 'only_me';

export interface PublicProfileViewProps {
  profile: PublicProfile;
  stats: PlayerStats;
  achievements: Achievement[];
  friends: FriendEntry[];
  activities: ActivityEvent[];
  eloHistory: EloPoint[];
}

export interface PrivacySettings {
  stats: PrivacySetting;
  matchHistory: PrivacySetting;
  achievements: PrivacySetting;
  friends: PrivacySetting;
  activityFeed: PrivacySetting;
}

export interface MatchWithPlayers {
  id: string;
  player1Id: string;
  player2Id: string;
  player1Username: string;
  player2Username: string;
  winnerId: string;
  gameType: string;
  score: string;
  date: string;
  tournamentName?: string;
  duration?: number;
  eloChange?: number;
}

export interface MatchHistoryFilters {
  gameType?: string;
  result?: 'win' | 'loss';
  opponentSearch?: string;
  dateRange?: {
    start: string;
    end: string;
  };
}

// Enhanced types for profile editing
export interface UserProfileUpdate {
  username?: string;
  bio?: string;
  avatar?: string;
  avatarDerivatives?: AvatarDerivatives;
  avatarProcessing?: AvatarProcessingStatus;
  socialLinks?: {
    twitter?: string;
    discord?: string;
    twitch?: string;
    github?: string;
  };
  customization?: ProfileCustomization;
}

// ELO history for charts
export interface EloPoint {
  date: string;
  elo: number;
  change?: number;
}
