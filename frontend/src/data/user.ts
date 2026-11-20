import { User, EloPoint } from "@/types/user";
import type { PublicProfile, PlayerStats, Achievement, FriendEntry, ActivityEvent } from '@/types/profile';
import { offlineQueue } from '@/lib/offlineQueue';
import { mutationRegistry } from '@/lib/mutationRegistry';
import { consumeReturnDestination } from '@/lib/returnDestination';

export const mockEloHistory: EloPoint[] = [
  { date: "2026-01-01", elo: 1000 },
  { date: "2026-01-08", elo: 1050 },
  { date: "2026-01-15", elo: 1030 },
  { date: "2026-01-22", elo: 1100 },
  { date: "2026-01-29", elo: 1150 },
  { date: "2026-02-05", elo: 1120 },
  { date: "2026-02-12", elo: 1200 },
  { date: "2026-02-19", elo: 1250 },
];

export const currentUser: User = {
  id: "user-123",
  username: "ProGamer99",
  email: "pro@arenax.com",
  isVerified: true,
  avatar: "https://api.dicebear.com/7.x/avataaars/svg?seed=ProGamer99",
  bio: "Competitive CS2 and Valorant player. Always looking for the next challenge. Founder of Team Zenith.",
  socialLinks: {
    twitter: "https://twitter.com/progamer99",
    discord: "ProGamer#0001",
    twitch: "https://twitch.tv/progamer99",
  },
  elo: 1250,
  createdAt: "2026-01-01T10:00:00Z",
};

export const MOCK_PROFILES: Record<string, {
  profile: PublicProfile;
  stats: PlayerStats;
  achievements: Achievement[];
  friends: FriendEntry[];
  activities: ActivityEvent[];
  eloHistory: EloPoint[];
}> = {
  'user-123': {
    profile: {
      id: 'user-123',
      username: 'ProGamer99',
      avatar: 'https://api.dicebear.com/7.x/avataaars/svg?seed=ProGamer99',
      bio: 'Competitive player.',
      elo: 1250,
      globalRank: 42,
      createdAt: '2026-01-01T10:00:00Z',
      isOnline: true,
      customization: { banner: 'default', colorTheme: 'blue' },
      privacySettings: {
        stats: 'everyone',
        matchHistory: 'everyone',
        achievements: 'everyone',
        friends: 'everyone',
        activityFeed: 'everyone',
      },
    },
    stats: {
      elo: 1250,
      globalRank: 42,
      wins: 67,
      losses: 33,
      winRate: 67.0,
      currentStreak: 3,
    },
    achievements: [],
    friends: [],
    activities: [],
    eloHistory: [
      { date: '2026-01-01', elo: 1000 },
      { date: '2026-01-08', elo: 1250 },
    ],
  },
};

export function getProfileById(id: string) {
  return MOCK_PROFILES[id] ?? null;
}

/**
 * Purge all offline commands scoped to the given user. Must be called on
 * logout and on account switch so queued mutations never leak across
 * identities or replay against a different server account.
 */
export function purgeUserOfflineState(userId: string | null | undefined): void {
  offlineQueue.purgeScope(userId ?? null);
  mutationRegistry.clearScope(userId ?? null);
}

/**
 * Restore the validated post-auth destination (if any) exactly once.
 * Returns `null` when no safe destination was recorded.
 */
export function restoreAuthReturnDestination(): string | null {
  return consumeReturnDestination();
}

/**
 * Route a user-scoped mutation through the centralized optimistic
 * reconciliation path. Unsafe operations are rejected from offline mode by
 * the queue itself; here we only declare the resource scope and rollback.
 */
export async function updateUserProfile(
  userId: string,
  patch: Partial<Pick<User, 'username' | 'bio' | 'avatar' | 'socialLinks'>>,
): Promise<User> {
  const result = await mutationRegistry.mutate<User>({
    resource: `user:${userId}`,
    optimistic: true,
    idempotencyKey: `user-profile:${userId}:${JSON.stringify(patch)}`,
    apply: (current) => (current ? { ...current, ...patch } : current),
    rollback: 'refetch',
    request: async () => {
      // Server remains authoritative; this is the canonical boundary.
      const base = MOCK_PROFILES[userId]?.profile;
      if (!base) throw new Error('Profile not found');
      return { ...currentUser, ...patch } as User;
    },
  });
  return result;
}
