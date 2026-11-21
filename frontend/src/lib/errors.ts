// ─── Error Severity & Category ───────────────────────────────────────────────

export enum ErrorSeverity {
  LOW = "low",
  MEDIUM = "medium",
  HIGH = "high",
  CRITICAL = "critical",
}

export enum ErrorCategory {
  NETWORK = "network",
  AUTHENTICATION = "authentication",
  VALIDATION = "validation",
  RUNTIME = "runtime",
  API = "api",
  UNKNOWN = "unknown",
}

// ─── Logged Error Shape ───────────────────────────────────────────────────────

export interface LoggedError {
  id: string;
  timestamp: number;
  message: string;
  stack?: string;
  category: ErrorCategory;
  severity: ErrorSeverity;
  metadata?: Record<string, unknown>;
  /** How many times an automatic recovery was attempted */
  recoveryAttempts?: number;
  /** Whether an automatic recovery succeeded */
  recovered?: boolean;
}

// ─── Custom Error Classes ─────────────────────────────────────────────────────

/**
 * Base application error.  All ArenaX-specific errors should extend this class
 * so that error boundaries and the logger can extract structured metadata.
 */
export class ArenaXError extends Error {
  public category: ErrorCategory;
  public severity: ErrorSeverity;
  public metadata?: Record<string, unknown>;

  constructor(
    message: string,
    category: ErrorCategory = ErrorCategory.UNKNOWN,
    severity: ErrorSeverity = ErrorSeverity.MEDIUM,
    metadata?: Record<string, unknown>,
  ) {
    super(message);
    this.name = "ArenaXError";
    this.category = category;
    this.severity = severity;
    this.metadata = metadata;

    if ((Error as { captureStackTrace?: (t: unknown, c: unknown) => void }).captureStackTrace) {
      (Error as { captureStackTrace: (t: unknown, c: unknown) => void }).captureStackTrace(this, ArenaXError);
    }
  }
}

/** Thrown when a network request fails or times out. */
export class NetworkError extends ArenaXError {
  constructor(message: string, metadata?: Record<string, unknown>) {
    super(message, ErrorCategory.NETWORK, ErrorSeverity.HIGH, metadata);
    this.name = "NetworkError";
  }
}

/** Thrown when the user is not authenticated or a token has expired. */
export class AuthenticationError extends ArenaXError {
  constructor(message: string, metadata?: Record<string, unknown>) {
    super(message, ErrorCategory.AUTHENTICATION, ErrorSeverity.HIGH, metadata);
    this.name = "AuthenticationError";
  }
}

/** Thrown when user-supplied data fails validation. */
export class ValidationError extends ArenaXError {
  public field?: string;
  /** Server-supplied error code (e.g. `EMAIL_INVALID`) when available. */
  public code?: string;

  constructor(
    message: string,
    field?: string,
    metadata?: Record<string, unknown>,
  ) {
    super(message, ErrorCategory.VALIDATION, ErrorSeverity.LOW, metadata);
    this.name = "ValidationError";
    this.field = field;
  }
}

/** Thrown when an API endpoint returns an error response. */
export class ApiError extends ArenaXError {
  public statusCode?: number;

  constructor(
    message: string,
    statusCode?: number,
    metadata?: Record<string, unknown>,
  ) {
    const severity =
      statusCode && statusCode >= 500 ? ErrorSeverity.HIGH : ErrorSeverity.MEDIUM;
    super(message, ErrorCategory.API, severity, metadata);
    this.name = "ApiError";
    this.statusCode = statusCode;
  }
}

// ─── Contract error mapping ───────────────────────────────────────────────────

/**
 * How a client should react to a given contract error.
 * - `retry`: safe to retry automatically (transient / duplicate delivery).
 * - `manual`: requires an operator or user action before retrying.
 * - `fatal`: do not retry; surface to the user and escalate.
 */
export type ContractRecoveryClass = "retry" | "manual" | "fatal";

export interface ContractErrorMapping {
  /** Stable machine-readable code (never changes once published). */
  code: string;
  /** Safe, user-readable message. Never contains signing material. */
  message: string;
  /** Documented retry / manual-recovery classification. */
  recovery: ContractRecoveryClass;
  /** Whether the client should invalidate cached server state. */
  invalidateCache: boolean;
}

/** Code used for any error not present in the published mapping table. */
export const UNKNOWN_CONTRACT_ERROR_CODE = "CONTRACT_ERROR_UNKNOWN";

/**
 * Canonical mapping of every public contract error to a stable code, a safe
 * message and a documented recovery classification. Keep this table in sync
 * with the Soroban contract error enum and the backend adapter.
 */
export const CONTRACT_ERROR_MAP: Readonly<Record<string, ContractErrorMapping>> = Object.freeze({
  // Authorization / identity
  UNAUTHORIZED: {
    code: "CONTRACT_UNAUTHORIZED",
    message: "You are not authorized to perform this action.",
    recovery: "manual",
    invalidateCache: true,
  },
  FORBIDDEN: {
    code: "CONTRACT_FORBIDDEN",
    message: "Your role does not allow this action.",
    recovery: "manual",
    invalidateCache: true,
  },
  // Finality / transaction lifecycle
  TX_ALREADY_FINALIZED: {
    code: "CONTRACT_TX_ALREADY_FINALIZED",
    message: "This transaction has already been finalized and cannot be changed.",
    recovery: "fatal",
    invalidateCache: true,
  },
  TX_NOT_FINALIZED: {
    code: "CONTRACT_TX_NOT_FINALIZED",
    message: "This transaction is not finalized yet. Please try again shortly.",
    recovery: "retry",
    invalidateCache: false,
  },
  DUPLICATE_DELIVERY: {
    code: "CONTRACT_DUPLICATE_DELIVERY",
    message: "This request was already processed. No further action is needed.",
    recovery: "fatal",
    invalidateCache: true,
  },
  // Validation
  INVALID_AMOUNT: {
    code: "CONTRACT_INVALID_AMOUNT",
    message: "The amount provided is invalid.",
    recovery: "manual",
    invalidateCache: false,
  },
  INSUFFICIENT_BALANCE: {
    code: "CONTRACT_INSUFFICIENT_BALANCE",
    message: "Insufficient balance to complete this transaction.",
    recovery: "manual",
    invalidateCache: true,
  },
  // Transient
  NETWORK_UNAVAILABLE: {
    code: "CONTRACT_NETWORK_UNAVAILABLE",
    message: "The network is temporarily unavailable. Please retry.",
    recovery: "retry",
    invalidateCache: false,
  },
  TIMEOUT: {
    code: "CONTRACT_TIMEOUT",
    message: "The request timed out. Please retry.",
    recovery: "retry",
    invalidateCache: false,
  },
});

const UNKNOWN_CONTRACT_ERROR_MAPPING: ContractErrorMapping = Object.freeze({
  code: UNKNOWN_CONTRACT_ERROR_CODE,
  message: "An unexpected error occurred. Please contact support with the reference ID.",
  recovery: "manual",
  invalidateCache: false,
});

/**
 * Maps a raw contract error code to its safe, user-readable mapping.
 * Unknown codes remain distinguishable via `UNKNOWN_CONTRACT_ERROR_CODE`.
 */
export function mapContractError(rawCode: string | undefined | null): ContractErrorMapping {
  if (!rawCode) return UNKNOWN_CONTRACT_ERROR_MAPPING;
  return CONTRACT_ERROR_MAP[rawCode] ?? UNKNOWN_CONTRACT_ERROR_MAPPING;
}

/** Returns true when the raw code is not part of the published mapping table. */
export function isUnknownContractError(rawCode: string | undefined | null): boolean {
  return !rawCode || !(rawCode in CONTRACT_ERROR_MAP);
}

/**
 * Builds a log-safe payload for a contract error. Includes transaction context
 * but never signing material (signatures, secret keys, XDR envelopes).
 */
export function buildContractErrorLogContext(
  rawCode: string | undefined | null,
  context: Record<string, unknown> = {},
): Record<string, unknown> {
  const mapping = mapContractError(rawCode);
  const safeContext: Record<string, unknown> = {};
  const REDACTED_KEYS = ["signature", "signedXdr", "secretKey", "privateKey", "seed", "signer"];

  for (const [key, value] of Object.entries(context)) {
    if (REDACTED_KEYS.some((k) => key.toLowerCase().includes(k.toLowerCase()))) {
      safeContext[key] = "[REDACTED]";
    } else {
      safeContext[key] = value;
    }
  }

  return {
    rawCode: rawCode ?? null,
    code: mapping.code,
    recovery: mapping.recovery,
    unknown: isUnknownContractError(rawCode),
    context: safeContext,
  };
}

// ─── Recovery strategies ──────────────────────────────────────────────────────

/**
 * Describes how the error recovery system should react to a given error.
 */
export interface RecoveryStrategy {
  /** Maximum number of automatic retry attempts (default: 3). */
  maxAttempts: number;
  /** Base delay in milliseconds between retries (default: 1 000 ms). */
  baseDelayMs: number;
  /** Whether to apply exponential back-off between retries (default: true). */
  exponentialBackoff: boolean;
  /** Maximum delay cap in milliseconds (default: 30 000 ms). */
  maxDelayMs: number;
}

export const DEFAULT_RECOVERY_STRATEGY: RecoveryStrategy = {
  maxAttempts: 3,
  baseDelayMs: 1_000,
  exponentialBackoff: true,
  maxDelayMs: 30_000,
};

/**
 * Returns the recommended recovery strategy for a given error category.
 * Network and API errors are retryable; auth/validation errors are not.
 */
export function getRecoveryStrategy(category: ErrorCategory): RecoveryStrategy | null {
  switch (category) {
    case ErrorCategory.NETWORK:
      return { maxAttempts: 3, baseDelayMs: 1_000, exponentialBackoff: true, maxDelayMs: 15_000 };
    case ErrorCategory.API:
      return { maxAttempts: 2, baseDelayMs: 2_000, exponentialBackoff: true, maxDelayMs: 10_000 };
    case ErrorCategory.RUNTIME:
      return { maxAttempts: 1, baseDelayMs: 500, exponentialBackoff: false, maxDelayMs: 500 };
    // Authentication and Validation errors should not be auto-retried
    case ErrorCategory.AUTHENTICATION:
    case ErrorCategory.VALIDATION:
    case ErrorCategory.UNKNOWN:
    default:
      return null;
  }
}

// ─── Utility Functions ────────────────────────────────────────────────────────

export function determineErrorCategory(error: Error): ErrorCategory {
  if (error instanceof ArenaXError) return error.category;

  const msg = error.message.toLowerCase();

  if (msg.includes("network") || msg.includes("fetch") || msg.includes("timeout")) {
    return ErrorCategory.NETWORK;
  }
  if (
    msg.includes("unauthorized") ||
    msg.includes("forbidden") ||
    msg.includes("authentication") ||
    msg.includes("401") ||
    msg.includes("403")
  ) {
    return ErrorCategory.AUTHENTICATION;
  }
  if (msg.includes("validation") || msg.includes("invalid")) {
    return ErrorCategory.VALIDATION;
  }
  if (msg.includes("api") || msg.includes("server") || msg.includes("500")) {
    return ErrorCategory.API;
  }

  return ErrorCategory.UNKNOWN;
}

export function determineErrorSeverity(error: Error): ErrorSeverity {
  if (error instanceof ArenaXError) return error.severity;

  const msg = error.message.toLowerCase();

  if (msg.includes("critical") || msg.includes("fatal")) return ErrorSeverity.CRITICAL;
  if (msg.includes("network") || msg.includes("timeout")) return ErrorSeverity.HIGH;

  return ErrorSeverity.MEDIUM;
}

/**
 * Returns whether an error should be considered retryable based on its category.
 */
export function isRetryableError(error: Error): boolean {
  const category = determineErrorCategory(error);
  return getRecoveryStrategy(category) !== null;
}

/**
 * Whether an error should invalidate cached server state (e.g. after an
 * authorization change or a finality conflict).
 */
export function shouldInvalidateCache(error: Error): boolean {
  if (error instanceof ApiError) {
    if (error.statusCode === 401 || error.statusCode === 403) return true;
    const rawCode = error.metadata?.contractCode;
    if (typeof rawCode === "string") return mapContractError(rawCode).invalidateCache;
  }
  if (error instanceof AuthenticationError) return true;
  return false;
}

export function generateErrorId(): string {
  return `${Date.now()}-${Math.random().toString(36).substring(2, 11)}`;
}

/**
 * Serialises an error into a plain object safe for JSON / logging.
 */
export function serializeError(error: Error): Record<string, unknown> {
  const base: Record<string, unknown> = {
    name: error.name,
    message: error.message,
    stack: error.stack,
  };

  if (error instanceof ArenaXError) {
    base.category = error.category;
    base.severity = error.severity;
    base.metadata = error.metadata;
  }

  return base;
}
