# Payment and Contract Divergence Incident Drill

This is the staging tabletop and fault-injection procedure for mismatches
between payment-provider callbacks, the ArenaX ledger, and Soroban contract
finality. Run it at least quarterly and after material payment or contract
settlement changes. Do not use production accounts, funds, or evidence.

## Drill roles

Assign named people before the exercise starts:

| Role | Responsibility |
|---|---|
| Incident commander (IC) | Declares the scenario, owns decisions and the timeline, and authorizes recovery gates. |
| Payments lead | Contains provider activity and reconciles provider event IDs, idempotency keys, charges, and refunds. |
| Contract lead | Verifies transaction hashes, ledger sequence/finality, and the contract's pause and settlement state. |
| Evidence scribe | Captures the evidence package and maintains the append-only event timeline. |
| Communications lead | Prepares internal and user-facing updates; does not claim settlement while state is unresolved. |
| Independent observer | Injects faults, records elapsed time and deviations, and confirms the recovery gates independently. |

One person may not act as both the IC and independent observer.

## Safe containment

1. The IC declares the incident and opens a unique incident ID. The scribe
   records UTC detection/declaration times and each subsequent action.
2. Pause creation of new payment intents and new risk-bearing escrow or payout
   operations using the approved staging controls. Do not disable reads or
   user-principal emergency exits. Contract pause authority must use the
   bounded Guardian control; record its expiry and the authorized actor.
3. Stop automated settlement/retry workers for affected transaction classes.
   Keep idempotency records, webhook receipt logs, the internal ledger, and
   contract event ingestion available for read-only reconciliation.
4. The communications lead posts an internal status stating what is paused,
   which transaction window is affected, and when the next update is due.
   User-facing messages must not expose confidential evidence or promise
   funds before finality is established.

## Evidence package

The scribe creates a restricted incident folder and records:

- Incident ID, scenario, environment, UTC timeline, named role holders, and
  approved change/pause identifiers.
- Provider event and charge/refund IDs, webhook receipt times and signature
  verification outcomes, payment idempotency keys, and sanitized callback
  payloads. Never include credentials, signing secrets, or full payment
  instruments.
- Internal ledger transaction IDs, amounts/currencies, state transitions,
  database snapshot/checksum references, and the reconciliation query/version.
- Soroban transaction hashes, ledger sequence numbers, finality status,
  contract identifiers, relevant event sequence, and pause/unpause transaction
  identifiers.
- Hashes of exported evidence files, source/owner, capture time, and any
  transformation lineage. Access to the restricted package is logged.

Preserve source records; annotate corrections as new entries rather than
overwriting captured evidence.

## Fault-injection and recovery sequence

The observer chooses one staging-only mismatch and records the exact injection
time: a delayed/duplicated provider callback, a ledger row with no matching
provider event, a provider charge without a ledger entry, or a submitted
contract transaction whose finality is delayed or disagrees with the ledger.

1. Detect and classify the divergence. Freeze the affected transaction IDs and
   idempotency keys; do not retry an uncertain charge or payout.
2. Reconcile each provider event against its idempotency key and ledger row.
   Confirm whether the provider reports one charge, a refund, or no final
   outcome. Verify webhook signatures and deduplicate by provider event ID.
3. Reconcile each ledger entry against the Soroban transaction hash and final
   ledger sequence. A submitted or pending transaction is not treated as
   finalized. Do not create a replacement payout while finality is unknown.
4. Assign every unmatched or ambiguous item to a named owner with a next
   action and due time. Record the evidence references and keep the affected
   item in a non-settled state.
5. Apply only the approved idempotent correction: ingest a missing verified
   callback, issue a ledger correction/refund with its own dedupe key, or
   resume a settlement after independently confirmed contract finality.
   Capture the before/after identifiers and reviewer.
6. The IC and independent observer verify all recovery gates below before
   releasing the relevant pause. Resume one transaction class at a time and
   monitor provider callbacks, ledger writes, and contract events.

## Recovery gates

The drill passes only when the evidence demonstrates all of the following:

- Every injected transaction reconciles to exactly one provider charge (or a
  documented no-charge/refund outcome) and no transaction is retried with a
  new key while its prior outcome is uncertain.
- Each intended payout has exactly one ledger settlement and exactly one
  finalized contract settlement; there are no duplicate charges or payouts.
- Every unresolved mismatch remains blocked from settlement, has a named
  owner, next action, due time, and evidence reference.
- The IC's pause release is recorded and does not disable emergency exits or
  read-only investigation paths.
- The measured detection-to-containment and detection-to-reconciliation
  timelines are compared with the applicable operational targets.

If any gate fails, keep the affected transaction class paused in staging,
record the gap, and assign corrective work before another drill.

## Drill record (complete for each run)

| Field | Result |
|---|---|
| Incident ID / date / staging environment | _Complete during the drill_ |
| Scenario and fault-injection time | _Complete during the drill_ |
| IC / leads / scribe / independent observer | _Complete during the drill_ |
| Detection → declaration → containment → reconciliation → resume (UTC) | _Record measured timestamps and elapsed durations_ |
| Duplicate charge count / duplicate payout count | _Expected: 0 / 0; attach reconciliation evidence_ |
| Unresolved items and assigned owners | _List each item, owner, next action, and due time; write “none” only when verified_ |
| Recovery gate outcome | _Pass / blocked, with evidence links_ |
| Gaps and corrective actions | _Owner and due date for each action_ |

The exercise is not complete until the observer signs off on the measured
timeline and the action owners accept their assignments. Store the completed
record with the incident artifacts; do not fill in measured results before
the staging drill has actually occurred.
