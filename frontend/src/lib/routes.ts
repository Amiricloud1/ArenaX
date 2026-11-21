export interface NavItem {
  label: string;
  href: string;
}

export const mainNav: NavItem[] = [
  { label: "Home", href: "/" },
  { label: "Tournaments", href: "/tournaments" },
  { label: "Wallet", href: "/wallet" },
  { label: "Leaderboard", href: "/leaderboard" },
  { label: "Reputation", href: "/reputation" },
  { label: "Governance", href: "/governance" },
];

export const authNav = {
  authenticated: [
    { label: "Profile", href: "/profile" },
    { label: "Wallet", href: "/wallet" },
  ],
  // #326: Use the same CTA labels on desktop and mobile so the call
  // to action doesn't jump between "Register" / "Get Started" depending
  // on viewport. The primary CTA reads as "Get Started"; the route
  // stays /register because /signup doesn't exist yet.
  unauthenticated: [
    { label: "Login", href: "/login" },
    { label: "Get Started", href: "/register" },
  ],
};

/**
 * #247: Route metadata used by the split/budget logic.
 *
 * Each entry describes a route that pulls in a heavy feature bundle
 * (governance, analytics, or game-only modules). The metadata is the
 * single source of truth for:
 *   - deriving dynamic-import boundaries (heavy feature code is only
 *     loaded when its route or interaction requires it), and
 *   - automated bundle-budget regression checks (see the budget check
 *     script that consumes `HEAVY_FEATURE_ROUTES`).
 *
 * `access` documents the server/client boundary: `public` routes may be
 * rendered without authentication, while `authenticated` routes require
 * a valid session and their private/financial state remains server
 * authoritative.
 */
export type RouteAccess = "public" | "authenticated";

export type HeavyFeatureModule = "governance" | "analytics" | "game";

export interface HeavyFeatureRouteMeta {
  /** Whether the route is reachable without authentication. */
  access: RouteAccess;
  /** The heavy feature bundle this route depends on. */
  feature: HeavyFeatureModule;
  /**
   * Whether the heavy feature module must be loaded eagerly on route
   * entry (critical interaction) or can be deferred until an
   * interaction requires it.
   */
  load: "eager" | "lazy";
}

export const HEAVY_FEATURE_ROUTES: Record<string, HeavyFeatureRouteMeta> = {
  "/governance": { access: "public", feature: "governance", load: "eager" },
  "/governance/proposals": {
    access: "authenticated",
    feature: "governance",
    load: "lazy",
  },
  "/analytics": { access: "authenticated", feature: "analytics", load: "eager" },
  "/analytics/leaderboard": {
    access: "public",
    feature: "analytics",
    load: "lazy",
  },
  "/game": { access: "authenticated", feature: "game", load: "eager" },
  "/game/play": { access: "authenticated", feature: "game", load: "eager" },
};

/**
 * Normalizes a pathname for lookup: strips query/hash, trailing slashes,
 * and collapses repeated slashes so `/governance/` matches `/governance`.
 */
function normalizePathname(pathname: string): string {
  if (!pathname) return "/";
  const withoutQuery = pathname.split(/[?#]/, 1)[0] ?? "/";
  const collapsed = withoutQuery.replace(/\/{2,}/g, "/");
  if (collapsed.length > 1 && collapsed.endsWith("/")) {
    return collapsed.slice(0, -1);
  }
  return collapsed || "/";
}

/**
 * Returns the heavy feature route metadata for `pathname`, or `undefined`
 * when the route does not depend on a heavy feature bundle.
 *
 * Matching is exact after normalization so that unrelated nested paths
 * (e.g. `/governance-archive`) are not accidentally treated as heavy.
 */
export function getHeavyFeatureRoute(
  pathname: string,
): HeavyFeatureRouteMeta | undefined {
  return HEAVY_FEATURE_ROUTES[normalizePathname(pathname)];
}

/**
 * Returns `true` when `pathname` depends on a heavy feature bundle and
 * therefore must be handled by the dynamic-import boundary and budget
 * checks.
 */
export function isHeavyFeatureRoute(pathname: string): boolean {
  return getHeavyFeatureRoute(pathname) !== undefined;
}