import { NextResponse } from "next/server";
import type { NextRequest } from "next/server";
import type { WebVitalReport } from "@/lib/webVitalsReporter";
import {
  isAnalyticsConsentGranted,
  resolveAnalyticsConsent,
} from "@/lib/analyticsConsent";

/**
 * POST /api/analytics/web-vitals (#1114)
 *
 * Receives the batched reports `webVitalsReporter.ts` ships from the
 * browser (`{ eventType: "web_vitals", data: { reports } }`). Mirrors the
 * in-memory ring-buffer pattern already used by
 * `app/api/analytics/events/route.ts` for this frontend's other analytics
 * sink, so a real backend/Datadog forwarder can be dropped in behind the
 * same buffer later without changing the client.
 *
 * Budget breaches (`withinBudget: false`) are logged at `warn` with the
 * route/device/connection context — the hook a real Slack/Grafana alert
 * pipeline would consume; wiring that pipeline itself is infra outside
 * this Next.js app's code (see `lighthouserc.json` for the CI-side budget
 * gate on LCP/CLS).
 */

const MAX_BATCH_SIZE = 100;
const MAX_REPORTS_IN_MEMORY = 1000;

const reportBuffer: WebVitalReport[] = [];

const VALID_METRIC_NAMES = new Set(["LCP", "CLS", "INP", "FCP", "TTFB"]);

function isValidReport(report: unknown): report is WebVitalReport {
  if (!report || typeof report !== "object") return false;
  const candidate = report as Record<string, unknown>;
  return (
    typeof candidate.name === "string" &&
    VALID_METRIC_NAMES.has(candidate.name) &&
    typeof candidate.value === "number" &&
    Number.isFinite(candidate.value) &&
    typeof candidate.route === "string"
  );
}

export async function POST(request: NextRequest) {
  try {
    const body = await request.json();

    // #250: web-vitals are non-essential telemetry unless explicitly
    // configured as essential. Withdrawal must halt them server-side, not
    // just hide the client widget.
    const consent = resolveAnalyticsConsent(request);
    if (!isAnalyticsConsentGranted(consent)) {
      return NextResponse.json(
        { received: 0, suppressed: true, reason: "consent_denied" },
        { status: 202 },
      );
    }

    const reports: WebVitalReport[] | undefined = body?.data?.reports;

    if (!Array.isArray(reports)) {
      return NextResponse.json({ error: "data.reports must be an array" }, { status: 400 });
    }

    if (reports.length > MAX_BATCH_SIZE) {
      return NextResponse.json(
        { error: `Batch size exceeds limit of ${MAX_BATCH_SIZE}` },
        { status: 400 },
      );
    }

    const validReports = reports.filter(isValidReport);

    for (const report of validReports) {
      reportBuffer.push(report);
      if (reportBuffer.length > MAX_REPORTS_IN_MEMORY) {
        reportBuffer.shift();
      }

      if (report.withinBudget === false) {
        console.warn("[WebVitals] budget exceeded", {
          name: report.name,
          value: report.value,
          budget: report.budget,
          route: report.route,
          device: report.device,
          connection: report.connection,
        });
      }
    }

    return NextResponse.json({ received: validReports.length }, { status: 200 });
  } catch {
    return NextResponse.json({ error: "Invalid JSON body" }, { status: 400 });
  }
}

export async function GET(request: NextRequest) {
  // #250: reading buffered telemetry is itself a non-essential analytics
  // operation; suppress it when consent is not granted.
  const consent = resolveAnalyticsConsent(request);
  if (!isAnalyticsConsentGranted(consent)) {
    return NextResponse.json(
      { buffered: 0, reports: [], suppressed: true, reason: "consent_denied" },
      { status: 202 },
    );
  }

  return NextResponse.json({
    buffered: reportBuffer.length,
    reports: reportBuffer.slice(-50),
  });
}
