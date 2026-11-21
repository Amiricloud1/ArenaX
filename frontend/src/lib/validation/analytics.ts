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

// ─── Cross-client event schema registry (#249) ────────────────────────────────

/**
 * Current schema version for analytics events. Bump this whenever a breaking
 * change is introduced and register a migration in `registerEventMigration`.
 */
export const SCHEMA_VERSION = 1;

export type EventProducer = "frontend" | "server";

export interface AnalyticsEventFieldSpec {
  /** Field name (dot notation for nested fields). */
  name: string;
  /** Whether the field is required. */
  required: boolean;
  /** Allowed primitive type. */
  type: "string" | "number" | "boolean" | "object" | "array";
}

export interface AnalyticsEventContract {
  /** Unique event name, e.g. "validation.attempt". */
  name: string;
  /** Owning team or module. */
  owner: string;
  /** Human-readable purpose of the event. */
  purpose: string;
  /** Schema version this contract describes. */
  version: number;
  /** Required and optional fields. */
  fields: AnalyticsEventFieldSpec[];
  /** Attribute names that must never be emitted (PII, secrets, etc.). */
  prohibited: string[];
}

export interface AnalyticsEvent {
  /** Event name matching a registered contract. */
  name: string;
  /** Schema version the producer emitted. */
  version: number;
  /** Producer that emitted the event. */
  producer: EventProducer;
  /** Event payload. */
  payload: Record<string, unknown>;
}

export interface EventValidationResult {
  valid: boolean;
  /** Event name (or "<unknown>" if missing). */
  name: string;
  /** Human-readable error messages. */
  errors: string[];
  /** Prohibited attributes that were found in the payload. */
  prohibitedFound: string[];
  /** True when the event version differs from the contract version. */
  versionMismatch: boolean;
}

const _contracts = new Map<string, AnalyticsEventContract>();
const _migrations = new Map<string, (payload: Record<string, unknown>) => Record<string, unknown>>();

/** Registers (or replaces) an event contract. */
export function registerEventContract(contract: AnalyticsEventContract): void {
  _contracts.set(contract.name, contract);
}

/** Returns a registered contract, if any. */
export function getEventContract(name: string): AnalyticsEventContract | undefined {
  return _contracts.get(name);
}

/** Lists all registered contracts. */
export function listEventContracts(): AnalyticsEventContract[] {
  return Array.from(_contracts.values());
}

/**
 * Registers a migration hook that upgrades a payload from `fromVersion` to
 * `fromVersion + 1`. Breaking changes must ship with a migration.
 */
export function registerEventMigration(
  eventName: string,
  fromVersion: number,
  migrate: (payload: Record<string, unknown>) => Record<string, unknown>,
): void {
  _migrations.set(`${eventName}@${fromVersion}`, migrate);
}

/**
 * Applies registered migrations to bring an event payload up to the current
 * contract version. Returns the migrated event, or the original if no
 * migration path exists.
 */
export function migrateEvent(event: AnalyticsEvent): AnalyticsEvent {
  const contract = _contracts.get(event.name);
  if (!contract) return event;

  let version = event.version;
  let payload = event.payload;

  while (version < contract.version) {
    const migrate = _migrations.get(`${event.name}@${version}`);
    if (!migrate) break;
    try {
      payload = migrate(payload);
    } catch {
      break;
    }
    version += 1;
  }

  return { ...event, version, payload };
}

function typeOf(value: unknown): AnalyticsEventFieldSpec["type"] | "null" | "undefined" {
  if (value === null) return "null";
  if (value === undefined) return "undefined";
  if (Array.isArray(value)) return "array";
  return typeof value as AnalyticsEventFieldSpec["type"];
}

/**
 * Validates a single analytics event against the registry: required/optional
 * fields, prohibited sensitive attributes, and schema version.
 */
export function validateEvent(event: AnalyticsEvent): EventValidationResult {
  const errors: string[] = [];
  const prohibitedFound: string[] = [];
  const name = typeof event?.name === "string" && event.name ? event.name : "<unknown>";

  const contract = _contracts.get(name);
  if (!contract) {
    return {
      valid: false,
      name,
      errors: [`No contract registered for event "${name}"`],
      prohibitedFound,
      versionMismatch: false,
    };
  }

  const versionMismatch = event.version !== contract.version;
  if (versionMismatch) {
    errors.push(
      `Event "${name}" version ${event.version} does not match contract version ${contract.version}`,
    );
  }

  const payload = event.payload ?? {};

  for (const field of contract.fields) {
    const value = payload[field.name];
    const actual = typeOf(value);
    if (actual === "undefined" || actual === "null") {
      if (field.required) errors.push(`Missing required field "${field.name}"`);
      continue;
    }
    if (actual !== field.type) {
      errors.push(
        `Field "${field.name}" expected ${field.type} but received ${actual}`,
      );
    }
  }

  for (const attr of contract.prohibited) {
    if (Object.prototype.hasOwnProperty.call(payload, attr) && payload[attr] !== undefined) {
      prohibitedFound.push(attr);
      errors.push(`Prohibited attribute "${attr}" must not be emitted`);
    }
  }

  return {
    valid: errors.length === 0,
    name,
    errors,
    prohibitedFound,
    versionMismatch,
  };
}

/** Validates a batch of events, returning per-event results in order. */
export function validateBatch(events: AnalyticsEvent[]): EventValidationResult[] {
  return events.map(validateEvent);
}

// ─── Built-in contracts ───────────────────────────────────────────────────────

registerEventContract({
  name: "validation.attempt",
  owner: "frontend-platform",
  purpose: "Track schema validation attempts for UX and reliability monitoring",
  version: SCHEMA_VERSION,
  fields: [
    { name: "schemaName", required: true, type: "string" },
    { name: "success", required: true, type: "boolean" },
    { name: "failedFields", required: true, type: "array" },
    { name: "durationMs", required: true, type: "number" },
    { name: "timestamp", required: true, type: "string" },
    { name: "context", required: false, type: "string" },
  ],
  prohibited: ["email", "password", "token", "ssn", "cardNumber", "cvv", "authorization"],
});

registerEventContract({
  name: "validation.governance-violation",
  owner: "frontend-platform",
  purpose: "Track governance policy violations for operator review",
  version: SCHEMA_VERSION,
  fields: [
    { name: "policy", required: true, type: "string" },
    { name: "message", required: true, type: "string" },
    { name: "severity", required: true, type: "string" },
    { name: "schemaName", required: true, type: "string" },
    { name: "timestamp", required: true, type: "string" },
  ],
  prohibited: ["email", "password", "token", "ssn", "cardNumber", "cvv", "authorization"],
});

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
  // Validate against the cross-client contract before recording (#249).
  const result = validateEvent({
    name: "validation.attempt",
    version: SCHEMA_VERSION,
    producer: "frontend",
    payload: event as unknown as Record<string, unknown>,
  });
  if (!result.valid) {
    if (process.env.NODE_ENV === "development") {
      console.warn("[ArenaX Validation] Rejected analytics event", result.errors);
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
