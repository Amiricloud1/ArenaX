# Integrity Cases and Appeals

## Case review

Integrity reports use the authenticated `/api/disputes` case workflow. Each
case retains its category, submitter, evidence references, staff assignment,
resolution reason, decision version, and append-only history. Only moderators
or administrators can assign or decide a case. Once assigned, only the
assigned reviewer may finalize it; the case opener cannot review their own
case. Finalization is serialized and a terminal case cannot be decided twice.

Opening a report or attaching evidence does not change player ratings or
payment state. A case decision is an explicit reviewed action; any separate
rating or payment correction must use its owning service and its own
authorization and audit trail. Confidential evidence metadata and download
access are limited to the submitting player or staff and are recorded in
`audit_logs`.

## Case appeals

A resolved or rejected case may receive one appeal within 30 days of its
decision. The request must include the exact `decision_version` returned by
the case API; stale versions fail. Appeal text is limited to 2,000 characters
and evidence references to five entries of at most 500 characters each.
Moderators and administrators review the pending-appeal queue. The original
decision maker and case opener cannot review the appeal. The result and
reviewer are appended to case history and the appellant receives an in-app
notification with the review reason. An appeal does not automatically alter
rating or payment state.

Endpoints:

- `POST /api/disputes/{id}/appeal`
- `GET /api/disputes/{id}/appeal`
- `GET /api/disputes/appeals` (staff only)
- `POST /api/disputes/appeals/{id}/review` (staff only)

## Suspension appeals

A player may submit one appeal per active suspension within 30 days of issue.
Appeals are capped at three submissions per player in a rolling 30-day window.
Text is limited to 2,000 characters and evidence references to five entries
of at most 500 characters each. Submission pins the current decision version.
The issuer cannot review the appeal, and the subject cannot act as reviewer.
Pending appeals do not restore privileges or otherwise change the suspension.
Only an accepted, version-matching review lifts it. The player receives an
in-app notification with the review reason; submissions, decisions, queue
access, and evidence access are audited.

Endpoints:

- `POST /api/suspensions/{id}/appeal`
- `GET /api/suspensions/appeals` (staff only)
- `POST /api/suspensions/{id}/appeal/review` (staff only)
