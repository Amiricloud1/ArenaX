"use client";

import { useEffect } from "react";
import { PageError } from "@/components/common/PageError";

/**
 * Error thrown when the realtime stream cannot be resynchronized with a
 * fresh authoritative snapshot (gap detected, stale watermark, or retention
 * expiry). Carries a recoverable flag so the boundary can offer a retry.
 */
export class SnapshotResyncError extends Error {
  readonly recoverable: boolean;
  readonly digest?: string;

  constructor(message: string, options?: { recoverable?: boolean; digest?: string }) {
    super(message);
    this.name = "SnapshotResyncError";
    this.recoverable = options?.recoverable ?? true;
    this.digest = options?.digest;
  }
}

function isSnapshotResyncError(error: Error): error is SnapshotResyncError {
  return (
    error instanceof SnapshotResyncError ||
    (error as { name?: string }).name === "SnapshotResyncError"
  );
}

export default function LeaderboardError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("[LeaderboardPage]", error);
  }, [error]);

  const resyncFailure = isSnapshotResyncError(error);
  const recoverable = resyncFailure ? error.recoverable : true;

  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <PageError
        title={
          resyncFailure
            ? "Leaderboard is out of sync"
            : "Failed to load leaderboard"
        }
        message={
          resyncFailure
            ? recoverable
              ? "The live rankings fell out of sync with the server. Reload the latest snapshot to continue."
              : "The live rankings could not be resynchronized. Please refresh the page in a moment."
            : "We couldn't fetch the rankings. Check your connection and try again."
        }
        onRetry={recoverable ? reset : undefined}
      />
    </div>
  );
}