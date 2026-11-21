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

## Return Destinations Through Authentication

When a signed-out user opens a deep link (for example a tournament or match invitation), the client may send them through login, registration, or an OAuth callback. The intended destination is carried in a `returnTo` parameter and restored **once** after authentication succeeds.

### Allowed destinations

`returnTo` must resolve to a **local, approved path** on the same origin. The client validates it before storing and again before redirecting:

- **Local paths only:** the value must start with a single `/` (e.g. `/tournaments/abc123`). Absolute URLs (`https://evil.example`), protocol-relative URLs (`//evil.example`), and backslash variants (`/\evil.example`) are rejected.
- **Approved prefixes:** only known app routes are accepted (`/tournaments/...`, `/matches/...`, `/wallet`, `/disputes/...`, `/notifications`, `/settings`). Anything else falls back to the default post-login destination (e.g. `/dashboard`).
- **Locale prefixes:** a leading locale segment (`/en/...`, `/fr/...`) is preserved and validated against the same approved prefixes, so `/en/tournaments/abc123` is accepted while `/en//evil.example` is not.
- **Encoded input:** the value is percent-decoded exactly once before validation. Double-encoded payloads (`%2F%2Fevil.example`) that decode to a non-local path are rejected.

### Query parameters

Only **validated, non-secret** query parameters are preserved:

- An allowlist of parameters is kept (e.g. `invite`, `ref`, `tab`). Unknown parameters are dropped.
- Secret or sensitive parameters (`token`, `access_token`, `refresh_token`, `password`, `code`, `secret`, `key`, `session`) are always stripped, even if they appear in the allowlist.
- Parameter values are length-bounded and re-encoded on redirect so they cannot break out of the query string.

### Loop avoidance

- The destination is restored **once**: after a successful login/registration/OAuth callback, the stored `returnTo` is consumed and cleared so a refresh or back-navigation does not replay it.
- If the target resource is **private or unavailable** (403/404, expired invitation, revoked access), the client does **not** bounce back to login. It shows the appropriate error state (e.g. "Invitation expired" or "You don't have access") on the destination route instead of redirecting again.
- Unauthenticated users hitting a protected route are sent to login with `returnTo`; authenticated users hitting the same route are never redirected to login, preventing redirect loops.

### OAuth callbacks

The OAuth callback (`/auth/callback`) validates `returnTo` with the same rules before redirecting. The `state` parameter is used only for CSRF protection and is never treated as a return destination. If `returnTo` is missing or invalid, the callback redirects to the default post-login destination.

### Example

```
GET /login?returnTo=%2Fen%2Ftournaments%2Fabc123%3Finvite%3Dxyz%26token%3Dsecret
```

After successful login the client redirects to:

```
/en/tournaments/abc123?invite=xyz
```

The `token` parameter is stripped and the locale prefix is preserved.

## Security Best Practices

- Store tokens in memory (not `localStorage`) in browser clients.
- Use `HttpOnly` cookies for refresh tokens in web apps.
- Always use HTTPS — tokens are bearer credentials.
- Rotate refresh tokens on every use (already enforced server-side).
- Call `/auth/revoke-sessions` if you suspect token compromise.
- Never trust a `returnTo` value from the URL without validating it against the local-path and allowlist rules above.
