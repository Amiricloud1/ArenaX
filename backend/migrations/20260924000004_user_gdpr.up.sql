-- GDPR / NDPR user account erasure & data portability (Issue #1078)
--
-- Adds soft-delete support to `users` and a queue table for asynchronous
-- data-export jobs (delivered via email link).

-- Soft-delete column: set to NOW() when the user exercises their right to
-- erasure. PII is anonymised in code at deletion time; the row is kept so
-- match history, tournament results and on-chain audit trails keep a stable
-- reference (never a hard delete).
ALTER TABLE users
ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_users_deleted_at ON users(deleted_at) WHERE deleted_at IS NOT NULL;

-- Data export jobs (async, delivered via email link once ready).
CREATE TABLE IF NOT EXISTS data_export_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status VARCHAR(20) NOT NULL DEFAULT 'queued', -- queued | processing | ready | failed
    export_link TEXT,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_data_export_jobs_user ON data_export_jobs(user_id);
CREATE INDEX IF NOT EXISTS idx_data_export_jobs_status ON data_export_jobs(status);

COMMENT ON COLUMN users.deleted_at IS 'Soft-delete marker (GDPR/NDPR erasure). PII anonymised at deletion time.';
COMMENT ON TABLE data_export_jobs IS 'Asynchronous GDPR data-export requests delivered by email link.';

-- ---------------------------------------------------------------------------
-- Privacy deletion tracking (Issue #258)
--
-- Deleting a profile from the primary database may leave identifiable records
-- in caches, analytics projections, or search indexes. These tables track the
-- per-dependent-system acknowledgement and retry status of a deletion
-- workflow so completion is only reported once every required system confirms
-- deletion (or a documented legal-retention exception applies).
--
-- IMPORTANT: these tables intentionally store NO PII. Only opaque identifiers,
-- status enums, attempt counters, timestamps and non-sensitive error codes are
-- persisted so that rows and logs remain redacted. Deleted values are never
-- written here.
-- ---------------------------------------------------------------------------

-- One row per erasure workflow. `requested_by` records the operator/user
-- identifier that initiated the request (an opaque id, never PII).
CREATE TABLE IF NOT EXISTS privacy_deletion_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    requested_by UUID,
    status VARCHAR(20) NOT NULL DEFAULT 'pending', -- pending | in_progress | completed | failed | retention_exception
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    retention_exception TEXT
);

CREATE INDEX IF NOT EXISTS idx_privacy_deletion_jobs_user ON privacy_deletion_jobs(user_id);
CREATE INDEX IF NOT EXISTS idx_privacy_deletion_jobs_status ON privacy_deletion_jobs(status);
-- Retry scanning: find unfinished jobs that are due for another pass.
CREATE INDEX IF NOT EXISTS idx_privacy_deletion_jobs_pending
    ON privacy_deletion_jobs(created_at)
    WHERE status IN ('pending', 'in_progress', 'failed');

-- Per-dependent-system acknowledgement and retry tracking.
CREATE TABLE IF NOT EXISTS privacy_deletion_acknowledgements (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id UUID NOT NULL REFERENCES privacy_deletion_jobs(id) ON DELETE CASCADE,
    system VARCHAR(32) NOT NULL
        CHECK (system IN ('cache', 'analytics_projection', 'search_index')),
    status VARCHAR(20) NOT NULL DEFAULT 'pending', -- pending | acknowledged | failed | retention_exception
    attempts INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    acknowledged_at TIMESTAMPTZ,
    error_code VARCHAR(64), -- non-sensitive machine-readable code only, never PII
    CONSTRAINT privacy_deletion_acknowledgements_job_system_key UNIQUE (job_id, system)
);

CREATE INDEX IF NOT EXISTS idx_privacy_deletion_acks_job ON privacy_deletion_acknowledgements(job_id);
-- Retry scanning: find unacknowledged systems ordered by last attempt.
CREATE INDEX IF NOT EXISTS idx_privacy_deletion_acks_retry
    ON privacy_deletion_acknowledgements(last_attempt_at)
    WHERE status IN ('pending', 'failed');

COMMENT ON TABLE privacy_deletion_jobs IS 'Tracks a privacy erasure workflow across dependent systems. Stores no PII.';
COMMENT ON COLUMN privacy_deletion_jobs.requested_by IS 'Opaque identifier of the requester; never PII.';
COMMENT ON COLUMN privacy_deletion_jobs.retention_exception IS 'Documented legal-retention exception reference, if any.';
COMMENT ON TABLE privacy_deletion_acknowledgements IS 'Per-system deletion acknowledgement and retry status. Stores no PII.';
COMMENT ON COLUMN privacy_deletion_acknowledgements.error_code IS 'Non-sensitive machine-readable error code; never contains deleted values.';