// Notification types for Global Notification Center

export type NotificationType = "info" | "success" | "warning" | "error" | "match";

export type NotificationPreferences = Record<NotificationType, boolean>;

export type ToastPosition =
  | "top-left"
  | "top-center"
  | "top-right"
  | "bottom-left"
  | "bottom-center"
  | "bottom-right";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

/**
 * A locale-aware plural message descriptor.
 *
 * `key` identifies the message in the catalog. `count` drives the plural
 * category selection via `Intl.PluralRules` for the active locale. `params`
 * supplies any additional ICU-style interpolation values (e.g. names).
 */
export interface PluralMessageDescriptor {
  key: string;
  count: number;
  params?: Record<string, string | number>;
}

/**
 * A locale-aware formatted value descriptor. The `kind` selects the
 * appropriate `Intl` formatter (number, currency, percent, duration, date,
 * time, or relative time) so screens never hand-roll formatting.
 */
export type FormattedValueKind =
  | "number"
  | "currency"
  | "percent"
  | "duration"
  | "date"
  | "time"
  | "datetime"
  | "relative";

export interface FormattedValueDescriptor {
  kind: FormattedValueKind;
  value: number | string | Date;
  /** ISO 4217 currency code, required when `kind` is "currency". */
  currency?: string;
  /** BCP 47 locale override; defaults to the active locale. */
  locale?: string;
  /** Intl.NumberFormat / Intl.DateTimeFormat option overrides. */
  options?: Intl.NumberFormatOptions | Intl.DateTimeFormatOptions;
}

/**
 * A notification message that may be a plain translated string, a plural
 * descriptor, or a formatted value descriptor.
 */
export type LocalizedMessage =
  | string
  | PluralMessageDescriptor
  | FormattedValueDescriptor;

export interface PersistentNotification {
  id: string;
  type: NotificationType;
  title: string;
  message: string;
  /** Optional plural-aware message; when present, screens prefer it over `message`. */
  messageDescriptor?: PluralMessageDescriptor;
  link?: string;
  linkLabel?: string;
  read: boolean;
  createdAt: string;
  /** Pre-formatted, locale-aware timestamp for display (e.g. via Intl.DateTimeFormat). */
  formattedCreatedAt?: string;
  /** Pre-formatted, locale-aware relative time (e.g. "3 minutes ago"). */
  relativeCreatedAt?: string;
  metadata?: Record<string, unknown>;
}

export interface ToastNotification {
  id: string;
  type: NotificationType;
  title: string;
  message?: string;
  /** Optional plural-aware message; when present, screens prefer it over `message`. */
  messageDescriptor?: PluralMessageDescriptor;
  duration?: number;
  createdAt: number;
  /** Pre-formatted, locale-aware timestamp for display. */
  formattedCreatedAt?: string;
  /** Pre-formatted, locale-aware relative time (e.g. "3 minutes ago"). */
  relativeCreatedAt?: string;
  position?: ToastPosition;
  action?: ToastAction;
  showProgress?: boolean;
}