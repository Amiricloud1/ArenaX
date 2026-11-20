"use client";

import { useRouter } from "next/navigation";
import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function MatchHubError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  const router = useRouter();

  return (
    <RouteErrorBoundary
      error={error}
      reset={reset}
      context="matches/[id]"
      title="Failed to load match"
      message="We couldn't fetch the match data. The match may no longer exist or there was a connection issue."
      retryLabel="Reload match"
      secondaryAction={{
        label: "Back to matches",
        onClick: () => router.push("/matches"),
      }}
    />
  );
}