/**
 * ArenaX — Standardized Response Types
 *
 * Defines the canonical shapes for all API responses flowing through the
 * response interceptor pipeline. Every endpoint response is normalized to
 * one of these envelopes before reaching the caller.
 */

// ─── Status ───────────────────────────────────────────────────────────────────

export type ResponseStatus = "success" | "error" | "partial";

// ─── Standard envelope ────────────────────────────────────────────────────────

/**
 * The canonical response envelope returned by the interceptor for every
 * successful API call.
 */
export interface StandardResponse<T = unknown> {
  /** Whether the request succeeded. */
  success: boolean;
  /** Normalized status discriminant. */
  status: ResponseStatus;
  /** The unwrapped, transformed payload. */
  data: T;
  /** Optional human-readable message from the server. */
  message?: string;
  /** Request-scoped metadata injected by the interceptor. */
  meta: ResponseMeta;
  /**
   * Optional realtime metadata. Present on snapshot responses that are
   * backed by an event stream, so clients can bootstrap a consistent view
   * and then apply subsequent events from the declared sequence window.
   */
  realtime?: RealtimeSnapshotMeta;
}

/**
 * Realtime bootstrap metadata attached to snapshot responses.
 *
 * A snapshot response declares the stream it belongs to and the sequence
 * watermark up to which its payload is authoritative. Clients must:
 *  - discard any event whose `seq` is `<= watermark` (already reflected),
 *  - detect gaps when an event's `seq` is greater than `watermark + 1`
 *    (or the last applied sequence) and request a replacement snapshot,
 *  - reject events from a different `streamId` or an older `snapshotVersion`.
 */
export interface RealtimeSnapshotMeta {
  /** Identifier of the event stream this snapshot belongs to. */
  streamId: string;
  /**
   * Sequence number of the last event already reflected in `data`.
   * Subsequent events with `seq > watermark` may be applied on top.
   */
  watermark: number;
  /**
   * Convenience alias for `watermark`; the stream sequence from which
   * subsequent events can be applied. When both are present they must match.
   */
  streamSeq?: number;
  /**
   * Monotonic version of the snapshot itself. A snapshot with a lower
   * version than one already applied must be rejected as stale.
   */
  snapshotVersion?: number;
  /** ISO-8601 timestamp when the snapshot was generated server-side. */
  generatedAt?: string;
  /**
   * How long (ms) the server retains events for replay from `watermark`.
   * Clients that reconnect after this window must request a fresh snapshot.
   */
  retentionMs?: number;
}

/**
 * Metadata attached to every intercepted response.
 */
export interface ResponseMeta {
  /** ISO-8601 timestamp when the response was received client-side. */
  receivedAt: string;
  /** Round-trip latency in milliseconds. */
  latencyMs: number;
  /** HTTP status code. */
  statusCode: number;
  /** Full request URL (origin stripped for security). */
  requestPath: string;
  /** HTTP method (GET, POST, …). */
  requestMethod: string;
  /** Unique trace ID for correlating logs (generated client-side if absent). */
  traceId: string;
  /** Number of retry attempts made before this response was received. */
  retries: number;
  /** Whether the response was served from the in-flight dedup cache. */
  cached: boolean;
  /** Applied transformation pipeline stages (for debugging). */
  transformations: string[];
}

// ─── Paginated envelope ───────────────────────────────────────────────────────

/**
 * Paginated variant of the standard envelope.
 */
export interface PaginatedStandardResponse<T = unknown>
  extends StandardResponse<T[]> {
  pagination: PaginationMeta;
}

export interface PaginationMeta {
  page: number;
  limit: number;
  total: number;
  totalPages: number;
  hasNextPage: boolean;
  hasPrevPage: boolean;
}

// ─── Error envelope ───────────────────────────────────────────────────────────

/**
 * Standardized error response shape surfaced to callers when a request fails
 * after all retries are exhausted.
 */
export interface StandardErrorResponse {
  success: false;
  status: "error";
  error: {
    /** Machine-readable error code (e.g. "VALIDATION_ERROR"). */
    code: string;
    /** Human-readable message. */
    message: string;
    /** The field that caused the error for validation failures. */
    field?: string;
    /** HTTP status code. */
    statusCode?: number;
  };
  meta: ResponseMeta;
}

// ─── Interceptor configuration ────────────────────────────────────────────────

export interface InterceptorConfig {
  /**
   * Enable camelCase key normalization for snake_case API responses.
   * Default: true
   */
  normalizeCasing?: boolean;
  /**
   * Auto-parse ISO-8601 date strings into `Date` objects.
   * Default: false (kept as strings to avoid hydration mismatches in SSR)
   */
  parseDates?: boolean;
  /**
   * Strip null / undefined leaf values from the payload before returning.
   * Default: false
   */
  stripNulls?: boolean;
  /**
   * Maximum age (ms) of a cached in-flight response to accept.
   * Default: 0 (no client-side response caching beyond TanStack Query)
   */
  maxCacheAgeMs?: number;
  /**
   * Emit response analytics events. Disable in tests / benchmarks.
   * Default: true
   */
  analyticsEnabled?: boolean;
  /**
   * Log slow responses (latency > threshold) as warnings.
   * Default: 2000 (ms)
   */
  slowResponseThresholdMs?: number;
}

// ─── Analytics event shapes ───────────────────────────────────────────────────

export interface ResponseAnalyticsEvent {
  traceId: string;
  endpoint: string;
  method: string;
  statusCode: number;
  latencyMs: number;
  success: boolean;
  retries: number;
  cached: boolean;
  timestamp: number;
  /** Size of the serialized response body in bytes (approximate). */
  payloadBytes?: number;
  /** Whether the response was considered "slow". */
  slow: boolean;
}

// ─── Monitoring snapshot ──────────────────────────────────────────────────────

export interface ResponseMonitorSnapshot {
  totalRequests: number;
  successCount: number;
  errorCount: number;
  cachedCount: number;
  successRate: number;
  averageLatencyMs: number;
  p50LatencyMs: number;
  p95LatencyMs: number;
  p99LatencyMs: number;
  slowRequestCount: number;
  slowRequestRate: number;
  errorsByCode: Record<string, number>;
  latencyByEndpoint: Record<string, number>;
  lastUpdated: number;
}

// ─── Governance ───────────────────────────────────────────────────────────────

export type GovernanceViolationSeverity = "warn" | "error" | "critical";

export interface GovernanceViolation {
  rule: string;
  message: string;
  severity: GovernanceViolationSeverity;
  endpoint: string;
  traceId: string;
  timestamp: number;
}

export interface GovernancePolicy {
  /** Policy name (for reporting). */
  name: string;
  /**
   * Validate a response. Return `null` when valid, or a violation message
   * when the policy is breached.
   */
  validate(response: StandardResponse, meta: ResponseMeta): string | null;
  severity: GovernanceViolationSeverity;
}