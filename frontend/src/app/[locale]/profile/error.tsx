"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function ProfileError({
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
      routeGroup="profile"
      title="Failed to load profile"
      message="We couldn't fetch your profile data. Try refreshing the page."
    />
  );
}