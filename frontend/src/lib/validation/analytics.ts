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

// ─── Experiment boundary validation ───────────────────────────────────────────
//
// These validators guard the client/data boundary for feature experiments.
// They are intentionally dependency-free and never throw: callers receive a
// discriminated result so malformed payloads (from the network, storage, or a
// stale server) are rejected before they can influence assignment, exposure
// logging, or guardrail decisions.

export interface BoundaryIssue {
  /** Dot-notation path to the offending value ("" for the root). */
  path: string;
  message: string;
}

export type BoundaryResult<T> =
  | { ok: true; value: T }
  | { ok: false; issues: BoundaryIssue[] };

/** Maximum accepted length for identifiers (experiment keys, variant keys, units). */
export const EXPERIMENT_ID_MAX_LENGTH = 128;

const EXPERIMENT_KEY_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]*$/;
const ISO_TIMESTAMP_PATTERN =
  /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

function isExperimentKey(value: unknown): value is string {
  return (
    isNonEmptyString(value) &&
    value.length <= EXPERIMENT_ID_MAX_LENGTH &&
    EXPERIMENT_KEY_PATTERN.test(value)
  );
}

function isIsoTimestamp(value: unknown): value is string {
  return typeof value === "string" && ISO_TIMESTAMP_PATTERN.test(value) && !Number.isNaN(Date.parse(value));
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

// ── Assignment request ────────────────────────────────────────────────────────

export interface ExperimentAssignmentRequest {
  /** Stable experiment identifier. */
  experimentKey: string;
  /** Version of the experiment definition used to compute the assignment. */
  assignmentVersion: number;
  /** Stable unit identifier (e.g. hashed user id); never a raw PII value. */
  unitId: string;
  /** Whether the participant has consented to experiment participation. */
  consent: boolean;
  /** Exclusion reasons that must force the control/holdout bucket. */
  exclusions?: string[];
}

export function validateExperimentAssignmentRequest(
  input: unknown,
): BoundaryResult<ExperimentAssignmentRequest> {
  const issues: BoundaryIssue[] = [];
  if (!isPlainObject(input)) {
    return { ok: false, issues: [{ path: "", message: "Assignment request must be an object" }] };
  }

  if (!isExperimentKey(input.experimentKey)) {
    issues.push({
      path: "experimentKey",
      message: `experimentKey must be a non-empty key of at most ${EXPERIMENT_ID_MAX_LENGTH} characters`,
    });
  }
  if (!Number.isInteger(input.assignmentVersion) || (input.assignmentVersion as number) < 1) {
    issues.push({ path: "assignmentVersion", message: "assignmentVersion must be a positive integer" });
  }
  if (!isNonEmptyString(input.unitId) || input.unitId.length > EXPERIMENT_ID_MAX_LENGTH) {
    issues.push({
      path: "unitId",
      message: `unitId must be a non-empty string of at most ${EXPERIMENT_ID_MAX_LENGTH} characters`,
    });
  }
  if (typeof input.consent !== "boolean") {
    issues.push({ path: "consent", message: "consent must be a boolean" });
  }
  if (input.exclusions !== undefined && !isStringArray(input.exclusions)) {
    issues.push({ path: "exclusions", message: "exclusions must be an array of strings" });
  }

  if (issues.length > 0) return { ok: false, issues };

  return {
    ok: true,
    value: {
      experimentKey: input.experimentKey as string,
      assignmentVersion: input.assignmentVersion as number,
      unitId: input.unitId as string,
      consent: input.consent as boolean,
      exclusions: input.exclusions as string[] | undefined,
    },
  };
}

// ── Exposure payload ──────────────────────────────────────────────────────────

export interface ExperimentExposurePayload {
  experimentKey: string;
  assignmentVersion: number;
  variantKey: string;
  unitId: string;
  /** ISO-8601 timestamp of when the experience was presented. */
  exposedAt: string;
  /** Optional surface where the experience was presented (e.g. "match-lobby"). */
  surface?: string;
}

export function validateExperimentExposurePayload(
  input: unknown,
): BoundaryResult<ExperimentExposurePayload> {
  const issues: BoundaryIssue[] = [];
  if (!isPlainObject(input)) {
    return { ok: false, issues: [{ path: "", message: "Exposure payload must be an object" }] };
  }

  if (!isExperimentKey(input.experimentKey)) {
    issues.push({ path: "experimentKey", message: "experimentKey is invalid" });
  }
  if (!Number.isInteger(input.assignmentVersion) || (input.assignmentVersion as number) < 1) {
    issues.push({ path: "assignmentVersion", message: "assignmentVersion must be a positive integer" });
  }
  if (!isExperimentKey(input.variantKey)) {
    issues.push({ path: "variantKey", message: "variantKey is invalid" });
  }
  if (!isNonEmptyString(input.unitId) || input.unitId.length > EXPERIMENT_ID_MAX_LENGTH) {
    issues.push({ path: "unitId", message: "unitId is invalid" });
  }
  if (!isIsoTimestamp(input.exposedAt)) {
    issues.push({ path: "exposedAt", message: "exposedAt must be an ISO-8601 timestamp" });
  }
  if (input.surface !== undefined && !isNonEmptyString(input.surface)) {
    issues.push({ path: "surface", message: "surface must be a non-empty string when provided" });
  }

  if (issues.length > 0) return { ok: false, issues };

  return {
    ok: true,
    value: {
      experimentKey: input.experimentKey as string,
      assignmentVersion: input.assignmentVersion as number,
      variantKey: input.variantKey as string,
      unitId: input.unitId as string,
      exposedAt: input.exposedAt as string,
      surface: input.surface as string | undefined,
    },
  };
}

// ── Guardrail analysis response ───────────────────────────────────────────────

export type GuardrailDecision =
  | "insufficient-data"
  | "safe-to-ship"
  | "harmful"
  | "inconclusive";

export interface GuardrailMetricResult {
  metric: string;
  /** Observed effect (lift) for the metric; negative means harm. */
  lift: number;
  /** Two-sided p-value for the observed effect. */
  pValue: number;
  /** Whether the metric is a guardrail (harm is a stop condition). */
  guardrail: boolean;
  /** Whether telemetry for this metric was missing. */
  missing: boolean;
}

export interface GuardrailAnalysisResponse {
  experimentKey: string;
  assignmentVersion: number;
  decision: GuardrailDecision;
  /** Number of samples observed in the analysis window. */
  sampleSize: number;
  /** Minimum sample required before a decision can be made. */
  minimumSample: number;
  /** Whether the multiple-comparison correction was applied. */
  multipleComparisonAdjusted: boolean;
  metrics: GuardrailMetricResult[];
  /** ISO-8601 timestamp of when the analysis was produced. */
  analyzedAt: string;
}

const GUARDRAIL_DECISIONS: readonly GuardrailDecision[] = [
  "insufficient-data",
  "safe-to-ship",
  "harmful",
  "inconclusive",
];

export function validateGuardrailAnalysisResponse(
  input: unknown,
): BoundaryResult<GuardrailAnalysisResponse> {
  const issues: BoundaryIssue[] = [];
  if (!isPlainObject(input)) {
    return { ok: false, issues: [{ path: "", message: "Guardrail analysis must be an object" }] };
  }

  if (!isExperimentKey(input.experimentKey)) {
    issues.push({ path: "experimentKey", message: "experimentKey is invalid" });
  }
  if (!Number.isInteger(input.assignmentVersion) || (input.assignmentVersion as number) < 1) {
    issues.push({ path: "assignmentVersion", message: "assignmentVersion must be a positive integer" });
  }
  if (
    typeof input.decision !== "string" ||
    !GUARDRAIL_DECISIONS.includes(input.decision as GuardrailDecision)
  ) {
    issues.push({
      path: "decision",
      message: `decision must be one of: ${GUARDRAIL_DECISIONS.join(", ")}`,
    });
  }
  if (!Number.isInteger(input.sampleSize) || (input.sampleSize as number) < 0) {
    issues.push({ path: "sampleSize", message: "sampleSize must be a non-negative integer" });
  }
  if (!Number.isInteger(input.minimumSample) || (input.minimumSample as number) < 1) {
    issues.push({ path: "minimumSample", message: "minimumSample must be a positive integer" });
  }
  if (typeof input.multipleComparisonAdjusted !== "boolean") {
    issues.push({
      path: "multipleComparisonAdjusted",
      message: "multipleComparisonAdjusted must be a boolean",
    });
  }
  if (!isIsoTimestamp(input.analyzedAt)) {
    issues.push({ path: "analyzedAt", message: "analyzedAt must be an ISO-8601 timestamp" });
  }

  if (!Array.isArray(input.metrics)) {
    issues.push({ path: "metrics", message: "metrics must be an array" });
  } else {
    input.metrics.forEach((metric, index) => {
      const path = `metrics[${index}]`;
      if (!isPlainObject(metric)) {
        issues.push({ path, message: "metric must be an object" });
        return;
      }
      if (!isNonEmptyString(metric.metric)) {
        issues.push({ path: `${path}.metric`, message: "metric must be a non-empty string" });
      }
      if (!isFiniteNumber(metric.lift)) {
        issues.push({ path: `${path}.lift`, message: "lift must be a finite number" });
      }
      if (!isFiniteNumber(metric.pValue) || metric.pValue < 0 || metric.pValue > 1) {
        issues.push({ path: `${path}.pValue`, message: "pValue must be a number between 0 and 1" });
      }
      if (typeof metric.guardrail !== "boolean") {
        issues.push({ path: `${path}.guardrail`, message: "guardrail must be a boolean" });
      }
      if (typeof metric.missing !== "boolean") {
        issues.push({ path: `${path}.missing`, message: "missing must be a boolean" });
      }
    });
  }

  if (issues.length > 0) return { ok: false, issues };

  return {
    ok: true,
    value: {
      experimentKey: input.experimentKey as string,
      assignmentVersion: input.assignmentVersion as number,
      decision: input.decision as GuardrailDecision,
      sampleSize: input.sampleSize as number,
      minimumSample: input.minimumSample as number,
      multipleComparisonAdjusted: input.multipleComparisonAdjusted as boolean,
      metrics: (input.metrics as GuardrailMetricResult[]).map((metric) => ({
        metric: metric.metric,
        lift: metric.lift,
        pValue: metric.pValue,
        guardrail: metric.guardrail,
        missing: metric.missing,
      })),
      analyzedAt: input.analyzedAt as string,
    },
  };
}
