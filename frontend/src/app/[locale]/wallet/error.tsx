"use client";

import { useEffect } from "react";
import { PageError } from "@/components/common/PageError";

export default function WalletError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("[WalletPage]", error);
  }, [error]);

  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <PageError
        title="Wallet unavailable"
        message="We couldn't load your wallet data. Check your connection and try again. Financial actions are never queued offline — any pending balance change was rolled back and will be re-checked against the server when you retry."
        onRetry={reset}
      />
      <p className="sr-only" role="status" aria-live="polite">
        Wallet data could not be loaded. No financial actions were queued
        offline. Use the retry button to reload your wallet from the server.
      </p>
    </div>
  );
}