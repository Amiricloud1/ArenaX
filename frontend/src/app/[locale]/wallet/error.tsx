"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

export default function WalletError({
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
      routeGroup="wallet"
      title="Wallet unavailable"
      message="We couldn't load your wallet data. Check your connection and try again."
    />
  );
}