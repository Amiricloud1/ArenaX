-- Migration: 20260824000001_player_stats_snapshots
-- Issue #904 — Player statistics aggregation
--
-- Adds a pre-computed daily snapshot cache table so the /stats/player/{id}/daily
-- endpoint stays fast at scale without re-scanning elo_history every request.
-- The table is populated by a nightly cron job or on-demand refresh.

CREATE TABLE IF NOT EXISTS player_stats_daily (
    id               UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id          UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    snapshot_date    TIMESTAMPTZ NOT NULL,   -- UTC midnight of the day
    game_mode        VARCHAR(50) NOT NULL,
    wins             BIGINT      NOT NULL DEFAULT 0,
    losses           BIGINT      NOT NULL DEFAULT 0,
    draws            BIGINT      NOT NULL DEFAULT 0,
    matches_played   BIGINT      NOT NULL DEFAULT 0,
    win_rate_pct     DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    avg_session_secs BIGINT      NOT NULL DEFAULT 0,
    computed_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, snapshot_date, game_mode)
);

CREATE INDEX IF NOT EXISTS idx_psd_user_date
    ON player_stats_daily (user_id, snapshot_date DESC);

CREATE INDEX IF NOT EXISTS idx_psd_user_mode
    ON player_stats_daily (user_id, game_mode);

COMMENT ON TABLE player_stats_daily IS
    'Pre-computed daily win/loss/draw snapshots per player per game mode. '
    'Populated nightly; used by GET /api/stats/player/{id}/daily for fast reads.';

-- ---------------------------------------------------------------------------
-- Issue #257 — Serve leaderboards from versioned, freshness-aware snapshots
--
-- Leaderboard reads are served from immutable, versioned snapshots so that a
-- single page (and every subsequent page of the same snapshot) observes the
-- exact same ranking. Each snapshot carries a version, a creation time and an
-- expiry so staleness is explicitly bounded and observable.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS leaderboard_snapshots (
    id           UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    category     VARCHAR(100) NOT NULL,      -- e.g. 'global', 'mode:ranked', 'season:12'
    version      BIGINT      NOT NULL,       -- monotonically increasing per category
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at   TIMESTAMPTZ NOT NULL,       -- hard upper bound on staleness
    entry_count  BIGINT      NOT NULL DEFAULT 0,
    UNIQUE (category, version)
);

CREATE INDEX IF NOT EXISTS idx_lb_snapshots_category_version
    ON leaderboard_snapshots (category, version DESC);

CREATE INDEX IF NOT EXISTS idx_lb_snapshots_expires_at
    ON leaderboard_snapshots (expires_at);

COMMENT ON TABLE leaderboard_snapshots IS
    'Immutable, versioned leaderboard snapshots. Readers pin a snapshot id/version '
    'for the whole pagination session so rankings cannot shift between pages.';

CREATE TABLE IF NOT EXISTS leaderboard_snapshot_entries (
    snapshot_id    UUID        NOT NULL REFERENCES leaderboard_snapshots(id) ON DELETE CASCADE,
    user_id        UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    username       VARCHAR(100) NOT NULL,
    avatar_url     TEXT,
    ranking        BIGINT      NOT NULL,     -- 1-based rank within the snapshot
    elo_rating     INTEGER     NOT NULL,
    matches_played BIGINT      NOT NULL DEFAULT 0,
    wins           BIGINT      NOT NULL DEFAULT 0,
    losses         BIGINT      NOT NULL DEFAULT 0,
    win_rate       DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),  -- source row's last update
    PRIMARY KEY (snapshot_id, user_id)
);

-- Stable ordering index: elo DESC, then updated_at ASC, then user_id ASC.
-- This is the canonical tie-break rule used for pagination across pages.
CREATE INDEX IF NOT EXISTS idx_lb_entries_stable_order
    ON leaderboard_snapshot_entries (snapshot_id, elo_rating DESC, updated_at ASC, user_id ASC);

CREATE INDEX IF NOT EXISTS idx_lb_entries_ranking
    ON leaderboard_snapshot_entries (snapshot_id, ranking ASC);

COMMENT ON TABLE leaderboard_snapshot_entries IS
    'Frozen leaderboard rows belonging to a single snapshot. Ordering is stable: '
    'elo_rating DESC, updated_at ASC, user_id ASC.';

CREATE TABLE IF NOT EXISTS leaderboard_snapshot_refresh_jobs (
    id             UUID        PRIMARY KEY DEFAULT uuid_generate_v4(),
    category       VARCHAR(100) NOT NULL,
    status         VARCHAR(20) NOT NULL DEFAULT 'pending',  -- pending|running|succeeded|failed
    scheduled_for  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    started_at     TIMESTAMPTZ,
    finished_at    TIMESTAMPTZ,
    snapshot_id    UUID        REFERENCES leaderboard_snapshots(id) ON DELETE SET NULL,
    attempts       INTEGER     NOT NULL DEFAULT 0,
    last_error     TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_lb_refresh_jobs_status_sched
    ON leaderboard_snapshot_refresh_jobs (status, scheduled_for ASC);

CREATE INDEX IF NOT EXISTS idx_lb_refresh_jobs_category
    ON leaderboard_snapshot_refresh_jobs (category, scheduled_for DESC);

COMMENT ON TABLE leaderboard_snapshot_refresh_jobs IS
    'Scheduled/queued jobs that generate leaderboard snapshots. Operators can '
    'inspect status, attempts and errors for observability.';

-- Observability view: per-category freshness of the latest snapshot.
CREATE OR REPLACE VIEW leaderboard_snapshot_freshness AS
SELECT
    s.category,
    s.id                                   AS snapshot_id,
    s.version,
    s.created_at,
    s.expires_at,
    s.entry_count,
    (NOW() - s.created_at)                 AS age,
    (s.expires_at - NOW())                 AS time_to_expiry,
    (s.expires_at <= NOW())                AS is_stale,
    (SELECT MAX(j.finished_at)
       FROM leaderboard_snapshot_refresh_jobs j
      WHERE j.category = s.category
        AND j.status = 'succeeded')        AS last_successful_refresh_at
FROM leaderboard_snapshots s
WHERE s.version = (
    SELECT MAX(s2.version)
      FROM leaderboard_snapshots s2
     WHERE s2.category = s.category
);

COMMENT ON VIEW leaderboard_snapshot_freshness IS
    'Latest snapshot per category with age, expiry and staleness flag for '
    'operator dashboards and alerting on stale leaderboards.';