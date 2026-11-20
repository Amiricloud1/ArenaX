'use client';

import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';

/**
 * Shared, privacy-safe route-level error boundary (issue #248).
 *
 * Guarantees:
 * - Never renders `error.message` or raw stack/implementation details.
 * - Generates a correlation ID for support/telemetry cross-referencing.
 * - Exposes accessible retry + navigation recovery actions.
 * - Caps retries with exponential backoff to prevent retry loops.
 * - Reports via a redacting telemetry helper that strips tokens, payment
 *   details, and raw form contents.
 */

export interface RouteErrorBoundaryProps {
  children: React.ReactNode;
  /** Optional correlation ID supplied by the caller (e.g. from a server). */
  correlationId?: string;
  /** Localized copy overrides. */
  title?: string;
  description?: string;
  retryLabel?: string;
  homeLabel?: string;
  backLabel?: string;
  /** Max retry attempts before the retry action is disabled. */
  maxAttempts?: number;
  /** Base backoff in ms; doubles each attempt. */
  baseBackoffMs?: number;
  /** Optional navigation handler; defaults to `window.location.assign('/')`. */
  onNavigateHome?: () => void;
  /** Optional back handler; defaults to `history.back()`. */
  onNavigateBack?: () => void;
  /** Optional telemetry sink; defaults to `reportRouteError`. */
  onReport?: (report: RouteErrorReport) => void;
  /** Optional route/group identifier for telemetry. */
  routeGroup?: string;
}

export interface RouteErrorReport {
  correlationId: string;
  routeGroup?: string;
  /** Redacted, non-identifying error name only. */
  errorName: string;
  /** Redacted, non-identifying error digest if provided by the framework. */
  digest?: string;
  attempt: number;
  timestamp: string;
}

interface ErrorBoundaryState {
  error: Error | null;
  correlationId: string | null;
}

const DEFAULT_MAX_ATTEMPTS = 3;
const DEFAULT_BASE_BACKOFF_MS = 500;
const MAX_BACKOFF_MS = 8000;

/** Generate a correlation ID with a safe fallback for older environments. */
export function createCorrelationId(): string {
  try {
    if (
      typeof globalThis !== 'undefined' &&
      typeof globalThis.crypto !== 'undefined' &&
      typeof globalThis.crypto.randomUUID === 'function'
    ) {
      return globalThis.crypto.randomUUID();
    }
  } catch {
    // fall through to fallback
  }
  // Fallback: timestamp + random, non-cryptographic but sufficient for correlation.
  const rand = Math.random().toString(36).slice(2, 10);
  return `cid-${Date.now().toString(36)}-${rand}`;
}

/**
 * Redact potentially sensitive substrings from a free-form string.
 * Defense-in-depth: we never send raw messages, but if a caller passes one
 * through a digest or name we still scrub obvious secrets.
 */
export function redactString(input: string): string {
  if (!input) return '';
  let out = input;
  // Bearer tokens / JWTs
  out = out.replace(/Bearer\s+[A-Za-z0-9._-]+/gi, 'Bearer [REDACTED]');
  out = out.replace(/eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g, '[REDACTED_JWT]');
  // Common token/secret query or body params
  out = out.replace(
    /(token|access_token|refresh_token|id_token|api[_-]?key|secret|password|authorization)=([^&\s"']+)/gi,
    '$1=[REDACTED]',
  );
  // Card-like numbers (13-19 digits, optional separators)
  out = out.replace(/\b(?:\d[ -]?){13,19}\b/g, '[REDACTED_CARD]');
  // Email addresses
  out = out.replace(/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g, '[REDACTED_EMAIL]');
  return out;
}

/**
 * Build a privacy-safe telemetry report from an error. Never includes
 * `error.message`, stack, or raw form contents.
 */
export function buildRouteErrorReport(
  error: unknown,
  correlationId: string,
  attempt: number,
  routeGroup?: string,
): RouteErrorReport {
  const err = error as { name?: unknown; digest?: unknown } | null;
  const rawName = typeof err?.name === 'string' ? err.name : 'Error';
  const rawDigest = typeof err?.digest === 'string' ? err.digest : undefined;
  return {
    correlationId,
    routeGroup,
    errorName: redactString(rawName).slice(0, 64),
    digest: rawDigest ? redactString(rawDigest).slice(0, 128) : undefined,
    attempt,
    timestamp: new Date().toISOString(),
  };
}

/** Default telemetry sink. Replace with your analytics client. */
export function reportRouteError(report: RouteErrorReport): void {
  try {
    if (typeof console !== 'undefined' && typeof console.warn === 'function') {
      // Structured, redacted log only.
      console.warn('[route-error]', JSON.stringify(report));
    }
    // Hook point for analytics: e.g. analytics.track('route_error', report)
  } catch {
    // Telemetry must never throw.
  }
}

function computeBackoff(attempt: number, baseMs: number): number {
  const exp = Math.min(baseMs * Math.pow(2, Math.max(0, attempt - 1)), MAX_BACKOFF_MS);
  // Add jitter to avoid thundering herd.
  const jitter = Math.floor(Math.random() * Math.min(250, exp * 0.25));
  return exp + jitter;
}

class RouteErrorBoundaryInner extends React.Component<
  RouteErrorBoundaryProps,
  ErrorBoundaryState
> {
  private attemptRef = 0;
  private reportedRef = new Set<string>();

  constructor(props: RouteErrorBoundaryProps) {
    super(props);
    this.state = { error: null, correlationId: null };
  }

  static getDerivedStateFromError(error: Error): Partial<ErrorBoundaryState> {
    return { error };
  }

  componentDidCatch(error: Error): void {
    const correlationId = this.state.correlationId ?? createCorrelationId();
    if (this.state.correlationId !== correlationId) {
      this.setState({ correlationId });
    }
    this.attemptRef += 1;
    this.emitReport(error, correlationId, this.attemptRef);
  }

  private emitReport(error: unknown, correlationId: string, attempt: number): void {
    // Deduplicate identical reports to avoid telemetry spam.
    const key = `${correlationId}:${attempt}`;
    if (this.reportedRef.has(key)) return;
    this.reportedRef.add(key);

    const report = buildRouteErrorReport(
      error,
      correlationId,
      attempt,
      this.props.routeGroup,
    );
    const sink = this.props.onReport ?? reportRouteError;
    try {
      sink(report);
    } catch {
      // never throw from telemetry
    }
  }

  private handleRetry = (): void => {
    const max = this.props.maxAttempts ?? DEFAULT_MAX_ATTEMPTS;
    if (this.attemptRef >= max) return;

    const base = this.props.baseBackoffMs ?? DEFAULT_BASE_BACKOFF_MS;
    const delay = computeBackoff(this.attemptRef + 1, base);

    // Reset the boundary after backoff to re-render children.
    window.setTimeout(() => {
      this.setState({ error: null });
    }, delay);
  };

  private handleHome = (): void => {
    if (this.props.onNavigateHome) {
      this.props.onNavigateHome();
      return;
    }
    if (typeof window !== 'undefined') {
      window.location.assign('/');
    }
  };

  private handleBack = (): void => {
    if (this.props.onNavigateBack) {
      this.props.onNavigateBack();
      return;
    }
    if (typeof window !== 'undefined' && window.history.length > 1) {
      window.history.back();
    } else if (typeof window !== 'undefined') {
      window.location.assign('/');
    }
  };

  render(): React.ReactNode {
    const { error, correlationId } = this.state;
    if (!error) {
      return this.props.children;
    }

    const max = this.props.maxAttempts ?? DEFAULT_MAX_ATTEMPTS;
    const attemptsUsed = this.attemptRef;
    const canRetry = attemptsUsed < max;

    return (
      <RouteErrorFallback
        correlationId={correlationId ?? createCorrelationId()}
        title={this.props.title}
        description={this.props.description}
        retryLabel={this.props.retryLabel}
        homeLabel={this.props.homeLabel}
        backLabel={this.props.backLabel}
        canRetry={canRetry}
        attemptsUsed={attemptsUsed}
        maxAttempts={max}
        onRetry={this.handleRetry}
        onHome={this.handleHome}
        onBack={this.handleBack}
      />
    );
  }
}

interface RouteErrorFallbackProps {
  correlationId: string;
  title?: string;
  description?: string;
  retryLabel?: string;
  homeLabel?: string;
  backLabel?: string;
  canRetry: boolean;
  attemptsUsed: number;
  maxAttempts: number;
  onRetry: () => void;
  onHome: () => void;
  onBack: () => void;
}

function RouteErrorFallback({
  correlationId,
  title,
  description,
  retryLabel,
  homeLabel,
  backLabel,
  canRetry,
  attemptsUsed,
  maxAttempts,
  onRetry,
  onHome,
  onBack,
}: RouteErrorFallbackProps): React.ReactElement {
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const [announcement, setAnnouncement] = useState('');

  // Move focus to the heading so screen readers announce the alert.
  useEffect(() => {
    headingRef.current?.focus();
  }, []);

  const resolvedTitle = title ?? 'Something went wrong';
  const resolvedDescription =
    description ??
    'We could not load this page. You can try again, or return to a safe page.';

  const handleRetry = useCallback(() => {
    if (!canRetry) {
      setAnnouncement('Retry limit reached. Please use the navigation options.');
      return;
    }
    setAnnouncement('Retrying…');
    onRetry();
  }, [canRetry, onRetry]);

  const retryDisabledReason = useMemo(
    () =>
      canRetry
        ? undefined
        : `Retry limit reached (${attemptsUsed} of ${maxAttempts}).`,
    [canRetry, attemptsUsed, maxAttempts],
  );

  return (
    <div
      role="alert"
      aria-live="assertive"
      aria-atomic="true"
      className="route-error-boundary"
      data-testid="route-error-boundary"
    >
      <h1 ref={headingRef} tabIndex={-1} className="route-error-boundary__title">
        {resolvedTitle}
      </h1>
      <p className="route-error-boundary__description">{resolvedDescription}</p>
      <p className="route-error-boundary__correlation">
        Reference ID: <code data-testid="route-error-correlation-id">{correlationId}</code>
      </p>
      <div className="route-error-boundary__actions">
        <button
          type="button"
          onClick={handleRetry}
          disabled={!canRetry}
          aria-disabled={!canRetry}
          aria-describedby={retryDisabledReason ? 'route-error-retry-reason' : undefined}
          data-testid="route-error-retry"
        >
          {retryLabel ?? 'Try again'}
        </button>
        <button type="button" onClick={onHome} data-testid="route-error-home">
          {homeLabel ?? 'Go to home'}
        </button>
        <button type="button" onClick={onBack} data-testid="route-error-back">
          {backLabel ?? 'Go back'}
        </button>
      </div>
      {retryDisabledReason ? (
        <p id="route-error-retry-reason" className="route-error-boundary__hint">
          {retryDisabledReason}
        </p>
      ) : null}
      <span className="route-error-boundary__sr-only" aria-live="polite">
        {announcement}
      </span>
    </div>
  );
}

export default function RouteErrorBoundary(
  props: RouteErrorBoundaryProps,
): React.ReactElement {
  return <RouteErrorBoundaryInner {...props} />;
}
</｜｜DSML｜｜ parameter>
</｜｜DSML｜｜ invoke>
</｜｜DSML｜｜ calls>