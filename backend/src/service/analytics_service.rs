/// Analytics service — aggregates on-chain and off-chain metrics.
/// Privacy: player-level data is only returned to the player themselves or admins.
/// Enhanced with data aggregation and revenue metrics for #689.
use crate::api_error::ApiError;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

// ─── DTOs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct GameMetricsResponse {
    pub game_id: i32,
    pub total_matches: i64,
    pub total_players: i64,
    pub total_wagered: i64,
    pub total_rewards_paid: i64,
    pub avg_match_duration_secs: i64,
    pub last_updated: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct PlatformMetricsResponse {
    pub total_matches_all_time: i64,
    pub active_players_30d: i64,
    pub total_staked: i64,
    pub total_volume: i64,
    pub last_updated: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct PlayerInsightsResponse {
    pub user_id: Uuid,
    pub game_id: i32,
    pub matches_played: i64,
    pub win_rate_pct: f64,
    pub avg_session_secs: i64,
    pub last_active: chrono::DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RecordMatchRequest {
    pub game_id: i32,
    pub match_id: Uuid,
    pub duration_secs: i64,
    pub wager_amount: i64,
    pub reward_amount: i64,
    pub player_count: i32,
}

// ─── Ingestion DTOs (#251) ───────────────────────────────────────────────────

/// A single analytics event as delivered by a producer.
/// `event_id` is a stable, producer-assigned identifier; `producer_id`
/// scopes it so two producers may reuse the same event_id safely.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnalyticsEvent {
    pub producer_id: String,
    pub event_id: String,
    pub schema_version: i32,
    pub event_name: String,
    pub occurred_at: chrono::DateTime<Utc>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Outcome of ingesting a single event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionOutcome {
    /// Event was newly accepted.
    Accepted,
    /// Event was already accepted (idempotent retry).
    Duplicate,
    /// Event failed validation and was recorded in the rejection path.
    Rejected,
}

#[derive(Debug, Serialize)]
pub struct IngestionResult {
    pub producer_id: String,
    pub event_id: String,
    pub outcome: IngestionOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

/// Supported schema versions for ingestion. Rollouts add versions here.
pub const SUPPORTED_SCHEMA_VERSIONS: &[i32] = &[1];

// ─── Identity stitching DTOs (#252) ──────────────────────────────────────────

/// Purpose-limited identity link. Raw auth identifiers are never stored here;
/// callers pass an already-pseudonymised subject id.
#[derive(Debug, Clone, Deserialize)]
pub struct IdentityLinkRequest {
    pub anonymous_subject: String,
    pub authenticated_subject: String,
    pub purpose: String,
    pub consent_granted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityLinkOutcome {
    Linked,
    AlreadyLinked,
    ConsentMissing,
    Collision,
}

#[derive(Debug, Serialize)]
pub struct IdentityLinkResult {
    pub outcome: IdentityLinkOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_subject: Option<String>,
}

// ─── Metric catalog DTOs (#253) ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct MetricCatalogEntry {
    pub name: String,
    pub owner: String,
    pub query_definition: String,
    pub source_events: Vec<String>,
    pub freshness_secs: i64,
    pub privacy_class: String,
    pub deprecated: bool,
}

// ─── Budget DTOs (#254) ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct QueryBudget {
    pub max_latency_ms: u64,
    pub max_freshness_secs: i64,
    pub max_range_secs: i64,
    pub max_rows: i64,
}

impl Default for QueryBudget {
    fn default() -> Self {
        Self {
            max_latency_ms: 2_000,
            max_freshness_secs: 3_600,
            max_range_secs: 90 * 24 * 3_600,
            max_rows: 1_000,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BudgetedResult<T> {
    pub data: T,
    /// True when the underlying data is older than the freshness budget.
    pub stale: bool,
    /// True when the result was truncated or could not be fully computed.
    pub incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
}

// ─── DB rows ──────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct GameMetricsRow {
    pub game_id: i32,
    pub total_matches: i64,
    pub total_players: i64,
    pub total_wagered: i64,
    pub total_rewards_paid: i64,
    pub avg_match_duration_secs: i64,
    pub last_updated: chrono::DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct PlayerInsightsRow {
    pub user_id: Uuid,
    pub game_id: i32,
    pub matches_played: i64,
    pub wins: i64,
    pub avg_session_secs: i64,
    pub last_active: chrono::DateTime<Utc>,
}

// ─── Service ──────────────────────────────────────────────────────────────────

pub struct AnalyticsService {
    db: PgPool,
}

impl AnalyticsService {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    // ── Ingestion deduplication (#251) ──────────────────────────────────────

    /// Ingest a batch of analytics events with durable deduplication.
    ///
    /// Semantics:
    /// - `(producer_id, event_id)` is unique; retries converge to one accepted event.
    /// - Out-of-order batches are fine: ordering is by `occurred_at`, not arrival.
    /// - Schema-invalid events are recorded in `analytics_event_rejections`
    ///   so operators can diagnose them.
    pub async fn ingest_events(
        &self,
        events: &[AnalyticsEvent],
    ) -> Result<Vec<IngestionResult>, ApiError> {
        let mut results = Vec::with_capacity(events.len());
        for event in events {
            results.push(self.ingest_event(event).await?);
        }
        Ok(results)
    }

    async fn ingest_event(&self, event: &AnalyticsEvent) -> Result<IngestionResult, ApiError> {
        // Validate schema before touching the accepted table.
        if let Some(reason) = validate_event(event) {
            self.record_rejection(event, &reason).await?;
            return Ok(IngestionResult {
                producer_id: event.producer_id.clone(),
                event_id: event.event_id.clone(),
                outcome: IngestionOutcome::Rejected,
                rejection_reason: Some(reason),
            });
        }

        // Insert-or-ignore makes retries idempotent.
        let inserted = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO analytics_events
                (producer_id, event_id, schema_version, event_name, occurred_at, payload, received_at)
            VALUES ($1, $2, $3, $4, $5, $6, NOW())
            ON CONFLICT (producer_id, event_id) DO NOTHING
            RETURNING 1
            "#,
        )
        .bind(&event.producer_id)
        .bind(&event.event_id)
        .bind(event.schema_version)
        .bind(&event.event_name)
        .bind(event.occurred_at)
        .bind(&event.payload)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(IngestionResult {
            producer_id: event.producer_id.clone(),
            event_id: event.event_id.clone(),
            outcome: if inserted.is_some() {
                IngestionOutcome::Accepted
            } else {
                IngestionOutcome::Duplicate
            },
            rejection_reason: None,
        })
    }

    async fn record_rejection(
        &self,
        event: &AnalyticsEvent,
        reason: &str,
    ) -> Result<(), ApiError> {
        sqlx::query(
            r#"
            INSERT INTO analytics_event_rejections
                (producer_id, event_id, schema_version, event_name, reason, payload, rejected_at)
            VALUES ($1, $2, $3, $4, $5, $6, NOW())
            ON CONFLICT (producer_id, event_id) DO NOTHING
            "#,
        )
        .bind(&event.producer_id)
        .bind(&event.event_id)
        .bind(event.schema_version)
        .bind(&event.event_name)
        .bind(reason)
        .bind(&event.payload)
        .execute(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        Ok(())
    }

    // ── Identity stitching (#252) ───────────────────────────────────────────

    /// Link an anonymous subject to an authenticated subject for a purpose.
    ///
    /// Deterministic semantics:
    /// - Requires explicit consent; otherwise `ConsentMissing`.
    /// - If the anonymous subject is already linked to a *different*
    ///   authenticated subject, returns `Collision` (no silent merge).
    /// - Re-linking the same pair is idempotent (`AlreadyLinked`).
    pub async fn link_identity(
        &self,
        req: &IdentityLinkRequest,
    ) -> Result<IdentityLinkResult, ApiError> {
        if !req.consent_granted {
            return Ok(IdentityLinkResult {
                outcome: IdentityLinkOutcome::ConsentMissing,
                canonical_subject: None,
            });
        }

        let existing = sqlx::query_scalar::<_, String>(
            r#"
            SELECT authenticated_subject
            FROM analytics_identity_links
            WHERE anonymous_subject = $1 AND purpose = $2 AND unlinked_at IS NULL
            "#,
        )
        .bind(&req.anonymous_subject)
        .bind(&req.purpose)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        match existing {
            Some(subject) if subject == req.authenticated_subject => Ok(IdentityLinkResult {
                outcome: IdentityLinkOutcome::AlreadyLinked,
                canonical_subject: Some(subject),
            }),
            Some(_) => Ok(IdentityLinkResult {
                outcome: IdentityLinkOutcome::Collision,
                canonical_subject: None,
            }),
            None => {
                sqlx::query(
                    r#"
                    INSERT INTO analytics_identity_links
                        (anonymous_subject, authenticated_subject, purpose, linked_at)
                    VALUES ($1, $2, $3, NOW())
                    "#,
                )
                .bind(&req.anonymous_subject)
                .bind(&req.authenticated_subject)
                .bind(&req.purpose)
                .execute(&self.db)
                .await
                .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

                Ok(IdentityLinkResult {
                    outcome: IdentityLinkOutcome::Linked,
                    canonical_subject: Some(req.authenticated_subject.clone()),
                })
            }
        }
    }

    /// Reversibly unlink an anonymous subject (e.g. consent withdrawal).
    pub async fn unlink_identity(
        &self,
        anonymous_subject: &str,
        purpose: &str,
    ) -> Result<bool, ApiError> {
        let affected = sqlx::query(
            r#"
            UPDATE analytics_identity_links
            SET unlinked_at = NOW()
            WHERE anonymous_subject = $1 AND purpose = $2 AND unlinked_at IS NULL
            "#,
        )
        .bind(anonymous_subject)
        .bind(purpose)
        .execute(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        .rows_affected();
        Ok(affected > 0)
    }

    // ── Metric catalog (#253) ───────────────────────────────────────────────

    /// Look up a catalogued metric definition by name.
    pub async fn get_metric_definition(
        &self,
        name: &str,
    ) -> Result<Option<MetricCatalogEntry>, ApiError> {
        let row = sqlx::query_as::<_, (String, String, String, serde_json::Value, i64, String, bool)>(
            r#"
            SELECT name, owner, query_definition, source_events, freshness_secs, privacy_class, deprecated
            FROM analytics_metric_catalog
            WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(row.map(|r| MetricCatalogEntry {
            name: r.0,
            owner: r.1,
            query_definition: r.2,
            source_events: serde_json::from_value(r.3).unwrap_or_default(),
            freshness_secs: r.4,
            privacy_class: r.5,
            deprecated: r.6,
        }))
    }

    // ── Budget enforcement (#254) ───────────────────────────────────────────

    /// Enforce a query budget around a metric lookup. Returns a degraded
    /// (stale/incomplete) marker rather than silently presenting old data
    /// as current.
    pub async fn get_budgeted_metric(
        &self,
        name: &str,
        budget: QueryBudget,
    ) -> Result<BudgetedResult<Option<MetricCatalogEntry>>, ApiError> {
        let started = std::time::Instant::now();
        let entry = self.get_metric_definition(name).await?;
        let elapsed_ms = started.elapsed().as_millis() as u64;

        let mut stale = false;
        let mut incomplete = false;
        let mut degraded_reason: Option<String> = None;

        if elapsed_ms > budget.max_latency_ms {
            incomplete = true;
            degraded_reason = Some(format!(
                "query latency {elapsed_ms}ms exceeded budget {}ms",
                budget.max_latency_ms
            ));
        }

        if let Some(ref e) = entry {
            if e.freshness_secs > budget.max_freshness_secs {
                stale = true;
                degraded_reason.get_or_insert_with(|| {
                    format!(
                        "metric freshness {}s exceeds budget {}s",
                        e.freshness_secs, budget.max_freshness_secs
                    )
                });
            }
        }

        Ok(BudgetedResult {
            data: entry,
            stale,
            incomplete,
            degraded_reason,
        })
    }

    /// Record a completed match (called by match service after resolution).
    pub async fn record_match(&self, req: &RecordMatchRequest) -> Result<(), ApiError> {
        sqlx::query!(
            r#"
            INSERT INTO analytics_game_metrics
                (game_id, total_matches, total_players, total_wagered,
                 total_rewards_paid, avg_match_duration_secs, last_updated)
            VALUES ($1, 1, $2, $3, $4, $5, NOW())
            ON CONFLICT (game_id) DO UPDATE SET
                total_matches            = analytics_game_metrics.total_matches + 1,
                total_players            = analytics_game_metrics.total_players + EXCLUDED.total_players,
                total_wagered            = analytics_game_metrics.total_wagered + EXCLUDED.total_wagered,
                total_rewards_paid       = analytics_game_metrics.total_rewards_paid + EXCLUDED.total_rewards_paid,
                avg_match_duration_secs  = (
                    analytics_game_metrics.avg_match_duration_secs
                    * analytics_game_metrics.total_matches
                    + EXCLUDED.avg_match_duration_secs
                ) / (analytics_game_metrics.total_matches + 1),
                last_updated             = NOW()
            "#,
            req.game_id,
            req.player_count as i64,
            req.wager_amount,
            req.reward_amount,
            req.duration_secs,
        )
        .execute(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        // Update platform totals
        sqlx::query!(
            r#"
            INSERT INTO analytics_platform (id, total_matches_all_time, total_volume, last_updated)
            VALUES (1, 1, $1, NOW())
            ON CONFLICT (id) DO UPDATE SET
                total_matches_all_time = analytics_platform.total_matches_all_time + 1,
                total_volume           = analytics_platform.total_volume + EXCLUDED.total_volume,
                last_updated           = NOW()
            "#,
            req.wager_amount,
        )
        .execute(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(())
    }

    /// Record player behaviour (privacy-safe: no PII in analytics tables).
    pub async fn record_player_behaviour(
        &self,
        user_id: Uuid,
        game_id: i32,
        won: bool,
        session_secs: i64,
    ) -> Result<(), ApiError> {
        sqlx::query!(
            r#"
            INSERT INTO analytics_player_behaviour
                (user_id, game_id, matches_played, wins, avg_session_secs, last_active)
            VALUES ($1, $2, 1, $3, $4, NOW())
            ON CONFLICT (user_id, game_id) DO UPDATE SET
                matches_played   = analytics_player_behaviour.matches_played + 1,
                wins             = analytics_player_behaviour.wins + EXCLUDED.wins,
                avg_session_secs = (
                    analytics_player_behaviour.avg_session_secs
                    * analytics_player_behaviour.matches_played
                    + EXCLUDED.avg_session_secs
                ) / (analytics_player_behaviour.matches_played + 1),
                last_active      = NOW()
            "#,
            user_id,
            game_id,
            if won { 1i64 } else { 0i64 },
            session_secs,
        )
        .execute(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(())
    }

    pub async fn get_game_metrics(
        &self,
        game_id: i32,
    ) -> Result<Option<GameMetricsResponse>, ApiError> {
        let row = sqlx::query_as!(
            GameMetricsRow,
            "SELECT * FROM analytics_game_metrics WHERE game_id = $1",
            game_id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(row.map(|r| GameMetricsResponse {
            game_id: r.game_id,
            total_matches: r.total_matches,
            total_players: r.total_players,
            total_wagered: r.total_wagered,
            total_rewards_paid: r.total_rewards_paid,
            avg_match_duration_secs: r.avg_match_duration_secs,
            last_updated: r.last_updated,
        }))
    }

    pub async fn get_platform_metrics(&self) -> Result<PlatformMetricsResponse, ApiError> {
        let row = sqlx::query!(
            r#"
            SELECT
                COALESCE(total_matches_all_time, 0) AS "total_matches_all_time!: i64",
                COALESCE(active_players_30d, 0)     AS "active_players_30d!: i64",
                COALESCE(total_staked, 0)            AS "total_staked!: i64",
                COALESCE(total_volume, 0)            AS "total_volume!: i64",
                last_updated
            FROM analytics_platform WHERE id = 1
            "#
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(row.map(|r| PlatformMetricsResponse {
            total_matches_all_time: r.total_matches_all_time,
            active_players_30d: r.active_players_30d,
            total_staked: r.total_staked,
            total_volume: r.total_volume,
            last_updated: r.last_updated,
        }).unwrap_or(PlatformMetricsResponse {
            total_matches_all_time: 0,
            active_players_30d: 0,
            total_staked: 0,
            total_volume: 0,
            last_updated: Utc::now(),
        }))
    }

    /// Player insights — only accessible by the player or an admin.
    pub async fn get_player_insights(
        &self,
        requesting_user_id: Uuid,
        target_user_id: Uuid,
        is_admin: bool,
        game_id: i32,
    ) -> Result<Option<PlayerInsightsResponse>, ApiError> {
        if requesting_user_id != target_user_id && !is_admin {
            return Err(ApiError::forbidden("not authorised to view this data"));
        }

        let row = sqlx::query_as!(
            PlayerInsightsRow,
            r#"
            SELECT user_id, game_id, matches_played, wins, avg_session_secs, last_active
            FROM analytics_player_behaviour
            WHERE user_id = $1 AND game_id = $2
            "#,
            target_user_id,
            game_id,
        )
        .fetch_optional(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(row.map(|r| {
            let win_rate = if r.matches_played > 0 {
                r.wins as f64 / r.matches_played as f64 * 100.0
            } else {
                0.0
            };
            PlayerInsightsResponse {
                user_id: r.user_id,
                game_id: r.game_id,
                matches_played: r.matches_played,
                win_rate_pct: (win_rate * 10.0).round() / 10.0,
                avg_session_secs: r.avg_session_secs,
                last_active: r.last_active,
            }
        }))
    }

    // ── Data Aggregation (#689) ─────────────────────────────────────────────

    /// Get aggregated statistics across games
    pub async fn get_aggregated_stats(
        &self,
        game_id: Option<i32>,
        _period_start: Option<i64>,
        _period_end: Option<i64>,
    ) -> Result<serde_json::Value, ApiError> {
        let game_filter = game_id.map(|_| "AND game_id = $1").unwrap_or("");
        let query = format!(
            r#"
            SELECT
                COUNT(*) as total_matches,
                SUM(total_players) as total_players,
                SUM(total_wagered) as total_wagered,
                SUM(total_rewards_paid) as total_rewards_paid,
                AVG(avg_match_duration_secs) as avg_duration
            FROM analytics_game_metrics
            WHERE 1=1 {}
            "#,
            game_filter
        );

        let row = sqlx::query_scalar::<_, (i64, Option<i64>, Option<i64>, Option<i64>, Option<f64>)>(&query)
            .fetch_one(&self.db)
            .await
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        Ok(serde_json::json!({
            "total_matches": row.0,
            "total_players": row.1.unwrap_or(0),
            "total_wagered": row.2.unwrap_or(0),
            "total_rewards": row.3.unwrap_or(0),
            "avg_duration": row.4.unwrap_or(0.0),
        }))
    }

    /// Get revenue metrics
    pub async fn get_revenue_metrics(
        &self,
        _game_id: Option<i32>,
        _period_start: Option<i64>,
        _period_end: Option<i64>,
    ) -> Result<serde_json::Value, ApiError> {
        let platform = self.get_platform_metrics().await?;
        let total_users = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT user_id) FROM analytics_player_behaviour"
        )
        .fetch_one(&self.db)
        .await
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;

        let revenue_per_user = if total_users > 0 {
            platform.total_volume / total_users
        } else {
            0
        };

        Ok(serde_json::json!({
            "total_volume": platform.total_volume,
            "total_rewards_paid": 0,
            "net_revenue": platform.total_volume,
            "revenue_per_user": revenue_per_user,
            "total_users": total_users,
        }))
    }
}

/// Validate an event against the supported schema. Returns a rejection reason
/// when invalid, or `None` when the event may be accepted.
fn validate_event(event: &AnalyticsEvent) -> Option<String> {
    if event.producer_id.trim().is_empty() {
        return Some("missing producer_id".to_string());
    }
    if event.event_id.trim().is_empty() {
        return Some("missing event_id".to_string());
    }
    if event.event_name.trim().is_empty() {
        return Some("missing event_name".to_string());
    }
    if !SUPPORTED_SCHEMA_VERSIONS.contains(&event.schema_version) {
        return Some(format!(
            "unsupported schema_version {}",
            event.schema_version
        ));
    }
    None
}
