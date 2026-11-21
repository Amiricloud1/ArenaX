"use client";

import { useEffect } from "react";
import { PageError } from "@/components/common/PageError";
import {
  classifyError,
  getErrorMapping,
  isAuthError,
} from "@/lib/errors/errorCatalog";
import { clearScopedCaches } from "@/lib/query/scopedCache";

export default function DashboardError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
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