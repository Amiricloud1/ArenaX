# Authentication Guide

ArenaX uses **JWT Bearer tokens** with short-lived access tokens and longer-lived refresh tokens.

## Contract version

The public API contract is versioned and published as an OpenAPI document:

- **Current contract version:** `1.0.0`
- **OpenAPI document:** served at `/api/docs/openapi.json` (also available in the repo under `docs/api/openapi.json`)
- **Source of truth:** the contract is generated/validated from the Rust and TypeScript server sources; the published document is the authoritative description of request/response schemas, auth requirements, pagination, and error codes.

Every endpoint in this guide (auth, tournament, match, wallet, dispute, notification) is described in that document with concrete schemas and examples. The examples in this guide are exercised by the endpoint contract tests in CI.

## Deprecation policy

- **Breaking changes require an explicit decision.** Any change that removes or renames a field, changes a type, tightens validation, changes an error code, or alters auth requirements is a breaking change.
- **Breaking changes require a version bump** of the contract (major version) and a corresponding **deprecation entry** in the OpenAPI document (`deprecated: true` on the affected operation/schema, plus a changelog note).
- **Minimum notice window:** deprecated endpoints/fields remain functional for at least **90 days** (or two minor releases, whichever is longer) after the deprecation entry is published.
- **CI enforcement:** a contract-diff check runs in CI against the published OpenAPI document. It fails the build when a breaking change is detected without a version bump and deprecation entry, so breaking changes cannot land silently.
- **Validation in CI:** schema linting and endpoint contract tests run on every pull request.

## Analytics Event Identity

Product analytics events are delivered to the ingestion boundary (`POST /api/analytics/events`) with a durable identity so retries cannot double-count and failures are diagnosable.

Every producer (web client, server jobs, operator tooling) must attach:

| Field | Required | Description |
|-------|----------|-------------|
| `event_id` | yes | Stable UUID generated once per logical event. Reused verbatim on every retry of that event. |
| `schema_version` | yes | Integer schema version the payload conforms to. Unknown or unsupported versions are rejected, not coerced. |
| `occurred_at` | yes | Producer-side timestamp (ISO 8601). Out-of-order batches are accepted; ordering is resolved by `occurred_at`, not arrival time. |
| `producer_id` | yes | Stable identifier of the emitting client/service, used to scope deduplication and diagnose producer restarts. |

Semantics:

- **Deduplication:** the ingestion boundary records each `(producer_id, event_id)` with an accepted/rejected outcome. A retry of an already-accepted event converges to the original accepted record and is not counted again.
- **Producer restart:** a restarted producer must persist its pending event IDs (e.g. in durable local storage) and replay them with the same `event_id`; new IDs must not be minted for replayed events.
- **Rejections:** malformed or schema-invalid events are recorded in a rejection path with the reason and schema version, so operators can diagnose and replay after a fix. Rejected events are never silently dropped.
- **Schema rollout:** producers and the boundary negotiate `schema_version`; during a rollout both the previous and current versions are accepted, and events with a version the boundary does not support are rejected with a diagnosable reason.

## Privacy-Safe Identity Stitching

Anonymous and authenticated sessions are linked through a separate identity-link store — never by embedding account identifiers in event payloads.

- **Purpose limitation:** identity links exist only to connect anonymous and authenticated sessions for funnel analysis. They are not used for authorization, billing, or any financial decision.
- **Consent-aware:** links are only created when the user has granted analytics consent. Withdrawing consent stops new links and removes existing links for that subject.
- **Reversible:** users can request unlinking; unlink removes the association and future events are attributed to a fresh anonymous identity.
- **Merge semantics:** logging in on a browser with an existing anonymous identity merges that anonymous identity into the authenticated one deterministically (the authenticated identity wins; the anonymous identity is retired). Shared devices never merge two authenticated accounts — a second login on the same browser starts a new link rather than merging accounts.
- **Collisions:** if an anonymous identity is already linked to a different authenticated account, the boundary refuses the merge and records a diagnosable collision instead of guessing.
- **Exports:** analytics exports never expose raw authentication identifiers (user UUIDs, emails, tokens); they expose only pseudonymous analytics IDs.

## Metric Catalog Governance

Every product and operational metric is defined in the governed metric catalog (`docs/analytics/metrics.yaml`). Each entry documents:

- **Owner** — the team accountable for the definition.
- **Query definition** — the canonical query/filters and time window.
- **Source events/tables** — where the numbers come from.
- **Freshness** — expected update cadence and maximum acceptable staleness.
- **Privacy class** — the sensitivity of the underlying data.
- **Deprecation state** — active, deprecated (with replacement), or retired.

Critical dashboards must reference catalogued definitions rather than ad-hoc queries. CI fails on duplicate metric names, unreviewed definition changes, and dashboards referencing uncatalogued metrics.

## Dashboard Query Budgets and Stale State

Each dashboard query declares a documented latency target, freshness target, and a bounded time range or pagination limit.

- Queries that exceed their latency or freshness budget render a visible **stale/incomplete** state; incomplete data is never presented as current.
- High-cardinality filters are bounded, and timeouts surface the stale state rather than partial results.
- Operators receive alerts when a dashboard query repeatedly exceeds its budget.
=======
=======

## Token Lifecycle

```
Register / Login
      │
      ▼
 access_token (15 min)  +  refresh_token (7 days)
      │                          │
      │ expires                  │ still valid
      ▼                          ▼
  Use /auth/refresh  ──────► new token pair
      │
      │ refresh also expired
      ▼
  Re-login required
```

## 1. Register

```http
POST /api/auth/register
Content-Type: application/json

{
  "username": "player_one",
  "email": "player@example.com",
  "phone_number": "+2348012345678",
  "password": "S3cur3P@ss!"
}
```

Response `201`:
```json
{
  "token": "<access_token>",
  "refresh_token": "<refresh_token>",
  "user": { "id": "...", "username": "player_one", ... }
}
```

## 2. Login

```http
POST /api/auth/login
Content-Type: application/json

{
  "email": "player@example.com",
  "password": "S3cur3P@ss!"
}
```

## 3. Authenticated Requests

Include the access token in every protected request:

```http
GET /api/auth/me
Authorization: Bearer <access_token>
```

## 4. Refresh Tokens

When the access token expires (HTTP 401 `Token expired`):

```http
POST /api/auth/refresh
Content-Type: application/json

{
  "refresh_token": "<refresh_token>"
}
```

Response `200`:
```json
{
  "access_token": "<new_access_token>",
  "refresh_token": "<new_refresh_token>",
  "expires_in": 900,
  "token_type": "Bearer"
}
```

> Both tokens are rotated on refresh. Store the new pair and discard the old ones.

## 5. Logout

Blacklists the current access token server-side:

```http
POST /api/auth/logout
Authorization: Bearer <access_token>
```

## 6. Session Management

List all active sessions:
```http
GET /api/auth/sessions
Authorization: Bearer <access_token>
```

Revoke all sessions (e.g. after a security incident):
```http
POST /api/auth/revoke-sessions
Authorization: Bearer <access_token>
```

## JWT Claims

The decoded access token payload:

```json
{
  "sub": "<user_uuid>",
  "exp": 1714000000,
  "iat": 1713999100,
  "jti": "<unique_token_id>",
  "token_type": "access",
  "device_id": null,
  "session_id": "<session_uuid>",
  "roles": ["user"]
}
```

Admin users have `"roles": ["admin", "user"]`.

## Error Responses

| Status | Meaning |
|--------|---------|
| 401 `Missing authorization header` | No `Authorization` header sent |
| 401 `Invalid authorization header format` | Header not in `Bearer <token>` format |
| 401 `Token expired` | Access token TTL elapsed — refresh it |
| 401 `Session expired or invalid` | Session was revoked server-side |
| 403 `Token has been revoked` | Token was explicitly blacklisted |

## Security Best Practices

- Store tokens in memory (not `localStorage`) in browser clients.
- Use `HttpOnly` cookies for refresh tokens in web apps.
- Always use HTTPS — tokens are bearer credentials.
- Rotate refresh tokens on every use (already enforced server-side).
- Call `/auth/revoke-sessions` if you suspect token compromise.
