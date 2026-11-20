-- Feature toggle management system (#948)
-- Remote flag configuration, per-user overrides, percentage rollout,
-- sticky A/B assignments, and evaluation analytics.

CREATE TABLE IF NOT EXISTS feature_flags (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flag_key VARCHAR(100) NOT NULL UNIQUE,
    name VARCHAR(200) NOT NULL,
    description TEXT,
    enabled BOOLEAN NOT NULL DEFAULT false,
    rollout_percentage INTEGER NOT NULL DEFAULT 0
        CHECK (rollout_percentage BETWEEN 0 AND 100),
    -- Weighted A/B variants, e.g. {"control": 50, "treatment": 50}.
    -- Empty object means a boolean on/off flag with no named variants.
    variants JSONB NOT NULL DEFAULT '{}'::jsonb,
    default_variant VARCHAR(50),
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS feature_flags_enabled_idx
    ON feature_flags (enabled);
CREATE INDEX IF NOT EXISTS feature_flags_created_at_idx
    ON feature_flags (created_at DESC);

CREATE TABLE IF NOT EXISTS feature_flag_overrides (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flag_id UUID NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL,
    variant VARCHAR(50),
    reason TEXT,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    UNIQUE (flag_id, user_id)
);

CREATE INDEX IF NOT EXISTS feature_flag_overrides_user_id_idx
    ON feature_flag_overrides (user_id);
CREATE INDEX IF NOT EXISTS feature_flag_overrides_expires_at_idx
    ON feature_flag_overrides (expires_at)
    WHERE expires_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS feature_flag_assignments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flag_id UUID NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    variant VARCHAR(50) NOT NULL,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (flag_id, user_id)
);

CREATE INDEX IF NOT EXISTS feature_flag_assignments_user_id_idx
    ON feature_flag_assignments (user_id);

CREATE TABLE IF NOT EXISTS feature_flag_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    flag_id UUID NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    flag_key VARCHAR(100) NOT NULL,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    event_name VARCHAR(100) NOT NULL,
    enabled BOOLEAN NOT NULL,
    variant VARCHAR(50),
    reason VARCHAR(40) NOT NULL,
    properties JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS feature_flag_events_flag_key_created_idx
    ON feature_flag_events (flag_key, created_at DESC);
CREATE INDEX IF NOT EXISTS feature_flag_events_flag_id_idx
    ON feature_flag_events (flag_id);
CREATE INDEX IF NOT EXISTS feature_flag_events_user_id_idx
    ON feature_flag_events (user_id)
    WHERE user_id IS NOT NULL;

CREATE OR REPLACE FUNCTION update_feature_flags_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS update_feature_flags_updated_at_trig ON feature_flags;
CREATE TRIGGER update_feature_flags_updated_at_trig
    BEFORE UPDATE ON feature_flags
    FOR EACH ROW
    EXECUTE FUNCTION update_feature_flags_updated_at();


-- Experiment platform (#255, #256)
-- Deterministic assignment with recorded assignment versions, presentation-time
-- exposure logging, and guardrail metric samples for stop-condition analysis.

CREATE TABLE IF NOT EXISTS experiment_definitions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    experiment_key VARCHAR(100) NOT NULL UNIQUE,
    name VARCHAR(200) NOT NULL,
    description TEXT,
    -- Assignment scope: what the deterministic hash is keyed on
    -- (e.g. 'user', 'account', 'device').
    scope VARCHAR(40) NOT NULL DEFAULT 'user'
        CHECK (scope IN ('user', 'account', 'device', 'session')),
    -- Bumped whenever assignment inputs change; recorded on every assignment
    -- so cohorts can be audited and never silently reassigned mid-session.
    assignment_version INTEGER NOT NULL DEFAULT 1
        CHECK (assignment_version >= 1),
    -- Stable salt mixed into the hash so experiments do not correlate.
    salt VARCHAR(100) NOT NULL,
    -- Percentage of the eligible population held out of the experiment.
    holdout_pct INTEGER NOT NULL DEFAULT 0
        CHECK (holdout_pct BETWEEN 0 AND 100),
    -- Exclusion rules, e.g. {"plans": ["free"], "countries": ["XX"],
    -- "user_ids": [...], "min_account_age_days": 7}.
    exclusions JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Weighted variants, e.g. {"control": 50, "treatment": 50}.
    variants JSONB NOT NULL DEFAULT '{"control": 50, "treatment": 50}'::jsonb,
    primary_metric VARCHAR(100) NOT NULL,
    -- Guardrail metrics that must not regress, e.g.
    -- [{"key": "payment_error_rate", "direction": "lower_is_better",
    --   "max_increase_pct": 5}].
    guardrail_metrics JSONB NOT NULL DEFAULT '[]'::jsonb,
    min_sample INTEGER NOT NULL DEFAULT 1000 CHECK (min_sample >= 0),
    -- Analysis window in days from experiment start.
    analysis_window_days INTEGER NOT NULL DEFAULT 14
        CHECK (analysis_window_days > 0),
    -- Stop thresholds, e.g. {"max_guardrail_breaches": 1,
    -- "min_primary_lift_pct": 0, "alpha": 0.05}.
    stop_thresholds JSONB NOT NULL DEFAULT '{}'::jsonb,
    status VARCHAR(20) NOT NULL DEFAULT 'draft'
        CHECK (status IN ('draft', 'running', 'paused', 'stopped', 'shipped')),
    started_at TIMESTAMPTZ,
    stopped_at TIMESTAMPTZ,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS experiment_definitions_status_idx
    ON experiment_definitions (status);
CREATE INDEX IF NOT EXISTS experiment_definitions_created_at_idx
    ON experiment_definitions (created_at DESC);

CREATE TABLE IF NOT EXISTS experiment_assignments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    experiment_id UUID NOT NULL REFERENCES experiment_definitions(id) ON DELETE CASCADE,
    -- Participant identifier within the experiment scope (user/account/device id).
    participant_id VARCHAR(200) NOT NULL,
    variant VARCHAR(50) NOT NULL,
    -- Version of the definition used to compute this assignment.
    assignment_version INTEGER NOT NULL CHECK (assignment_version >= 1),
    -- True when the participant was held out rather than bucketed.
    is_holdout BOOLEAN NOT NULL DEFAULT false,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (experiment_id, participant_id)
);

CREATE INDEX IF NOT EXISTS experiment_assignments_participant_idx
    ON experiment_assignments (participant_id);
CREATE INDEX IF NOT EXISTS experiment_assignments_experiment_variant_idx
    ON experiment_assignments (experiment_id, variant);

CREATE TABLE IF NOT EXISTS experiment_exposures (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    assignment_id UUID NOT NULL REFERENCES experiment_assignments(id) ON DELETE CASCADE,
    -- Logged at presentation time, not assignment time.
    presented_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    surface VARCHAR(100) NOT NULL,
    properties JSONB NOT NULL DEFAULT '{}'::jsonb,
    UNIQUE (assignment_id, surface)
);

CREATE INDEX IF NOT EXISTS experiment_exposures_presented_at_idx
    ON experiment_exposures (presented_at DESC);
CREATE INDEX IF NOT EXISTS experiment_exposures_assignment_idx
    ON experiment_exposures (assignment_id);

CREATE TABLE IF NOT EXISTS guardrail_metric_samples (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    experiment_id UUID NOT NULL REFERENCES experiment_definitions(id) ON DELETE CASCADE,
    metric_key VARCHAR(100) NOT NULL,
    variant VARCHAR(50) NOT NULL,
    -- Aggregation bucket the sample covers (e.g. hourly window start).
    bucket_start TIMESTAMPTZ NOT NULL,
    sample_count BIGINT NOT NULL DEFAULT 0 CHECK (sample_count >= 0),
    -- Numerator/denominator for rate metrics; value for gauges.
    numerator DOUBLE PRECISION,
    denominator DOUBLE PRECISION,
    value DOUBLE PRECISION,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (experiment_id, metric_key, variant, bucket_start)
);

CREATE INDEX IF NOT EXISTS guardrail_metric_samples_experiment_metric_idx
    ON guardrail_metric_samples (experiment_id, metric_key, bucket_start DESC);
CREATE INDEX IF NOT EXISTS guardrail_metric_samples_bucket_start_idx
    ON guardrail_metric_samples (bucket_start DESC);

CREATE OR REPLACE FUNCTION update_experiment_definitions_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS update_experiment_definitions_updated_at_trig ON experiment_definitions;
CREATE TRIGGER update_experiment_definitions_updated_at_trig
    BEFORE UPDATE ON experiment_definitions
    FOR EACH ROW
    EXECUTE FUNCTION update_experiment_definitions_updated_at();
