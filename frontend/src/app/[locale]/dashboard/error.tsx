"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function DashboardError({
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
      routeGroup="dashboard"
      title="Dashboard unavailable"
      message="We couldn't load your dashboard data. Try refreshing the page."
    />
  );
}