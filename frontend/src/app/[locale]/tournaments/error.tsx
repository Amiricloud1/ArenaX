"use client";

import { useEffect, useMemo, useState } from "react";
import { PageError } from "@/components/common/PageError";

type ConflictKind = "offline-conflict" | "rollback-failed" | "unknown";

interface ConflictDetails {
  kind: ConflictKind;
  title: string;
  message: string;
  recoverable: boolean;
}

function classifyError(error: Error & { digest?: string }): ConflictDetails {
  const name = (error?.name ?? "").toLowerCase();
  const message = (error?.message ?? "").toLowerCase();

  const isConflict =
    name.includes("conflict") ||
    message.includes("conflict") ||
    message.includes("stale") ||
    message.includes("newer server") ||
    message.includes("version mismatch");

  const isRollback =
    name.includes("rollback") ||
    message.includes("rollback") ||
    message.includes("reconcile") ||
    message.includes("optimistic");

  if (isConflict) {
    return {
      kind: "offline-conflict",
      title: "Tournament data changed while you were offline",
      message:
        "Your queued change conflicts with newer server data. We kept the server's version to avoid overwriting it. Review the latest tournaments and try again.",
      recoverable: true,
    };
  }

  if (isRollback) {
    return {
      kind: "rollback-failed",
      title: "We couldn't confirm your last change",
      message:
        "Your optimistic update could not be reconciled with the server. The latest tournament data has been restored. Please retry.",
      recoverable: true,
    };
  }

  return {
    kind: "unknown",
    title: "Failed to load tournaments",
    message:
      "We couldn't fetch the tournament list. Check your connection and try again.",
    recoverable: true,
  };
}

export default function TournamentsError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  const [retrying, setRetrying] = useState(false);
  const details = useMemo(() => classifyError(error), [error]);

  useEffect(() => {
    console.error("[TournamentsPage]", {
      kind: details.kind,
      digest: error?.digest,
      error,
    });
  }, [error, details.kind]);

  const handleRetry = () => {
    if (retrying) return;
    setRetrying(true);
    try {
      reset();
    } finally {
      // Allow retry again if the boundary re-renders with the same error.
      setRetrying(false);
    }
  };

  return (
    <div className="min-h-screen px-4 py-8 bg-background flex items-center justify-center">
      <div role="alert" aria-live="assertive" className="w-full max-w-lg">
        <PageError
          title={details.title}
          message={details.message}
          onRetry={details.recoverable ? handleRetry : undefined}
        />
        {details.kind !== "unknown" && (
          <p className="mt-4 text-center text-sm text-muted-foreground">
            Your offline queue was not replayed to avoid overwriting newer
            server state. Reloading fetches the latest tournaments.
          </p>
        )}
      </div>
    </div>
  );
}