use crate::api_error::ApiError;
use crate::models::{
    CloseSeasonResponse, LeaderboardEntry, LeaderboardResponse, LeaderboardStats,
    PlayerRankResponse, PlayerStatsSnapshot, RankHistory, RankHistoryEntry, Season,
    SeasonalLeaderboard,
};
use crate::middleware::cache::{keys as cache_keys, policies as cache_policies, ResponseCache};
use crate::realtime::leaderboard_broadcaster::{LeaderboardBroadcaster, RankObservation};
use chrono::{DateTime, Utc, Duration};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

/// How long a snapshot may be served before it is considered stale (#257).
///
/// Bounded and observable: every response carries `snapshot_age_seconds` and
/// `is_stale`, so a caller can tell that the board it is paginating is older
/// than this window instead of silently mixing in newer ratings.
pub const LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS: i64 = 300;

/// Stable tie-break used for every snapshot page (#257).
///
/// `elo_rating DESC, updated_at ASC, user_id ASC` — deterministic across pages
/// so a refresh never moves a user between pages within one snapshot.
pub const LEADERBOARD_TIE_BREAK: &str = "elo_rating DESC, updated_at ASC, user_id ASC";

/// Metadata describing the snapshot a leaderboard response was served from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// Snapshot version/identifier. All pages of one board share this value.
    pub snapshot_id: Uuid,
    /// When the snapshot was generated.
    pub snapshot_created_at: DateTime<Utc>,
    /// Age of the snapshot in seconds at response time.
    pub snapshot_age_seconds: i64,
    /// True when the snapshot is older than `LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS`.
    pub is_stale: bool,
    /// Upper bound (seconds) after which a snapshot is reported stale.
    pub max_age_seconds: i64,
    /// Tie-break rule applied within the snapshot, for client-side verification.
    pub tie_break: String,
}

impl SnapshotMetadata {
    pub fn new(snapshot_id: Uuid, snapshot_created_at: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        let age = (now - snapshot_created_at).num_seconds().max(0);
        Self {
            snapshot_id,
            snapshot_created_at,
            snapshot_age_seconds: age,
            is_stale: age > LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS,
            max_age_seconds: LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS,
            tie_break: LEADERBOARD_TIE_BREAK.to_string(),
        }
    }
}

/// A single row of a generated leaderboard snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotRow {
    pub user_id: Uuid,
    pub username: String,
    pub avatar_url: Option<String>,
    pub ranking: i32,
    pub elo_rating: i32,
    pub matches_played: i32,
    pub wins: i32,
    pub losses: i32,
    pub win_rate: f64,
    pub updated_at: DateTime<Utc>,
}

/// Result of generating a new snapshot for a category.
#[derive(Debug, Clone)]
pub struct SnapshotGeneration {
    pub snapshot_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub row_count: usize,
}

/// Pure, deterministic ordering used to assign ranks within a snapshot (#257).
///
/// Mirrors `LEADERBOARD_TIE_BREAK` so the in-memory path (used by tests and by
/// snapshot generation) and the SQL path agree exactly.
pub fn order_snapshot_rows(rows: &mut [SnapshotRow]) {
    rows.sort_by(|a, b| {
        b.elo_rating
            .cmp(&a.elo_rating)
            .then_with(|| a.updated_at.cmp(&b.updated_at))
            .then_with(|| a.user_id.cmp(&b.user_id))
    });
}

/// Assigns 1-based ranks after ordering. Ties keep distinct ranks so that a
/// user's page position is stable for a given snapshot.
pub fn rank_snapshot_rows(rows: &mut [SnapshotRow]) {
    order_snapshot_rows(rows);
    for (idx, row) in rows.iter_mut().enumerate() {
        row.ranking = (idx + 1) as i32;
    }
}

/// Paginates an already-ordered snapshot. Returns the page and total count.
pub fn paginate_snapshot(rows: &[SnapshotRow], limit: i64, offset: i64) -> (Vec<SnapshotRow>, i64) {
    let total = rows.len() as i64;
    let offset = offset.max(0) as usize;
    let limit = limit.max(0) as usize;
    let page = rows
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    (page, total)
}

pub struct LeaderboardService {
    db_pool: PgPool,
    /// Pushes rank changes to subscribed clients (Issue #900).
    ///
    /// Optional so the service stays constructible without the realtime stack —
    /// migrations, batch jobs and tests have no WebSocket clients to notify,
    /// and rank updates must not depend on Redis being reachable.
    broadcaster: Option<Arc<LeaderboardBroadcaster>>,
    /// Response cache for the read path (Issue #910).
    ///
    /// Optional for the same reason as `broadcaster`: the service must stay
    /// usable without Redis, and a cache that is absent simply means every
    /// read goes to Postgres, which is the behaviour this replaced.
    cache: Option<ResponseCache>,
}

impl LeaderboardService {
    pub fn new(db_pool: PgPool) -> Self {
        Self {
            db_pool,
            broadcaster: None,
            cache: None,
        }
    }

    /// Attaches the response cache to the leaderboard read path.
    pub fn with_cache(mut self, cache: ResponseCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Attaches a broadcaster so rank updates are pushed in real time.
    pub fn with_broadcaster(mut self, broadcaster: Arc<LeaderboardBroadcaster>) -> Self {
        self.broadcaster = Some(broadcaster);
        self
    }

    /// Publishes a rank change, if a broadcaster is attached.
    ///
    /// Deliberately infallible: a client that misses a push re-syncs from the
    /// REST board, so a broadcast failure must never fail the rank update that
    /// has already been committed.
    async fn broadcast_rank(&self, category: &str, observation: RankObservation) {
        if let Some(broadcaster) = &self.broadcaster {
            broadcaster.publish_changes(category, &[observation]).await;
        }
    }

    /// Get leaderboard rankings for a category from the latest snapshot (#257).
    ///
    /// Every page is served from a single `snapshot_id`, ordered by the stable
    /// tie-break, so a refresh never moves a user between pages mid-pagination.
    /// The response carries snapshot version/time and staleness metadata.
    pub async fn get_leaderboard(
        &self,
        category: &str,
        limit: i64,
        offset: i64,
    ) -> Result<LeaderboardResponse, ApiError> {
        // Resolve the latest snapshot for this category. Wrapped in
        // `time_query` (#1084) — hit on every leaderboard page load.
        let snapshot = crate::metrics::time_query(
            "leaderboard.get_leaderboard",
            sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                r#"
                SELECT id, created_at
                FROM leaderboard_snapshots
                WHERE game = $1 AND period = 'all_time'
                ORDER BY version DESC
                LIMIT 1
                "#
            )
            .bind(category)
            .fetch_optional(&self.db_pool),
        )
        .await
        .map_err(ApiError::DatabaseError)?;

        let (snapshot_id, snapshot_created_at) = match snapshot {
            Some(s) => s,
            None => {
                // No snapshot yet: generate one so the board is never empty
                // just because the refresh job has not run.
                let generated = self.generate_snapshot(category).await?;
                (generated.snapshot_id, generated.created_at)
            }
        };

        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>, i32, i32, i32, i32, i32, f64, DateTime<Utc>, i64)>(
            r#"
            SELECT
                s.id, s.user_id, u.username, u.avatar_url,
                s.ranking, s.elo_rating, s.matches_played, s.wins, s.losses, s.win_rate,
                s.updated_at,
                COUNT(*) OVER() as total_count
            FROM leaderboard_snapshot_entries s
            INNER JOIN users u ON s.user_id = u.id
            WHERE s.snapshot_id = $1
            ORDER BY s.elo_rating DESC, s.updated_at ASC, s.user_id ASC
            LIMIT $2 OFFSET $3
            "#
        )
        .bind(snapshot_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db_pool)
        .await
        .map_err(ApiError::DatabaseError)?;

        let total_count = rows.first().map(|e| e.11).unwrap_or(0);

        let leaderboard_entries = rows
            .into_iter()
            .map(|(id, user_id, username, avatar_url, ranking, elo_rating, matches_played, wins, losses, win_rate, updated_at, _)| {
                LeaderboardEntry {
                    id,
                    user_id,
                    username,
                    avatar_url,
                    ranking,
                    elo_rating,
                    matches_played,
                    wins,
                    losses,
                    win_rate,
                    period: "all_time".to_string(),
                    updated_at,
                }
            })
            .collect();

        let metadata = SnapshotMetadata::new(snapshot_id, snapshot_created_at, Utc::now());

        let response = LeaderboardResponse {
            entries: leaderboard_entries,
            total_count,
            period: "all_time".to_string(),
            category: category.to_string(),
            snapshot: Some(metadata),
        };

        if let Some(cache) = &self.cache {
            // Tagged by category so a new snapshot drops every cached page of
            // that board at once, rather than leaving page 3 stale while page 1
            // refreshes.
            cache
                .set(
                    &cache_keys::leaderboard(category, limit, offset),
                    &response,
                    cache_policies::LEADERBOARD,
                    &[cache_keys::leaderboard_tag(category)],
                )
                .await;
        }

        Ok(response)
    }

    /// Generate a new immutable snapshot for a category (#257).
    ///
    /// Reads the current `leaderboards` rows, orders them by the stable
    /// tie-break, assigns ranks, and persists the result under a new version.
    /// Concurrent score updates that land after the read are not reflected in
    /// this snapshot — they are picked up by the next generation, so a single
    /// snapshot is internally consistent.
    pub async fn generate_snapshot(&self, category: &str) -> Result<SnapshotGeneration, ApiError> {
        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::DatabaseError)?;

        // Serialize snapshot generation per category so two concurrent
        // refreshes cannot interleave versions.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(format!("leaderboard_snapshot:{category}"))
            .execute(&mut *tx)
            .await
            .map_err(ApiError::DatabaseError)?;

        let mut rows = sqlx::query_as::<_, (Uuid, String, Option<String>, i32, i32, i32, i32, f64, DateTime<Utc>)>(
            r#"
            SELECT
                l.user_id, u.username, u.avatar_url,
                l.elo_rating, l.matches_played, l.wins, l.losses, l.win_rate,
                l.updated_at
            FROM leaderboards l
            INNER JOIN users u ON l.user_id = u.id
            WHERE l.game = $1 AND l.period = 'all_time'
            "#
        )
        .bind(category)
        .fetch_all(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?
        .into_iter()
        .map(|(user_id, username, avatar_url, elo_rating, matches_played, wins, losses, win_rate, updated_at)| {
            SnapshotRow {
                user_id,
                username,
                avatar_url,
                ranking: 0,
                elo_rating,
                matches_played,
                wins,
                losses,
                win_rate,
                updated_at,
            }
        })
        .collect::<Vec<_>>();

        rank_snapshot_rows(&mut rows);

        let next_version = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COALESCE(MAX(version), 0) + 1
            FROM leaderboard_snapshots
            WHERE game = $1 AND period = 'all_time'
            "#
        )
        .bind(category)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        let (snapshot_id, created_at) = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
            r#"
            INSERT INTO leaderboard_snapshots (game, period, version, created_at)
            VALUES ($1, 'all_time', $2, NOW())
            RETURNING id, created_at
            "#
        )
        .bind(category)
        .bind(next_version)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        for row in &rows {
            sqlx::query(
                r#"
                INSERT INTO leaderboard_snapshot_entries
                    (snapshot_id, user_id, ranking, elo_rating, matches_played, wins, losses, win_rate, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                "#
            )
            .bind(snapshot_id)
            .bind(row.user_id)
            .bind(row.ranking)
            .bind(row.elo_rating)
            .bind(row.matches_played)
            .bind(row.wins)
            .bind(row.losses)
            .bind(row.win_rate)
            .bind(row.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::DatabaseError)?;
        }

        tx.commit().await.map_err(ApiError::DatabaseError)?;

        // A new snapshot supersedes every cached page of this board.
        if let Some(cache) = &self.cache {
            cache
                .invalidate_tags(&[cache_keys::leaderboard_tag(category)])
                .await;
        }

        Ok(SnapshotGeneration {
            snapshot_id,
            created_at,
            row_count: rows.len(),
        })
    }

    /// Get seasonal leaderboard rankings (optimized with single query)
    pub async fn get_seasonal_leaderboard(
        &self,
        category: &str,
        season: &str,
        limit: i64,
        offset: i64,
    ) -> Result<SeasonalLeaderboard, ApiError> {
        // Optimized: use window function to get count in same query
        let entries = sqlx::query_as::<_, (Uuid, Uuid, String, Option<String>, i32, i32, i32, i32, i32, f64, String, DateTime<Utc>, i64)>(
            r#"
            SELECT 
                l.id, l.user_id, u.username, u.avatar_url,
                l.ranking, l.elo_rating, l.matches_played, l.wins, l.losses, l.win_rate,
                l.period, l.updated_at,
                COUNT(*) OVER() as total_count
            FROM leaderboards l
            INNER JOIN users u ON l.user_id = u.id
            WHERE l.game = $1 AND l.period = $2
            ORDER BY l.ranking ASC
            LIMIT $3 OFFSET $4
            "#
        )
        .bind(category)
        .bind(season)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        let total_count = entries.first().map(|e| e.12).unwrap_or(0);

        let leaderboard_entries = entries
            .into_iter()
            .map(|(id, user_id, username, avatar_url, ranking, elo_rating, matches_played, wins, losses, win_rate, period, updated_at, _)| {
                LeaderboardEntry {
                    id,
                    user_id,
                    username,
                    avatar_url,
                    ranking,
                    elo_rating,
                    matches_played,
                    wins,
                    losses,
                    win_rate,
                    period,
                    updated_at,
                }
            })
            .collect();

        Ok(SeasonalLeaderboard {
            season_id: season.to_string(),
            season_name: format!("Season {}", season),
            start_date: Utc::now() - Duration::days(30),
            end_date: Utc::now(),
            entries: leaderboard_entries,
            total_participants: total_count,
        })
    }

    /// Get player's rank in a category
    pub async fn get_player_rank(
        &self,
        category: &str,
        player_id: Uuid,
    ) -> Result<PlayerRankResponse, ApiError> {
        let player = sqlx::query_as::<_, (Uuid, String, Option<String>, i32, i32, i32, i32, i32, f64, DateTime<Utc>)>(
            r#"
            SELECT 
                l.user_id, u.username, u.avatar_url,
                l.ranking, l.elo_rating, l.matches_played, l.wins, l.losses, l.win_rate,
                l.updated_at
            FROM leaderboards l
            JOIN users u ON l.user_id = u.id
            WHERE l.game = $1 AND l.user_id = $2 AND l.period = 'all_time'
            "#
        )
        .bind(category)
        .bind(player_id)
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?
        .ok_or_else(|| ApiError::NotFound)?;

        let (user_id, username, avatar_url, ranking, elo_rating, matches_played, wins, losses, win_rate, updated_at) = player;

        // Get rank change from previous period
        let previous_rank = sqlx::query_scalar::<_, Option<i32>>(
            r#"
            SELECT ranking FROM leaderboards 
            WHERE game = $1 AND user_id = $2 AND period = 'weekly'
            ORDER BY updated_at DESC LIMIT 1
            "#
        )
        .bind(category)
        .bind(player_id)
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?
        .flatten();

        let rank_change = previous_rank.map(|prev| prev - ranking);

        Ok(PlayerRankResponse {
            user_id,
            username,
            avatar_url,
            current_rank: ranking,
            elo_rating,
            matches_played,
            wins,
            losses,
            win_rate,
            rank_change,
            updated_at,
        })
    }

    /// Get player's rank history
    pub async fn get_rank_history(
        &self,
        player_id: Uuid,
        category: &str,
        days: i64,
    ) -> Result<RankHistory, ApiError> {
        let username = sqlx::query_scalar::<_, String>(
            "SELECT username FROM users WHERE id = $1"
        )
        .bind(player_id)
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?
        .ok_or_else(|| ApiError::NotFound)?;

        let history = sqlx::query_as::<_, (i32, i32, String, DateTime<Utc>)>(
            r#"
            SELECT ranking, elo_rating, period, updated_at
            FROM leaderboards
            WHERE user_id = $1 AND game = $2 AND updated_at > NOW() - INTERVAL '1 day' * $3
            ORDER BY updated_at DESC
            "#
        )
        .bind(player_id)
        .bind(category)
        .bind(days)
        .fetch_all(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        let history_entries = history
            .into_iter()
            .map(|(rank, elo_rating, period, timestamp)| {
                RankHistoryEntry {
                    rank,
                    elo_rating,
                    period,
                    timestamp,
                }
            })
            .collect();

        Ok(RankHistory {
            user_id: player_id,
            username,
            history: history_entries,
        })
    }

    /// Update player rank (optimized with batching support)
    pub async fn update_player_rank(
        &self,
        category: &str,
        player_id: Uuid,
    ) -> Result<(), ApiError> {
        // Optimized: batch Elo rating and match stats queries
        let player_stats = sqlx::query_as::<_, (Option<i32>, i32, i32, i32)>(
            r#"
            SELECT 
                ue.current_rating,
                COUNT(m.id)::int as matches_played,
                SUM(CASE WHEN m.winner_id = $1 THEN 1 ELSE 0 END)::int as wins,
                SUM(CASE WHEN m.winner_id != $1 AND (m.player1_id = $1 OR m.player2_id = $1) THEN 1 ELSE 0 END)::int as losses
            FROM user_elo ue
            LEFT JOIN matches m ON (m.player1_id = $1 OR m.player2_id = $1) AND m.game_mode = $2 AND m.status = 3
            WHERE ue.user_id = $1 AND ue.game = $2
            GROUP BY ue.current_rating
            "#
        )
        .bind(player_id)
        .bind(category)
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        let (elo_rating_opt, matches_played, wins, losses) = player_stats.unwrap_or((None, 0, 0, 0));
        let elo_rating = elo_rating_opt.unwrap_or(1200);

        let win_rate = if matches_played > 0 {
            (wins as f64 / matches_played as f64) * 100.0
        } else {
            0.0
        };

        // Optimized: get ranking with a more efficient subquery
        let new_ranking = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*) + 1 FROM user_elo 
            WHERE game = $1 AND current_rating > $2
            "#
        )
        .bind(category)
        .bind(elo_rating)
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))? as i32;

        // Upsert leaderboard entry
        sqlx::query(
            r#"
            INSERT INTO leaderboards (user_id, game, period, ranking, elo_rating, matches_played, wins, losses, win_rate, period_start, period_end, updated_at)
            VALUES ($1, $2, 'all_time', $3, $4, $5, $6, $7, $8, NOW(), NOW() + INTERVAL '1 year', NOW())
            ON CONFLICT (user_id, game, period, period_start) DO UPDATE SET
                ranking = $3,
                elo_rating = $4,
                matches_played = $5,
                wins = $6,
                losses = $7,
                win_rate = $8,
                updated_at = NOW()
            "#
        )
        .bind(player_id)
        .bind(category)
        .bind(new_ranking)
        .bind(elo_rating)
        .bind(matches_played)
        .bind(wins)
        .bind(losses)
        .bind(win_rate)
        .execute(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        // The board just changed, so every cached page of it is wrong.
        if let Some(cache) = &self.cache {
            cache
                .invalidate_tags(&[
                    cache_keys::leaderboard_tag(category),
                    cache_keys::player_tag(&player_id),
                ])
                .await;
        }

        // Push the change to subscribed clients. The tracker drops it if the
        // rank did not actually move, so calling this unconditionally is free.
        self.broadcast_rank(
            category,
            RankObservation {
                user_id: player_id,
                ranking: new_ranking,
                elo_rating,
            },
        )
        .await;

        Ok(())
    }

    /// Batch update player ranks for multiple players (optimized for refresh)
    pub async fn batch_update_player_ranks(
        &self,
        category: &str,
        player_ids: &[Uuid],
    ) -> Result<(), ApiError> {
        if player_ids.is_empty() {
            return Ok(());
        }

        // Batch process in chunks of 100 to avoid overwhelming the database
        const BATCH_SIZE: usize = 100;
        
        for chunk in player_ids.chunks(BATCH_SIZE) {
            // Process chunk concurrently using futures
            let mut tasks = Vec::new();
            for &player_id in chunk {
                tasks.push(self.update_player_rank(category, player_id));
            }
            
            // Wait for all tasks in this chunk to complete
            for task in tasks {
                task.await?;
            }
        }

        // A refresh is exactly the burst the per-player throttle holds back, so
        // flush once at the end rather than leaving the last change for each
        // player sitting until some unrelated update arrives.
        if let Some(broadcaster) = &self.broadcaster {
            broadcaster.flush(category).await;
        }

        Ok(())
    }

    /// Refresh the leaderboard and publish a new immutable snapshot (#257).
    ///
    /// Recomputes ranks, then generates a fresh snapshot so subsequent reads
    /// paginate a consistent board and cached pages are invalidated.
    pub async fn refresh_leaderboard_snapshot(
        &self,
        category: &str,
    ) -> Result<SnapshotGeneration, ApiError> {
        self.refresh_leaderboard(category).await?;
        self.generate_snapshot(category).await
    }

    /// Refresh entire leaderboard for a category (optimized with batching)
    pub async fn refresh_leaderboard(&self, category: &str) -> Result<(), ApiError> {
        // Get all players with Elo ratings for this category
        let players = sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT user_id FROM user_elo WHERE game = $1"
        )
        .bind(category)
        .fetch_all(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        // Use batch update instead of sequential processing
        self.batch_update_player_ranks(category, &players).await?;

        Ok(())
    }

    /// Get leaderboard statistics
    pub async fn get_leaderboard_stats(&self, category: &str) -> Result<LeaderboardStats, ApiError> {
        let stats = sqlx::query_as::<_, (i64, Option<f64>, Option<i32>, Option<i32>)>(
            r#"
            SELECT 
                COUNT(DISTINCT user_id) as total_players,
                AVG(elo_rating)::float as average_elo,
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY elo_rating) as median_elo,
                MAX(elo_rating) as top_player_elo
            FROM leaderboards
            WHERE game = $1 AND period = 'all_time'
            "#
        )
        .bind(category)
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        let (total_players, average_elo, median_elo, top_player_elo) = stats;

        Ok(LeaderboardStats {
            total_players,
            average_elo: average_elo.unwrap_or(0.0),
            median_elo: median_elo.unwrap_or(1200),
            top_player_elo: top_player_elo.unwrap_or(1200),
            last_updated: Utc::now(),
        })
    }

    // ── Season close (#1075) ────────────────────────────────────────────────

    /// Close `season_id`: snapshot the top `top_n` leaderboard rows into
    /// `player_stats_snapshots`, decay every rated player of that game 10%
    /// toward the 1200 baseline (floor 800), and open the next season.
    pub async fn close_season(
        &self,
        season_id: Uuid,
        top_n: i64,
    ) -> Result<CloseSeasonResponse, ApiError> {
        let season = sqlx::query_as::<_, Season>("SELECT * FROM seasons WHERE id = $1")
            .bind(season_id)
            .fetch_optional(&self.db_pool)
            .await
            .map_err(|e| ApiError::DatabaseError(e))?
            .ok_or_else(|| ApiError::not_found("Season not found"))?;

        if season.status == "closed" {
            return Err(ApiError::conflict("Season is already closed"));
        }

        let top_rows = sqlx::query_as::<_, (Uuid, i32, i32, i32, i32)>(
            r#"
            SELECT user_id, ranking, elo_rating, matches_played, wins
            FROM leaderboards
            WHERE game = $1 AND period = 'all_time'
            ORDER BY ranking ASC
            LIMIT $2
            "#,
        )
        .bind(&season.game)
        .bind(top_n)
        .fetch_all(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        for (user_id, ranking, elo_rating, matches_played, wins) in &top_rows {
            let losses = (*matches_played - *wins).max(0);
            sqlx::query(
                r#"
                INSERT INTO player_stats_snapshots
                    (season_id, user_id, game, rank, elo_rating, matches_played, wins, losses)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(season_id)
            .bind(user_id)
            .bind(&season.game)
            .bind(ranking)
            .bind(elo_rating)
            .bind(matches_played)
            .bind(wins)
            .bind(losses)
            .execute(&self.db_pool)
            .await
            .map_err(|e| ApiError::DatabaseError(e))?;
        }

        // Soft-reset every rated player of this game 10% toward the 1200
        // baseline (floor 800). Decayed in Rust via `decay_elo` (unit-tested
        // below) rather than in raw SQL, so the formula itself is verifiable
        // independent of a live database.
        let ratings = sqlx::query_as::<_, (Uuid, i32)>(
            "SELECT user_id, current_rating FROM user_elo WHERE game = $1",
        )
        .bind(&season.game)
        .fetch_all(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        let mut players_decayed = 0usize;
        for (user_id, current_rating) in &ratings {
            let new_rating = decay_elo(*current_rating);
            sqlx::query(
                "UPDATE user_elo SET current_rating = $1 WHERE game = $2 AND user_id = $3",
            )
            .bind(new_rating)
            .bind(&season.game)
            .bind(user_id)
            .execute(&self.db_pool)
            .await
            .map_err(|e| ApiError::DatabaseError(e))?;
            players_decayed += 1;
        }

        let closed_season = sqlx::query_as::<_, Season>(
            r#"
            UPDATE seasons SET status = 'closed', closed_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(season_id)
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        let next_name = next_season_name(&season.name);
        let new_season = sqlx::query_as::<_, Season>(
            r#"
            INSERT INTO seasons (game, name, status, started_at)
            VALUES ($1, $2, 'active', NOW())
            RETURNING *
            "#,
        )
        .bind(&season.game)
        .bind(&next_name)
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        Ok(CloseSeasonResponse {
            closed_season,
            new_season,
            players_snapshotted: top_rows.len(),
            players_decayed,
        })
    }

    /// Historical leaderboard snapshot for a closed season (#1075).
    pub async fn get_season_history(
        &self,
        season_id: Uuid,
    ) -> Result<Vec<PlayerStatsSnapshot>, ApiError> {
        sqlx::query_as::<_, PlayerStatsSnapshot>(
            r#"
            SELECT * FROM player_stats_snapshots
            WHERE season_id = $1
            ORDER BY rank ASC
            "#,
        )
        .bind(season_id)
        .fetch_all(&self.db_pool)
        .await
        .map_err(ApiError::from)
    }

    pub async fn get_elo_history(
        &self,
        player_id: Uuid,
        category: &str,
        start_date: DateTime<Utc>,
        end_date: DateTime<Utc>,
    ) -> Result<serde_json::Value, ApiError> {
        let entries = sqlx::query!(
            r#"
            SELECT id, user_id, game, match_id, rating_before, rating_after,
                   rating_change, opponent_id, opponent_rating, result, created_at
            FROM elo_history
            WHERE user_id = $1 AND game = $2 AND created_at BETWEEN $3 AND $4
            ORDER BY created_at ASC
            "#,
            player_id,
            category,
            start_date,
            end_date
        )
        .fetch_all(&self.db_pool)
        .await
        .map_err(ApiError::DatabaseError)?;

        let history: Vec<_> = entries
            .into_iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "user_id": row.user_id,
                    "game": row.game,
                    "match_id": row.match_id,
                    "rating_before": row.rating_before,
                    "rating_after": row.rating_after,
                    "rating_change": row.rating_change,
                    "opponent_id": row.opponent_id,
                    "opponent_rating": row.opponent_rating,
                    "result": row.result,
                    "created_at": row.created_at,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "player_id": player_id,
            "category": category,
            "start_date": start_date,
            "end_date": end_date,
            "history": history
        }))
    }
}

const ELO_BASELINE: f64 = 1200.0;
const ELO_DECAY_RATE: f64 = 0.10;
const ELO_MINIMUM: i32 = 800;

/// `new_elo = current_elo - (current_elo - 1200) * 0.10`, floored at 800
/// (#1075). A pure function so the decay math is unit-testable without a
/// live database.
fn decay_elo(current_rating: i32) -> i32 {
    let decayed = current_rating as f64 - (current_rating as f64 - ELO_BASELINE) * ELO_DECAY_RATE;
    (decayed.round() as i32).max(ELO_MINIMUM)
}

/// "Season 3" -> "Season 4"; falls back to appending " 2" when the name has
/// no trailing number to increment.
fn next_season_name(current: &str) -> String {
    let prefix = current.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = &current[prefix.len()..];
    match digits.parse::<u32>() {
        Ok(n) => format!("{prefix}{}", n + 1),
        Err(_) => format!("{current} 2"),
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::{
        order_snapshot_rows, paginate_snapshot, rank_snapshot_rows, SnapshotMetadata, SnapshotRow,
        LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS,
    };
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    fn row(user_id: Uuid, elo: i32, updated_at: chrono::DateTime<Utc>) -> SnapshotRow {
        SnapshotRow {
            user_id,
            username: format!("user-{user_id}"),
            avatar_url: None,
            ranking: 0,
            elo_rating: elo,
            matches_played: 10,
            wins: 5,
            losses: 5,
            win_rate: 50.0,
            updated_at,
        }
    }

    #[test]
    fn empty_ranking_paginates_to_nothing() {
        let rows: Vec<SnapshotRow> = Vec::new();
        let (page, total) = paginate_snapshot(&rows, 10, 0);
        assert!(page.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn pagination_across_ties_is_stable() {
        // Three users share an Elo; tie-break is updated_at ASC then user_id.
        let now = Utc::now();
        let a = Uuid::parse_str("00000000-0000-0000-0000-00000000000a").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-00000000000b").unwrap();
        let c = Uuid::parse_str("00000000-0000-0000-0000-00000000000c").unwrap();
        let mut rows = vec![
            row(c, 1500, now),
            row(a, 1500, now),
            row(b, 1500, now),
        ];
        rank_snapshot_rows(&mut rows);

        // Deterministic order: a, b, c by user_id.
        assert_eq!(rows[0].user_id, a);
        assert_eq!(rows[1].user_id, b);
        assert_eq!(rows[2].user_id, c);

        // Page 1 (limit 2) and page 2 (offset 2) never overlap.
        let (page1, total) = paginate_snapshot(&rows, 2, 0);
        let (page2, _) = paginate_snapshot(&rows, 2, 2);
        assert_eq!(total, 3);
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 1);
        assert_eq!(page1[0].user_id, a);
        assert_eq!(page1[1].user_id, b);
        assert_eq!(page2[0].user_id, c);
    }

    #[test]
    fn updated_at_breaks_elo_ties_before_user_id() {
        let now = Utc::now();
        let earlier = now - Duration::seconds(60);
        let a = Uuid::parse_str("00000000-0000-0000-0000-00000000000a").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-00000000000b").unwrap();
        let mut rows = vec![row(a, 1500, now), row(b, 1500, earlier)];
        order_snapshot_rows(&mut rows);
        // Earlier updated_at sorts first even though its user_id is larger.
        assert_eq!(rows[0].user_id, b);
        assert_eq!(rows[1].user_id, a);
    }

    #[test]
    fn concurrent_score_updates_do_not_reorder_a_fixed_snapshot() {
        // Simulate a snapshot taken, then concurrent updates arriving. The
        // snapshot rows are immutable, so re-paginating yields the same pages.
        let now = Utc::now();
        let a = Uuid::parse_str("00000000-0000-0000-0000-00000000000a").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-00000000000b").unwrap();
        let mut snapshot = vec![row(a, 1600, now), row(b, 1500, now)];
        rank_snapshot_rows(&mut snapshot);

        let (before, _) = paginate_snapshot(&snapshot, 1, 0);
        assert_eq!(before[0].user_id, a);

        // A "concurrent" update would change live data, but the snapshot is
        // untouched; paginating again returns the same first page.
        let (after, _) = paginate_snapshot(&snapshot, 1, 0);
        assert_eq!(after[0].user_id, a);
    }

    #[test]
    fn metadata_reports_staleness_bound() {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let fresh = SnapshotMetadata::new(id, now - Duration::seconds(10), now);
        assert!(!fresh.is_stale);
        assert_eq!(fresh.max_age_seconds, LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS);

        let stale = SnapshotMetadata::new(
            id,
            now - Duration::seconds(LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS + 1),
            now,
        );
        assert!(stale.is_stale);
        assert!(stale.snapshot_age_seconds > LEADERBOARD_SNAPSHOT_MAX_AGE_SECONDS);
    }
}

#[cfg(test)]
mod season_close_tests {
    use super::{decay_elo, next_season_name, ELO_MINIMUM};

    #[test]
    fn increments_trailing_number() {
        assert_eq!(next_season_name("Season 3"), "Season 4");
        assert_eq!(next_season_name("S9"), "S10");
    }

    #[test]
    fn falls_back_when_no_trailing_number() {
        assert_eq!(next_season_name("Preseason"), "Preseason 2");
    }

    #[test]
    fn decays_ten_varied_ratings_toward_baseline() {
        // (current_rating, expected new_rating) for
        // new_elo = current - (current - 1200) * 0.10
        let cases: [(i32, i32); 10] = [
            (2400, 2280), // (2400 - 1200)*0.1 = 120 -> 2280
            (2000, 1920),
            (1800, 1740),
            (1600, 1560),
            (1400, 1380),
            (1200, 1200), // already at baseline: no change
            (1000, 1020), // below baseline decays *up* toward it
            (800, 840),
            (600, 800),   // (600-1200)*0.1 = -60 -> 660, but floor is 800
            (400, 800),   // (400-1200)*0.1 = -80 -> 480, but floor is 800
        ];

        for (current, expected) in cases {
            let actual = decay_elo(current);
            assert_eq!(
                actual, expected,
                "decay_elo({current}) = {actual}, expected {expected}"
            );
            assert!(actual >= ELO_MINIMUM, "decayed rating must never drop below {ELO_MINIMUM}");
        }
    }

    #[test]
    fn decay_never_drops_below_the_floor() {
        assert_eq!(decay_elo(0), ELO_MINIMUM);
        assert_eq!(decay_elo(-500), ELO_MINIMUM);
    }
}
