"use client";

import { useEffect, useMemo } from "react";
import { PageError } from "@/components/common/PageError";
import { logError } from "@/lib/errorLogger";
import {
  classifyMutationError,
  getMutationErrorMetadata,
  isMutationError,
  type MutationErrorKind,
} from "@/lib/mutationRegistry";

const FALLBACK_TITLE = "Something went wrong";
const FALLBACK_MESSAGE =
  "We couldn't complete that action. Please try again.";

const KIND_COPY: Record<
  MutationErrorKind,
  { title: string; message: string; retryLabel: string }
> = {
  conflict: {
    title: "This was updated elsewhere",
    message:
      "Someone else changed this while you were working. Retry to load the latest version and try again.",
    retryLabel: "Reload latest",
  },
  timeout: {
    title: "The request timed out",
    message:
      "We didn't get a response in time. Your change may or may not have been applied. Retry to reconcile with the server.",
    retryLabel: "Retry",
  },
  stale: {
    title: "A newer version is available",
    message:
      "Your change was based on an older version. Retry to refresh and reapply your update.",
    retryLabel: "Refresh and retry",
  },
  validation: {
    title: "That change couldn't be saved",
    message:
      "Some of the information was rejected by the server. Review it and try again.",
    retryLabel: "Try again",
  },
  authorization: {
    title: "You don't have permission",
    message:
      "Your session may have expired or you lack access to this action. Sign in again or contact an administrator.",
    retryLabel: "Try again",
  },
  unknown: {
    title: FALLBACK_TITLE,
    message: FALLBACK_MESSAGE,
    retryLabel: "Try again",
  },
};

export default function RootError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    logError(error, {
      digest: error.digest,
      mutationKind: isMutationError(error)
        ? classifyMutationError(error)
        : undefined,
    });
  }, [error]);

  const copy = useMemo(() => {
    if (!isMutationError(error)) {
      return {
        title: FALLBACK_TITLE,
        message: error.message || FALLBACK_MESSAGE,
        retryLabel: "Try again",
      };
    }

    const kind = classifyMutationError(error);
    const base = KIND_COPY[kind] ?? KIND_COPY.unknown;
    const metadata = getMutationErrorMetadata(error);

    return {
      title: base.title,
      message: metadata?.detail
        ? `${base.message} ${metadata.detail}`
        : base.message,
      retryLabel: base.retryLabel,
    };
  }, [error]);

  const handleRetry = () => {
    // Reset the boundary. The mutation registry deduplicates optimistic
    // updates by idempotency key, so retrying here will not enqueue a
    // duplicate optimistic update for the same mutation.
    reset();
  };

  return (
    <PageError
      title={copy.title}
      message={copy.message}
      onRetry={handleRetry}
      retryLabel={copy.retryLabel}
    />
  );
}