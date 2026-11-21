ALTER TABLE player_suspensions
    DROP COLUMN IF EXISTS appeal_evidence_refs,
    DROP COLUMN IF EXISTS appeal_decision_version,
    DROP COLUMN IF EXISTS appeal_deadline_at,
    DROP COLUMN IF EXISTS decision_version;

DROP TRIGGER IF EXISTS trg_match_dispute_evidence_versions_immutable
    ON match_dispute_evidence_versions;
DROP FUNCTION IF EXISTS prevent_evidence_custody_mutation();
DROP TABLE IF EXISTS match_dispute_evidence_deletions;
DROP TABLE IF EXISTS match_dispute_evidence_versions;
UPDATE match_disputes
SET evidence_retention_until = resolved_at + INTERVAL '90 days'
WHERE evidence_s3_key IS NOT NULL
  AND evidence_deleted_at IS NULL
  AND resolved_at IS NOT NULL;

DROP TABLE IF EXISTS dispute_appeals;
ALTER TABLE disputes DROP COLUMN IF EXISTS decision_version;
