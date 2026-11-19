-- Integrity case appeals are version-bound, single-submission, and expire
-- after 30 days. Evidence references are limited to five per appeal.
ALTER TABLE disputes
    ADD COLUMN decision_version INTEGER NOT NULL DEFAULT 1;

ALTER TABLE dispute_history
    DROP CONSTRAINT dispute_history_action_check,
    ADD CONSTRAINT dispute_history_action_check
        CHECK (action IN (
            'opened', 'assigned', 'status_changed', 'escalated',
            'evidence_added', 'resolved', 'rejected', 'reopened', 'closed',
            'appeal_submitted', 'appeal_reviewed'
        ));

CREATE TABLE dispute_appeals (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    dispute_id UUID NOT NULL UNIQUE REFERENCES disputes(id) ON DELETE CASCADE,
    appellant_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    decision_version INTEGER NOT NULL CHECK (decision_version > 0),
    appeal_text TEXT NOT NULL CHECK (length(trim(appeal_text)) BETWEEN 1 AND 2000),
    evidence_refs JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(evidence_refs) = 'array' AND jsonb_array_length(evidence_refs) <= 5),
    status VARCHAR(16) NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'accepted', 'rejected')),
    reviewer_id UUID REFERENCES users(id) ON DELETE SET NULL,
    decision_reason TEXT,
    submitted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deadline_at TIMESTAMPTZ NOT NULL,
    reviewed_at TIMESTAMPTZ,
    CONSTRAINT dispute_appeals_deadline_check CHECK (deadline_at > submitted_at),
    CONSTRAINT dispute_appeals_review_check CHECK (
        (status = 'pending' AND reviewer_id IS NULL AND reviewed_at IS NULL)
        OR (status <> 'pending' AND reviewed_at IS NOT NULL)
    )
);

CREATE INDEX idx_dispute_appeals_pending
    ON dispute_appeals(submitted_at) WHERE status = 'pending';

-- Each upload is a separate immutable custody record. Replacing evidence
-- appends a version and links it to the previous version instead of erasing it.
CREATE TABLE match_dispute_evidence_versions (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    dispute_id UUID NOT NULL REFERENCES match_disputes(id) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK (version > 0),
    sha256 VARCHAR(64) NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    object_key TEXT NOT NULL,
    uploaded_by UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    uploaded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    transformation_parent_id UUID REFERENCES match_dispute_evidence_versions(id) ON DELETE RESTRICT,
    retention_class VARCHAR(32) NOT NULL DEFAULT 'dispute_90d'
        CHECK (retention_class = 'dispute_90d'),
    UNIQUE (dispute_id, version)
);

CREATE INDEX idx_match_dispute_evidence_versions_dispute
    ON match_dispute_evidence_versions(dispute_id, version);

CREATE TABLE match_dispute_evidence_deletions (
    evidence_version_id UUID PRIMARY KEY
        REFERENCES match_dispute_evidence_versions(id) ON DELETE RESTRICT,
    deleted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE FUNCTION prevent_evidence_custody_mutation()
RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'evidence custody records are immutable';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_match_dispute_evidence_versions_immutable
    BEFORE UPDATE OR DELETE ON match_dispute_evidence_versions
    FOR EACH ROW EXECUTE FUNCTION prevent_evidence_custody_mutation();

REVOKE UPDATE, DELETE ON match_dispute_evidence_versions FROM PUBLIC;

-- Suspension appeals use the same bounded lifecycle. Pending appeals never
-- change the active restriction; only an accepted review lifts it.
ALTER TABLE player_suspensions
    ADD COLUMN decision_version INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN appeal_deadline_at TIMESTAMPTZ,
    ADD COLUMN appeal_decision_version INTEGER,
    ADD COLUMN appeal_evidence_refs JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(appeal_evidence_refs) = 'array'
            AND jsonb_array_length(appeal_evidence_refs) <= 5);

UPDATE player_suspensions
SET appeal_deadline_at = issued_at + INTERVAL '30 days'
WHERE appeal_deadline_at IS NULL;

ALTER TABLE player_suspensions
    ALTER COLUMN appeal_deadline_at SET NOT NULL,
    ALTER COLUMN appeal_deadline_at SET DEFAULT (NOW() + INTERVAL '30 days');
