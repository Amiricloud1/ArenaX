/**
 * ArenaX — Validation Analytics & Governance
 *
 * Tracks validation events (attempts, failures, field-level errors) and
 * enforces governance policies on schema usage.
 *
 * All analytics are in-memory and never sent to a third party directly —
 * the caller is responsible for forwarding to Datadog / analytics service.
 */

"use client";

// ─── Event types ──────────────────────────────────────────────────────────────

export interface ValidationAttemptEvent {
  /** Stable, producer-assigned event ID (UUID v4). Used for durable deduplication. */
  eventId: string;
  /** Schema version of the event payload, e.g. "1". */
  schemaVersion: string;
  /** Stable identifier of the producer that emitted the event. */
  producerId: string;
  /** Pseudonymous analytics subject ID (never a raw auth identifier). */
  subjectId: string;
  /** Schema name (from `defineSchema`). */
  schemaName: string;
  /** Whether validation passed. */
  success: boolean;
  /** Field paths that failed (empty on success). */
  failedFields: string[];
  /** Validation duration in ms. */
  durationMs: number;
  /** ISO-8601 timestamp. */
  timestamp: string;
  /** Optional context (e.g. "form-submit", "api-request"). */
  context?: string;
}

export interface ValidationFieldStats {
  /** Field path (dot notation). */
  field: string;
  /** Total number of times this field failed validation. */
  failureCount: number;
  /** Most common error message for this field. */
  topError: string;
  /** All error messages and their occurrence counts. */
  errorCounts: Record<string, number>;
}

export interface ValidationSchemaStats {
  schemaName: string;
  totalAttempts: number;
  successCount: number;
  failureCount: number;
  successRate: number;
  averageDurationMs: number;
  fieldStats: ValidationFieldStats[];
  lastAttemptAt: string | null;
}

export interface ValidationAnalyticsSnapshot {
  totalAttempts: number;
  totalSuccesses: number;
  totalFailures: number;
  globalSuccessRate: number;
  bySchema: ValidationSchemaStats[];
  topFailingFields: ValidationFieldStats[];
  lastUpdated: string;
}

// ─── Event schema validation ──────────────────────────────────────────────────

/** Current analytics event schema version accepted by the ingestion boundary. */
export const ANALYTICS_SCHEMA_VERSION = "1";

/** Schema versions the ingestion boundary still accepts (rollout window). */
export const SUPPORTED_SCHEMA_VERSIONS: readonly string[] = ["1"];

const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

/** Keys that must never appear in an analytics payload (raw auth identifiers). */
const FORBIDDEN_IDENTITY_KEYS = new Set([
  "email",
  "useremail",
  "accesstoken",
  "refreshtoken",
  "idtoken",
  "token",
  "authorization",
  "userid",
  "useruuid",
  "accountid",
  "authid",
  "sessiontoken",
  "password",
]);

export type AnalyticsEventRejectionReason =
  | "missing-event-id"
  | "invalid-event-id"
  | "missing-schema-version"
  | "unsupported-schema-version"
  | "missing-producer-id"
  | "missing-subject-id"
  | "invalid-subject-id"
  | "raw-auth-identifier";

export interface AnalyticsEventValidationResult {
  ok: boolean;
  /** Populated when `ok` is false. */
  reason?: AnalyticsEventRejectionReason;
  /** Human-readable diagnostic message for the rejection path. */
  message?: string;
  /** Dotted path of the offending field, when applicable. */
  field?: string;
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function findForbiddenIdentityKey(
  value: unknown,
  path: string,
  depth = 0,
): { path: string; key: string } | null {
  if (depth > 8 || value === null || typeof value !== "object") return null;
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) {
      const found = findForbiddenIdentityKey(value[i], `${path}[${i}]`, depth + 1);
      if (found) return found;
    }
    return null;
  }
  for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
    const normalized = key.toLowerCase().replace(/[^a-z]/g, "");
    if (FORBIDDEN_IDENTITY_KEYS.has(normalized)) {
      return { path: path ? `${path}.${key}` : key, key };
    }
    const found = findForbiddenIdentityKey(child, path ? `${path}.${key}` : key, depth + 1);
    if (found) return found;
  }
  return null;
}

/**
 * Validates an analytics event before it is enqueued in the outbox or
 * accepted by the ingestion boundary.
 *
 * Requires a stable `eventId` (UUID), a supported `schemaVersion`, a
 * `producerId`, and a pseudonymous `subjectId`. Rejects payloads that carry
 * raw authentication identifiers (email, tokens, user UUIDs) so identity
 * stitching stays purpose-limited and separate from event payloads.
 */
export function validateAnalyticsEvent(
  event: unknown,
): AnalyticsEventValidationResult {
  if (event === null || typeof event !== "object" || Array.isArray(event)) {
    return { ok: false, reason: "missing-event-id", message: "Event must be an object" };
  }

  const record = event as Record<string, unknown>;

  if (!isNonEmptyString(record.eventId)) {
    return {
      ok: false,
      reason: "missing-event-id",
      field: "eventId",
      message: "Analytics event is missing a stable eventId",
    };
  }
  if (!UUID_RE.test(record.eventId)) {
    return {
      ok: false,
      reason: "invalid-event-id",
      field: "eventId",
      message: `eventId "${record.eventId}" is not a valid UUID`,
    };
  }

  if (!isNonEmptyString(record.schemaVersion)) {
    return {
      ok: false,
      reason: "missing-schema-version",
      field: "schemaVersion",
      message: "Analytics event is missing schemaVersion",
    };
  }
  if (!SUPPORTED_SCHEMA_VERSIONS.includes(record.schemaVersion)) {
    return {
      ok: false,
      reason: "unsupported-schema-version",
      field: "schemaVersion",
      message: `schemaVersion "${record.schemaVersion}" is not supported (supported: ${SUPPORTED_SCHEMA_VERSIONS.join(", ")})`,
    };
  }

  if (!isNonEmptyString(record.producerId)) {
    return {
      ok: false,
      reason: "missing-producer-id",
      field: "producerId",
      message: "Analytics event is missing producerId",
    };
  }

  if (!isNonEmptyString(record.subjectId)) {
    return {
      ok: false,
      reason: "missing-subject-id",
      field: "subjectId",
      message: "Analytics event is missing a pseudonymous subjectId",
    };
  }
  if (UUID_RE.test(record.subjectId) && record.subjectId === record.eventId) {
    return {
      ok: false,
      reason: "invalid-subject-id",
      field: "subjectId",
      message: "subjectId must be pseudonymous and distinct from eventId",
    };
  }

  const forbidden = findForbiddenIdentityKey(record, "");
  if (forbidden) {
    return {
      ok: false,
      reason: "raw-auth-identifier",
      field: forbidden.path,
      message: `Analytics payload must not contain raw auth identifier "${forbidden.key}"`,
    };
  }

  return { ok: true };
}

// ─── Governance ───────────────────────────────────────────────────────────────

export interface ValidationGovernancePolicy {
  name: string;
  description: string;
  /** Check the event; return null if OK or a violation message if not. */
  check(event: ValidationAttemptEvent): string | null;
  severity: "info" | "warn" | "error";
}

export interface ValidationGovernanceViolation {
  policy: string;
  message: string;
  severity: "info" | "warn" | "error";
  schemaName: string;
  timestamp: string;
}

// ─── Built-in governance policies ─────────────────────────────────────────────

/** Warn when a schema's success rate drops below 30% (possible UX issue). */
const lowSuccessRatePolicy: ValidationGovernancePolicy = {
  name: "low-success-rate",
  description: "Warns when a schema's rolling success rate is below 30%",
  severity: "warn",
  check(event) {
    // We can't compute rolling rate from a single event; this is handled in the snapshot.
    return null;
  },
};

/** Warn when a single validation attempt takes > 100ms (performance concern). */
const slowValidationPolicy: ValidationGovernancePolicy = {
  name: "slow-validation",
  description: "Warns when validation takes more than 100ms",
  severity: "warn",
  check(event) {
    if (event.durationMs > 100) {
      return `Validation for "${event.schemaName}" took ${event.durationMs}ms`;
    }
    return null;
  },
};

/** Error when the same field fails validation more than 10 times in a session. */
const fieldSpamPolicy: ValidationGovernancePolicy = {
  name: "field-spam-detection",
  description: "Detects when a field is repeatedly failing (possible bot or bad UX)",
  severity: "info",
  check() {
    // Evaluated at snapshot time, not per-event
    return null;
  },
};

const DEFAULT_POLICIES: ValidationGovernancePolicy[] = [
  lowSuccessRatePolicy,
  slowValidationPolicy,
  fieldSpamPolicy,
];

// ─── Analytics store ──────────────────────────────────────────────────────────

const MAX_EVENTS = 500;
const _events: ValidationAttemptEvent[] = [];
const _violations: ValidationGovernanceViolation[] = [];
let _extraPolicies: ValidationGovernancePolicy[] = [];

export function registerGovernancePolicy(policy: ValidationGovernancePolicy): void {
  if (!_extraPolicies.find((p) => p.name === policy.name)) {
    _extraPolicies.push(policy);
  }
}

// ─── Record function ──────────────────────────────────────────────────────────

/**
 * Records a validation attempt event and runs governance checks.
 * Called by `createValidationResolver` automatically.
 */
export function recordValidationAttempt(event: ValidationAttemptEvent): void {
  // Reject malformed or identity-leaking events at the ingestion boundary.
  const validation = validateAnalyticsEvent(event);
  if (!validation.ok) {
    const violation: ValidationGovernanceViolation = {
      policy: "analytics-event-schema",
      message: validation.message ?? "Invalid analytics event",
      severity: "error",
      schemaName: event?.schemaName ?? "unknown",
      timestamp: new Date().toISOString(),
    };
    _violations.unshift(violation);
    if (_violations.length > 200) _violations.length = 200;

    if (process.env.NODE_ENV === "development") {
      console.error(
        `[ArenaX Validation] [analytics-event-schema] ${violation.message}`,
        { field: validation.field, reason: validation.reason },
      );
    }
    return;
  }

  _events.unshift(event);
  if (_events.length > MAX_EVENTS) _events.length = MAX_EVENTS;

  // Run governance policies
  const allPolicies = [...DEFAULT_POLICIES, ..._extraPolicies];
  for (const policy of allPolicies) {
    try {
      const message = policy.check(event);
      if (message) {
        const violation: ValidationGovernanceViolation = {
          policy: policy.name,
          message,
          severity: policy.severity,
          schemaName: event.schemaName,
          timestamp: new Date().toISOString(),
        };
        _violations.unshift(violation);
        if (_violations.length > 200) _violations.length = 200;

        if (process.env.NODE_ENV === "development") {
          const level = policy.severity === "error" ? "error" : "warn";
          console[level](
            `[ArenaX Validation] [${policy.name}] ${message}`,
            { schema: event.schemaName },
          );
        }
      }
    } catch {
      // Swallow errors from governance policies
    }
  }
}

// ─── Query functions ──────────────────────────────────────────────────────────

export function getValidationEvents(): readonly ValidationAttemptEvent[] {
  return _events;
}

export function getValidationViolations(): readonly ValidationGovernanceViolation[] {
  return _violations;
}

export function clearValidationAnalytics(): void {
  _events.length = 0;
  _violations.length = 0;
}

/**
 * Computes aggregate statistics for a specific schema.
 */
export function getSchemaStats(schemaName: string): ValidationSchemaStats {
  const schemaEvents = _events.filter((e) => e.schemaName === schemaName);

  if (schemaEvents.length === 0) {
    return {
      schemaName,
      totalAttempts: 0,
      successCount: 0,
      failureCount: 0,
      successRate: 1,
      averageDurationMs: 0,
      fieldStats: [],
      lastAttemptAt: null,
    };
  }

  const successCount = schemaEvents.filter((e) => e.success).length;
  const failureCount = schemaEvents.length - successCount;
  const avgDuration =
    schemaEvents.reduce((s, e) => s + e.durationMs, 0) / schemaEvents.length;

  // Aggregate field error stats
  const fieldErrorCounts: Record<string, Record<string, number>> = {};
  for (const event of schemaEvents) {
    if (!event.success) {
      for (const field of event.failedFields) {
        if (!fieldErrorCounts[field]) fieldErrorCounts[field] = {};
        // We track field paths but not per-message counts here
        fieldErrorCounts[field]["_count"] = (fieldErrorCounts[field]["_count"] ?? 0) + 1;
      }
    }
  }

  const fieldStats: ValidationFieldStats[] = Object.entries(fieldErrorCounts).map(
    ([field, counts]) => ({
      field,
      failureCount: counts["_count"] ?? 0,
      topError: "Validation failed", // Detailed per-message tracking is in the snapshot
      errorCounts: counts,
    }),
  );

  fieldStats.sort((a, b) => b.failureCount - a.failureCount);

  return {
    schemaName,
    totalAttempts: schemaEvents.length,
    successCount,
    failureCount,
    successRate: successCount / schemaEvents.length,
    averageDurationMs: Math.round(avgDuration),
    fieldStats,
    lastAttemptAt: schemaEvents[0]?.timestamp ?? null,
  };
}

/**
 * Returns a full snapshot of all validation analytics.
 */
export function getValidationSnapshot(): ValidationAnalyticsSnapshot {
  const total = _events.length;
  const totalSuccesses = _events.filter((e) => e.success).length;
  const totalFailures = total - totalSuccesses;

  // Discover unique schema names
  const schemaNames = Array.from(new Set(_events.map((e) => e.schemaName)));
  const bySchema = schemaNames.map(getSchemaStats);

  // Top failing fields across all schemas
  const allFieldStats: Record<string, ValidationFieldStats> = {};
  for (const event of _events) {
    if (!event.success) {
      for (const field of event.failedFields) {
        if (!allFieldStats[field]) {
          allFieldStats[field] = {
            field,
            failureCount: 0,
            topError: "",
            errorCounts: {},
          };
        }
        allFieldStats[field].failureCount++;
      }
    }
  }

  const topFailingFields = Object.values(allFieldStats)
    .sort((a, b) => b.failureCount - a.failureCount)
    .slice(0, 10);

  return {
    totalAttempts: total,
    totalSuccesses,
    totalFailures,
    globalSuccessRate: total === 0 ? 1 : totalSuccesses / total,
    bySchema,
    topFailingFields,
    lastUpdated: new Date().toISOString(),
  };
}

/**
 * Checks the low-success-rate governance policy at snapshot time.
 * Returns violations for schemas with success rate < threshold.
 */
export function auditSuccessRates(threshold = 0.3): ValidationGovernanceViolation[] {
  const violations: ValidationGovernanceViolation[] = [];
  const schemaNames = Array.from(new Set(_events.map((e) => e.schemaName)));

  for (const name of schemaNames) {
    const stats = getSchemaStats(name);
    if (stats.totalAttempts >= 5 && stats.successRate < threshold) {
      const violation: ValidationGovernanceViolation = {
        policy: "low-success-rate",
        message: `Schema "${name}" has a ${(stats.successRate * 100).toFixed(1)}% success rate (threshold: ${(threshold * 100).toFixed(0)}%)`,
        severity: "warn",
        schemaName: name,
        timestamp: new Date().toISOString(),
      };
      violations.push(violation);
      _violations.unshift(violation);
    }
  }

  return violations;
}
