import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  // The state-sync project owns its own specs; keep them out of the default
  // chromium projects so each scenario runs exactly once per run.
  testIgnore: /state-sync\/.*\.spec\.ts/,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? "github" : "list",
  // Visual regression baselines are platform-specific (font rendering and
  // anti-aliasing differ across OSes), so keep them in a dedicated snapshot
  // directory that can be reviewed and approved per platform.
  snapshotDir: "./e2e/__screenshots__",
  // Fail the run when a committed baseline is missing instead of silently
  // writing a new one, so intentional visual changes must be reviewed and
  // approved rather than masked by an auto-generated baseline.
  updateSnapshots: process.env.UPDATE_SNAPSHOTS ? "all" : "missing",
  expect: {
    // Allow a tiny, deterministic pixel tolerance for sub-pixel rendering
    // differences while still surfacing real layout regressions (text
    // overflow, obscured bracket state, money amounts, primary actions).
    toHaveScreenshot: {
      maxDiffPixelRatio: 0.01,
      animations: "disabled",
      caret: "hide",
      scale: "css",
    },
  },
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? "http://localhost:3000",
    trace: "on-first-retry",
    // Deterministic rendering for screenshots: fixed viewport, fixed device
    // scale factor, and a fixed color scheme so baselines are stable.
    viewport: { width: 1280, height: 720 },
    deviceScaleFactor: 1,
    colorScheme: "light",
    locale: "en-US",
    timezoneId: "UTC",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
      // Failure-injection and consent specs run against the production build
      // (see webServer below) so bundle-budget assertions reflect shipped code.
      testMatch: [
        "**/error-boundaries.spec.ts",
        "**/analytics-consent.spec.ts",
        "**/bundle-budget.spec.ts",
      ],
    },
    {
      name: "chromium-mobile",
      use: { ...devices["Pixel 5"] },
      testMatch: [
        "**/error-boundaries.spec.ts",
        "**/analytics-consent.spec.ts",
        "**/bundle-budget.spec.ts",
      ],
    },
    {
      // Browser-level coverage for realtime snapshot/event reconciliation,
      // the offline command queue, optimistic rollback, and deep-link
      // preservation. These specs drive the app through mocked network and
      // websocket boundaries, so they run on a single deterministic project
      // and are excluded from the visual-regression projects above.
      name: "state-sync",
      testMatch: /state-sync\/.*\.spec\.ts/,
      use: { ...devices["Desktop Chrome"] },
    },
    // RTL locale coverage. The app uses `localePrefix: "always"`, so RTL
    // specs navigate to "/ar/..." and assert mirrored layout, navigation
    // order, and semantic numeric/sign order. Snapshots are kept in a
    // locale-specific directory so LTR baselines are never overwritten.
    {
      name: "chromium-rtl",
      use: {
        ...devices["Desktop Chrome"],
        locale: "ar",
        // RTL layout is exercised at the same deterministic viewport as LTR
        // so mirrored-layout diffs are attributable to direction, not size.
        viewport: { width: 1280, height: 720 },
        deviceScaleFactor: 1,
        colorScheme: "light",
        timezoneId: "UTC",
      },
      snapshotPathTemplate:
        "{snapshotDir}/{testFilePath}/{arg}-{projectName}-{platform}{ext}",
    },
    // Gamepad / switch-input coverage. Specs in `e2e/gamepad` install the
    // synthetic gamepad fixture before navigation; this project only pins
    // the deterministic environment those specs rely on.
    {
      name: "chromium-gamepad",
      testMatch: /gamepad\/.*\.spec\.ts/,
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1280, height: 720 },
        deviceScaleFactor: 1,
        colorScheme: "light",
        locale: "en-US",
        timezoneId: "UTC",
      },
    },
  ],
  webServer: {
    // Build and serve the production bundle so bundle-budget and
    // hydration-cost checks measure the shipped output, not the dev server.
    command: "npm run build && npm run start",
    // NOTE: with next-intl `localePrefix: "always"`, "/" is a 307 redirect to
    // "/en", so probing "/" never yields a 200 and Playwright waits until
    // timeout. Probe the default-locale home (public, renders 200) instead.
    url: "http://localhost:3000/en",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
