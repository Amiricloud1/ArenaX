#!/usr/bin/env node
/**
 * Fails if any user-facing page route exists under src/app/ outside of
 * src/app/[locale]/ (issue #729).
 *
 * next-intl's middleware (localePrefix: "always") already redirects every
 * non-locale-prefixed request to its /<locale>/... equivalent, so a page
 * file placed directly under src/app/ (e.g. src/app/matches/[id]/page.tsx)
 * is never actually reachable by real traffic -- it's dead code that's
 * easy to add by accident (e.g. copy-pasting a route while it's still
 * being built) and easy to forget to delete once the [locale] version
 * lands. This check keeps routes from silently duplicating again.
 *
 * Allowed outside src/app/[locale]/:
 *  - src/app/api/**            (API routes are not localized pages)
 *  - src/app/layout.tsx        (root layout is required by Next.js)
 *  - src/app/{sitemap,robots}.ts / manifest.ts  (site-wide metadata routes)
 *  - global-error.tsx           (must be defined at the root per Next.js)
 */

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const APP_DIR = fileURLToPath(new URL("../src/app", import.meta.url));

const ALLOWED_TOP_LEVEL_DIRS = new Set(["[locale]", "api"]);
const ALLOWED_TOP_LEVEL_FILES = new Set([
  "layout.tsx",
  "global-error.tsx",
  "robots.ts",
  "sitemap.ts",
  "manifest.ts",
]);

const PAGE_FILE_PATTERN = /^page\.(tsx|ts|jsx|js)$/;

// ---------------------------------------------------------------------------
// Metric catalog governance (issue #253)
//
// Every metric definition lives in its own file under src/metrics/catalog/.
// Each file must export a definition with all required metadata fields, and
// metric names must be unique across the catalog. Definition changes must be
// reviewed: when running in CI (or with --check-reviewed), any catalog file
// that differs from the merge base must also appear in the reviewed-changes
// allowlist (src/metrics/catalog/reviewed.json), which is updated only after
// a reviewer signs off on the change.
// ---------------------------------------------------------------------------

const CATALOG_DIR = fileURLToPath(
  new URL("../src/metrics/catalog", import.meta.url),
);
const REVIEWED_ALLOWLIST = join(CATALOG_DIR, "reviewed.json");

const REQUIRED_METADATA_FIELDS = [
  "owner",
  "query",
  "sources",
  "freshness",
  "privacyClass",
  "deprecation",
];

const PRIVACY_CLASSES = new Set(["public", "internal", "confidential", "restricted"]);
const DEPRECATION_STATES = new Set(["active", "deprecated", "removed"]);

const METRIC_FILE_PATTERN = /\.(ts|tsx|js|mjs)$/;

function listMetricFiles(dir) {
  if (!existsSync(dir)) return [];
  const files = [];
  for (const entry of readdirSync(dir)) {
    const fullPath = join(dir, entry);
    const stat = statSync(fullPath);
    if (stat.isDirectory()) {
      files.push(...listMetricFiles(fullPath));
    } else if (
      METRIC_FILE_PATTERN.test(entry) &&
      !entry.endsWith(".test.ts") &&
      !entry.endsWith(".test.tsx") &&
      entry !== "reviewed.json"
    ) {
      files.push(fullPath);
    }
  }
  return files;
}

/**
 * Extracts the metric name and metadata fields from a catalog source file.
 * Catalog files are plain data modules, so a lightweight parse of the
 * `name:` and `metadata` object keys is sufficient and avoids executing
 * untrusted code in CI.
 */
function parseMetricDefinition(source) {
  const nameMatch = source.match(/\bname\s*:\s*["'`]([^"'`]+)["'`]/);
  const name = nameMatch ? nameMatch[1] : null;

  const missing = [];
  for (const field of REQUIRED_METADATA_FIELDS) {
    const fieldPattern = new RegExp(`\\b${field}\\s*:`);
    if (!fieldPattern.test(source)) missing.push(field);
  }

  const privacyMatch = source.match(/\bprivacyClass\s*:\s*["'`]([^"'`]+)["'`]/);
  const deprecationMatch = source.match(
    /\bdeprecation\s*:\s*["'`]([^"'`]+)["'`]/,
  );

  return {
    name,
    missing,
    privacyClass: privacyMatch ? privacyMatch[1] : null,
    deprecation: deprecationMatch ? deprecationMatch[1] : null,
  };
}

function loadReviewedAllowlist() {
  if (!existsSync(REVIEWED_ALLOWLIST)) return new Set();
  try {
    const parsed = JSON.parse(readFileSync(REVIEWED_ALLOWLIST, "utf8"));
    const entries = Array.isArray(parsed) ? parsed : parsed.reviewed ?? [];
    return new Set(entries.map((entry) => String(entry)));
  } catch (error) {
    console.error(
      `❌ Could not parse ${relative(process.cwd(), REVIEWED_ALLOWLIST)}: ${error.message}`,
    );
    process.exit(1);
  }
}

function changedCatalogFiles() {
  const baseRef = process.env.METRIC_CATALOG_BASE_REF || "origin/main";
  try {
    const output = execFileSync(
      "git",
      ["diff", "--name-only", `${baseRef}...HEAD`, "--", "src/metrics/catalog"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    );
    return output
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
  } catch {
    // No git history / base ref available (e.g. shallow clone): skip the
    // reviewed-changes check rather than failing the whole build.
    return null;
  }
}

function checkMetricCatalog({ checkReviewed }) {
  const files = listMetricFiles(CATALOG_DIR);
  const errors = [];
  const seen = new Map();

  for (const file of files) {
    const relPath = relative(process.cwd(), file);
    const source = readFileSync(file, "utf8");
    const definition = parseMetricDefinition(source);

    if (!definition.name) {
      errors.push(`${relPath}: missing metric "name".`);
      continue;
    }

    if (seen.has(definition.name)) {
      errors.push(
        `Duplicate metric name "${definition.name}" in ${relPath} ` +
          `(already defined in ${seen.get(definition.name)}).`,
      );
    } else {
      seen.set(definition.name, relPath);
    }

    if (definition.missing.length > 0) {
      errors.push(
        `${relPath}: metric "${definition.name}" is missing required ` +
          `metadata field(s): ${definition.missing.join(", ")}.`,
      );
    }

    if (
      definition.privacyClass &&
      !PRIVACY_CLASSES.has(definition.privacyClass)
    ) {
      errors.push(
        `${relPath}: metric "${definition.name}" has invalid privacyClass ` +
          `"${definition.privacyClass}" (expected one of ${[...PRIVACY_CLASSES].join(", ")}).`,
      );
    }

    if (definition.deprecation && !DEPRECATION_STATES.has(definition.deprecation)) {
      errors.push(
        `${relPath}: metric "${definition.name}" has invalid deprecation ` +
          `state "${definition.deprecation}" (expected one of ${[...DEPRECATION_STATES].join(", ")}).`,
      );
    }
  }

  if (checkReviewed) {
    const changed = changedCatalogFiles();
    if (changed) {
      const reviewed = loadReviewedAllowlist();
      for (const file of changed) {
        if (file.endsWith("reviewed.json")) continue;
        if (!reviewed.has(file)) {
          errors.push(
            `${file}: metric definition changed without review. Add it to ` +
              `src/metrics/catalog/reviewed.json after a reviewer signs off.`,
          );
        }
      }
    }
  }

  if (errors.length > 0) {
    console.error("\n❌ Metric catalog validation failed:\n");
    for (const error of errors) console.error(`  - ${error}`);
    console.error(
      "\nEvery metric must document owner, query, sources, freshness, " +
        "privacyClass, and deprecation, and names must be unique " +
        "(see src/metrics/catalog/README.md).\n",
    );
    process.exit(1);
  }

  console.log(
    `✅ Metric catalog valid (${seen.size} metric${seen.size === 1 ? "" : "s"}).`,
  );
}

function findPageFiles(dir) {
  const found = [];
  for (const entry of readdirSync(dir)) {
    const fullPath = join(dir, entry);
    const stat = statSync(fullPath);
    if (stat.isDirectory()) {
      found.push(...findPageFiles(fullPath));
    } else if (PAGE_FILE_PATTERN.test(entry)) {
      found.push(fullPath);
    }
  }
  return found;
}

function main() {
  const topLevelEntries = readdirSync(APP_DIR);
  const offendingPages = [];

  for (const entry of topLevelEntries) {
    const fullPath = join(APP_DIR, entry);
    const stat = statSync(fullPath);

    if (stat.isDirectory()) {
      if (ALLOWED_TOP_LEVEL_DIRS.has(entry)) continue;
      offendingPages.push(...findPageFiles(fullPath));
    } else if (PAGE_FILE_PATTERN.test(entry)) {
      offendingPages.push(fullPath);
    } else if (!ALLOWED_TOP_LEVEL_FILES.has(entry)) {
      // Non-page files (helpers, styles) outside an allowed dir are fine;
      // only page.tsx files define a navigable route.
      continue;
    }
  }

  if (offendingPages.length > 0) {
    console.error(
      "\n❌ Found page route(s) outside src/app/[locale]/. These are " +
        "unreachable in production (next-intl's middleware always redirects " +
        "to a locale-prefixed path first) and should live under " +
        "src/app/[locale]/ instead:\n",
    );
    for (const page of offendingPages) {
      console.error(`  - ${relative(process.cwd(), page)}`);
    }
    console.error(
      "\nMove the page under src/app/[locale]/, or add its directory to " +
        "ALLOWED_TOP_LEVEL_DIRS in scripts/check-no-duplicate-routes.mjs " +
        "if it's genuinely meant to be non-localized.\n",
    );
    process.exit(1);
  }

  console.log("✅ No duplicate non-localized routes found.");
}

const checkReviewed =
  process.argv.includes("--check-reviewed") ||
  process.env.CI === "true" ||
  process.env.CI === "1";

checkMetricCatalog({ checkReviewed });
main();
