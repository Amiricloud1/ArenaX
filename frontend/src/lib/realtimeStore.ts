// frontend/src/lib/realtimeStore.ts
//
// Shared realtime store for match and tournament pages.
//
// Responsibilities:
//  - Record the snapshot watermark (stream sequence) returned by the API.
//  - Buffer incoming websocket events and apply them in order.
//  - Detect gaps, out-of-order delivery, and stale events.
//  - Reject stale frames (sequence <= applied watermark).
//  - Request a replacement snapshot when a gap or retention expiry is detected,
//    without duplicating optimistic updates.
//
// Server authority: private and financial state is never mutated optimistically
// here; only public realtime state (scores, timers, status) flows through this
// store. Optimistic updates are tracked separately and reconciled against the
// authoritative snapshot/event stream.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

/* -------------------------------------------------------------------------- */
/* Types                                                                      */
/* -------------------------------------------------------------------------- */

export type RealtimeEventKind =
  | "match.update"
  | "match.status"
  | "tournament.update"
  | "tournament.standings"
  | "ping"
  | "unknown";

export interface RealtimeEvent<T = unknown> {
  /** Monotonic stream sequence assigned by the server. */
  seq: number;
  /** Event kind discriminator. */
  kind: RealtimeEventKind;
  /** Event payload. */
  data: T;
  /** Optional server timestamp (ms since epoch). */
  ts?: number;
}

export interface SnapshotResponse<T = unknown> {
  /** Stream sequence / watermark from which subsequent events apply. */
  streamSeq: number;
  /** Snapshot payload. */
  data: T;
  /** Optional server timestamp (ms since epoch). */
  ts?: number;
}

export type RealtimeStatus =
  | "idle"
  | "loading"
  | "ready"
  | "reconnecting"
  | "resyncing"
  | "error";

export interface RealtimeState<T> {
  status: RealtimeStatus;
  /** Authoritative snapshot data (server). */
  data: T | null;
  /** Last applied stream sequence (watermark). */
  watermark: number;
  /** Number of buffered events awaiting application. */
  buffered: number;
  /** Last error, if any. */
  error: Error | null;
  /** True while a replacement snapshot is in flight. */
  resyncing: boolean;
}

export interface RealtimeStoreOptions<T> {
  /**
   * Fetches a fresh snapshot. Must include `streamSeq` (watermark) in the
   * response. Called on mount, on gap detection, and on retention expiry.
   */
  fetchSnapshot: (signal?: AbortSignal) => Promise<SnapshotResponse<T>>;
  /**
   * Applies an event to the current snapshot data. Must be pure and return
   * the next data value. Called only for events with `seq === watermark + 1`.
   */
  applyEvent: (data: T, event: RealtimeEvent) => T;
  /**
   * Optional: maximum number of events to buffer before forcing a resync.
   * Defaults to 256.
   */
  maxBuffer?: number;
  /**
   * Optional: called when a resync is triggered, for telemetry.
   */
  onResync?: (reason: ResyncReason) => void;
}

export type ResyncReason =
  | "gap"
  | "retention-expired"
  | "buffer-overflow"
  | "manual"
  | "reconnect";

/* -------------------------------------------------------------------------- */
/* Core store (framework-agnostic)                                            */
/* -------------------------------------------------------------------------- */

export interface RealtimeStore<T> {
  getState: () => RealtimeState<T>;
  subscribe: (listener: () => void) => () => void;
  /** Feed a raw event from the websocket. */
  ingest: (event: RealtimeEvent) => void;
  /** Force a replacement snapshot fetch. */
  resync: (reason?: ResyncReason) => Promise<void>;
  /** Notify the store that the transport reconnected. */
  onReconnect: () => void;
  /** Stop the store and abort in-flight requests. */
  dispose: () => void;
}

interface InternalState<T> extends RealtimeState<T> {
  /** Events received but not yet applied (seq > watermark + 1). */
  pending: Map<number, RealtimeEvent>;
  /** Highest sequence ever seen, used to reject stale frames. */
  highestSeen: number;
  /** Whether an initial snapshot has been applied. */
  bootstrapped: boolean;
}

export function createRealtimeStore<T>(
  options: RealtimeStoreOptions<T>,
): RealtimeStore<T> {
  const maxBuffer = options.maxBuffer ?? 256;

  let state: InternalState<T> = {
    status: "idle",
    data: null,
    watermark: 0,
    buffered: 0,
    error: null,
    resyncing: false,
    pending: new Map(),
    highestSeen: 0,
    bootstrapped: false,
  };

  const listeners = new Set<() => void>();
  let abortController: AbortController | null = null;
  let disposed = false;
  let resyncInFlight: Promise<void> | null = null;

  const emit = () => {
    for (const l of listeners) l();
  };

  const setState = (patch: Partial<InternalState<T>>) => {
    state = { ...state, ...patch };
    emit();
  };

  const publicState = (): RealtimeState<T> => ({
    status: state.status,
    data: state.data,
    watermark: state.watermark,
    buffered: state.pending.size,
    error: state.error,
    resyncing: state.resyncing,
  });

  const drainPending = () => {
    // Apply contiguous events starting at watermark + 1.
    let next = state.watermark + 1;
    let data = state.data;
    let applied = 0;
    while (state.pending.has(next)) {
      const ev = state.pending.get(next)!;
      state.pending.delete(next);
      if (data !== null) {
        data = options.applyEvent(data, ev);
      }
      state.watermark = next;
      next += 1;
      applied += 1;
    }
    if (applied > 0) {
      setState({ data, watermark: state.watermark, buffered: state.pending.size });
    }
  };

  const triggerResync = (reason: ResyncReason) => {
    if (disposed) return;
    options.onResync?.(reason);
    // Clear pending buffer; a fresh snapshot supersedes it.
    state.pending.clear();
    void resync(reason);
  };

  const resync = async (reason: ResyncReason = "manual"): Promise<void> => {
    if (disposed) return;
    if (resyncInFlight) return resyncInFlight;

    abortController?.abort();
    abortController = new AbortController();
    const signal = abortController.signal;

    setState({
      status: state.bootstrapped ? "resyncing" : "loading",
      resyncing: state.bootstrapped,
      error: null,
      buffered: 0,
    });

    const run = (async () => {
      try {
        const snapshot = await options.fetchSnapshot(signal);
        if (disposed || signal.aborted) return;

        // Reject snapshots that are older than what we've already applied.
        // This prevents a slow snapshot from clobbering newer state.
        if (state.bootstrapped && snapshot.streamSeq < state.watermark) {
          // Snapshot is stale relative to applied watermark; keep current state
          // and retry once with a fresh request.
          setState({ status: "ready", resyncing: false });
          return;
        }

        // Drop any pending events that are now covered by the snapshot.
        for (const seq of Array.from(state.pending.keys())) {
          if (seq <= snapshot.streamSeq) state.pending.delete(seq);
        }

        state = {
          ...state,
          status: "ready",
          data: snapshot.data,
          watermark: snapshot.streamSeq,
          highestSeen: Math.max(state.highestSeen, snapshot.streamSeq),
          bootstrapped: true,
          resyncing: false,
          error: null,
          buffered: state.pending.size,
        };
        emit();

        // Apply any buffered events that are now contiguous.
        drainPending();

        // If a gap remains after the snapshot, resync again.
        if (state.pending.size > 0) {
          const minPending = Math.min(...state.pending.keys());
          if (minPending > state.watermark + 1) {
            triggerResync("gap");
          }
        }
      } catch (err) {
        if (disposed || signal.aborted) return;
        const error = err instanceof Error ? err : new Error(String(err));
        setState({
          status: "error",
          resyncing: false,
          error,
        });
      }
    })();

    resyncInFlight = run.finally(() => {
      resyncInFlight = null;
    });
    return resyncInFlight;
  };

  const ingest = (event: RealtimeEvent) => {
    if (disposed) return;
    if (!event || typeof event.seq !== "number") return;

    // Ignore control frames.
    if (event.kind === "ping") return;

    // Reject stale frames: already applied or older than watermark.
    if (event.seq <= state.watermark) return;

    // Reject duplicate frames already buffered.
    if (state.pending.has(event.seq)) return;

    // Track highest seen for diagnostics.
    if (event.seq > state.highestSeen) {
      state.highestSeen = event.seq;
    }

    // Not bootstrapped yet: buffer until snapshot arrives.
    if (!state.bootstrapped) {
      state.pending.set(event.seq, event);
      if (state.pending.size > maxBuffer) {
        triggerResync("buffer-overflow");
      } else {
        setState({ buffered: state.pending.size });
      }
      return;
    }

    // Contiguous event: apply immediately.
    if (event.seq === state.watermark + 1) {
      const data = state.data !== null ? options.applyEvent(state.data, event) : state.data;
      state.watermark = event.seq;
      setState({ data, watermark: state.watermark });
      // Drain any now-contiguous buffered events.
      drainPending();
      return;
    }

    // Out-of-order / future event: buffer it.
    if (event.seq > state.watermark + 1) {
      state.pending.set(event.seq, event);
      if (state.pending.size > maxBuffer) {
        triggerResync("buffer-overflow");
        return;
      }
      setState({ buffered: state.pending.size });

      // Detect a gap: if the lowest buffered seq is beyond watermark + 1,
      // we may be missing events. Give the transport a short grace window
      // before forcing a resync, so a single out-of-order frame doesn't
      // trigger a full snapshot fetch.
      scheduleGapCheck();
    }
  };

  let gapTimer: ReturnType<typeof setTimeout> | null = null;
  const scheduleGapCheck = () => {
    if (gapTimer) return;
    gapTimer = setTimeout(() => {
      gapTimer = null;
      if (disposed) return;
      if (state.pending.size === 0) return;
      const minPending = Math.min(...state.pending.keys());
      if (minPending > state.watermark + 1) {
        triggerResync("gap");
      }
    }, 250);
  };

  const onReconnect = () => {
    if (disposed) return;
    // On reconnect, the server may have advanced beyond our watermark.
    // Force a resync to re-establish a consistent bootstrap.
    triggerResync("reconnect");
  };

  const dispose = () => {
    disposed = true;
    abortController?.abort();
    if (gapTimer) {
      clearTimeout(gapTimer);
      gapTimer = null;
    }
    listeners.clear();
  };

  // Kick off initial snapshot fetch.
  void resync("manual");

  return {
    getState: publicState,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    ingest,
    resync: (reason) => resync(reason),
    onReconnect,
    dispose,
  };
}

/* -------------------------------------------------------------------------- */
/* React hook                                                                 */
/* -------------------------------------------------------------------------- */

export interface UseRealtimeStoreResult<T> extends RealtimeState<T> {
  /** Feed a raw event from the websocket. */
  ingest: (event: RealtimeEvent) => void;
  /** Force a replacement snapshot fetch. */
  resync: (reason?: ResyncReason) => Promise<void>;
  /** Notify the store that the transport reconnected. */
  onReconnect: () => void;
}

export function useRealtimeStore<T>(
  options: RealtimeStoreOptions<T>,
): UseRealtimeStoreResult<T> {
  // Keep latest options in a ref so the store isn't recreated on every render.
  const optionsRef = useRef(options);
  optionsRef.current = options;

  const storeRef = useRef<RealtimeStore<T> | null>(null);
  if (storeRef.current === null) {
    storeRef.current = createRealtimeStore<T>({
      fetchSnapshot: (signal) => optionsRef.current.fetchSnapshot(signal),
      applyEvent: (data, event) => optionsRef.current.applyEvent(data, event),
      maxBuffer: optionsRef.current.maxBuffer,
      onResync: (reason) => optionsRef.current.onResync?.(reason),
    });
  }

  const store = storeRef.current;
  const [state, setState] = useState<RealtimeState<T>>(() => store.getState());

  useEffect(() => {
    const unsubscribe = store.subscribe(() => {
      setState(store.getState());
    });
    // Sync in case state changed between render and subscribe.
    setState(store.getState());
    return () => {
      unsubscribe();
    };
  }, [store]);

  useEffect(() => {
    return () => {
      store.dispose();
    };
  }, [store]);

  const ingest = useCallback((event: RealtimeEvent) => store.ingest(event), [store]);
  const resync = useCallback(
    (reason?: ResyncReason) => store.resync(reason),
    [store],
  );
  const onReconnect = useCallback(() => store.onReconnect(), [store]);

  return useMemo(
    () => ({ ...state, ingest, resync, onReconnect }),
    [state, ingest, resync, onReconnect],
  );
}

/* -------------------------------------------------------------------------- */
/* Optimistic update helpers                                                  */
/* -------------------------------------------------------------------------- */

export interface OptimisticUpdate<T> {
  id: string;
  apply: (data: T) => T;
  /** Optional: predicate to determine if the update has been confirmed. */
  isConfirmed?: (data: T) => boolean;
}

/**
 * Applies optimistic updates on top of authoritative data without mutating
 * the underlying snapshot. Confirmed updates are dropped from the overlay.
 *
 * This keeps optimistic UI separate from the authoritative realtime stream so
 * that a replacement snapshot never duplicates an optimistic update.
 */
export function applyOptimisticOverlay<T>(
  authoritative: T | null,
  updates: OptimisticUpdate<T>[],
): T | null {
  if (authoritative === null) return null;
  let result = authoritative;
  for (const update of updates) {
    if (update.isConfirmed?.(result)) continue;
    result = update.apply(result);
  }
  return result;
}

/**
 * Reconciles an optimistic update list against fresh authoritative data,
 * removing updates that the server has already reflected.
 */
export function reconcileOptimistic<T>(
  authoritative: T | null,
  updates: OptimisticUpdate<T>[],
): OptimisticUpdate<T>[] {
  if (authoritative === null) return updates;
  return updates.filter((u) => !(u.isConfirmed?.(authoritative) ?? false));
}

/* -------------------------------------------------------------------------- */
/* Websocket transport helper                                                 */
/* -------------------------------------------------------------------------- */

export interface RealtimeSocketOptions {
  url: string;
  /** Called for every parsed event frame. */
  onEvent: (event: RealtimeEvent) => void;
  /** Called when the socket reconnects after a drop. */
  onReconnect?: () => void;
  /** Called on unrecoverable errors. */
  onError?: (error: Error) => void;
  /** Reconnect backoff base (ms). Defaults to 500. */
  backoffBase?: number;
  /** Max reconnect backoff (ms). Defaults to 15000. */
  backoffMax?: number;
}

export interface RealtimeSocket {
  close: () => void;
}

/**
 * Minimal reconnecting websocket wrapper. Parses JSON frames into
 * `RealtimeEvent` objects and forwards them to the store.
 */
export function connectRealtimeSocket(
  options: RealtimeSocketOptions,
): RealtimeSocket {
  const backoffBase = options.backoffBase ?? 500;
  const backoffMax = options.backoffMax ?? 15000;

  let socket: WebSocket | null = null;
  let closed = false;
  let attempt = 0;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let hasConnectedBefore = false;

  const scheduleReconnect = () => {
    if (closed) return;
    const delay = Math.min(backoffMax, backoffBase * 2 ** attempt);
    attempt += 1;
    reconnectTimer = setTimeout(open, delay);
  };

  const open = () => {
    if (closed) return;
    try {
      socket = new WebSocket(options.url);
    } catch (err) {
      options.onError?.(err instanceof Error ? err : new Error(String(err)));
      scheduleReconnect();
      return;
    }

    socket.onopen = () => {
      attempt = 0;
      if (hasConnectedBefore) {
        options.onReconnect?.();
      }
      hasConnectedBefore = true;
    };

    socket.onmessage = (msg) => {
      try {
        const parsed = JSON.parse(String(msg.data)) as RealtimeEvent;
        if (parsed && typeof parsed.seq === "number") {
          options.onEvent(parsed);
        }
      } catch (err) {
        options.onError?.(err instanceof Error ? err : new Error(String(err)));
      }
    };

    socket.onerror = () => {
      // onclose will handle reconnect.
    };

    socket.onclose = () => {
      socket = null;
      if (!closed) scheduleReconnect();
    };
  };

  open();

  return {
    close: () => {
      closed = true;
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      socket?.close();
      socket = null;
    },
  };
}