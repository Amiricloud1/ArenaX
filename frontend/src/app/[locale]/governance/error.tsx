"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function GovernanceError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  return (
    <RouteErrorBoundary
      error={error}
      reset={reset}
      routeGroup="governance"
      title="Failed to load governance"
      message="We couldn't fetch governance proposals. Check your connection and try again."
    />
  );
}