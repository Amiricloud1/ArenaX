import { User, EloPoint } from "@/types/user";
import type { PublicProfile, PlayerStats, Achievement, FriendEntry, ActivityEvent } from '@/types/profile';

/**
 * Identity / permission scope that must accompany every user-data access.
 *
 * Cache identity is derived from this scope so a cached response can never
 * outlive an account, workspace, or authorization change. `permissions` is a
 * sorted list of granted permission strings (e.g. `profile:read`).
 */
export interface UserDataScope {
  accountId: string;
  workspaceId: string;
  permissions: readonly string[];
}

/** Resource families that user data can be grouped into for invalidation. */
export type UserResourceFamily = 'profile' | 'stats' | 'achievements' | 'friends' | 'activity' | 'elo';

const ALL_FAMILIES: readonly UserResourceFamily[] = [
  'profile',
  'stats',
  'achievements',
  'friends',
  'activity',
  'elo',
];

/** Permission required to read each resource family. */
const FAMILY_PERMISSIONS: Record<UserResourceFamily, string> = {
  profile: 'profile:read',
  stats: 'stats:read',
  achievements: 'achievements:read',
  friends: 'friends:read',
  activity: 'activity:read',
  elo: 'elo:read',
};

/**
 * Build a stable cache key for user data. The key embeds account, workspace
 * and the sorted permission set so private responses are never shared across
 * identities or privilege levels.
 */
export function getUserDataCacheKey(scope: UserDataScope, family: UserResourceFamily | 'all' = 'all'): string {
  const perms = [...scope.permissions].sort().join(',');
  return `user:${scope.accountId}:${scope.workspaceId}:${perms}:${family}`;
}

/** True when the scope grants the permission required for a resource family. */
export function canReadFamily(scope: UserDataScope, family: UserResourceFamily): boolean {
  return scope.permissions.includes(FAMILY_PERMISSIONS[family]);
}

/** Filter a list of families down to those the scope is authorized to view. */
export function authorizedFamilies(
  scope: UserDataScope,
  families: readonly UserResourceFamily[] = ALL_FAMILIES,
): UserResourceFamily[] {
  return families.filter((family) => canReadFamily(scope, family));
}

interface ScopedCacheEntry<T> {
  key: string;
  value: T;
}

/**
 * In-memory, scope-aware cache for user data. Entries are keyed by the full
 * identity scope; `invalidate` drops entries for a scope (logout / account
 * switch / privilege change) and `invalidateAll` clears everything.
 */
export class ScopedUserCache {
  private entries = new Map<string, ScopedCacheEntry<unknown>>();

  get<T>(scope: UserDataScope, family: UserResourceFamily | 'all'): T | undefined {
    const key = getUserDataCacheKey(scope, family);
    return this.entries.get(key)?.value as T | undefined;
  }

  set<T>(scope: UserDataScope, family: UserResourceFamily | 'all', value: T): void {
    const key = getUserDataCacheKey(scope, family);
    this.entries.set(key, { key, value });
  }

  /** Drop every entry belonging to the given account/workspace. */
  invalidate(scope: Pick<UserDataScope, 'accountId' | 'workspaceId'>): void {
    const prefix = `user:${scope.accountId}:${scope.workspaceId}:`;
    for (const key of [...this.entries.keys()]) {
      if (key.startsWith(prefix)) this.entries.delete(key);
    }
  }

  /** Drop every entry (logout of all identities). */
  invalidateAll(): void {
    this.entries.clear();
  }

  get size(): number {
    return this.entries.size;
  }
}

/**
 * Cross-tab invalidation protocol.
 *
 * Messages are versioned and bounded, and carry only resource-family names —
 * never private payloads. Receivers refetch only families they are authorized
 * to view. Malformed or unknown-version messages are ignored safely.
 */
export const CROSS_TAB_INVALIDATION_CHANNEL = 'arenax:invalidate';
export const CROSS_TAB_PROTOCOL_VERSION = 1;
const MAX_FAMILIES_PER_MESSAGE = ALL_FAMILIES.length;

export interface CrossTabInvalidationMessage {
  version: number;
  type: 'invalidate';
  accountId: string;
  workspaceId: string;
  families: UserResourceFamily[];
}

function isResourceFamily(value: unknown): value is UserResourceFamily {
  return typeof value === 'string' && (ALL_FAMILIES as readonly string[]).includes(value);
}

/** Validate an untrusted message payload; returns null when it must be ignored. */
export function parseCrossTabInvalidation(raw: unknown): CrossTabInvalidationMessage | null {
  if (!raw || typeof raw !== 'object') return null;
  const msg = raw as Record<string, unknown>;
  if (msg.version !== CROSS_TAB_PROTOCOL_VERSION) return null;
  if (msg.type !== 'invalidate') return null;
  if (typeof msg.accountId !== 'string' || msg.accountId.length === 0) return null;
  if (typeof msg.workspaceId !== 'string' || msg.workspaceId.length === 0) return null;
  if (!Array.isArray(msg.families)) return null;
  if (msg.families.length === 0 || msg.families.length > MAX_FAMILIES_PER_MESSAGE) return null;
  const families = msg.families.filter(isResourceFamily);
  if (families.length !== msg.families.length) return null;
  return {
    version: CROSS_TAB_PROTOCOL_VERSION,
    type: 'invalidate',
    accountId: msg.accountId,
    workspaceId: msg.workspaceId,
    families: [...new Set(families)],
  };
}

/** Build a protocol message for the given scope and affected families. */
export function buildCrossTabInvalidation(
  scope: Pick<UserDataScope, 'accountId' | 'workspaceId'>,
  families: readonly UserResourceFamily[],
): CrossTabInvalidationMessage {
  return {
    version: CROSS_TAB_PROTOCOL_VERSION,
    type: 'invalidate',
    accountId: scope.accountId,
    workspaceId: scope.workspaceId,
    families: [...new Set(families)].slice(0, MAX_FAMILIES_PER_MESSAGE),
  };
}

export interface CrossTabInvalidationOptions {
  /** Broadcast primitive; defaults to BroadcastChannel when available. */
  channelFactory?: (name: string) => CrossTabChannelLike | null;
  /** Called with the families the receiver is authorized to refetch. */
  onInvalidate: (families: UserResourceFamily[]) => void;
}

interface CrossTabChannelLike {
  postMessage(message: unknown): void;
  close(): void;
  onmessage: ((event: { data: unknown }) => void) | null;
}

/**
 * Subscribe to cross-tab invalidation messages for a scope. Returns a
 * disposer. When no broadcast primitive exists, this is a safe no-op.
 */
export function subscribeCrossTabInvalidation(
  scope: UserDataScope,
  options: CrossTabInvalidationOptions,
): () => void {
  const factory =
    options.channelFactory ??
    ((name: string): CrossTabChannelLike | null => {
      if (typeof BroadcastChannel === 'undefined') return null;
      return new BroadcastChannel(name) as unknown as CrossTabChannelLike;
    });

  const channel = factory(CROSS_TAB_INVALIDATION_CHANNEL);
  if (!channel) return () => {};

  channel.onmessage = (event) => {
    const msg = parseCrossTabInvalidation(event.data);
    if (!msg) return;
    // Only react to mutations for the identity this tab is currently viewing.
    if (msg.accountId !== scope.accountId || msg.workspaceId !== scope.workspaceId) return;
    const allowed = authorizedFamilies(scope, msg.families);
    if (allowed.length > 0) options.onInvalidate(allowed);
  };

  return () => {
    channel.onmessage = null;
    channel.close();
  };
}

/** Broadcast an invalidation for the given families (no private payloads). */
export function broadcastCrossTabInvalidation(
  scope: Pick<UserDataScope, 'accountId' | 'workspaceId'>,
  families: readonly UserResourceFamily[],
  channelFactory?: (name: string) => CrossTabChannelLike | null,
): void {
  const factory =
    channelFactory ??
    ((name: string): CrossTabChannelLike | null => {
      if (typeof BroadcastChannel === 'undefined') return null;
      return new BroadcastChannel(name) as unknown as CrossTabChannelLike;
    });
  const channel = factory(CROSS_TAB_INVALIDATION_CHANNEL);
  if (!channel) return;
  try {
    channel.postMessage(buildCrossTabInvalidation(scope, families));
  } finally {
    channel.close();
  }
}

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

/**
 * Look up a profile by id, scoped by identity and permissions.
 *
 * Returns null when the scope lacks `profile:read`, so private data is never
 * served outside an authorized identity. The result is cached under the full
 * scope key and dropped on logout / account switch / privilege change.
 */
export function getProfileById(
  id: string,
  scope: UserDataScope,
  cache?: ScopedUserCache,
): (typeof MOCK_PROFILES)[string] | null {
  if (!canReadFamily(scope, 'profile')) return null;

  const cached = cache?.get<(typeof MOCK_PROFILES)[string]>(scope, 'profile');
  if (cached) return cached;

  const result = MOCK_PROFILES[id] ?? null;
  if (result && cache) cache.set(scope, 'profile', result);
  return result;
}
