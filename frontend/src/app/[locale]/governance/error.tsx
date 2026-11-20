"use client";

import { useEffect } from "react";
import { PageError } from "@/components/common/PageError";

type GovernanceErrorLike = Error & {
  digest?: string;
  /** Optional machine-readable code set by the optimistic mutation layer. */
  code?: string;
  /** Optional flag indicating the failure came from an optimistic mutation. */
  optimistic?: boolean;
  /** Optional flag indicating the server rejected the mutation due to a conflict. */
  conflict?: boolean;
};

const CONFLICT_MESSAGE =
  "Your vote could not be saved because the proposal changed on the server. We've restored the latest state — review it and try again.";
const ROLLBACK_MESSAGE =
  "Your vote could not be confirmed, so we rolled back the change. Check your connection and try again.";
const GENERIC_MESSAGE =
  "We couldn't fetch governance proposals. Check your connection and try again.";

function resolveMessage(error: GovernanceErrorLike): string {
  if (error.conflict || error.code === "CONFLICT" || error.code === "STALE_VERSION") {
    return CONFLICT_MESSAGE;
  }
  if (error.optimistic || error.code === "OPTIMISTIC_ROLLBACK") {
    return ROLLBACK_MESSAGE;
  }
  return GENERIC_MESSAGE;
}

export default function GovernanceError({
  error,
  reset,
}: {
  error: GovernanceErrorLike;
  reset: () => void;
}) {
  useEffect(() => {
    console.error("[GovernancePage]", error);
  }, [error]);

  const isConflict =
    error.conflict ||
    error.code === "CONFLICT" ||
    error.code === "STALE_VERSION";
  const isOptimistic =
    error.optimistic || error.code === "OPTIMISTIC_ROLLBACK";

  const title = isConflict
    ? "Vote conflict"
    : isOptimistic
      ? "Vote not saved"
      : "Failed to load governance";

  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <PageError
        title={title}
        message={resolveMessage(error)}
        onRetry={reset}
      />
    </div>
  );
}