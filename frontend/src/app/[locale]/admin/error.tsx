"use client";

import { useEffect } from "react";
import { PageError } from "@/components/common/PageError";

type RealtimeErrorKind = "resync" | "mutation";

interface RealtimeErrorLike extends Error {
  digest?: string;
  kind?: RealtimeErrorKind;
  recoverable?: boolean;
  requestId?: string;
  resource?: string;
}

function classifyError(error: RealtimeErrorLike): RealtimeErrorKind | undefined {
  if (error.kind === "resync" || error.kind === "mutation") {
    return error.kind;
  }
  const message = (error.message || "").toLowerCase();
  if (
    message.includes("resync") ||
    message.includes("snapshot") ||
    message.includes("sequence") ||
    message.includes("watermark") ||
    message.includes("stream gap")
  ) {
    return "resync";
  }
  if (
    message.includes("optimistic") ||
    message.includes("rollback") ||
    message.includes("reconcil") ||
    message.includes("idempotency") ||
    message.includes("conflict")
  ) {
    return "mutation";
  }
  return undefined;
}

export default function AdminError({
  error,
  reset,
}: {
  error: RealtimeErrorLike;
  reset: () => void;
}) {
  useEffect(() => {
    console.error("[AdminPage]", error);
  }, [error]);

  const kind = classifyError(error);

  if (kind === "resync") {
    return (
      <div className="container mx-auto p-6 flex min-h-[60vh] items-center justify-center">
        <PageError
          title="Realtime data out of sync"
          message="This admin view fell out of sync with the server. Retry to load a fresh snapshot and resume live updates."
          onRetry={reset}
        />
      </div>
    );
  }

  if (kind === "mutation") {
    return (
      <div className="container mx-auto p-6 flex min-h-[60vh] items-center justify-center">
        <PageError
          title="Change could not be confirmed"
          message="Your recent change could not be confirmed by the server, so it has been rolled back. Retry to reconcile with the latest server state."
          onRetry={reset}
        />
      </div>
    );
  }

  return (
    <div className="container mx-auto p-6 flex min-h-[60vh] items-center justify-center">
      <PageError
        title="Admin panel error"
        message="Something went wrong loading this admin page. Try again or contact support if the problem persists."
        onRetry={reset}
      />
    </div>
  );
}