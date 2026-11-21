# Idempotency Guide

ArenaX implements idempotency for all mutating financial and game-state operations to prevent duplicate side effects from retried requests.

## How It Works

1. Client generates a UUID and sends it in the `Idempotency-Key` header.
2. Server hashes the request (method + path + headers + body) and stores it alongside the key.
3. If the same key is replayed within 24 hours with an **identical** request, the cached response is returned immediately — no re-execution.
4. If the same key is replayed with a **different** request, a `409 Conflict` is returned.

## Enabled Routes

| Route | Notes |
|-------|-------|
| `POST /api/payments/create` | Payment initiation |
| `POST /api/payments/refund` | Refund requests |
| `POST /api/wallets/deposit` | Wallet top-up |
| `POST /api/wallets/withdraw` | Withdrawal |
| `POST /api/tournaments/join` | Tournament registration |
| `POST /api/matchmaking/join` | Queue entry |

## Using Idempotency Keys

Always generate a fresh UUID per logical operation:

```http
POST /api/matchmaking/join
Authorization: Bearer <token>
Idempotency-Key: 7f3d9a2b-1c4e-4f8a-9b0d-2e5f6a7c8d9e
Content-Type: application/json

{ "game": "chess", "game_mode": "ranked" }
```

On a cached hit, the response includes:
```
X-Idempotency-Cached: true
```

## Key Lifecycle

- TTL: **24 hours** (configurable)
- Max cached response size: **1 MB**
- Keys are scoped to the authenticated user — the same UUID used by two different users is treated as two separate keys.
- Wallet ledger idempotency keys and request hashes are stored on immutable wallet transactions and retained for the lifetime of those ledger records; the 24-hour middleware cache does not expire them.

## Conflict Types

| Type | Meaning |
|------|---------|
| `PayloadMismatch` | Same key, different request body |
| `MethodMismatch` | Same key, different HTTP method |
| `RouteMismatch` | Same key, different endpoint |
| `UserMismatch` | Same key, different user |

## Managing Keys via API

```http
# Generate a server-side key
POST /api/idempotency/generate-key

# List your active keys
GET /api/idempotency/user-keys

# Invalidate a key early
DELETE /api/idempotency/invalidate/{key}

# Framework info (public)
GET /api/idempotency/info
```

## Offline Command Queue

The client only replays mutations from an explicit **allowlist** of offline-safe commands. Every queued command must declare the following metadata; a command missing any field is rejected from the queue:

| Field | Description |
|-------|-------------|
| `idempotencyKey` | UUID sent as `Idempotency-Key` on every replay attempt |
| `expiresAt` | Absolute expiry; expired commands are dropped, never replayed |
| `userScope` | The authenticated user the command belongs to |
| `conflictStrategy` | How to resolve a conflict on replay (see below) |

### Allowlisted Commands

Only non-financial, non-competitive mutations are queueable, for example:

| Command | Conflict strategy |
|---------|-------------------|
| Update profile preferences | `server-wins` (refetch) |
| Mark notification as read | `idempotent` |
| Save draft / local settings sync | `client-wins` (only if server version unchanged) |

### Rejected Offline Operations

The following are **never** queued and must fail fast while offline, surfacing a recoverable error to the user:

- Any payment, refund, deposit, or withdrawal (`/api/payments/*`, `/api/wallets/*`)
- Tournament registration (`/api/tournaments/join`)
- Matchmaking queue entry (`/api/matchmaking/join`)
- Any other financial or competitive action

These require a live connection so the server remains authoritative.

### Purge on Logout / Account Switch

Queued commands are scoped to `userScope`. On logout or account switch the client purges all commands for the previous user; commands are never replayed under a different account.

### Conflict Surfacing on Replay

On reconnect, each command is replayed with its stored idempotency key. If the server reports a conflict (e.g. `409`, or a newer server version), the client **surfaces the conflict** to the user with a recoverable prompt (retry, discard, or review) rather than silently overwriting newer server state.

## Optimistic Mutations: Rollback & Reconciliation

Every optimistic mutation declares whether it is optimistic and provides a safe rollback/refetch strategy for each affected resource.

- **Rollback**: on validation error or timeout, the affected resource is restored to its last known server snapshot (or refetched).
- **Version guard**: each resource carries a server version/ETag. A late response whose version is older than the current server version is discarded and cannot overwrite newer state.
- **Recoverable failure state**: failures are surfaced accessibly (announced to assistive tech) with a retry action; retries reuse the original idempotency key so duplicates are not created.

## Best Practices

- Generate a new UUID for each distinct user action — never reuse keys across different operations.
- Store the key alongside the pending operation in your client state so you can replay it on network failure.
- If you receive a `409 PayloadMismatch`, it means you accidentally reused a key — generate a new one.
- Keys are automatically cleaned up after TTL expiry; you don't need to delete them manually.
- Only enqueue offline-safe commands; never queue payments or match actions.
- Always set an expiry and user scope on queued commands, and purge on logout/account switch.
