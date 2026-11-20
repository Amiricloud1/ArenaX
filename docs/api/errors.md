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
| 403 on experiment assignment | Consent not granted | Prompt user to grant consent for the scope |
| 410 on experiment assignment | Experiment shut down | Stop requesting assignments; keep existing assignment version |
| 503 on experiment assignment | Guardrail stop active | Do not retry; operator must review the breach |
| 410 on leaderboard page | Snapshot expired | Restart pagination with a fresh snapshot |
| 503 on deletion status | Pending acknowledgements or retry scheduled | Poll again after `next_retry_at`; never report completion early |
