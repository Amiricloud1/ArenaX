// User-related types

/**
 * Stable, non-secret identifier for the account/workspace a cached response
 * belongs to. Used as the minimum scope component in authenticated query keys
 * so a cached response can never outlive an account or workspace switch.
 */
export interface AccountScope {
  /** Stable account identifier (never a token or other secret). */
  accountId: string;
  /** Stable workspace identifier, when the account belongs to one. */
  workspaceId?: string;
}

/**
 * Permission identifiers a user may hold. Kept as a string union with an
 * open-ended fallback so new server-side permissions do not break clients.
 */
export type Permission =
  | "user:read"
  | "user:write"
  | "profile:read"
  | "profile:write"
  | "wallet:read"
  | "wallet:write"
  | "membership:read"
  | "membership:write"
  | "admin:read"
  | "admin:write"
  | "moderator:read"
  | "moderator:write"
  | (string & {});

/**
 * Resource families that can be invalidated across tabs. These identifiers
 * describe *what* changed, never the private payload itself.
 */
export type ResourceFamily =
  | "user"
  | "profile"
  | "wallet"
  | "membership"
  | "notifications"
  | "analytics";

export const RESOURCE_FAMILIES: readonly ResourceFamily[] = [
  "user",
  "profile",
  "wallet",
  "membership",
  "notifications",
  "analytics",
] as const;

export function isResourceFamily(value: unknown): value is ResourceFamily {
  return (
    typeof value === "string" &&
    (RESOURCE_FAMILIES as readonly string[]).includes(value)
  );
}

/**
 * Security context attached to authenticated cache entries. The query-key
 * factory derives keys from this so identity and permissions are explicit.
 */
export interface CacheIdentity {
  scope: AccountScope;
  permissions: readonly Permission[];
}

export interface User {
  id: string;
  username: string;
  email: string;
  isVerified: boolean;
  avatar?: string;
  bio?: string;
  socialLinks?: {
    twitter?: string;
    discord?: string;
    twitch?: string;
    github?: string;
  };
  elo: number;
  createdAt: string;
  role?: "user" | "admin" | "moderator";
  /**
   * Stable account/workspace scope for cache identity. Optional for backward
   * compatibility; consumers must treat a missing scope as unauthenticated.
   */
  accountScope?: AccountScope;
  /** Permissions granted to this user, used to scope cache identity. */
  permissions?: readonly Permission[];
}

export interface UserProfileUpdate {
  username?: string;
  bio?: string;
  avatar?: string;
  socialLinks?: {
    twitter?: string;
    discord?: string;
    twitch?: string;
    github?: string;
  };
}

export interface EloPoint {
  date: string;
  elo: number;
}

export interface AuthUser extends User {
  /**
   * @deprecated Tokens are stored in httpOnly cookies. This field is always
   * an empty string and exists only for backward compatibility with existing
   * type consumers.
   */
  token: string;
  /**
   * @deprecated See `token` above.
   */
  refreshToken: string;
}

export interface LoginRequest {
  email: string;
  password: string;
}

export interface RegisterRequest {
  username: string;
  email: string;
  password: string;
  confirmPassword: string;
}

export interface AuthResponse {
  token: string;
  refreshToken: string;
  user: User;
}

/**
 * Derive the cache identity from a user, or `null` when the user has no
 * stable scope (i.e. is unauthenticated). Centralizing this keeps query-key
 * construction and cross-tab invalidation consistent.
 */
export function getCacheIdentity(user: User | null | undefined): CacheIdentity | null {
  if (!user || !user.accountScope || !user.accountScope.accountId) {
    return null;
  }
  return {
    scope: {
      accountId: user.accountScope.accountId,
      workspaceId: user.accountScope.workspaceId,
    },
    permissions: user.permissions ?? [],
  };
}

/**
 * Whether the identity is authorized to view a given resource family. Used by
 * the cross-tab invalidation receiver to refetch only authorized resources.
 */
export function canViewFamily(
  identity: CacheIdentity | null,
  family: ResourceFamily,
): boolean {
  if (!identity) {
    return false;
  }
  const required: Record<ResourceFamily, Permission> = {
    user: "user:read",
    profile: "profile:read",
    wallet: "wallet:read",
    membership: "membership:read",
    notifications: "user:read",
    analytics: "user:read",
  };
  return identity.permissions.includes(required[family]);
}