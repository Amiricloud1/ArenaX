"use client";

import { RouteErrorBoundary } from "@/components/common/RouteErrorBoundary";

type ProfileErrorKind = "load" | "optimistic" | "offline-conflict";

function classifyError(error: Error & { digest?: string }): ProfileErrorKind {
  const name = error.name || "";
  const message = (error.message || "").toLowerCase();

  if (
    name === "OfflineQueueConflictError" ||
    message.includes("offline queue conflict") ||
    message.includes("conflict with newer server state")
  ) {
    return "offline-conflict";
  }

  if (
    name === "OptimisticRollbackError" ||
    message.includes("optimistic") ||
    message.includes("rollback")
  ) {
    return "optimistic";
  }

  return "load";
}

const COPY: Record<
  ProfileErrorKind,
  { title: string; message: string; retryLabel: string }
> = {
  load: {
    title: "Failed to load profile",
    message: "We couldn't fetch your profile data. Try refreshing the page.",
    retryLabel: "Try again",
  },
  optimistic: {
    title: "Profile update couldn't be saved",
    message:
      "Your recent profile change was rolled back because it couldn't be confirmed by the server. Your saved profile is unchanged. Try again to reapply your change.",
    retryLabel: "Retry update",
  },
  "offline-conflict": {
    title: "Profile changed elsewhere",
    message:
      "A profile change you made while offline conflicts with newer changes saved on the server. We kept the server version to avoid overwriting it. Review your profile and try again.",
    retryLabel: "Reload profile",
  },
};

export default function ProfileError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error("[ProfilePage]", error);
  }, [error]);

  const kind = classifyError(error);
  const copy = COPY[kind];

  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <div role="alert" aria-live="assertive" className="w-full">
        <PageError
          title={copy.title}
          message={copy.message}
          onRetry={reset}
          retryLabel={copy.retryLabel}
        />
      </div>
    </div>
  );
}