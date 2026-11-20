# Security Best Practices

## Authentication

- **Never** log or expose JWT tokens in error messages, logs, or analytics.
- Access tokens have a 15-minute TTL -- keep them short-lived.
- Refresh tokens are rotated on every use. If a refresh token is replayed after rotation, the session is invalidated (token reuse detection).
- Call `POST /api/auth/revoke-sessions` immediately if you suspect a token leak.

## Token Storage (Client-Side)

| Storage | Recommendation |
-------------------|
| Memory (JS variable) | 💥 Best for access tokens in SPAs |
| `HttpOnly` cookie | 💥 Best for refresh tokens in web apps |
| `localStorage` | ⚅ Vulnerable to XSS -- avoid for tokens |
| `sessionStorage` | ㊀ Cleared on tab close -- poor UX |

## Transport Security

- All API traffic must use HTTPS/TLS 1.2+.
- WebSocket connections must use WWS (not WS).
- The server enforces CORS -- only approved origins can make cross-origin requests.

## Input Validation

- Stellar public keys are validated to exactly 56 characters.
- Usernames: 3-50 characters.
- Tournament names: 3-255 characters.
- All UUIDs are validated server-side before database queries.

## Rate Limiting

The API enforces a fixed-window rate limit per client IP (`X-Forwarded-For`.     Every response includes the following headers:

| Header | Description |
--------------------------------------------------------------------------|
| `RateLimit-Limit`  | Maximum number of requests allowed during the current window. |
| `RateLimit-Remaining` | Number of requests left in the current window. |
| `RateLimit-Reset`  | Unix timestamp (seconds) when the current window resets. |
| `Retry-After`  | Only on `429 Too Many Requests`; number of seconds to wait before retrying. |

When the limit is exceeded, the API returns `429` with a `Retry-After` header. Clients MUST respect this header and implement exponential backoff:

1. Wait at least `Retry-After` seconds before resending.
2. Increase the wait for subsequent retries (e.g., multiply the previous wait by a factor of 2).
3. Add jitter to avoid synchronized reconnection (e.g., `rand(0, 100ms)`).
4. Never retry more than a maximum number of times (e.g., 5) before surfacing the error to the user.

Example exponential backoff calculation:

```
retry_after = 1s
for attempt in 0.5.5:
    if got 429:
        wait = retry_after * 2^attempt + jitter
        sleep(wait)
    else:
        break
```

## Idempotency Keys

- Keys are scoped per user -- a key from user A cannot collide with user B's key.
- Replaying a key with a different payload returns `409` -- this prevents accidental double-submissions.

## Admin Endpoints

Admin-only endpoints (`/api/auth/analytics`, `/api/reputation/bad-actors`) check for the `admin` role in JWT claims. Requests from non-admin users receive `403 Forbidden`.

## Operator Sessions

Privileged operator actions (admin actions, contract submissions, and other privileged mutations) require a short-lived **operator session** in addition to the normal access token. Long-lived operator authorization is not accepted.

### Session Binding

Every operator session binds the following attributes, and all of them are validated on each privileged request:

| Attribute | Description |
|-----------|-------------|
| `actor` | The authenticated operator identity (user ID) that created the session. |
| `purpose` | The declared reason for the session (e.g. `match-reconcile`, `contract-upgrade`). |
| `scope` | The exact set of privileged actions the session may perform. Actions outside the scope are rejected. |
| `expiry` | Absolute expiry timestamp. Operator sessions are short-lived (minutes, not hours) and cannot be extended in place -- create a new session instead. |
| `approval_ref` | Reference to the approval record that authorized the session (e.g. a change ticket or second-operator approval ID). |

A session missing any of these attributes, or whose `actor` does not match the caller, is rejected with `403 Forbidden`.

### Pre-Submission Checks

The session is re-validated **immediately before submission** of the privileged action -- not only when the request is accepted or queued. The check verifies that the session is unexpired, unrevoked, still bound to the same `actor`, and that the requested action is within `scope`. If any check fails, the action is rejected and never submitted.

### Revocation

- Revoking an operator session (`POST /api/auth/revoke-sessions` or the operator session revocation endpoint) takes effect immediately.
- Revocation **prevents queued but not-yet-submitted actions**: an action that was accepted into a queue but has not yet been submitted is rejected at the pre-submission check. Queued actions are never submitted under a revoked session.
- Revocation is idempotent; revoking an already-revoked session returns success.

### Signing Keys

Signing keys (e.g. `STELLAR_ADMIN_SECRET`) are **never** stored in the operator session store. The session store holds only the binding attributes above; key material is loaded from the environment at submission time and is not readable through session APIs.

### Retry, Failure, and Duplicate Delivery

- A rejected privileged action (expired or revoked session, wrong scope) is **not** retried automatically. The operator must create a new session and resubmit.
- Retries of a privileged action must reuse the same idempotency key so duplicate delivery cannot double-submit. Replaying a key with a different payload returns `409`.
- Duplicate delivery of an already-submitted action returns the original result rather than submitting again.
- Every rejected privileged request is recorded as audit evidence: actor, purpose, scope, `approval_ref`, rejection reason (expired / revoked / wrong-scope / actor-mismatch), and timestamp.
=======

## Stellar / Blockchain

- Private keys (`STELLAR_ADMIN_SECRET`) are loaded from environment variables -- never hardcoded.
- Smart contract addresses are configured per environment.
- All on-chain operations are logged with transaction hashes for auditability.
- The reconciliation endpoint (`Post /api/matches/{id}/reconcile`) can detect and resolve off-chain/on-chain divergence.

## Reporting Vulnerabilities

Contact `security@arenax.gg` with a description of the issue. Do not disclose publicly until a fix is deployed.