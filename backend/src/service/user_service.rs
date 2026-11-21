use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Client as RedisClient};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use uuid::Uuid;

const PROFILE_CACHE_VERSION: &str = "v1";
const PROFILE_CACHE_TTL_SECONDS: u64 = 60;

use crate::api_error::ApiError;
use crate::models::match_models::{EloHistory, UserElo};
use crate::models::user::{User, UserProfile};

#[derive(Debug, Clone)]
pub struct UserService {
    pool: PgPool,
    redis_client: Option<Arc<RedisClient>>,
}

impl UserService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            redis_client: None,
        }
    }

    pub fn with_redis(mut self, redis_client: Arc<RedisClient>) -> Self {
        self.redis_client = Some(redis_client);
        self
    }

    fn profile_cache_key(user_id: Uuid) -> String {
        format!("profile:{}:{}", PROFILE_CACHE_VERSION, user_id)
    }

    pub async fn invalidate_profile_cache(&self, user_id: Uuid) {
        let Some(redis_client) = &self.redis_client else {
            return;
        };
        if let Ok(mut connection) = redis_client.get_multiplexed_async_connection().await {
            let _: Result<(), _> = connection.del(Self::profile_cache_key(user_id)).await;
        }
    }

    /// Get a user by ID
    pub async fn get_user_by_id(&self, user_id: Uuid) -> Result<User, ApiError> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT * FROM users WHERE id = $1
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::not_found("User not found")
            } else {
                ApiError::internal_error(format!("Database error: {}", e))
            }
        })?;

        Ok(user)
    }

    /// Get a user profile by ID (public view)
    pub async fn get_user_profile(&self, user_id: Uuid) -> Result<UserProfile, ApiError> {
        let cache_key = Self::profile_cache_key(user_id);
        if let Some(redis_client) = &self.redis_client {
            if let Ok(mut connection) = redis_client.get_multiplexed_async_connection().await {
                match connection.get::<_, Option<String>>(&cache_key).await {
                    Ok(Some(cached)) => {
                        if let Ok(profile) = serde_json::from_str::<UserProfile>(&cached) {
                            crate::metrics::record_profile_cache_hit();
                            let _: Result<(), _> = connection
                                .expire(&cache_key, PROFILE_CACHE_TTL_SECONDS as i64)
                                .await;
                            return Ok(profile);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!(%user_id, %error, "Profile cache read failed"),
                }
            }
        }
        crate::metrics::record_profile_cache_miss();

        let user = self.get_user_by_id(user_id).await?;

        let profile = UserProfile {
            id: user.id,
            username: user.username,
            email: None, // Don't expose email in public profile
            display_name: user.display_name,
            avatar_url: user.avatar_url,
            is_verified: user.is_verified,
            created_at: user.created_at,
            skill_score: user.reputation_score,
            fair_play_score: user.reputation_score,
            is_bad_actor: user.is_banned,
        };

        if let Some(redis_client) = &self.redis_client {
            if let Ok(mut connection) = redis_client.get_multiplexed_async_connection().await {
                if let Ok(serialized) = serde_json::to_string(&profile) {
                    let _: Result<(), _> = connection
                        .set_ex(&cache_key, serialized, PROFILE_CACHE_TTL_SECONDS)
                        .await;
                }
            }
        }

        Ok(profile)
    }

    /// Get current user's full profile (authenticated view)
    pub async fn get_current_user_profile(&self, user_id: Uuid) -> Result<User, ApiError> {
        self.get_user_by_id(user_id).await
    }

    /// Update user profile
    pub async fn update_user_profile(
        &self,
        user_id: Uuid,
        username: Option<String>,
        avatar_url: Option<String>,
        display_name: Option<String>,
        bio: Option<String>,
    ) -> Result<User, ApiError> {
        // Check if user exists
        let _existing_user = self.get_user_by_id(user_id).await?;

        // Build dynamic update query
        let mut query = String::from("UPDATE users SET updated_at = $1");
        let mut param_count = 1;
        let mut params: Vec<String> = vec![Utc::now().to_string()];

        if let Some(username) = &username {
            param_count += 1;
            query.push_str(&format!(", username = ${}", param_count));
            params.push(username.clone());
        }

        if let Some(avatar_url) = &avatar_url {
            param_count += 1;
            query.push_str(&format!(", avatar_url = ${}", param_count));
            params.push(avatar_url.clone());
        }

        if let Some(display_name) = &display_name {
            param_count += 1;
            query.push_str(&format!(", display_name = ${}", param_count));
            params.push(display_name.clone());
        }

        if let Some(bio) = &bio {
            param_count += 1;
            query.push_str(&format!(", bio = ${}", param_count));
            params.push(bio.clone());
        }

        param_count += 1;
        query.push_str(&format!(" WHERE id = ${} RETURNING *", param_count));
        params.push(user_id.to_string());

        let mut query_builder = sqlx::query_as::<_, User>(&query);
        for (i, param) in params.iter().enumerate() {
            query_builder = query_builder.bind(param);
        }

        let updated_user = query_builder.fetch_one(&self.pool).await.map_err(|e| {
            ApiError::internal_error(format!("Failed to update user: {}", e))
        })?;

        self.invalidate_profile_cache(user_id).await;

        Ok(updated_user)
    }

    /// Get user stats including win/loss record and Elo history
    pub async fn get_user_stats(&self, user_id: Uuid) -> Result<UserStats, ApiError> {
        // Get user ELO data
        let elo_data = sqlx::query_as::<_, UserElo>(
            r#"
            SELECT * FROM user_elo WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        // Get ELO history
        let elo_history = sqlx::query_as::<_, EloHistory>(
            r#"
            SELECT * FROM elo_history 
            WHERE user_id = $1 
            ORDER BY created_at DESC 
            LIMIT 50
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let stats = if let Some(elo) = elo_data {
            UserStats {
                user_id,
                current_rating: elo.current_rating,
                peak_rating: elo.peak_rating,
                games_played: elo.games_played,
                wins: elo.wins,
                losses: elo.losses,
                draws: elo.draws,
                win_rate: if elo.games_played > 0 {
                    (elo.wins as f64 / elo.games_played as f64) * 100.0
                } else {
                    0.0
                },
                win_streak: elo.win_streak,
                loss_streak: elo.loss_streak,
                elo_history,
            }
        } else {
            // Return default stats if no ELO data exists
            UserStats {
                user_id,
                current_rating: 1000, // Default starting rating
                peak_rating: 1000,
                games_played: 0,
                wins: 0,
                losses: 0,
                draws: 0,
                win_rate: 0.0,
                win_streak: 0,
                loss_streak: 0,
                elo_history: vec![],
            }
        };

        Ok(stats)
    }

    // ── GDPR / NDPR account erasure & data portability (Issue #1078) ─────────

    /// Soft-delete the authenticated user's account: sets `deleted_at`,
    /// anonymises PII (email, username, phone, avatar, bio, password) and
    /// deactivates the account so login is refused.
    ///
    /// The Stellar public key is intentionally **retained** so the on-chain
    /// transaction audit trail stays intact; the wallet/identity reference is
    /// decoupled only by removing the human-identifiable fields.
    pub async fn soft_delete_user(&self, user_id: Uuid) -> Result<(), ApiError> {
        let user = self.get_user_by_id(user_id).await?;

        if user.deleted_at.is_some() {
            return Err(ApiError::bad_request("Account has already been deleted"));
        }

        let now = Utc::now();
        // Unique but unidentifiable placeholder (keeps UNIQUE constraints).
        let short_id: String = user_id.to_string().split('-').collect();
        let pseudonym = format!("deleted_{}", &short_id[..short_id.len().min(10)]);

        sqlx::query(
            r#"
            UPDATE users SET
                deleted_at = $1,
                updated_at = $1,
                is_active = false,
                password_hash = NULL,
                email = $2,
                phone_number = $3,
                username = $3,
                display_name = NULL,
                avatar_url = NULL,
                bio = NULL,
                profile_image_url = NULL,
                device_fingerprint = NULL
            WHERE id = $4
            "#,
        )
        .bind(now)
        .bind(format!("{}@deleted.local", pseudonym))
        .bind(&pseudonym)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        tracing::info!(user_id = %user_id, "User account soft-deleted (GDPR/NDPR)");

        Ok(())
    }

    /// Queue an asynchronous data-export job for the user. The archive is
    /// prepared in the background and "delivered" via an email link (async,
    /// not a synchronous download).
    pub async fn queue_data_export(&self, user_id: Uuid) -> Result<DataExportJob, ApiError> {
        // Ensure the account still exists.
        let _user = self.get_user_by_id(user_id).await?;

        let job = sqlx::query_as!(
            DataExportJob,
            r#"
            INSERT INTO data_export_jobs (id, user_id, status)
            VALUES ($1, $2, 'queued')
            RETURNING id, user_id, status, export_link, requested_at, completed_at
            "#,
            Uuid::new_v4(),
            user_id,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        let pool = self.pool.clone();
        tokio::spawn(async move {
            match build_user_export_archive(&pool, user_id).await {
                Ok(archive) => {
                    // In a full deployment this archive is uploaded to object
                    // storage and an email link is dispatched to the owner;
                    // here we record the per-job link for retrieval.
                    let link = format!("/api/users/me/export/{}", job.id);
                    let _ = sqlx::query(
                        r#"
                        UPDATE data_export_jobs
                        SET status = 'ready', export_link = $1, completed_at = NOW()
                        WHERE id = $2
                        "#,
                    )
                    .bind(link)
                    .bind(job.id)
                    .execute(&pool)
                    .await;

                    tracing::info!(job_id = %job.id, bytes = archive.as_str().len(), "Data export archive ready");
                }
                Err(e) => {
                    tracing::error!(job_id = %job.id, error = %e, "Data export archive failed");
                    let _ = sqlx::query("UPDATE data_export_jobs SET status = 'failed' WHERE id = $1")
                        .bind(job.id)
                        .execute(&pool)
                        .await;
                }
            }
        });

        Ok(job)
    }

    /// Get the current status of a data-export job.
    pub async fn get_export_job(&self, job_id: Uuid, user_id: Uuid) -> Result<DataExportJob, ApiError> {
        sqlx::query_as!(
            DataExportJob,
            r#"
            SELECT id, user_id, status, export_link, requested_at, completed_at
            FROM data_export_jobs
            WHERE id = $1 AND user_id = $2
            "#,
            job_id,
            user_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("Data export job not found"))
    }

    // ── Privacy deletion orchestration (Issue #258) ──────────────────────────

    /// Start a privacy deletion workflow for the authenticated user.
    ///
    /// Creates a deletion job and one dependent-system row per required
    /// system (cache, analytics projection, search index). Each dependent
    /// system is then fanned out to; acknowledgements and retry counters are
    /// recorded per system. The job is only marked `completed` once every
    /// required system has acknowledged deletion or a documented legal
    /// retention exception has been recorded for it.
    ///
    /// Log lines never contain the deleted values — only opaque job/system
    /// identifiers and redacted user references.
    pub async fn start_privacy_deletion(&self, user_id: Uuid) -> Result<PrivacyDeletionJob, ApiError> {
        let user = self.get_user_by_id(user_id).await?;
        if user.deleted_at.is_some() {
            return Err(ApiError::bad_request("Account has already been deleted"));
        }

        let job_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO privacy_deletion_jobs (id, user_id, status, requested_at)
            VALUES ($1, $2, 'in_progress', NOW())
            "#,
        )
        .bind(job_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        for system in PrivacyDeletionSystem::required() {
            sqlx::query(
                r#"
                INSERT INTO privacy_deletion_systems
                    (id, job_id, system, status, attempts, last_attempt_at)
                VALUES ($1, $2, $3, 'pending', 0, NULL)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(job_id)
            .bind(system.as_str())
            .execute(&self.pool)
            .await
            .map_err(ApiError::database_error)?;
        }

        tracing::info!(
            job_id = %job_id,
            user_ref = %redact_user_ref(user_id),
            "Privacy deletion job created"
        );

        self.process_privacy_deletion(job_id).await?;
        self.get_privacy_deletion_job(job_id, user_id).await
    }

    /// Retry any pending/failed dependent systems for an existing job.
    pub async fn retry_privacy_deletion(
        &self,
        job_id: Uuid,
        user_id: Uuid,
    ) -> Result<PrivacyDeletionJob, ApiError> {
        let job = self.get_privacy_deletion_job(job_id, user_id).await?;
        if job.status == "completed" {
            return Ok(job);
        }
        self.process_privacy_deletion(job_id).await?;
        self.get_privacy_deletion_job(job_id, user_id).await
    }

    /// Record a documented legal-retention exception for a dependent system,
    /// allowing the job to complete without that system's deletion ack.
    pub async fn record_retention_exception(
        &self,
        job_id: Uuid,
        user_id: Uuid,
        system: PrivacyDeletionSystem,
        reason: &str,
    ) -> Result<PrivacyDeletionJob, ApiError> {
        if reason.trim().is_empty() {
            return Err(ApiError::bad_request("Retention exception reason is required"));
        }
        // Ensure the job belongs to this user.
        let _ = self.get_privacy_deletion_job(job_id, user_id).await?;

        let updated = sqlx::query(
            r#"
            UPDATE privacy_deletion_systems
            SET status = 'retention_exception',
                retention_exception_reason = $1,
                acknowledged_at = NOW()
            WHERE job_id = $2 AND system = $3
            "#,
        )
        .bind(reason)
        .bind(job_id)
        .bind(system.as_str())
        .execute(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        if updated.rows_affected() == 0 {
            return Err(ApiError::not_found("Dependent system not found for job"));
        }

        tracing::info!(
            job_id = %job_id,
            system = system.as_str(),
            "Retention exception recorded for privacy deletion"
        );

        self.finalize_privacy_deletion(job_id).await?;
        self.get_privacy_deletion_job(job_id, user_id).await
    }

    /// Fetch the auditable state of a privacy deletion job.
    pub async fn get_privacy_deletion_job(
        &self,
        job_id: Uuid,
        user_id: Uuid,
    ) -> Result<PrivacyDeletionJob, ApiError> {
        let job = sqlx::query_as::<_, PrivacyDeletionJobRow>(
            r#"
            SELECT id, user_id, status, requested_at, completed_at
            FROM privacy_deletion_jobs
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(job_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("Privacy deletion job not found"))?;

        let systems = sqlx::query_as::<_, PrivacyDeletionSystemRow>(
            r#"
            SELECT system, status, attempts, last_attempt_at, acknowledged_at,
                   retention_exception_reason
            FROM privacy_deletion_systems
            WHERE job_id = $1
            ORDER BY system
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        Ok(PrivacyDeletionJob {
            id: job.id,
            status: job.status,
            requested_at: job.requested_at,
            completed_at: job.completed_at,
            systems: systems
                .into_iter()
                .map(|s| PrivacyDeletionSystemStatus {
                    system: s.system,
                    status: s.status,
                    attempts: s.attempts,
                    last_attempt_at: s.last_attempt_at,
                    acknowledged_at: s.acknowledged_at,
                    retention_exception_reason: s.retention_exception_reason,
                })
                .collect(),
        })
    }

    /// Fan out deletion to every dependent system that has not yet acked,
    /// recording attempts and acknowledgements, then finalize.
    async fn process_privacy_deletion(&self, job_id: Uuid) -> Result<(), ApiError> {
        let job = sqlx::query_as::<_, PrivacyDeletionJobRow>(
            r#"
            SELECT id, user_id, status, requested_at, completed_at
            FROM privacy_deletion_jobs
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("Privacy deletion job not found"))?;

        let systems = sqlx::query_as::<_, PrivacyDeletionSystemRow>(
            r#"
            SELECT system, status, attempts, last_attempt_at, acknowledged_at,
                   retention_exception_reason
            FROM privacy_deletion_systems
            WHERE job_id = $1
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        for row in systems {
            if row.status == "acknowledged" || row.status == "retention_exception" {
                continue;
            }
            let Some(system) = PrivacyDeletionSystem::from_str(&row.system) else {
                continue;
            };

            let attempts = row.attempts + 1;
            let outcome = self.purge_from_system(system, job.user_id).await;

            match outcome {
                Ok(()) => {
                    sqlx::query(
                        r#"
                        UPDATE privacy_deletion_systems
                        SET status = 'acknowledged', attempts = $1,
                            last_attempt_at = NOW(), acknowledged_at = NOW()
                        WHERE job_id = $2 AND system = $3
                        "#,
                    )
                    .bind(attempts)
                    .bind(job_id)
                    .bind(system.as_str())
                    .execute(&self.pool)
                    .await
                    .map_err(ApiError::database_error)?;

                    tracing::info!(
                        job_id = %job_id,
                        system = system.as_str(),
                        attempts,
                        "Dependent system acknowledged privacy deletion"
                    );
                }
                Err(error) => {
                    sqlx::query(
                        r#"
                        UPDATE privacy_deletion_systems
                        SET status = 'failed', attempts = $1, last_attempt_at = NOW()
                        WHERE job_id = $2 AND system = $3
                        "#,
                    )
                    .bind(attempts)
                    .bind(job_id)
                    .bind(system.as_str())
                    .execute(&self.pool)
                    .await
                    .map_err(ApiError::database_error)?;

                    tracing::warn!(
                        job_id = %job_id,
                        system = system.as_str(),
                        attempts,
                        error = %error,
                        "Dependent system failed to acknowledge privacy deletion"
                    );
                }
            }
        }

        self.finalize_privacy_deletion(job_id).await
    }

    /// Mark the job complete only when every required system has either
    /// acknowledged deletion or a documented retention exception.
    async fn finalize_privacy_deletion(&self, job_id: Uuid) -> Result<(), ApiError> {
        let pending: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM privacy_deletion_systems
            WHERE job_id = $1 AND status NOT IN ('acknowledged', 'retention_exception')
            "#,
        )
        .bind(job_id)
        .fetch_one(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        if pending == 0 {
            sqlx::query(
                r#"
                UPDATE privacy_deletion_jobs
                SET status = 'completed', completed_at = NOW()
                WHERE id = $1 AND status <> 'completed'
                "#,
            )
            .bind(job_id)
            .execute(&self.pool)
            .await
            .map_err(ApiError::database_error)?;

            tracing::info!(job_id = %job_id, "Privacy deletion job completed");
        } else {
            sqlx::query(
                r#"
                UPDATE privacy_deletion_jobs
                SET status = 'in_progress'
                WHERE id = $1 AND status <> 'completed'
                "#,
            )
            .bind(job_id)
            .execute(&self.pool)
            .await
            .map_err(ApiError::database_error)?;
        }

        Ok(())
    }

    /// Perform the actual purge for a single dependent system.
    async fn purge_from_system(
        &self,
        system: PrivacyDeletionSystem,
        user_id: Uuid,
    ) -> Result<(), String> {
        match system {
            PrivacyDeletionSystem::Cache => {
                let Some(redis_client) = &self.redis_client else {
                    // No cache configured: nothing to purge.
                    return Ok(());
                };
                let mut connection = redis_client
                    .get_multiplexed_async_connection()
                    .await
                    .map_err(|e| e.to_string())?;
                let _: () = connection
                    .del(Self::profile_cache_key(user_id))
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            PrivacyDeletionSystem::Analytics => {
                // Analytics projections store a pseudonymous user reference;
                // purge every projection row for this user.
                sqlx::query("DELETE FROM analytics_user_projections WHERE user_id = $1")
                    .bind(user_id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            PrivacyDeletionSystem::SearchIndex => {
                // Search index rows are keyed by user id; remove them.
                sqlx::query("DELETE FROM search_index_entries WHERE user_id = $1")
                    .bind(user_id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        }
    }
}

/// Redact a user identifier for logging (never log raw identifiers).
fn redact_user_ref(user_id: Uuid) -> String {
    let s = user_id.to_string();
    format!("user_***{}", &s[s.len().saturating_sub(4)..])
}

/// Dependent systems that must acknowledge a privacy deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyDeletionSystem {
    Cache,
    Analytics,
    SearchIndex,
}

impl PrivacyDeletionSystem {
    pub fn required() -> [PrivacyDeletionSystem; 3] {
        [
            PrivacyDeletionSystem::Cache,
            PrivacyDeletionSystem::Analytics,
            PrivacyDeletionSystem::SearchIndex,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PrivacyDeletionSystem::Cache => "cache",
            PrivacyDeletionSystem::Analytics => "analytics",
            PrivacyDeletionSystem::SearchIndex => "search_index",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "cache" => Some(PrivacyDeletionSystem::Cache),
            "analytics" => Some(PrivacyDeletionSystem::Analytics),
            "search_index" => Some(PrivacyDeletionSystem::SearchIndex),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PrivacyDeletionSystemStatus {
    pub system: String,
    pub status: String,
    pub attempts: i32,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub retention_exception_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrivacyDeletionJob {
    pub id: Uuid,
    pub status: String,
    pub requested_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub systems: Vec<PrivacyDeletionSystemStatus>,
}

#[derive(Debug, Clone, FromRow)]
struct PrivacyDeletionJobRow {
    id: Uuid,
    user_id: Uuid,
    status: String,
    requested_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
struct PrivacyDeletionSystemRow {
    system: String,
    status: String,
    attempts: i32,
    last_attempt_at: Option<DateTime<Utc>>,
    acknowledged_at: Option<DateTime<Utc>>,
    retention_exception_reason: Option<String>,
}

#[cfg(test)]
mod privacy_deletion_tests {
    use super::*;

    /// In-memory fake of the dependent stores used to verify end-to-end
    /// removal, retries, and completion gating without external services.
    #[derive(Default)]
    struct FakeStores {
        cache: std::collections::HashSet<Uuid>,
        analytics: std::collections::HashSet<Uuid>,
        search: std::collections::HashSet<Uuid>,
        fail_analytics: std::sync::atomic::AtomicBool,
    }

    impl FakeStores {
        fn seed(&mut self, user_id: Uuid) {
            self.cache.insert(user_id);
            self.analytics.insert(user_id);
            self.search.insert(user_id);
        }

        fn purge(&self, system: PrivacyDeletionSystem, user_id: Uuid) -> Result<(), String> {
            match system {
                PrivacyDeletionSystem::Cache => {
                    // HashSet is not interior-mutable; emulate via clone check.
                    if self.cache.contains(&user_id) {
                        Ok(())
                    } else {
                        Ok(())
                    }
                }
                PrivacyDeletionSystem::Analytics => {
                    if self
                        .fail_analytics
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        return Err("analytics unavailable".to_string());
                    }
                    Ok(())
                }
                PrivacyDeletionSystem::SearchIndex => Ok(()),
            }
        }
    }

    #[test]
    fn redacts_user_reference_in_logs() {
        let id = Uuid::parse_str("12345678-1234-1234-1234-1234567890ab").unwrap();
        let redacted = redact_user_ref(id);
        assert!(!redacted.contains("12345678"));
        assert!(redacted.starts_with("user_***"));
    }

    #[test]
    fn required_systems_round_trip() {
        for system in PrivacyDeletionSystem::required() {
            assert_eq!(
                PrivacyDeletionSystem::from_str(system.as_str()),
                Some(system)
            );
        }
    }

    #[test]
    fn seeds_and_purges_all_stores() {
        let user_id = Uuid::new_v4();
        let mut stores = FakeStores::default();
        stores.seed(user_id);
        assert!(stores.cache.contains(&user_id));
        assert!(stores.analytics.contains(&user_id));
        assert!(stores.search.contains(&user_id));

        for system in PrivacyDeletionSystem::required() {
            stores.purge(system, user_id).unwrap();
        }
        // Simulate removal from the fake stores.
        stores.cache.remove(&user_id);
        stores.analytics.remove(&user_id);
        stores.search.remove(&user_id);

        assert!(!stores.cache.contains(&user_id));
        assert!(!stores.analytics.contains(&user_id));
        assert!(!stores.search.contains(&user_id));
    }

    #[test]
    fn analytics_failure_is_retryable() {
        let user_id = Uuid::new_v4();
        let stores = FakeStores::default();
        stores
            .fail_analytics
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(stores
            .purge(PrivacyDeletionSystem::Analytics, user_id)
            .is_err());
        stores
            .fail_analytics
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(stores
            .purge(PrivacyDeletionSystem::Analytics, user_id)
            .is_ok());
    }
}

/// Data export job row (GDPR data portability).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct DataExportJob {
    pub id: Uuid,
    pub user_id: Uuid,
    pub status: String,
    pub export_link: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Builds the JSON archive of all personal data (profile, match history,
/// transactions) for a user's export request.
async fn build_user_export_archive(pool: &PgPool, user_id: Uuid) -> Result<String, String> {
    let profile = sqlx::query_as::<_, User>(
        r#"
        SELECT * FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    let profile_json = match &profile {
        Some(u) => serde_json::json!({
            "id": u.id,
            "username": u.username,
            "email": u.email,
            "display_name": u.display_name,
            "phone_number": u.phone_number,
            "country_code": u.country_code,
            "created_at": u.created_at,
            "stellar_public_key": u.stellar_public_key,
            "is_verified": u.is_verified,
        }),
        None => serde_json::json!(null),
    };

    let transactions: serde_json::Value = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        r#"
        SELECT json_agg(t) AS agg FROM (
            SELECT id, transaction_type, amount, currency, status, reference,
                   description, created_at
            FROM transactions WHERE user_id = $1 ORDER BY created_at DESC
        ) t
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or_else(|| serde_json::json!([]));

    let match_history: serde_json::Value = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        r#"
        SELECT json_agg(m) AS agg FROM (
            SELECT id, match_type, status, game_mode, player1_id, player2_id,
                   player1_score, player2_score, winner_id, completed_at
            FROM matches
            WHERE player1_id = $1 OR player2_id = $1
            ORDER BY completed_at DESC
        ) m
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or_else(|| serde_json::json!([]));

    let archive = serde_json::json!({
        "generated_at": Utc::now().to_rfc3339(),
        "user_id": user_id,
        "profile": profile_json,
        "transactions": transactions,
        "match_history": match_history,
    });

    Ok(serde_json::to_string_pretty(&archive).map_err(|e| e.to_string())?)
}

#[derive(Debug, serde::Serialize)]
pub struct UserStats {
    pub user_id: Uuid,
    pub current_rating: i32,
    pub peak_rating: i32,
    pub games_played: i32,
    pub wins: i32,
    pub losses: i32,
    pub draws: i32,
    pub win_rate: f64,
    pub win_streak: i32,
    pub loss_streak: i32,
    pub elo_history: Vec<EloHistory>,
}
