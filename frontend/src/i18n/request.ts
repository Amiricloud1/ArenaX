import { notFound } from "next/navigation";
import { getRequestConfig } from "next-intl/server";
import { routing } from "./routing";

/**
 * Server-side i18n request configuration.
 *
 * Messages are loaded from per-locale JSON files that contain all
 * namespaces (common, auth, gameplay, settings, errors).  The import
 * is dynamic so each locale bundle is only fetched when that locale
 * is actually served — non-active locales are never sent to the client.
 */
export default getRequestConfig(async ({ locale }) => {
  // Reject unknown locales with a 404 instead of a cryptic runtime error.
  if (!routing.locales.includes(locale as (typeof routing.locales)[number])) {
    notFound();
  }

  const messages = (
    await import(`../messages/${locale}.json`)
  ).default;

  return {
    messages,
    // Forward the timezone so server and client agree on date formatting.
    timeZone: "UTC",
    // Provide a global onError handler so missing keys are surfaced in
    // dev/staging rather than silently returning the key string.
    onError(error) {
      if (process.env.NODE_ENV !== "production") {
        console.warn("[i18n] Translation error:", error.message);
      }
    },
    // Return the raw key path on missing key so the UI still renders
    // something meaningful (shows developers exactly which key is absent).
    getMessageFallback({ namespace, key }) {
      return [namespace, key].filter(Boolean).join(".");
    },
  };
});

/**
 * Locale-aware return destination validation.
 *
 * Return destinations restored after authentication must be local,
 * approved paths.  Locale-prefixed routes (e.g. `/en/tournaments/123`)
 * must be validated against the known locale list so that a crafted
 * prefix cannot be used to smuggle an off-site or unapproved path.
 *
 * This helper is exported so the auth redirect boundary (login,
 * registration, OAuth callback) can share the exact same validation
 * rules used by the i18n layer, keeping locale handling consistent.
 */
const APPROVED_PATH_PREFIXES = [
  "/tournaments",
  "/matches",
  "/dashboard",
  "/settings",
  "/invitations",
] as const;

const LOCALE_PREFIX_PATTERN = /^\/([a-z]{2}(?:-[A-Za-z]{2,})?)(?=\/|$)/;

/**
 * Strips a leading locale prefix from a path if present.
 * Returns the locale (or undefined) and the remaining path.
 */
export function splitLocalePrefix(path: string): {
  locale: string | undefined;
  path: string;
} {
  const match = path.match(LOCALE_PREFIX_PATTERN);
  if (!match) {
    return { locale: undefined, path };
  }
  const candidate = match[1];
  if (!routing.locales.includes(candidate as (typeof routing.locales)[number])) {
    // Unknown prefix — treat the whole thing as the path so validation
    // can reject it rather than silently accepting an arbitrary segment.
    return { locale: undefined, path };
  }
  return { locale: candidate, path: path.slice(match[0].length) || "/" };
}

/**
 * Validates a return destination and returns a safe, locale-prefixed
 * local path, or `null` if the destination is not approved.
 *
 * Rules:
 *  - Must be a relative path starting with a single `/` (no protocol,
 *    no `//` protocol-relative URLs, no backslashes).
 *  - Must not contain control characters or encoded traversal.
 *  - After stripping an optional known locale prefix, the remaining
 *    path must start with an approved prefix.
 *  - Query parameters are preserved only when they are non-secret and
 *    explicitly allow-listed.
 */
export function sanitizeReturnDestination(
  raw: string | null | undefined,
  fallbackLocale?: string,
): string | null {
  if (!raw) return null;

  let decoded: string;
  try {
    decoded = decodeURIComponent(raw);
  } catch {
    // Malformed percent-encoding — reject outright.
    return null;
  }

  // Reject protocol-relative, absolute, or backslash-based paths.
  if (
    !decoded.startsWith("/") ||
    decoded.startsWith("//") ||
    decoded.includes("\\") ||
    // eslint-disable-next-line no-control-regex
    /[\u0000-\u001f\u007f]/.test(decoded)
  ) {
    return null;
  }

  // Reject encoded traversal segments after decoding.
  if (/(^|\/)\.\.(\/|$)/.test(decoded)) {
    return null;
  }

  const [rawPath, rawQuery = ""] = decoded.split("?", 2);
  const { locale, path } = splitLocalePrefix(rawPath);

  const isApproved = APPROVED_PATH_PREFIXES.some(
    (prefix) => path === prefix || path.startsWith(`${prefix}/`),
  );
  if (!isApproved) {
    return null;
  }

  // Preserve only non-secret, allow-listed query parameters.
  const allowedParams = new Set(["invite", "ref", "utm_source", "utm_medium"]);
  const params = new URLSearchParams(rawQuery);
  const safeParams = new URLSearchParams();
  for (const [key, value] of params.entries()) {
    if (allowedParams.has(key) && value.length <= 256) {
      safeParams.append(key, value);
    }
  }

  const effectiveLocale = locale ?? fallbackLocale;
  const prefix = effectiveLocale ? `/${effectiveLocale}` : "";
  const query = safeParams.toString();
  return `${prefix}${path}${query ? `?${query}` : ""}`;
}