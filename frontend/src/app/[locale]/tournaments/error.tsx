"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function TournamentsError({
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
      routeGroup="tournaments"
      title="Failed to load tournaments"
      message="We couldn't fetch the tournament list. Check your connection and try again."
    />
  );
}