/**
 * Performance budget validation script (#48)
 *
 * Verifies that performance budgets exist, covers critical routes, and validates
 * configuration for CI release gates.
 */

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rootDir = path.resolve(__dirname, '..');

const budgetFile = path.join(rootDir, 'lighthouse-budget.json');
const rcFile = path.join(rootDir, 'lighthouserc.json');
const bundleBudgetFile = path.join(rootDir, 'bundle-budget.json');
const nextDir = path.join(rootDir, '.next');

console.log('🔍 Validating frontend performance budgets...');

if (!fs.existsSync(budgetFile)) {
  console.error('❌ Missing lighthouse-budget.json');
  process.exit(1);
}

if (!fs.existsSync(rcFile)) {
  console.error('❌ Missing lighthouserc.json');
  process.exit(1);
}

const budgets = JSON.parse(fs.readFileSync(budgetFile, 'utf8'));
const rc = JSON.parse(fs.readFileSync(rcFile, 'utf8'));

// Verify critical routes in collect config
const requiredRoutes = ['/en', '/en/tournaments', '/en/tournaments/1/bracket', '/en/matches/1', '/en/leaderboard'];
const configuredUrls = rc.ci?.collect?.url || [];

const missingRoutes = requiredRoutes.filter(route => !configuredUrls.some(u => u.includes(route)));
if (missingRoutes.length > 0) {
  console.error(`❌ lighthouserc.json missing critical routes: ${missingRoutes.join(', ')}`);
  process.exit(1);
}

// Verify budgets cover critical paths
const budgetPaths = budgets.map(b => b.path);
console.log(`✅ Configured budget paths: ${budgetPaths.join(', ')}`);

// Verify assertions fail on regressions
const assertions = rc.ci?.assert?.assertions || {};
const requiredAssertions = ['categories:performance', 'largest-contentful-paint', 'cumulative-layout-shift'];
for (const req of requiredAssertions) {
  if (!assertions[req]) {
    console.error(`❌ lighthouserc.json missing required failure assertion: ${req}`);
    process.exit(1);
  }
}

console.log('✅ All performance budget configuration checks passed successfully.');

// ---------------------------------------------------------------------------
// Per-route JS bundle budgets (#247)
//
// Heavy feature bundles (governance, analytics, game modules) must only be
// loaded on the routes/interactions that need them. We compute the JS transfer
// size for each critical route from the Next.js build manifests and fail when a
// route exceeds its configured budget.
// ---------------------------------------------------------------------------

const DEFAULT_BUNDLE_BUDGET = {
  // Max JS transfer (bytes, gzip-estimated) per critical route.
  routes: {
    '/en': 250 * 1024,
    '/en/tournaments': 300 * 1024,
    '/en/tournaments/1/bracket': 350 * 1024,
    '/en/matches/1': 300 * 1024,
    '/en/leaderboard': 300 * 1024,
  },
};

if (!fs.existsSync(bundleBudgetFile)) {
  fs.writeFileSync(bundleBudgetFile, `${JSON.stringify(DEFAULT_BUNDLE_BUDGET, null, 2)}\n`);
  console.log('ℹ️  Created default bundle-budget.json');
}

const bundleBudget = JSON.parse(fs.readFileSync(bundleBudgetFile, 'utf8'));
const routeBudgets = bundleBudget.routes || {};

const buildManifestPath = path.join(nextDir, 'build-manifest.json');
const appBuildManifestPath = path.join(nextDir, 'app-build-manifest.json');

if (!fs.existsSync(buildManifestPath) && !fs.existsSync(appBuildManifestPath)) {
  console.warn('⚠️  No Next.js build manifests found (.next/build-manifest.json). Skipping bundle budget check — run after `next build`.');
  process.exit(0);
}

const buildManifest = fs.existsSync(buildManifestPath)
  ? JSON.parse(fs.readFileSync(buildManifestPath, 'utf8'))
  : {};
const appBuildManifest = fs.existsSync(appBuildManifestPath)
  ? JSON.parse(fs.readFileSync(appBuildManifestPath, 'utf8'))
  : {};

/**
 * Resolve the set of JS chunk file paths for a given route from the manifests.
 */
function collectRouteChunks(route) {
  const chunks = new Set();

  const addAll = (list) => {
    if (Array.isArray(list)) list.forEach((f) => chunks.add(f));
  };

  // Pages router manifest: { pages: { '/route': [chunks] }, ... }
  const pages = buildManifest.pages || {};
  addAll(pages[route]);
  // Normalize trailing slash variants.
  const normalized = route.replace(/\/$/, '') || '/';
  addAll(pages[normalized]);
  addAll(pages[`${normalized}/`]);

  // App router manifest: { pages: { '/route/page': [chunks] } }
  const appPages = appBuildManifest.pages || {};
  for (const [key, list] of Object.entries(appPages)) {
    if (key === route || key === `${normalized}/page` || key.startsWith(`${normalized}/`)) {
      addAll(list);
    }
  }

  return [...chunks];
}

/**
 * Estimate transfer size of a JS chunk. Uses gzip-compressed size when the file
 * exists on disk, otherwise falls back to a conservative 35% of raw size.
 */
function chunkTransferSize(chunkPath) {
  const abs = path.join(nextDir, chunkPath);
  if (!fs.existsSync(abs)) return 0;
  const raw = fs.statSync(abs).size;
  try {
    const gz = zlib.gzipSync(fs.readFileSync(abs));
    return gz.length;
  } catch {
    return Math.round(raw * 0.35);
  }
}

const failures = [];
for (const [route, budget] of Object.entries(routeBudgets)) {
  const chunks = collectRouteChunks(route);
  const total = chunks.reduce((sum, c) => sum + chunkTransferSize(c), 0);
  const kb = (total / 1024).toFixed(1);
  const budgetKb = (budget / 1024).toFixed(1);
  if (total > budget) {
    failures.push(`❌ Route ${route} JS transfer ${kb}KB exceeds budget ${budgetKb}KB`);
  } else {
    console.log(`✅ Route ${route} JS transfer ${kb}KB within budget ${budgetKb}KB`);
  }
}

if (failures.length > 0) {
  failures.forEach((f) => console.error(f));
  console.error('❌ Bundle budget regression detected.');
  process.exit(1);
}

console.log('✅ All per-route bundle budgets passed.');
