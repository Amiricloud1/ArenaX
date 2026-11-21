/**
 * Formatting helpers (#704 – Enhanced Utilities Library)
 *
 * Currency (XLM / tokens / NGN), dates, addresses, durations, file sizes.
 * No runtime dependencies beyond the standard browser/Node.js globals.
 *
 * Locale-aware (#245): every formatter accepts an optional BCP-47 `locale`
 * and delegates to Intl. Numeric/sign order is always preserved semantically
 * (the sign is attached to the number by Intl, never re-ordered by RTL
 * bidi layout), so RTL locales render correct values.
 */

import { DEFAULT_LOCALE, normalizeLocale } from "../i18n/locale";

// ─── Internal Intl helpers ───────────────────────────────────────────────────

function resolveLocale(locale?: string): string {
  return normalizeLocale(locale ?? DEFAULT_LOCALE);
}

function safeNumberFormat(
  locale: string,
  options: Intl.NumberFormatOptions
): Intl.NumberFormat | null {
  try {
    return new Intl.NumberFormat(locale, options);
  } catch {
    return null;
  }
}

// ─── Stellar / Token Formatting ──────────────────────────────────────────────

/**
 * Formats a XLM amount with up to 7 decimal places and appends ' XLM'.
 * @param amount  - numeric value or numeric string
 * @param decimals - number of decimal places (0–7, default 7)
 * @param locale  - BCP-47 locale (defaults to the active locale)
 */
export function formatXLM(
  amount: number | string,
  decimals = 7,
  locale?: string
): string {
  const n = typeof amount === "string" ? parseFloat(amount) : amount;
  const clamped = Math.max(0, Math.min(7, Math.round(decimals)));
  const loc = resolveLocale(locale);
  if (!isFinite(n)) {
    const zero = safeNumberFormat(loc, {
      minimumFractionDigits: clamped,
      maximumFractionDigits: clamped,
    });
    return `${zero ? zero.format(0) : (0).toFixed(clamped)} XLM`;
  }
  const nf = safeNumberFormat(loc, {
    minimumFractionDigits: clamped,
    maximumFractionDigits: clamped,
  });
  return `${nf ? nf.format(n) : n.toFixed(clamped)} XLM`;
}

/**
 * Formats a token amount with the given symbol.
 * @param amount   - numeric value or numeric string
 * @param symbol   - token ticker (e.g. 'AX', 'USDC')
 * @param decimals - number of decimal places (default 2)
 * @param locale   - BCP-47 locale (defaults to the active locale)
 */
export function formatToken(
  amount: number | string,
  symbol: string,
  decimals = 2,
  locale?: string
): string {
  const n = typeof amount === "string" ? parseFloat(amount) : amount;
  const clamped = Math.max(0, decimals);
  const loc = resolveLocale(locale);
  const nf = safeNumberFormat(loc, {
    minimumFractionDigits: clamped,
    maximumFractionDigits: clamped,
  });
  if (!isFinite(n)) {
    return `${nf ? nf.format(0) : `0.${"0".repeat(clamped)}`} ${symbol}`;
  }
  return `${nf ? nf.format(n) : n.toFixed(clamped)} ${symbol}`;
}

/**
 * Formats a value as Nigerian Naira using the given locale (default en-NG).
 * Falls back to a manual prefix if Intl is unavailable.
 */
export function formatNGN(amount: number | string, locale?: string): string {
  const n = typeof amount === "string" ? parseFloat(amount) : amount;
  const loc = resolveLocale(locale);
  const nf = safeNumberFormat(loc, {
    style: "currency",
    currency: "NGN",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
  if (!isFinite(n)) {
    return nf ? nf.format(0) : "₦0.00";
  }
  if (nf) return nf.format(n);
  const sign = n < 0 ? "-" : "";
  return `${sign}₦${Math.abs(n).toFixed(2).replace(/\B(?=(\d{3})+(?!\d))/g, ",")}`;
}

/**
 * Abbreviates large numbers with K / M / B suffixes.
 * @example formatLargeNumber(1_500_000) → '1.5M'
 */
export function formatLargeNumber(n: number, locale?: string): string {
  if (!isFinite(n)) return "0";
  const loc = resolveLocale(locale);
  const abs = Math.abs(n);
  const sign = n < 0 ? "-" : "";
  const nf = safeNumberFormat(loc, {
    minimumFractionDigits: 0,
    maximumFractionDigits: 1,
  });
  const fmt = (v: number) => (nf ? nf.format(v) : v.toFixed(1).replace(/\.0$/, ""));
  if (abs >= 1_000_000_000) return `${sign}${fmt(abs / 1_000_000_000)}B`;
  if (abs >= 1_000_000) return `${sign}${fmt(abs / 1_000_000)}M`;
  if (abs >= 1_000) return `${sign}${fmt(abs / 1_000)}K`;
  const plain = safeNumberFormat(loc, { maximumFractionDigits: 0 });
  return `${sign}${plain ? plain.format(abs) : abs}`;
}

// ─── Address Truncation ───────────────────────────────────────────────────────

/**
 * Truncates a Stellar address to a readable format, e.g. 'GBXXXX...YYYY'.
 * @param address - full 56-char Stellar public key
 * @param start   - chars to keep at the start (default 6)
 * @param end     - chars to keep at the end (default 4)
 */
export function truncateStellarAddress(
  address: string,
  start = 6,
  end = 4
): string {
  if (typeof address !== "string" || address.length === 0) return "";
  if (address.length <= start + end) return address;
  return `${address.slice(0, start)}...${address.slice(-end)}`;
}

// ─── Date & Time Formatting ───────────────────────────────────────────────────

const MONTH_NAMES = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/**
 * Converts a timestamp to a human-readable string: 'DD MMM YYYY, HH:MM'.
 * @param ts     - Unix timestamp (ms), ISO string, or Date
 * @param locale - BCP-47 locale (defaults to the active locale)
 */
export function formatTimestamp(
  ts: number | string | Date,
  locale?: string
): string {
  const d = ts instanceof Date ? ts : new Date(ts);
  if (isNaN(d.getTime())) return "Invalid Date";
  const loc = resolveLocale(locale);
  try {
    return new Intl.DateTimeFormat(loc, {
      day: "2-digit",
      month: "short",
      year: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    }).format(d);
  } catch {
    const dd = String(d.getDate()).padStart(2, "0");
    const mon = MONTH_NAMES[d.getMonth()];
    const yyyy = d.getFullYear();
    const hh = String(d.getHours()).padStart(2, "0");
    const mm = String(d.getMinutes()).padStart(2, "0");
    return `${dd} ${mon} ${yyyy}, ${hh}:${mm}`;
  }
}

/**
 * Converts a duration in milliseconds to a readable string.
 * @example formatDuration(9000000) → '2h 30m'
 * @example formatDuration(45000)   → '45s'
 */
export function formatDuration(ms: number, locale?: string): string {
  if (!isFinite(ms) || ms < 0) return "0s";
  const loc = resolveLocale(locale);
  const totalSeconds = Math.floor(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const nf = safeNumberFormat(loc, { maximumFractionDigits: 0 });
  const num = (v: number) => (nf ? nf.format(v) : String(v));
  const parts: string[] = [];
  if (hours > 0) parts.push(`${num(hours)}h`);
  if (minutes > 0) parts.push(`${num(minutes)}m`);
  if (seconds > 0 || parts.length === 0) parts.push(`${num(seconds)}s`);
  return parts.join(" ");
}

// ─── File Size Formatting ─────────────────────────────────────────────────────

/**
 * Formats a byte count as a human-readable file size.
 * @example formatFileSize(1536) → '1.50 KB'
 */
export function formatFileSize(bytes: number): string {
  if (!isFinite(bytes) || bytes < 0) return "0 B";
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log2(bytes) / 10), units.length - 1);
  const value = bytes / Math.pow(1024, i);
  return i === 0 ? `${bytes} B` : `${value.toFixed(2)} ${units[i]}`;
}

// ─── Miscellaneous ────────────────────────────────────────────────────────────

/**
 * Formats a percentage from a value / total ratio.
 * @param value    - numerator
 * @param total    - denominator
 * @param decimals - decimal places (default 1)
 * @example formatPercentage(45, 100) → '45.0%'
 */
export function formatPercentage(
  value: number,
  total: number,
  decimals = 1,
  locale?: string
): string {
  const loc = resolveLocale(locale);
  const clamped = Math.max(0, decimals);
  if (!isFinite(value) || !isFinite(total) || total === 0) {
    const zero = safeNumberFormat(loc, {
      style: "percent",
      minimumFractionDigits: clamped,
      maximumFractionDigits: clamped,
    });
    return zero ? zero.format(0) : "0%";
  }
  const pct = value / total;
  const nf = safeNumberFormat(loc, {
    style: "percent",
    minimumFractionDigits: clamped,
    maximumFractionDigits: clamped,
  });
  return nf ? nf.format(pct) : `${(pct * 100).toFixed(clamped)}%`;
}

/**
 * Returns the ordinal string for a number.
 * @example formatOrdinal(1) → '1st'
 * @example formatOrdinal(11) → '11th'
 */
export function formatOrdinal(n: number, locale?: string): string {
  const loc = resolveLocale(locale);
  const abs = Math.abs(Math.floor(n));
  const nf = safeNumberFormat(loc, { maximumFractionDigits: 0 });
  const num = nf ? nf.format(n) : String(n);
  const mod100 = abs % 100;
  if (mod100 >= 11 && mod100 <= 13) return `${num}th`;
  switch (abs % 10) {
    case 1:
      return `${num}st`;
    case 2:
      return `${num}nd`;
    case 3:
      return `${num}rd`;
    default:
      return `${num}th`;
  }
}
