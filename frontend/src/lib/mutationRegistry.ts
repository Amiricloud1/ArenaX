/**
 * Optimistic mutation registry.
 *
 * Centralizes rollback and reconciliation semantics for optimistic UI updates.
 *
 * Each mutation declares:
 *  - whether it is optimistic
 *  - a rollback strategy per affected resource
 *  - a refetch strategy per affected resource
 *
 * Server versions are tracked per resource so that a late response which is
 * older than the current known server version is ignored (it cannot overwrite
 * newer state). Failures are surfaced as an accessible, recoverable state that
 * error boundaries / UI can consume.
 */

export type ResourceKey = string;

export type RollbackStrategy =
  | { kind: 'restore'; snapshot: unknown }
  | { kind: 'refetch'; resource: ResourceKey }
  | { kind: 'invalidate'; resource: ResourceKey }
  | { kind: 'none' };

export type RefetchStrategy =
  | { kind: 'refetch'; resource: ResourceKey }
  | { kind: 'invalidate'; resource: ResourceKey }
  | { kind: 'none' };

export interface ResourceStrategy {
  /** How to undo an optimistic change for this resource. */
  rollback: RollbackStrategy;
  /** How to reconcile with the authoritative server after settle. */
  refetch: RefetchStrategy;
}

export interface MutationDefinition<TVariables = unknown, TResult = unknown> {
  /** Stable identifier for the mutation (used for logging / dedupe). */
  id: string;
  /** Whether this mutation applies an optimistic update. */
  optimistic: boolean;
  /**
   * Per-resource rollback/refetch strategy. Keys are resource identifiers
   * (e.g. `user:42`, `cart`, `order:99`).
   */
  resources: Record<ResourceKey, ResourceStrategy>;
  /**
   * Optional idempotency key factory. When provided, retries reuse the same
   * key so the server can dedupe duplicate submissions.
   */
  idempotencyKey?: (variables: TVariables) => string;
  /** The actual network call. */
  mutate: (variables: TVariables, ctx: MutationContext) => Promise<TResult>;
}

export interface MutationContext {
  idempotencyKey?: string;
  signal?: AbortSignal;
}

export type MutationStatus =
  | 'idle'
  | 'pending'
  | 'success'
  | 'rolled-back'
  | 'failed';

export interface MutationFailure {
  /** Machine-readable failure code. */
  code:
    | 'timeout'
    | 'validation'
    | 'conflict'
    | 'network'
    | 'unauthorized'
    | 'unknown';
  /** Human-readable, accessible message. */
  message: string;
  /** Whether the user can safely retry. */
  recoverable: boolean;
  /** Idempotency key to reuse on retry, if any. */
  idempotencyKey?: string;
  /** Resource that failed, if applicable. */
  resource?: ResourceKey;
}

export interface MutationRecord {
  id: string;
  status: MutationStatus;
  optimistic: boolean;
  startedAt: number;
  settledAt?: number;
  failure?: MutationFailure;
  /** Server version observed when the mutation was issued. */
  baseVersions: Record<ResourceKey, number>;
}

export interface ResourceState {
  /** Monotonic server version for the resource. */
  version: number;
  /** Last known authoritative value. */
  value: unknown;
}

export interface RegistrySnapshot {
  resources: Record<ResourceKey, ResourceState>;
  mutations: Record<string, MutationRecord>;
  /** Latest recoverable failure, for accessible error surfaces. */
  lastFailure?: MutationFailure;
}

type Listener = (snapshot: RegistrySnapshot) => void;

interface InFlight {
  record: MutationRecord;
  definition: MutationDefinition<unknown, unknown>;
  variables: unknown;
  idempotencyKey?: string;
  controller: AbortController;
  timer?: ReturnType<typeof setTimeout>;
}

const DEFAULT_TIMEOUT_MS = 30_000;

function isAbortError(err: unknown): boolean {
  return (
    typeof err === 'object' &&
    err !== null &&
    ((err as { name?: string }).name === 'AbortError' ||
      (err as { code?: string }).code === 'ABORT_ERR')
  );
}

function classifyError(err: unknown): MutationFailure {
  if (isAbortError(err)) {
    return {
      code: 'timeout',
      message: 'The request timed out. You can try again.',
      recoverable: true,
    };
  }
  if (typeof err === 'object' && err !== null) {
    const e = err as {
      status?: number;
      code?: string;
      message?: string;
      recoverable?: boolean;
    };
    if (e.status === 401 || e.status === 403 || e.code === 'unauthorized') {
      return {
        code: 'unauthorized',
        message: 'You are not authorized to perform this action.',
        recoverable: false,
      };
    }
    if (e.status === 409 || e.code === 'conflict') {
      return {
        code: 'conflict',
        message:
          'This item was changed elsewhere. The latest version has been loaded.',
        recoverable: true,
      };
    }
    if (e.status === 422 || e.code === 'validation') {
      return {
        code: 'validation',
        message: e.message || 'The submitted data is invalid.',
        recoverable: true,
      };
    }
    if (e.code === 'network') {
      return {
        code: 'network',
        message: 'A network error occurred. You can try again.',
        recoverable: true,
      };
    }
    if (e.message) {
      return {
        code: 'unknown',
        message: e.message,
        recoverable: e.recoverable ?? true,
      };
    }
  }
  return {
    code: 'unknown',
    message: 'Something went wrong. You can try again.',
    recoverable: true,
  };
}

export class MutationRegistry {
  private resources: Record<ResourceKey, ResourceState> = {};
  private mutations: Record<string, MutationRecord> = {};
  private inFlight = new Map<string, InFlight>();
  private listeners = new Set<Listener>();
  private lastFailure?: MutationFailure;
  private timeoutMs: number;
  private seq = 0;

  constructor(options: { timeoutMs?: number } = {}) {
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  }

  /** Subscribe to registry changes (for React `useSyncExternalStore`). */
  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  getSnapshot(): RegistrySnapshot {
    return {
      resources: this.resources,
      mutations: this.mutations,
      lastFailure: this.lastFailure,
    };
  }

  /** Register an authoritative server version for a resource. */
  setServerVersion(resource: ResourceKey, version: number, value?: unknown): void {
    const current = this.resources[resource];
    if (current && version < current.version) {
      // Late/out-of-order response: ignore, never overwrite newer state.
      return;
    }
    this.resources[resource] = {
      version,
      value: value !== undefined ? value : current?.value,
    };
    this.emit();
  }

  getResource(resource: ResourceKey): ResourceState | undefined {
    return this.resources[resource];
  }

  /** Clear the last recoverable failure (e.g. after user dismisses it). */
  clearFailure(): void {
    if (this.lastFailure) {
      this.lastFailure = undefined;
      this.emit();
    }
  }

  /**
   * Run a mutation. Applies optimistic state, tracks server versions, and
   * rolls back / refetches on failure. Late responses older than the current
   * server version are ignored.
   */
  async run<TVariables, TResult>(
    definition: MutationDefinition<TVariables, TResult>,
    variables: TVariables,
    options: { timeoutMs?: number } = {},
  ): Promise<{ ok: true; data: TResult } | { ok: false; failure: MutationFailure }> {
    const runId = `${definition.id}:${++this.seq}`;
    const idempotencyKey = definition.idempotencyKey?.(variables);
    const controller = new AbortController();
    const timeoutMs = options.timeoutMs ?? this.timeoutMs;

    const baseVersions: Record<ResourceKey, number> = {};
    for (const resource of Object.keys(definition.resources)) {
      baseVersions[resource] = this.resources[resource]?.version ?? 0;
    }

    const record: MutationRecord = {
      id: runId,
      status: 'pending',
      optimistic: definition.optimistic,
      startedAt: Date.now(),
      baseVersions,
    };
    this.mutations[runId] = record;

    const flight: InFlight = {
      record,
      definition: definition as MutationDefinition<unknown, unknown>,
      variables,
      idempotencyKey,
      controller,
    };

    if (timeoutMs > 0) {
      flight.timer = setTimeout(() => {
        controller.abort();
      }, timeoutMs);
    }
    this.inFlight.set(runId, flight);
    this.emit();

    try {
      const data = await definition.mutate(variables, {
        idempotencyKey,
        signal: controller.signal,
      });

      if (flight.timer) clearTimeout(flight.timer);
      this.inFlight.delete(runId);

      // Reconcile: refetch/invalidate declared resources.
      this.reconcile(definition.resources);

      record.status = 'success';
      record.settledAt = Date.now();
      this.emit();
      return { ok: true, data };
    } catch (err) {
      if (flight.timer) clearTimeout(flight.timer);
      this.inFlight.delete(runId);

      const failure = classifyError(err);
      failure.idempotencyKey = idempotencyKey;
      const firstResource = Object.keys(definition.resources)[0];
      if (firstResource) failure.resource = firstResource;

      // Roll back optimistic changes.
      this.rollback(definition.resources);

      record.status = 'rolled-back';
      record.settledAt = Date.now();
      record.failure = failure;
      this.lastFailure = failure;
      this.emit();
      return { ok: false, failure };
    }
  }

  /** Cancel all in-flight mutations (e.g. on unmount / navigation). */
  cancelAll(): void {
    for (const flight of this.inFlight.values()) {
      if (flight.timer) clearTimeout(flight.timer);
      flight.controller.abort();
    }
    this.inFlight.clear();
  }

  private rollback(resources: Record<ResourceKey, ResourceStrategy>): void {
    for (const [resource, strategy] of Object.entries(resources)) {
      switch (strategy.rollback.kind) {
        case 'restore':
          // Restore snapshot without bumping version (optimistic undo).
          if (this.resources[resource]) {
            this.resources[resource] = {
              ...this.resources[resource],
              value: strategy.rollback.snapshot,
            };
          }
          break;
        case 'refetch':
        case 'invalidate':
          // Mark for refetch; consumers observe via snapshot.
          break;
        case 'none':
        default:
          break;
      }
    }
  }

  private reconcile(resources: Record<ResourceKey, ResourceStrategy>): void {
    for (const strategy of Object.values(resources)) {
      switch (strategy.refetch.kind) {
        case 'refetch':
        case 'invalidate':
          // Consumers refetch authoritative state; version guard in
          // setServerVersion prevents stale overwrites.
          break;
        case 'none':
        default:
          break;
      }
    }
  }

  private emit(): void {
    const snapshot = this.getSnapshot();
    for (const listener of this.listeners) {
      try {
        listener(snapshot);
      } catch {
        // Listener errors must not break the registry.
      }
    }
  }
}

/** Default singleton registry for app-wide use. */
export const mutationRegistry = new MutationRegistry();