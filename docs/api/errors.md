# Error Handling Guide

## Error Response Format

All errors follow a consistent JSON envelope:

```json
{
  "error": "Human-readable message",
  "code": 400,
  "details": "Additional context (may be null)"
}
```

## HTTP Status Codes

| Code | Type | When it occurs |
|------|------|----------------|
| 400 | Bad Request | Invalid input, missing required fields, validation failure |
| 401 | Unauthorized | Missing/expired/invalid JWT token |
| 403 | Forbidden | Valid token but insufficient permissions (e.g. non-admin hitting admin route) |
| 404 | Not Found | Resource doesn't exist or doesn't belong to the requesting user |
| 409 | Conflict | Duplicate resource, idempotency key conflict, invalid state transition |
| 410 | Gone | Experiment shut down, leaderboard snapshot expired |
| 422 | Unprocessable | Semantic validation failure (e.g. ELO range invalid) |
| 429 | Too Many Requests | Rate limit exceeded |
| 500 | Internal Server Error | Unexpected server error, database failure, blockchain error |
| 503 | Service Unavailable | Guardrail stop active, privacy deletion pending acknowledgements |

## Common Error Scenarios

### Authentication Errors

```json
// Missing header
{ "error": "Missing authorization header", "code": 401 }

// Expired token
{ "error": "Token expired", "code": 401 }

// Revoked token
{ "error": "Token has been revoked", "code": 403 }

// Session gone (e.g. revoke-sessions was called)
{ "error": "Session expired or invalid", "code": 401 }
```

**Fix**: Refresh the access token via `POST /api/auth/refresh`. If the refresh token is also expired, re-authenticate.

---

### Validation Errors

```json
{
  "error": "Bad request: username must be between 3 and 50 characters",
  "code": 400,
  "details": "Bad request: username must be between 3 and 50 characters"
}
```

Stellar address validation (match creation):
```json
{
  "error": "Bad request: player_a must be exactly 56 characters",
  "code": 400
}
```

---

### Idempotency Conflicts (409)

When you replay an `Idempotency-Key` with a **different** request body:

```json
{
  "error": "Conflict: idempotency key already used with different payload",
  "code": 409,
  "details": "PayloadMismatch"
}
```

Conflict types:
- `PayloadMismatch` — same key, different body
- `MethodMismatch` — same key, different HTTP method
- `RouteMismatch` — same key, different endpoint
- `UserMismatch` — same key, different authenticated user

**Fix**: Generate a new idempotency key for a genuinely new request.

---

### Match State Transition Errors

```json
{
  "error": "Conflict: invalid state transition from FINALIZED",
  "code": 409
}
```

Valid transitions:
```
CREATED → STARTED → COMPLETED → DISPUTED → FINALIZED
                              ↘ FINALIZED
```

---

### Rate Limiting (429)

```json
{
  "error": "Too many requests: rate limit exceeded",
  "code": 429
}
```

The response includes a `Retry-After` header (seconds). Back off and retry after that interval.

---

### Experiment Errors

#### Consent Denied (403)

Returned when a participant has not granted the consent required by the experiment scope:

```json
{
  "error": "Forbidden: experiment consent not granted",
  "code": 403,
  "details": "experiment_id=exp_123, scope=feature_experiments"
}
```

**Fix**: Prompt the user to grant consent for the experiment scope. Assignment is never created without consent; retrying without consent returns the same error.

#### Experiment Shut Down (410)

Returned when the experiment has been stopped and no new assignments or exposures are accepted:

```json
{
  "error": "Gone: experiment is shut down",
  "code": 410,
  "details": "experiment_id=exp_123, shutdown_at=2024-06-01T00:00:00Z"
}
```

**Fix**: Stop requesting assignments for this experiment. Existing participants keep their recorded assignment version; do not silently reassign them.

#### Guardrail Stop (503)

Returned when an experiment's guardrail metrics have breached a stop threshold and the experiment is halted:

```json
{
  "error": "Service Unavailable: experiment halted by guardrail",
  "code": 503,
  "details": "experiment_id=exp_123, guardrail=payment_error_rate, threshold=0.02, observed=0.031"
}
```

**Fix**: Do not retry automatically. The experiment operator must review the guardrail breach and explicitly resume or end the experiment.

---

### Leaderboard Snapshot Errors

#### Stale / Expired Snapshot (410)

Returned when a requested leaderboard snapshot version is older than the retention bound:

```json
{
  "error": "Gone: leaderboard snapshot expired",
  "code": 410,
  "details": "snapshot_version=42, max_age_seconds=300"
}
```

**Fix**: Request a fresh snapshot (omit `snapshot_version`). Pagination within a single snapshot never moves users between pages; if the snapshot expires mid-pagination, restart pagination from page 1 of a new snapshot.

---

### Privacy Deletion Errors

#### Deletion Pending Acknowledgements (503)

Returned when a deletion job is still waiting on one or more dependent systems (caches, analytics projections, search indexes):

```json
{
  "error": "Service Unavailable: deletion pending acknowledgements",
  "code": 503,
  "details": "job_id=del_456, pending=[analytics_projection, search_index]"
}
```

**Fix**: Poll the job status endpoint. Do not report completion to the user until all required systems acknowledge deletion or a documented legal-retention exception applies.

#### Retry Scheduled (503)

Returned when a dependent system failed to acknowledge and a retry has been scheduled:

```json
{
  "error": "Service Unavailable: deletion retry scheduled",
  "code": 503,
  "details": "job_id=del_456, system=search_index, attempt=2, next_retry_at=2024-06-01T00:05:00Z"
}
```

**Fix**: Wait until `next_retry_at` before polling again. Deleted values are never included in logs or error details.

#### Retention Exception (200 with exception flag)

When a documented legal-retention exception applies, the job completes with an explicit exception record rather than a plain success:

```json
{
  "job_id": "del_456",
  "status": "completed_with_exception",
  "exception": {
    "system": "financial_ledger",
    "reason": "legal_retention",
    "retain_until": "2029-06-01T00:00:00Z"
  }
}
```

**Fix**: Surface the exception to the operator; completion is only reported once every non-exempt system has acknowledged deletion.

---

### Blockchain / Stellar Errors (500)

```json
{
  "error": "Blockchain error",
  "code": 500,
  "details": "Transaction submission failed: insufficient fee"
}
```

These are transient. Retry with exponential back-off. If the match is in an inconsistent state, call `POST /api/matches/{id}/reconcile` to sync off-chain state with the blockchain.

---

## Contract Error Catalog

Errors raised at the Soroban contract boundary are surfaced through a stable catalog. Each public contract error has a fixed numeric code, a safe user-facing message (never raw contract diagnostics or signing material), and a recovery classification. The same catalog is used by the client and backend adapters, so a code means the same thing everywhere.

### Recovery classifications

| Classification | Meaning | Client action |
|----------------|---------|---------------|
| `retry` | Transient condition; the same request may succeed later | Retry with exponential back-off (see below) |
| `manual_recovery` | Terminal for this request; requires operator or user intervention | Do not retry blindly; surface guidance and/or call reconcile |
| `duplicate` | The operation was already applied (idempotent replay) | Treat as success; do not resubmit |

### Public error codes

| Code | Name | Safe message | Classification |
|------|------|--------------|----------------|
| 1001 | `Unauthorized` | "You are not authorized to perform this action." | `manual_recovery` |
| 1002 | `InvalidSignature` | "The transaction signature could not be verified." | `manual_recovery` |
| 1003 | `InsufficientFunds` | "The account does not have sufficient funds for this operation." | `manual_recovery` |
| 1004 | `MatchNotFound` | "The requested match could not be found." | `manual_recovery` |
| 1005 | `InvalidStateTransition` | "This action is not allowed in the match's current state." | `manual_recovery` |
| 1006 | `AlreadyFinalized` | "This match has already been finalized." | `duplicate` |
| 1007 | `DuplicateOperation` | "This operation has already been processed." | `duplicate` |
| 1008 | `EscrowLocked` | "Funds for this match are currently locked." | `retry` |
| 1009 | `DeadlineExpired` | "The deadline for this operation has passed." | `manual_recovery` |
| 1010 | `NetworkCongestion` | "The network is busy. Please try again shortly." | `retry` |
| 1011 | `ContractPaused` | "Contract operations are temporarily paused." | `retry` |

### Unknown errors

Any contract error not in the catalog above is mapped to a single distinguishable code:

| Code | Name | Safe message | Classification |
|------|------|--------------|----------------|
| 1099 | `UnknownContractError` | "An unexpected contract error occurred. Reference the request ID when contacting support." | `manual_recovery` |

Unknown errors are never collapsed into a known code. The backend logs them with transaction context — request ID, match ID, contract ID, transaction hash, ledger sequence, and the raw error code — but **never** logs signing material, secret keys, or signed transaction envelopes. The response body includes the request ID so support can correlate the log entry.

```json
{
  "error": "An unexpected contract error occurred. Reference the request ID when contacting support.",
  "code": 500,
  "details": "UnknownContractError",
  "request_id": "req_01H..."
}
```

### Client handling guidance

**Retry (`retry` classification).** Retry with exponential back-off and jitter, capped at a small number of attempts (e.g. 5). Honour `Retry-After` when present. If retries are exhausted, escalate to manual recovery.

**Failure (`manual_recovery` classification).** Do not resubmit the same request. Show the safe message to the user, and where applicable call `POST /api/matches/{id}/reconcile` to resync off-chain state before attempting a new operation.

**Duplicate delivery (`duplicate` classification).** Treat as success. `AlreadyFinalized` and `DuplicateOperation` mean the effect is already applied; do not resubmit, and do not treat the response as an error in UI flows. Replaying an `Idempotency-Key` with the same body returns the original result rather than a new error.

---

## Troubleshooting Checklist

| Symptom | Likely cause | Fix |
|---------|-------------|-----|
| All requests return 401 | Token expired | Refresh token |
| 401 on fresh token | Clock skew > 5 min | Sync system clock |
| 403 on admin endpoint | Missing `admin` role | Use an admin account |
| 409 on match create | Duplicate idempotency key | Generate a new key |
| 409 on state transition | Wrong current state | Check match state via `GET /api/matches/{id}` |
| 500 on finalize | Stellar network issue | Retry after 30s; call reconcile if needed |
| 429 on matchmaking join | Too many queue attempts | Wait for `Retry-After` seconds |
| WebSocket closes immediately | Invalid/expired token | Refresh token before connecting |
| Contract code 1008/1010/1011 | Transient contract condition | Retry with back-off |
| Contract code 1006/1007 | Duplicate delivery | Treat as success; do not resubmit |
| Contract code 1099 | Unknown contract error | Capture `request_id`, contact support; do not retry blindly |
