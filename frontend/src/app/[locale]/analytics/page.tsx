"use client";

import dynamic from "next/dynamic";

// Code-split (#1086): recharts is the single largest contributor to this
// route's bundle — deferring it keeps the initial JS payload lean and avoids
// blocking LCP on a chart library nothing above the fold needs.
const PlatformAnalyticsDashboard = dynamic(
  () =>
    import("@/components/analytics/PlatformAnalyticsDashboard").then(
      (m) => m.PlatformAnalyticsDashboard,
    ),
  {
    ssr: false,
    loading: () => (
      <main className="min-h-screen bg-gray-950 p-4 text-white sm:p-6" aria-hidden="true">
        <div className="mb-6 h-6 w-48 animate-pulse rounded bg-gray-800" />
        <div className="mb-6 grid grid-cols-2 gap-3 sm:grid-cols-4">
          {Array.from({ length: 4 }).map((_, i) => (
            <div key={i} className="h-20 animate-pulse rounded-lg bg-gray-800" />
          ))}
        </div>
        <div className="mb-6 h-52 animate-pulse rounded-lg bg-gray-800" />
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
          <div className="h-56 animate-pulse rounded-lg bg-gray-800" />
          <div className="h-56 animate-pulse rounded-lg bg-gray-800" />
        </div>
      </main>
    ),
  },
);

// Code-split (#255/#256): the experiment guardrail panel pulls in the
// experiment assignment/exposure client and guardrail decision logic. It is
// only needed once the dashboard is interactive, so keep it out of the
// initial payload alongside the chart library.
const ExperimentGuardrailPanel = dynamic(
  () =>
    import("@/components/analytics/ExperimentGuardrailPanel").then(
      (m) => m.ExperimentGuardrailPanel,
    ),
  {
    ssr: false,
    loading: () => (
      <section
        className="mb-6 rounded-lg bg-gray-900 p-4"
        aria-hidden="true"
        aria-busy="true"
      >
        <div className="mb-4 h-5 w-56 animate-pulse rounded bg-gray-800" />
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          {Array.from({ length: 3 }).map((_, i) => (
            <div key={i} className="h-24 animate-pulse rounded-lg bg-gray-800" />
          ))}
        </div>
      </section>
    ),
  },
);

export default function AnalyticsDashboardPage() {
  return (
    <main className="min-h-screen bg-gray-950 p-4 text-white sm:p-6">
      {/*
        #256: Guardrail status for every product experiment — distinguishes
        insufficient data from safe-to-ship and flags statistically or
        operationally harmful outcomes. Each card surfaces the primary metric,
        guardrail metrics, minimum sample, analysis window, and stop
        thresholds so operators can see why a decision was reached.

        #255: The panel logs exposure at presentation time (not assignment
        time) for any experiment surfaced here, using the deterministic
        assignment recorded for this participant. Assignment is never
        recomputed mid-session, so users cannot be silently reassigned.
      */}
      <ExperimentGuardrailPanel />
      <PlatformAnalyticsDashboard />
    </main>
  );
}
</｜｜DSML｜｜ parameter>