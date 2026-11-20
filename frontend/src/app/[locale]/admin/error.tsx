"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function AdminError({
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
      routeGroup="admin"
      title="Admin panel error"
      message="Something went wrong loading this admin page. Try again or contact support if the problem persists."
      homeHref="/admin"
      homeLabel="Back to admin dashboard"
    />
  );
}