// User-related types
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

/**
 * A validated, local-only post-authentication return destination.
 *
 * Instances of this type are produced exclusively by the return-destination
 * validator, which guarantees:
 * - `path` is a local, approved path (never an absolute or protocol-relative
 *   URL, and never a path outside the allowlist).
 * - `query` contains only validated, non-secret query parameters.
 *
 * Consumers must treat this as an opaque, already-validated value and must not
 * re-derive it from raw user input.
 */
export interface ReturnDestination {
  /** Local, approved path (e.g. `/tournaments/123`). */
  path: string;
  /** Validated, non-secret query parameters to preserve. */
  query?: Record<string, string>;
  /** Optional locale prefix that was present on the original route. */
  locale?: string;
  /** When the destination was captured; used to expire stale invitations. */
  capturedAt?: string;
}

/**
 * Conflict-resolution strategy applied when replaying an offline command
 * against newer server state.
 */
export type OfflineConflictStrategy =
  | "reject"
  | "server-wins"
  | "client-wins"
  | "merge";

/**
 * Metadata every queued offline command must declare.
 *
 * Only explicitly safe (allowlisted) operations may be queued; unsafe
 * operations (payments, match results, etc.) are rejected from offline mode.
 */
export interface OfflineCommandMeta {
  /** Stable idempotency key so duplicate replays are deduplicated server-side. */
  idempotencyKey: string;
  /** ISO timestamp after which the command must not be replayed. */
  expiresAt: string;
  /** User scope the command belongs to; used to purge on logout/switch. */
  userScope: string;
  /** How to resolve conflicts with newer server state during replay. */
  conflictStrategy: OfflineConflictStrategy;
  /** When the command was enqueued (ISO timestamp). */
  enqueuedAt?: string;
  /** Optional optimistic-concurrency version observed at enqueue time. */
  baseVersion?: string | number;
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
  /**
   * Stable scope identifier for this authenticated session's user. Offline
   * commands are tagged with this scope and purged on logout or account
   * switch.
   */
  offlineScope?: string;
  /**
   * Session identifier used to detect account switches and to invalidate
   * scoped offline commands belonging to a previous session.
   */
  sessionId?: string;
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