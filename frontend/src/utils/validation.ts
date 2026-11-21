/**
 * Validation helpers (#704 – Enhanced Utilities Library)
 *
 * Pure functions with no runtime dependencies.
 * All validators return a boolean and never throw.
 */

/**
 * Validates an email address against a practical RFC5322-like pattern.
 * Handles the vast majority of real-world addresses without over-engineering.
 */
export function isValidEmail(email: string): boolean {
  if (typeof email !== "string" || email.trim().length === 0) return false;
  // Local part @ domain.tld  (local ≤ 64 chars, total ≤ 254)
  const pattern =
    /^[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*\.[a-zA-Z]{2,}$/;
  return email.length <= 254 && pattern.test(email.trim());
}

/**
 * Validates a Stellar public key (G… 56-char base32 string).
 */
export function isValidStellarAddress(address: string): boolean {
  if (typeof address !== "string") return false;
  // Stellar public keys: exactly 56 chars, start with 'G', base32 alphabet
  return /^G[A-Z2-7]{55}$/.test(address);
}

/**
 * Validates a Stellar transaction memo (max 28 bytes UTF-8).
 */
export function isValidStellarMemo(memo: string): boolean {
  if (typeof memo !== "string") return false;
  const byteLength = new TextEncoder().encode(memo).length;
  return byteLength <= 28;
}

/**
 * Validates a username:
 * – 3 to 20 characters
 * – Alphanumeric + underscore only
 * – Must not start or end with an underscore
 */
export function isValidUsername(username: string): boolean {
  if (typeof username !== "string") return false;
  return /^[a-zA-Z0-9][a-zA-Z0-9_]{1,18}[a-zA-Z0-9]$|^[a-zA-Z0-9]{1,20}$/.test(
    username
  ) && !username.startsWith("_") && !username.endsWith("_") &&
    username.length >= 3 && username.length <= 20;
}

/**
 * Validates a phone number in E.164 format.
 * Allows an optional leading '+', then 7–15 digits.
 */
export function isValidPhoneNumber(phone: string): boolean {
  if (typeof phone !== "string") return false;
  return /^\+?[0-9]{7,15}$/.test(phone.trim());
}

/**
 * Returns true if value is a number (or numeric string) and > 0.
 */
export function isPositiveNumber(value: unknown): boolean {
  if (value === null || value === undefined || value === "") return false;
  const n = Number(value);
  return isFinite(n) && n > 0;
}

/**
 * Returns true if value can be parsed as a positive, finite number.
 * Accepts both string and number inputs.
 */
export function isValidAmount(amount: string | number): boolean {
  if (amount === "" || amount === null || amount === undefined) return false;
  const n = typeof amount === "string" ? parseFloat(amount) : amount;
  return isFinite(n) && n > 0;
}

/**
 * Type guard: returns true (and narrows to `string`) if value is a non-empty
 * string after trimming.
 */
export function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

/**
 * Returns true if the string is a valid http:// or https:// URL.
 */
export function isValidUrl(url: string): boolean {
  if (typeof url !== "string" || url.trim().length === 0) return false;
  try {
    const parsed = new URL(url.trim());
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

/* ------------------------------------------------------------------ */
/* Return-destination validation (#242)                               */
/* ------------------------------------------------------------------ */

/**
 * Approved top-level route segments. Anything not listed here is rejected
 * so that a crafted `?returnTo=` cannot bounce a user to an arbitrary page.
 */
export const APPROVED_RETURN_ROUTE_PREFIXES: readonly string[] = [
  "tournaments",
  "matches",
  "dashboard",
  "profile",
  "settings",
  "wallet",
  "notifications",
];

/**
 * Query parameters that are safe to carry across an auth redirect.
 * Everything else (tokens, secrets, PII) is dropped.
 */
export const APPROVED_RETURN_QUERY_PARAMS: readonly string[] = [
  "tab",
  "view",
  "filter",
  "sort",
  "page",
  "q",
  "invite",
];

/** Supported locale prefixes (e.g. `/en/tournaments/...`). */
export const SUPPORTED_LOCALES: readonly string[] = [
  "en",
  "es",
  "fr",
  "de",
  "pt",
  "ar",
  "zh",
];

/** Auth routes that must never be used as a return destination (loop guard). */
const AUTH_ROUTE_PREFIXES: readonly string[] = [
  "login",
  "register",
  "signup",
  "auth",
  "oauth",
  "callback",
  "forgot-password",
  "reset-password",
  "verify",
];

const MAX_RETURN_DESTINATION_LENGTH = 2048;

/**
 * Returns true if the raw string looks like an absolute URL, a
 * protocol-relative URL, or a backslash-tricked URL — all of which are
 * open-redirect vectors and must be rejected outright.
 */
function isAbsoluteOrProtocolRelative(raw: string): boolean {
  // `//evil.com`, `\\evil.com`, `/\evil.com`, `\/evil.com`
  if (/^[\\/]{2}/.test(raw)) return true;
  if (/^[\\/][^\\/]/.test(raw) && /^[\\/]{1}[\\]/.test(raw)) return true;
  // `scheme:` (http:, javascript:, data:, mailto:, …)
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(raw)) return true;
  return false;
}

/**
 * Decodes a value once. Returns `null` when the value is still encoded
 * after a single decode (double-encoded payloads are rejected) or when
 * decoding throws.
 */
function decodeOnce(value: string): string | null {
  let decoded: string;
  try {
    decoded = decodeURIComponent(value);
  } catch {
    return null;
  }
  // If it still contains an encoded scheme/authority after one decode,
  // treat it as a double-encoded open-redirect attempt.
  if (/%[0-9a-fA-F]{2}/.test(decoded) && /%2f|%3a|%5c/i.test(decoded)) {
    return null;
  }
  return decoded;
}

/**
 * Validates and normalises a return destination.
 *
 * Only local, approved paths are accepted. Locale prefixes are preserved.
 * Only approved, non-secret query parameters survive. Returns `null` for
 * anything unsafe (absolute URLs, protocol-relative URLs, encoded URLs,
 * auth routes, unknown route prefixes).
 */
export function sanitizeReturnDestination(
  raw: string | null | undefined
): string | null {
  if (typeof raw !== "string") return null;
  const trimmed = raw.trim();
  if (trimmed.length === 0 || trimmed.length > MAX_RETURN_DESTINATION_LENGTH) {
    return null;
  }

  // Reject absolute / protocol-relative / scheme URLs before decoding.
  if (isAbsoluteOrProtocolRelative(trimmed)) return null;

  const decoded = decodeOnce(trimmed);
  if (decoded === null) return null;
  // Re-check after decoding to catch `%2F%2Fevil.com` style payloads.
  if (isAbsoluteOrProtocolRelative(decoded)) return null;
  if (!decoded.startsWith("/")) return null;
  // Reject control characters and whitespace tricks.
  if (/[\u0000-\u001f\u007f\s]/.test(decoded)) return null;

  let parsed: URL;
  try {
    // Base is a placeholder; we only inspect pathname/search.
    parsed = new URL(decoded, "http://local.invalid");
  } catch {
    return null;
  }
  if (parsed.origin !== "http://local.invalid") return null;

  const segments = parsed.pathname.split("/").filter(Boolean);
  if (segments.length === 0) return null;

  let index = 0;
  if (SUPPORTED_LOCALES.includes(segments[0])) {
    index = 1;
  }
  const routeSegment = segments[index];
  if (!routeSegment) return null;
  if (AUTH_ROUTE_PREFIXES.includes(routeSegment)) return null;
  if (!APPROVED_RETURN_ROUTE_PREFIXES.includes(routeSegment)) return null;

  // Preserve only approved, non-secret query params.
  const safeParams = new URLSearchParams();
  parsed.searchParams.forEach((value, key) => {
    if (!APPROVED_RETURN_QUERY_PARAMS.includes(key)) return;
    if (value.length > 256) return;
    safeParams.append(key, value);
  });

  const query = safeParams.toString();
  const pathname = parsed.pathname.replace(/\/{2,}/g, "/");
  return query ? `${pathname}?${query}` : pathname;
}

/**
 * Returns true when `raw` is a safe return destination.
 */
export function isValidReturnDestination(raw: string | null | undefined): boolean {
  return sanitizeReturnDestination(raw) !== null;
}

/** Storage key used to persist the pending return destination. */
export const RETURN_DESTINATION_STORAGE_KEY = "pendingReturnDestination";

/**
 * Minimal storage contract so this module stays runtime-agnostic and
 * testable without a browser.
 */
export interface ReturnDestinationStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function defaultStorage(): ReturnDestinationStorage | null {
  try {
    if (typeof window !== "undefined" && window.sessionStorage) {
      return window.sessionStorage;
    }
  } catch {
    // Access to storage can throw in privacy modes.
  }
  return null;
}

/**
 * Persists a validated return destination. Unsafe values are ignored and
 * any previously stored value is cleared so a stale destination cannot be
 * replayed.
 */
export function storeReturnDestination(
  raw: string | null | undefined,
  storage: ReturnDestinationStorage | null = defaultStorage()
): string | null {
  const safe = sanitizeReturnDestination(raw);
  if (!storage) return safe;
  try {
    if (safe) {
      storage.setItem(RETURN_DESTINATION_STORAGE_KEY, safe);
    } else {
      storage.removeItem(RETURN_DESTINATION_STORAGE_KEY);
    }
  } catch {
    // Ignore storage failures (quota, privacy mode).
  }
  return safe;
}

/**
 * Reads and clears the stored return destination. Because the value is
 * removed on read, the destination is restored exactly once — preventing
 * redirect loops when the resource is private or unavailable.
 */
export function consumeReturnDestination(
  storage: ReturnDestinationStorage | null = defaultStorage()
): string | null {
  if (!storage) return null;
  let stored: string | null = null;
  try {
    stored = storage.getItem(RETURN_DESTINATION_STORAGE_KEY);
    storage.removeItem(RETURN_DESTINATION_STORAGE_KEY);
  } catch {
    return null;
  }
  return sanitizeReturnDestination(stored);
}

/**
 * Resolves the destination to use after authentication.
 *
 * Precedence: explicit `returnTo` query value → stored destination →
 * `fallback`. The stored value is always consumed so it cannot be replayed.
 */
export function resolvePostAuthDestination(
  returnTo: string | null | undefined,
  fallback = "/dashboard",
  storage: ReturnDestinationStorage | null = defaultStorage()
): string {
  const fromQuery = sanitizeReturnDestination(returnTo);
  const fromStorage = consumeReturnDestination(storage);
  return fromQuery ?? fromStorage ?? sanitizeReturnDestination(fallback) ?? "/dashboard";
}
