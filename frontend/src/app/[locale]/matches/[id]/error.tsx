"use client";

import { useEffect, useMemo } from "react";
import { useRouter } from "next/navigation";
import { PageError } from "@/components/common/PageError";

type RealtimeErrorKind = "gap" | "stale" | "resync" | "reconcile" | "unknown";

interface RealtimeErrorLike extends Error {
  digest?: string;
  code?: string;
  kind?: RealtimeErrorKind;
  recoverable?: boolean;
  requestSnapshot?: () => void;
}

const GAP_CODES = new Set([
  "REALTIME_GAP",
  "REALTIME_SEQUENCE_GAP",
  "STREAM_GAP",
  "EVENT_RETENTION_EXPIRED",
  "SNAPSHOT_STALE",
  "STALE_EVENT",
  "RESYNC_REQUIRED",
]);

const RECONCILE_CODES = new Set([
  "OPTIMISTIC_CONFLICT",
  "RECONCILIATION_FAILED",
  "MUTATION_TIMEOUT",
  "DUPLICATE_RESPONSE",
  "IDEMPOTENCY_CONFLICT",
]);

function classifyError(error: RealtimeErrorLike): RealtimeErrorKind {
  if (error.kind) return error.kind;
  const code = error.code ?? "";
  if (GAP_CODES.has(code)) {
    return code === "SNAPSHOT_STALE" || code === "STALE_EVENT" ? "stale" : "gap";
  }
  if (RECONCILE_CODES.has(code)) return "reconcile";
  if (/gap|sequence|watermark|resync/i.test(error.message)) return "gap";
  if (/stale|out.?of.?order/i.test(error.message)) return "stale";
  if (/optimistic|reconcil|conflict|idempoten/i.test(error.message)) return "reconcile";
  return "unknown";
}

const COPY: Record<
  RealtimeErrorKind,
  { title: string; message: string; retryLabel: string }
> = {
  gap: {
    title: "Live updates fell out of sync",
    message:
      "We missed part of the live match stream. Reloading will fetch a fresh snapshot so the match is consistent again.",
    retryLabel: "Resync match",
  },
  stale: {
    title: "Match data was out of date",
    message:
      "Some live updates arrived out of order and were discarded. Reload to fetch the latest snapshot.",
    retryLabel: "Refresh snapshot",
  },
  resync: {
    title: "Reconnecting to live match",
    message:
      "The live connection was interrupted. Reload to request a replacement snapshot and resume updates.",
    retryLabel: "Reconnect",
  },
  reconcile: {
    title: "Your change couldn't be confirmed",
    message:
      "We couldn't confirm your last update with the server. Reloading will show the authoritative match state; nothing was lost.",
    retryLabel: "Reload match state",
  },
  unknown: {
    title: "Failed to load match",
    message:
      "We couldn't fetch the match data. The match may no longer exist or there was a connection issue.",
    retryLabel: "Reload match",
  },
};

export default function MatchHubError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  const router = useRouter();
  const realtimeError = error as RealtimeErrorLike;
  const kind = useMemo(() => classifyError(realtimeError), [realtimeError]);

  useEffect(() => {
    console.error("[MatchHubPage]", { kind, code: realtimeError.code, error });
  }, [error, kind, realtimeError.code]);

  const handleRetry = () => {
    // Prefer an explicit snapshot request so we never replay stale events.
    if (typeof realtimeError.requestSnapshot === "function") {
      try {
        realtimeError.requestSnapshot();
      } catch (snapshotError) {
        console.error("[MatchHubPage] snapshot request failed", snapshotError);
      }
    }
    // Reset the boundary and refresh server components so a replacement
    // snapshot is fetched from the server rather than reusing cached state.
    reset();
    router.refresh();
  };

  const copy = COPY[kind];

  return (
    <div className="min-h-screen bg-background flex items-center justify-center px-4">
      <PageError
        title={copy.title}
        message={copy.message}
        onRetry={handleRetry}
        retryLabel={copy.retryLabel}
      />
    </div>
  );
}