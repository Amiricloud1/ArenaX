import { NextResponse } from "next/server";
import type { NextRequest } from "next/server";

const MAX_BATCH_SIZE = 100;
const MAX_EVENTS_IN_MEMORY = 1000;

const eventBuffer: Array<{ event: string; timestamp: number; [key: string]: unknown }> = [];

// ---------------------------------------------------------------------------
// Shared analytics event schema registry (mirrors the canonical registry).
// Each event declares owner, purpose, schema version, required/optional fields,
// and prohibited sensitive attributes. Keep in sync with the client registry.
// ---------------------------------------------------------------------------

type FieldType = "string" | "number" | "boolean" | "object";

interface EventSchema {
  owner: string;
  purpose: string;
  version: number;
  required: Record<string, FieldType>;
  optional: Record<string, FieldType>;
  prohibited: string[];
}

const PROHIBITED_SENSITIVE_ATTRIBUTES = [
  "email",
  "phone",
  "ssn",
  "password",
  "creditCard",
  "cardNumber",
  "cvv",
  "address",
  "fullName",
  "dateOfBirth",
  "ip",
  "ipAddress",
  "authToken",
  "accessToken",
  "refreshToken",
];

const EVENT_SCHEMA_REGISTRY: Record<string, EventSchema> = {
  page_view: {
    owner: "frontend",
    purpose: "product_analytics",
    version: 1,
    required: { path: "string" },
    optional: { referrer: "string", title: "string" },
    prohibited: PROHIBITED_SENSITIVE_ATTRIBUTES,
  },
  button_click: {
    owner: "frontend",
    purpose: "product_analytics",
    version: 1,
    required: { elementId: "string" },
    optional: { label: "string", path: "string" },
    prohibited: PROHIBITED_SENSITIVE_ATTRIBUTES,
  },
  checkout_started: {
    owner: "frontend",
    purpose: "product_analytics",
    version: 1,
    required: { cartId: "string" },
    optional: { itemCount: "number" },
    prohibited: PROHIBITED_SENSITIVE_ATTRIBUTES,
  },
  payment_completed: {
    owner: "server",
    purpose: "financial_audit",
    version: 1,
    required: { orderId: "string", amount: "number" },
    optional: { currency: "string" },
    prohibited: PROHIBITED_SENSITIVE_ATTRIBUTES,
  },
};

const SUPPORTED_VERSIONS: Record<string, number[]> = Object.fromEntries(
  Object.entries(EVENT_SCHEMA_REGISTRY).map(([name, schema]) => [name, [schema.version]])
);

// Events whose purpose is essential and therefore exempt from consent gating.
const ESSENTIAL_PURPOSES = new Set(["financial_audit", "security"]);

// ---------------------------------------------------------------------------
// Authoritative, versioned consent record.
// The consent record is read from a cookie (preferred) or header. It is
// versioned so that older records can be interpreted/migrated explicitly.
// ---------------------------------------------------------------------------

const CONSENT_COOKIE = "analytics_consent";
const CONSENT_HEADER = "x-analytics-consent";
const CURRENT_CONSENT_VERSION = 1;

interface ConsentRecord {
  version: number;
  granted: boolean;
  subjectId?: string;
  withdrawnAt?: number;
}

function parseConsentRecord(raw: string | null | undefined): ConsentRecord | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(decodeURIComponent(raw));
    if (typeof parsed !== "object" || parsed === null) return null;
    const version = typeof parsed.version === "number" ? parsed.version : 0;
    const granted = parsed.granted === true;
    const subjectId = typeof parsed.subjectId === "string" ? parsed.subjectId : undefined;
    const withdrawnAt = typeof parsed.withdrawnAt === "number" ? parsed.withdrawnAt : undefined;
    return { version, granted, subjectId, withdrawnAt };
  } catch {
    return null;
  }
}

function readConsent(request: NextRequest): ConsentRecord | null {
  const cookieValue = request.cookies.get(CONSENT_COOKIE)?.value;
  if (cookieValue) return parseConsentRecord(cookieValue);
  const headerValue = request.headers.get(CONSENT_HEADER);
  if (headerValue) return parseConsentRecord(headerValue);
  return null;
}

// ---------------------------------------------------------------------------
// Per-event validation against the registry.
// ---------------------------------------------------------------------------

interface ValidationResult {
  index: number;
  event: string;
  status: "accepted" | "rejected" | "suppressed";
  reason?: string;
}

function isType(value: unknown, type: FieldType): boolean {
  switch (type) {
    case "string":
      return typeof value === "string";
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "boolean":
      return typeof value === "boolean";
    case "object":
      return typeof value === "object" && value !== null && !Array.isArray(value);
    default:
      return false;
  }
}

function validateEvent(event: unknown): { ok: true; name: string; schema: EventSchema } | { ok: false; reason: string; name: string } {
  if (typeof event !== "object" || event === null) {
    return { ok: false, reason: "event must be an object", name: "<unknown>" };
  }

  const record = event as Record<string, unknown>;
  const name = typeof record.event === "string" ? record.event : "";
  if (!name) {
    return { ok: false, reason: "missing event name", name: "<unknown>" };
  }

  const schema = EVENT_SCHEMA_REGISTRY[name];
  if (!schema) {
    return { ok: false, reason: `unknown event: ${name}`, name };
  }

  const version = record.schemaVersion;
  if (typeof version !== "number") {
    return { ok: false, reason: "missing schemaVersion", name };
  }
  if (!SUPPORTED_VERSIONS[name]?.includes(version)) {
    return { ok: false, reason: `unsupported schemaVersion ${version} for ${name}`, name };
  }

  // Prohibited sensitive attributes must never appear.
  for (const key of Object.keys(record)) {
    if (schema.prohibited.includes(key)) {
      return { ok: false, reason: `prohibited attribute: ${key}`, name };
    }
  }

  // Required fields.
  for (const [field, type] of Object.entries(schema.required)) {
    if (!(field in record)) {
      return { ok: false, reason: `missing required field: ${field}`, name };
    }
    if (!isType(record[field], type)) {
      return { ok: false, reason: `invalid type for required field: ${field}`, name };
    }
  }

  // Optional fields, if present, must match type.
  for (const [field, type] of Object.entries(schema.optional)) {
    if (field in record && !isType(record[field], type)) {
      return { ok: false, reason: `invalid type for optional field: ${field}`, name };
    }
  }

  return { ok: true, name, schema };
}

export async function POST(request: NextRequest) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return NextResponse.json({ error: "Invalid JSON body" }, { status: 400 });
  }

  const events = (body as { events?: unknown })?.events;

  if (!Array.isArray(events)) {
    return NextResponse.json({ error: "events must be an array" }, { status: 400 });
  }

  if (events.length > MAX_BATCH_SIZE) {
    return NextResponse.json(
      { error: `Batch size exceeds limit of ${MAX_BATCH_SIZE}` },
      { status: 400 }
    );
  }

  // Authoritative consent check. A missing or unparseable record is treated as
  // denied for non-essential events (fail closed).
  const consent = readConsent(request);
  const consentGranted =
    consent !== null && consent.version === CURRENT_CONSENT_VERSION && consent.granted === true;
  const consentWithdrawn = consent !== null && consent.withdrawnAt !== undefined;

  const results: ValidationResult[] = [];
  let accepted = 0;
  let rejected = 0;
  let suppressed = 0;

  for (let index = 0; index < events.length; index++) {
    const validation = validateEvent(events[index]);

    if (!validation.ok) {
      rejected++;
      results.push({ index, event: validation.name, status: "rejected", reason: validation.reason });
      continue;
    }

    const { schema, name } = validation;
    const isEssential = ESSENTIAL_PURPOSES.has(schema.purpose);

    // Consent gating: non-essential events are suppressed unless consent is
    // currently granted. Withdrawal halts future non-essential events.
    if (!isEssential && (!consentGranted || consentWithdrawn)) {
      suppressed++;
      results.push({
        index,
        event: name,
        status: "suppressed",
        reason: consentWithdrawn ? "consent withdrawn" : "consent not granted",
      });
      continue;
    }

    // Retention: do not buffer user-linked events for withdrawn subjects.
    const eventRecord = events[index] as Record<string, unknown>;
    const subjectId = typeof eventRecord.subjectId === "string" ? eventRecord.subjectId : undefined;
    if (consentWithdrawn && subjectId && consent?.subjectId && subjectId === consent.subjectId) {
      suppressed++;
      results.push({
        index,
        event: name,
        status: "suppressed",
        reason: "retention policy: subject withdrew consent",
      });
      continue;
    }

    // Ring buffer: drop oldest when full.
    eventBuffer.push(eventRecord as { event: string; timestamp: number; [key: string]: unknown });
    if (eventBuffer.length > MAX_EVENTS_IN_MEMORY) {
      eventBuffer.shift();
    }
    accepted++;
    results.push({ index, event: name, status: "accepted" });
  }

  // If every event was rejected for validation reasons, surface a 422 so the
  // producer can fix its payloads. Suppressions are not errors.
  if (rejected > 0 && accepted === 0 && suppressed === 0) {
    return NextResponse.json(
      { received: events.length, accepted, rejected, suppressed, results },
      { status: 422 }
    );
  }

  return NextResponse.json(
    { received: events.length, accepted, rejected, suppressed, results },
    { status: 200 }
  );
}

export async function GET() {
  return NextResponse.json({
    buffered: eventBuffer.length,
    events: eventBuffer.slice(-50),
  });
}