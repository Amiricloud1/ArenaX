"use client";

import { useEffect, useMemo } from "react";
import { PageError } from "@/components/common/PageError";
import {
  classifyError,
  getErrorMapping,
  isAuthError,
} from "@/lib/errors/errorCatalog";
import { clearScopedCaches } from "@/lib/query/scopedCache";

type DashboardErrorKind =
  | "snapshot"
  | "realtime"
  | "optimistic"
  | "unknown";

interface DashboardErrorLike extends Error {
  digest?: string;
  kind?: DashboardErrorKind;
  code?: string;
  retryable?: boolean;
  recoverable?: boolean;
}

const REALTIME_CODES = new Set([
  "REALTIME_GAP",
  "REALTIME_STALE_EVENT",
  "REALTIME_RESYNC_REQUIRED",
  "REALTIME_RETENTION_EXPIRED",
  "STREAM_SEQUENCE_GAP",
]);

const OPTIMISTIC_CODES = new Set([
  "OPTIMISTIC_CONFLICT",
  "OPTIMISTIC_TIMEOUT",
  "OPTIMISTIC_ROLLBACK",
  "OPTIMISTIC_STALE_RESPONSE",
  "MUTATION_CONFLICT",
  "MUTATION_TIMEOUT",
  "IDEMPOTENCY_CONFLICT",
]);

function classifyError(error: DashboardErrorLike): DashboardErrorKind {
  if (error.kind) {
    return error.kind;
  }
  const code = error.code ?? "";
  if (REALTIME_CODES.has(code)) {
    return "realtime";
  }
  if (OPTIMISTIC_CODES.has(code)) {
    return "optimistic";
  }
  return "unknown";
}

const COPY: Record<
  DashboardErrorKind,
  { title: string; message: string; retryLabel: string }
> = {
  snapshot: {
    title: "Dashboard unavailable",
    message:
      "We couldn't load your dashboard data. Try refreshing the page.",
    retryLabel: "Reload dashboard",
  },
  realtime: {
    title: "Live updates out of sync",
    message:
      "Your dashboard fell out of sync with the server. We'll fetch a fresh snapshot so live updates resume correctly.",
    retryLabel: "Resync live updates",
  },
  optimistic: {
    title: "Change couldn't be saved",
    message:
      "Your recent change couldn't be confirmed by the server, so we rolled it back to the last saved state. Review the latest data and try again.",
    retryLabel: "Retry change",
  },
  unknown: {
    title: "Dashboard unavailable",
    message:
      "We couldn't load your dashboard data. Try refreshing the page.",
    retryLabel: "Try again",
  },
};

export default function DashboardError({
  error,
  reset,
}: {
  error: DashboardErrorLike;
  reset: () => void;
}) {
  useEffect(() => {
    const classification = classifyError(error);
    const mapping = getErrorMapping(classification.code);

    // Log with transaction context only; never include signing material.
    console.error("[DashboardPage]", {
      code: classification.code,
      classification: mapping.classification,
      retryable: mapping.retryable,
      digest: error.digest,
      message: mapping.safeMessage,
      unknown: classification.unknown,
    });

    if (isAuthError(classification.code)) {
      // Authorization/authentication failures invalidate scoped caches so a
      // stale response cannot outlive an account or privilege change.
      clearScopedCaches();
    }
  }, [error]);

  const classification = classifyError(error);
  const mapping = getErrorMapping(classification.code);

  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <PageError
        title="Dashboard unavailable"
        message={mapping.safeMessage}
        onRetry={mapping.retryable ? reset : undefined}
      />
    </div>
  );
}