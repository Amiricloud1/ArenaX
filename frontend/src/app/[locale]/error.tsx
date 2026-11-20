"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function RootError({
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
      routeGroup="root"
      title="Something went wrong"
      message="We couldn't load this page. Please try again or return to the home page."
      retryLabel="Try again"
    />
  );
}