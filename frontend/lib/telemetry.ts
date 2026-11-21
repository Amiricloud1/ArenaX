/**
 * Telemetry helpers for the TypeScript gateway.
 *
 * Responsibilities (issue #98):
 *  - Propagate W3C trace context (traceparent/tracestate) into the Rust backend.
 *  - Attach consistent service/resource attributes on every span.
 *  - Emit dependency spans (PostgreSQL, Redis, payment providers, Stellar RPC)
 *    with latency and failure status recorded separately from request spans.
 *  - Redact credentials, auth headers, payment secrets, and evidence contents
 *    from spans, logs, and Sentry/Datadog payloads.
 *
 * Sampling / cardinality notes:
 *  - Head sampling is driven by OTEL_TRACES_SAMPLER (default: parentbased_traceidratio).
 *  - Keep OTEL_TRACES_SAMPLER_ARG low in production (e.g. 0.05) to bound cost.
 *  - Never put unbounded values (user ids, wallet addresses, evidence hashes) in
 *    span attribute keys; use the fixed attribute set below.
 */

export const SERVICE_NAME = process.env.OTEL_SERVICE_NAME ?? "gateway";
export const SERVICE_VERSION =
  process.env.SERVICE_VERSION ?? process.env.GIT_SHA ?? "0.0.0";
export const DEPLOYMENT_ENVIRONMENT =
  process.env.DEPLOYMENT_ENVIRONMENT ?? process.env.NODE_ENV ?? "development";
export const SERVICE_INSTANCE_ID =
  process.env.SERVICE_INSTANCE_ID ??
  process.env.HOSTNAME ??
  `gateway-${process.pid}`;

/** Consistent resource attributes shared by gateway and backend. */
export const RESOURCE_ATTRIBUTES: Record<string, string> = {
  "service.name": SERVICE_NAME,
  "service.version": SERVICE_VERSION,
  "deployment.environment": DEPLOYMENT_ENVIRONMENT,
  "service.instance.id": SERVICE_INSTANCE_ID,
};

/** Dependencies we instrument with dedicated spans. */
export type DependencyKind =
  | "postgresql"
  | "redis"
  | "payment_provider"
  | "stellar_rpc";

const DEPENDENCY_SYSTEM: Record<DependencyKind, string> = {
  postgresql: "postgresql",
  redis: "redis",
  payment_provider: "payment",
  stellar_rpc: "stellar",
};

/**
 * Keys whose values must never leave the process. Matched case-insensitively
 * against attribute keys, header names, and log field names.
 */
const SENSITIVE_KEY_PATTERNS: RegExp[] = [
  /authorization/i,
  /cookie/i,
  /set-cookie/i,
  /x-api-key/i,
  /api[-_]?key/i,
  /secret/i,
  /password/i,
  /passwd/i,
  /token/i,
  /credential/i,
  /private[-_]?key/i,
  /mnemonic/i,
  /seed[-_]?phrase/i,
  /signature/i,
  /evidence/i,
  /payload/i,
  /body/i,
  // Payment details (issue #248): never let card data or amounts tied to a
  // payer leave the process through error reports.
  /card/i,
  /cvv/i,
  /cvc/i,
  /pan/i,
  /iban/i,
  /account[-_]?number/i,
  /routing/i,
  /payment/i,
  /billing/i,
  /amount/i,
  /wallet/i,
  /form/i,
  /field/i,
  /input/i,
  /value/i,
];

const REDACTED = "[REDACTED]";

/** True when a key name looks like it carries a secret or evidence content. */
export function isSensitiveKey(key: string): boolean {
  return SENSITIVE_KEY_PATTERNS.some((pattern) => pattern.test(key));
}

/**
 * Recursively redact sensitive fields from an object before it is attached to
 * a span, log line, or Sentry/Datadog payload. Returns a new object; the input
 * is never mutated.
 */
export function redact<T>(value: T, depth = 0): T {
  if (depth > 6 || value === null || typeof value !== "object") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => redact(item, depth + 1)) as unknown as T;
  }
  const out: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
    out[key] = isSensitiveKey(key) ? REDACTED : redact(entry, depth + 1);
  }
  return out as T;
}

/** Redact a set of HTTP headers, preserving names but masking sensitive values. */
export function redactHeaders(
  headers: Record<string, string | string[] | undefined>,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [name, value] of Object.entries(headers)) {
    if (value === undefined) continue;
    out[name] = isSensitiveKey(name) ? REDACTED : String(value);
  }
  return out;
}

/**
 * Safe, whitelisted context for client-side error reports (issue #248).
 *
 * Error boundaries must never forward raw error objects, props, form state, or
 * request payloads. Only these keys survive; everything else is dropped rather
 * than redacted so new fields cannot leak by default.
 */
export interface ErrorReportContext {
  route?: string;
  correlationId?: string;
  errorName?: string;
  digest?: string;
}

const SAFE_ERROR_CONTEXT_KEYS: ReadonlyArray<keyof ErrorReportContext> = [
  "route",
  "correlationId",
  "errorName",
  "digest",
];

/** Correlation IDs are opaque tokens; bound their length and charset. */
const CORRELATION_ID_PATTERN = /^[A-Za-z0-9._:-]{1,128}$/;

function sanitizeScalar(value: unknown, maxLength = 256): string | undefined {
  if (typeof value !== "string" && typeof value !== "number") return undefined;
  const text = String(value);
  if (text.length === 0) return undefined;
  return text.length > maxLength ? text.slice(0, maxLength) : text;
}

/**
 * Reduce arbitrary error-boundary context to the privacy-safe whitelist.
 * Unknown keys, nested objects, and non-scalar values are discarded.
 */
export function redactErrorContext(
  context: Record<string, unknown> = {},
): ErrorReportContext {
  const out: ErrorReportContext = {};
  for (const key of SAFE_ERROR_CONTEXT_KEYS) {
    const raw = context[key];
    if (key === "correlationId") {
      const id = sanitizeScalar(raw, 128);
      if (id && CORRELATION_ID_PATTERN.test(id)) out.correlationId = id;
      continue;
    }
    const value = sanitizeScalar(raw);
    if (value !== undefined) out[key] = value;
  }
  return out;
}

/**
 * Extract W3C trace context from an incoming request so the gateway can join
 * the caller's trace. Returns an empty object when no valid context is present.
 */
export function extractTraceContext(
  headers: Record<string, string | string[] | undefined>,
): Record<string, string> {
  const ctx: Record<string, string> = {};
  const traceparent = headers["traceparent"];
  const tracestate = headers["tracestate"];
  if (typeof traceparent === "string" && traceparent.length > 0) {
    ctx.traceparent = traceparent;
  }
  if (typeof tracestate === "string" && tracestate.length > 0) {
    ctx.tracestate = tracestate;
  }
  return ctx;
}

/**
 * Build the headers used to propagate trace context into the Rust backend.
 * The backend reads traceparent/tracestate to continue the same trace.
 */
export function buildPropagationHeaders(
  traceContext: Record<string, string>,
): Record<string, string> {
  const headers: Record<string, string> = {};
  if (traceContext.traceparent) headers["traceparent"] = traceContext.traceparent;
  if (traceContext.tracestate) headers["tracestate"] = traceContext.tracestate;
  return headers;
}

/** Minimal span surface we depend on (compatible with OTel JS API). */
export interface SpanLike {
  setAttribute(key: string, value: string | number | boolean): void;
  setStatus(status: { code: number; message?: string }): void;
  recordException(error: unknown): void;
  end(): void;
}

export interface TracerLike {
  startSpan(
    name: string,
    options?: { attributes?: Record<string, string | number | boolean> },
  ): SpanLike;
}

const SPAN_STATUS_ERROR = 2;

/**
 * Wrap a dependency call in its own span so latency and failure are measurable
 * separately from the enclosing request span.
 */
export async function withDependencySpan<T>(
  tracer: TracerLike,
  kind: DependencyKind,
  operation: string,
  fn: () => Promise<T>,
): Promise<T> {
  const span = tracer.startSpan(`${DEPENDENCY_SYSTEM[kind]}.${operation}`, {
    attributes: {
      ...RESOURCE_ATTRIBUTES,
      "span.kind": "client",
      "peer.service": DEPENDENCY_SYSTEM[kind],
      "db.system": kind === "postgresql" ? "postgresql" : undefined,
      "messaging.system": kind === "redis" ? "redis" : undefined,
    } as Record<string, string | number | boolean>,
  });
  const startedAt = Date.now();
  try {
    const result = await fn();
    span.setAttribute("dependency.success", true);
    return result;
  } catch (error) {
    span.setAttribute("dependency.success", false);
    span.setAttribute(
      "dependency.error.type",
      error instanceof Error ? error.name : "unknown",
    );
    span.setStatus({ code: SPAN_STATUS_ERROR });
    span.recordException(redact(error instanceof Error ? error.message : error));
    throw error;
  } finally {
    span.setAttribute("dependency.duration_ms", Date.now() - startedAt);
    span.end();
  }
}

/**
 * Build a Sentry/Datadog-safe event payload: resource attributes plus redacted
 * context. Never include raw headers, bodies, or evidence contents.
 */
export function buildTelemetryEvent(
  name: string,
  context: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    ...RESOURCE_ATTRIBUTES,
    event: name,
    context: redact(context),
  };
}

/**
 * Build a Sentry/Datadog-safe error event for a client error boundary.
 * Only the whitelisted {@link ErrorReportContext} fields are included, so
 * tokens, payment details, and raw form contents can never be attached.
 */
export function buildErrorTelemetryEvent(
  name: string,
  context: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    ...RESOURCE_ATTRIBUTES,
    event: name,
    context: redactErrorContext(context),
  };
}
