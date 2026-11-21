"use client";

import { useEffect, useMemo } from "react";
import { PageError } from "@/components/common/PageError";
import {
  classifyError,
  getErrorMapping,
  isAuthorizationError,
  logErrorWithContext,
} from "@/lib/errors/errorCatalog";
import { clearIdentityScopedCaches } from "@/lib/query/identityScope";

export default function AdminError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  const mapping = useMemo(() => {
    const code = classifyError(error);
    return getErrorMapping(code);
  }, [error]);

  const authorizationFailure = useMemo(
    () => isAuthorizationError(error),
    [error],
  );

  useEffect(() => {
    logErrorWithContext(error, {
      boundary: "AdminPage",
      digest: error.digest,
    });
  }, [error]);

  useEffect(() => {
    if (authorizationFailure) {
      // A role revocation or permission change may have occurred; drop any
      // identity/permission-scoped caches so stale authorized data is not
      // served after the boundary recovers.
      clearIdentityScopedCaches("authorization-change");
    }
  }, [authorizationFailure]);

  return (
    <div className="container mx-auto p-6 flex min-h-[60vh] items-center justify-center">
      <PageError
        title={mapping.title}
        message={mapping.message}
        code={mapping.code}
        recovery={mapping.recovery}
        onRetry={reset}
      />
    </div>
  );
}