# Webhook Verification Key Rotation

This document describes how webhook verification keys are rotated for each
provider, how overlapping keys are handled, and the bounds applied to signature
timestamps and replay protection.

## Key lifecycle

Each provider maintains a set of verification keys. Every key has:

- `key_id` — the identifier the provider uses to select the signing key.
- `secret` — the shared secret used to compute the signature.
- `status` — one of `active` or `retiring`.
- `not_before` — the earliest time the key may be used to validate a signature.
- `not_after` — the time after which the key must no longer validate signatures.
- `owner` — the auditable identity responsible for the rotation.

At most one key per provider is `active`. During a rotation the previous key is
moved to `retiring` with an explicit `not_after` expiry, and the new key becomes
`active`. Both keys validate signatures until the retiring key reaches its
`not_after` time, which provides a controlled overlap window.

## Overlap window

- The overlap window is bounded by the retiring key's `not_after` value.
- A retiring key stops validating signatures once `now >= not_after`.
- A key whose `not_before` is in the future does not validate signatures yet.
- Rotation must be performed by an authorized owner; the owner and timestamps
  are recorded for auditability.

## Timestamp and replay bounds

- Incoming signatures carry a timestamp. Requests whose timestamp is older than
the allowed skew (stale) or further in the future than the allowed skew are
rejected.
- Duplicate deliveries are rejected within the replay window using a bounded
  record of recently seen signature identifiers.
- The replay window and timestamp skew are fixed bounds; they are not extended
  by key rotation.

## Rotation and rollback

- A rotation is applied atomically: either the new active key and the retiring
  key are both recorded, or no change is made.
- If a rotation fails partway, the previous key set is restored so that
  validation continues with the prior active key.
- Rollback restores the prior active/retiring assignment and their timestamps.

## Validation coverage

The following cases are covered by automated tests:

- Valid overlap: both active and retiring keys validate within the window.
- Expired key: a retiring key stops validating after `not_after`.
- Stale timestamp: signatures older than the allowed skew are rejected.
- Future timestamp: signatures beyond the allowed skew are rejected.
- Duplicate delivery: repeated signatures within the replay window are rejected.
- Failed rotation rollback: a failed rotation leaves the prior key set intact.
