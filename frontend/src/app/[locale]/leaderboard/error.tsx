"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function LeaderboardError({
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
      routeGroup="leaderboard"
      title="Failed to load leaderboard"
      message="We couldn't fetch the rankings. Check your connection and try again."
    />
  );
}