# Dispute Evidence Retention

Evidence objects are private and may be retrieved only by a player in the
associated match or an authorized moderator/admin operation. The backend
validates content type, size, and file signature at upload, stores a SHA-256
digest with the dispute, and verifies that digest before returning an object.

Evidence remains available while the dispute is unresolved. On resolution, the
backend sets a 90-day retention deadline. A moderator or administrator may set
or clear a legal hold; held evidence is not deleted by the scheduled retention
sweep. After the deadline and once any hold is cleared, the object is deleted
from private storage. The dispute's SHA-256 digest, resolution, and audit
entries remain in the database after object deletion so the record of retained
evidence integrity is not erased.

The retention worker runs every six hours. Object deletion is idempotent, and
the evidence pointer is cleared only after the storage deletion succeeds.

## Custody and replacements

Each successful upload also appends a row to
`match_dispute_evidence_versions`. The row records the server-observed upload
time, authenticated uploader, SHA-256 digest, private object key, fixed
`dispute_90d` retention class, and the previous version as
`transformation_parent_id` when one exists. The table rejects updates and
deletes; replacing evidence creates a new version and never rewrites the
evidence bytes associated with an earlier decision. The evidence body is
re-hashed when downloaded.

`GET /api/matches/{id}/dispute/evidence/custody` returns the authorized
player's or staff member's custody chain. Reads of evidence and custody
metadata are recorded in `audit_logs`; the version rows retain their hashes
and lineage after the object is purged. Retention deletion state is stored
separately so the custody records remain immutable. A failed object-store
write triggers a cleanup attempt; if cleanup also fails, the object key and
failure are logged for reconciliation.