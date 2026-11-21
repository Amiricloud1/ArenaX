"use client";

import { useEffect } from "react";
import { PageError } from "@/components/common/PageError";
import {
  classifyError,
  getErrorMapping,
  isAuthError,
  logErrorWithContext,
} from "@/lib/errors/errorCatalog";
import { clearScopedCaches } from "@/lib/query/scopedCache";

export default function WalletError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    // Log with transaction context but never expose signing material.
    logErrorWithContext(error, { boundary: "WalletPage" });

    // Authentication/authorization failures invalidate scoped caches so a
    // stale cached response cannot outlive an account or privilege change.
    if (isAuthError(error)) {
      void clearScopedCaches({ reason: "wallet-auth-error" });
    }
  }, [error]);

  const mapping = getErrorMapping(error);
  const classification = classifyError(error);

  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <PageError
        title={mapping.title}
        message={mapping.message}
        code={mapping.code}
        classification={classification}
        onRetry={classification === "retry" ? reset : undefined}
        onManualRecovery={
          classification === "manual-recovery" ? reset : undefined
        }
      />
      <p className="sr-only" role="status" aria-live="polite">
        Wallet data could not be loaded. No financial actions were queued
        offline. Use the retry button to reload your wallet from the server.
      </p>
    </div>
  );
}