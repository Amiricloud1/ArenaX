// frontend/src/lib/offlineQueue.ts
//
// Explicit allowlisted offline command queue.
//
// Offline retry can duplicate or reorder financial and competitive actions if
// every failed request is treated as replayable. This module therefore only
// accepts an explicit allowlist of "safe" commands. Each queued command
// declares:
//   - an idempotency key (so duplicate replays are collapsed server-side),
//   - an expiry (so stale commands are dropped rather than replayed),
//   - a user scope (so logout / account switch purges the right commands),
//   - a conflict strategy (so replay surfaces conflicts instead of silently
//     overwriting newer server state).
//
// Unsafe operations (payments, match submissions, etc.) are rejected from
// offline mode entirely. Server authority for private and financial state is
// preserved: this queue never mutates server state locally, it only records
// intent to replay a safe command later.

export type ConflictStrategy =
  // Reject the replay if the server reports a newer version; surface a conflict.
  | "reject-on-conflict"
  // Replay only if the server's version matches the version captured at enqueue.
  | "version-match"
  // Replay is safe to apply regardless of server version (idempotent, additive).
  | "idempotent";

export type OfflineCommandType =
  // --- Allowlisted (safe) commands -----------------------------------------
  | "profile.update"
  | "preferences.update"
  | "notification.read"
  | "draft.save"
  | "bookmark.toggle"
  | "analytics.event";

// Commands that are explicitly NOT queueable offline. Kept as a named type so
// callers get a clear compile-time and runtime rejection.
export type UnsafeCommandType =
  | "payment.create"
  | "payment.confirm"
  | "payment.refund"
  | "match.submit"
  | "match.forfeit"
  | "match.join"
  | "match.leave"
  | "wallet.withdraw"
  | "wallet.deposit"
  | "auth.login"
  | "auth.logout";

export interface OfflineCommandMeta {
  /** Stable idempotency key; server must dedupe on this. */
  idempotencyKey: string;
  /** Epoch ms after which the command must not be replayed. */
  expiresAt: number;
  /** User the command belongs to; used for scoping and purge. */
  userScope: string;
  /** How replay should behave when server state has moved on. */
  conflictStrategy: ConflictStrategy;
  /** Optional server version captured at enqueue time. */
  baseVersion?: string | number | null;
}

export interface OfflineCommand<TType extends OfflineCommandType = OfflineCommandType> {
  id: string;
  type: TType;
  /** Serialized payload for the command. */
  payload: unknown;
  meta: OfflineCommandMeta;
  /** Epoch ms when the command was enqueued. */
  enqueuedAt: number;
  /** Number of replay attempts so far. */
  attempts: number;
}

export type ReplayOutcome =
  | { status: "applied"; command: OfflineCommand }
  | { status: "duplicate"; command: OfflineCommand }
  | { status: "expired"; command: OfflineCommand }
  | { status: "conflict"; command: OfflineCommand; serverVersion?: string | number | null }
  | { status: "failed"; command: OfflineCommand; error: unknown };

export interface ReplayResult {
  outcomes: ReplayOutcome[];
  /** Commands that remain queued (failed / retryable). */
  remaining: OfflineCommand[];
  /** Commands removed from the queue (applied / duplicate / expired / conflict). */
  removed: OfflineCommand[];
}

/**
 * The allowlist. A command type is queueable offline only if it appears here.
 * Everything else (payments, match actions, auth) is rejected.
 */
export const QUEUEABLE_COMMANDS: ReadonlySet<OfflineCommandType> = new Set<OfflineCommandType>([
  "profile.update",
  "preferences.update",
  "notification.read",
  "draft.save",
  "bookmark.toggle",
  "analytics.event",
]);

const UNSAFE_COMMANDS: ReadonlySet<string> = new Set<string>([
  "payment.create",
  "payment.confirm",
  "payment.refund",
  "match.submit",
  "match.forfeit",
  "match.join",
  "match.leave",
  "wallet.withdraw",
  "wallet.deposit",
  "auth.login",
  "auth.logout",
]);

export function isQueueable(type: string): type is OfflineCommandType {
  return QUEUEABLE_COMMANDS.has(type as OfflineCommandType);
}

export function isUnsafe(type: string): type is UnsafeCommandType {
  return UNSAFE_COMMANDS.has(type);
}

export class OfflineQueueError extends Error {
  constructor(
    message: string,
    readonly code:
      | "unsafe_command"
      | "unknown_command"
      | "invalid_meta"
      | "scope_mismatch"
      | "storage_unavailable",
  ) {
    super(message);
    this.name = "OfflineQueueError";
  }
}

/** Minimal storage abstraction so this works in browser and in tests. */
export interface QueueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

const STORAGE_PREFIX = "offlineQueue:v1:";

function storageKeyFor(userScope: string): string {
  return `${STORAGE_PREFIX}${userScope}`;
}

function defaultStorage(): QueueStorage | null {
  try {
    if (typeof window !== "undefined" && window.localStorage) {
      return window.localStorage;
    }
  } catch {
    // Access can throw in privacy modes; fall through to null.
  }
  return null;
}

function generateId(): string {
  // Prefer crypto.randomUUID when available; fall back to a random string.
  try {
    if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
      return crypto.randomUUID();
    }
  } catch {
    // ignore
  }
  return `cmd_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 10)}`;
}

function generateIdempotencyKey(): string {
  return generateId();
}

export interface EnqueueOptions<TType extends OfflineCommandType> {
  type: TType;
  payload: unknown;
  userScope: string;
  /** Time-to-live in ms from now. Defaults to 24h. */
  ttlMs?: number;
  conflictStrategy?: ConflictStrategy;
  baseVersion?: string | number | null;
  /** Override idempotency key (e.g. derived from payload). */
  idempotencyKey?: string;
  /** Injectable clock for tests. */
  now?: number;
}

const DEFAULT_TTL_MS = 24 * 60 * 60 * 1000;

const DEFAULT_CONFLICT_STRATEGY: Record<OfflineCommandType, ConflictStrategy> = {
  "profile.update": "version-match",
  "preferences.update": "version-match",
  "notification.read": "idempotent",
  "draft.save": "reject-on-conflict",
  "bookmark.toggle": "idempotent",
  "analytics.event": "idempotent",
};

export interface ReplayHandlers {
  /**
   * Execute a single command against the server. Must return the server's
   * current version (if any) so conflict detection can work. Throwing signals a
   * retryable failure.
   */
  execute: (command: OfflineCommand) => Promise<{
    serverVersion?: string | number | null;
    /** True if the server reports this idempotency key was already applied. */
    duplicate?: boolean;
  }>;
  /** Injectable clock for tests. */
  now?: () => number;
}

export class OfflineQueue {
  private storage: QueueStorage | null;

  constructor(storage?: QueueStorage | null) {
    this.storage = storage === undefined ? defaultStorage() : storage;
  }

  private read(userScope: string): OfflineCommand[] {
    if (!this.storage) return [];
    try {
      const raw = this.storage.getItem(storageKeyFor(userScope));
      if (!raw) return [];
      const parsed = JSON.parse(raw);
      if (!Array.isArray(parsed)) return [];
      return parsed.filter(isValidCommand);
    } catch {
      return [];
    }
  }

  private write(userScope: string, commands: OfflineCommand[]): void {
    if (!this.storage) {
      throw new OfflineQueueError("Offline storage is unavailable", "storage_unavailable");
    }
    try {
      this.storage.setItem(storageKeyFor(userScope), JSON.stringify(commands));
    } catch (err) {
      throw new OfflineQueueError(
        `Failed to persist offline queue: ${(err as Error).message}`,
        "storage_unavailable",
      );
    }
  }

  /**
   * Enqueue a command. Rejects unsafe / unknown command types and invalid meta.
   */
  enqueue<TType extends OfflineCommandType>(options: EnqueueOptions<TType>): OfflineCommand<TType> {
    const { type } = options;

    if (isUnsafe(type)) {
      throw new OfflineQueueError(
        `Command "${type}" is not allowed in offline mode`,
        "unsafe_command",
      );
    }
    if (!isQueueable(type)) {
      throw new OfflineQueueError(`Unknown offline command type "${type}"`, "unknown_command");
    }
    if (!options.userScope) {
      throw new OfflineQueueError("userScope is required", "invalid_meta");
    }

    const now = options.now ?? Date.now();
    const ttlMs = options.ttlMs ?? DEFAULT_TTL_MS;
    if (!Number.isFinite(ttlMs) || ttlMs <= 0) {
      throw new OfflineQueueError("ttlMs must be a positive number", "invalid_meta");
    }

    const command: OfflineCommand<TType> = {
      id: generateId(),
      type,
      payload: options.payload,
      meta: {
        idempotencyKey: options.idempotencyKey ?? generateIdempotencyKey(),
        expiresAt: now + ttlMs,
        userScope: options.userScope,
        conflictStrategy: options.conflictStrategy ?? DEFAULT_CONFLICT_STRATEGY[type],
        baseVersion: options.baseVersion ?? null,
      },
      enqueuedAt: now,
      attempts: 0,
    };

    const existing = this.read(options.userScope);
    // Dedupe by idempotency key within the same scope.
    if (existing.some((c) => c.meta.idempotencyKey === command.meta.idempotencyKey)) {
      return command;
    }
    this.write(options.userScope, [...existing, command]);
    return command;
  }

  /** List queued commands for a user scope. */
  list(userScope: string): OfflineCommand[] {
    return this.read(userScope);
  }

  /** Purge all commands for a user scope (logout / account switch). */
  purge(userScope: string): void {
    if (!this.storage) return;
    try {
      this.storage.removeItem(storageKeyFor(userScope));
    } catch {
      // best-effort
    }
  }

  /**
   * Purge every scope except the given active scope. Used on account switch so
   * commands belonging to the previous account are never replayed.
   */
  purgeAllExcept(activeScope: string | null): void {
    if (!this.storage) return;
    const keys: string[] = [];
    try {
      // QueueStorage may not expose key enumeration; fall back to known scopes.
      const maybe = this.storage as unknown as { key?: (i: number) => string | null; length?: number };
      if (typeof maybe.key === "function" && typeof maybe.length === "number") {
        for (let i = 0; i < maybe.length; i += 1) {
          const k = maybe.key(i);
          if (k && k.startsWith(STORAGE_PREFIX)) keys.push(k);
        }
      }
    } catch {
      // ignore
    }
    for (const key of keys) {
      const scope = key.slice(STORAGE_PREFIX.length);
      if (scope !== activeScope) {
        try {
          this.storage.removeItem(key);
        } catch {
          // best-effort
        }
      }
    }
  }

  /**
   * Replay queued commands for a user scope. Expired commands are dropped,
   * duplicates are collapsed, and conflicts are surfaced (never silently
   * overwritten). Failed commands remain queued for a later retry.
   */
  async replay(userScope: string, handlers: ReplayHandlers): Promise<ReplayResult> {
    const now = handlers.now ? handlers.now() : Date.now();
    const commands = this.read(userScope);
    const outcomes: ReplayOutcome[] = [];
    const remaining: OfflineCommand[] = [];
    const removed: OfflineCommand[] = [];

    for (const command of commands) {
      if (command.meta.userScope !== userScope) {
        // Defensive: never replay a command outside its declared scope.
        remaining.push(command);
        continue;
      }

      if (command.meta.expiresAt <= now) {
        outcomes.push({ status: "expired", command });
        removed.push(command);
        continue;
      }

      try {
        const result = await handlers.execute(command);

        if (result.duplicate) {
          outcomes.push({ status: "duplicate", command });
          removed.push(command);
          continue;
        }

        const conflict = detectConflict(command, result.serverVersion);
        if (conflict) {
          outcomes.push({
            status: "conflict",
            command,
            serverVersion: result.serverVersion ?? null,
          });
          // Surface the conflict; do not overwrite newer server state.
          removed.push(command);
          continue;
        }

        outcomes.push({ status: "applied", command });
        removed.push(command);
      } catch (error) {
        const failed: OfflineCommand = { ...command, attempts: command.attempts + 1 };
        outcomes.push({ status: "failed", command: failed, error });
        remaining.push(failed);
      }
    }

    this.write(userScope, remaining);
    return { outcomes, remaining, removed };
  }
}

function detectConflict(
  command: OfflineCommand,
  serverVersion: string | number | null | undefined,
): boolean {
  const strategy = command.meta.conflictStrategy;
  if (strategy === "idempotent") return false;

  const base = command.meta.baseVersion;
  const server = serverVersion ?? null;

  if (strategy === "version-match") {
    // Conflict if the server version differs from what we based the command on.
    return base !== server;
  }

  // reject-on-conflict: conflict if the server has advanced past our base.
  if (base === null || base === undefined) {
    // No base version captured; treat any server version as a potential conflict.
    return server !== null && server !== undefined;
  }
  if (server === null || server === undefined) return false;
  return String(server) !== String(base);
}

function isValidCommand(value: unknown): value is OfflineCommand {
  if (!value || typeof value !== "object") return false;
  const c = value as Partial<OfflineCommand>;
  if (typeof c.id !== "string") return false;
  if (typeof c.type !== "string") return false;
  if (!c.meta || typeof c.meta !== "object") return false;
  const m = c.meta as Partial<OfflineCommandMeta>;
  if (typeof m.idempotencyKey !== "string") return false;
  if (typeof m.expiresAt !== "number") return false;
  if (typeof m.userScope !== "string") return false;
  if (
    m.conflictStrategy !== "reject-on-conflict" &&
    m.conflictStrategy !== "version-match" &&
    m.conflictStrategy !== "idempotent"
  ) {
    return false;
  }
  if (typeof c.enqueuedAt !== "number") return false;
  if (typeof c.attempts !== "number") return false;
  return true;
}

/** Default singleton queue backed by localStorage when available. */
export const offlineQueue = new OfflineQueue();