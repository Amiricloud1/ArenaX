use crate::api_error::ApiError;
use crate::db::DbPool;
use crate::models::idempotency::*;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx;
use uuid::Uuid;

/// Conflict strategy declared by a queued (offline) command.
///
/// The strategy determines how a replay is reconciled against newer server
/// state. It never allows a replay to silently overwrite newer state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStrategy {
    /// Reject the replay if the server already has a newer version.
    Reject,
    /// Surface the conflict to the client for manual resolution.
    SurfaceConflict,
    /// Only apply if the request hash matches the original (pure idempotent replay).
    MatchOriginal,
}

impl ConflictStrategy {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "reject" => Ok(ConflictStrategy::Reject),
            "surface_conflict" => Ok(ConflictStrategy::SurfaceConflict),
            "match_original" => Ok(ConflictStrategy::MatchOriginal),
            _ => Err(ApiError::bad_request(&format!(
                "Unknown conflict strategy: {}",
                value
            ))),
        }
    }
}

/// A user-scoped, expiring idempotency key used for offline replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedIdempotencyKey {
    pub key: String,
    pub user_id: Uuid,
    pub request_hash: String,
    pub conflict_strategy: ConflictStrategy,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
pub struct IdempotencyService {
    pub db_pool: DbPool,
    pub policy: IdempotencyPolicy,
}

impl IdempotencyService {
    pub fn new(db_pool: DbPool, policy: IdempotencyPolicy) -> Self {
        Self { db_pool, policy }
    }

    pub fn default(db_pool: DbPool) -> Self {
        Self::new(db_pool, IdempotencyPolicy::default())
    }

    /// Store a new idempotency key with response data.
    ///
    /// The key is scoped to `user_id` so that logout/account switch can purge
    /// only the affected user's commands. `expires_at` is bounded by the
    /// policy TTL; callers cannot extend it beyond the policy.
    pub async fn store_key(
        &self,
        user_id: Uuid,
        key: String,
        request_hash: String,
        response: CachedResponse,
        conflict_strategy: ConflictStrategy,
    ) -> Result<(), ApiError> {
        Self::validate_key_format(&key)?;

        // Check response size
        if response.size_kb() > self.policy.max_response_size_kb {
            return Err(ApiError::bad_request(&format!(
                "Response too large for idempotency caching (max {}KB)",
                self.policy.max_response_size_kb
            )));
        }

        let expires_at = Utc::now() + chrono::Duration::seconds(self.policy.default_ttl_seconds as i64);

        // ON CONFLICT DO NOTHING: never overwrite an existing key. If a key
        // already exists we surface a conflict rather than silently replacing
        // newer server state.
        let result = sqlx::query!(
            r#"
            INSERT INTO idempotency_keys (
                id, key, request_hash, response_status, 
                response_headers, response_body, created_at, expires_at,
                user_id, conflict_strategy
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10
            ) ON CONFLICT (key) DO NOTHING
            "#,
            Uuid::new_v4(),
            key,
            request_hash,
            response.status as i16,
            response.headers,
            response.body,
            Utc::now(),
            expires_at,
            user_id,
            conflict_strategy.to_string()
        )
        .execute(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        if result.rows_affected() == 0 {
            return Err(ApiError::conflict(
                "Idempotency key already exists; refusing to overwrite newer state",
            ));
        }

        Ok(())
    }

    /// Get cached response for an idempotency key, scoped to the user.
    ///
    /// Returns `None` for expired keys or keys belonging to another user so
    /// that a replay can never read another account's cached response.
    pub async fn get_cached_response(
        &self,
        user_id: Uuid,
        key: &str,
    ) -> Result<Option<IdempotencyKey>, ApiError> {
        let result = sqlx::query_as!(
            IdempotencyKey,
            r#"
            SELECT 
                id, key, request_hash, response_status, response_headers, response_body,
                created_at, expires_at, used_at
            FROM idempotency_keys 
            WHERE key = $1 
            AND user_id = $2
            AND expires_at > NOW()
            AND used_at IS NOT NULL
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            key,
            user_id
        )
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(result)
    }

    /// Check if there's a conflict with existing idempotency key, scoped to
    /// the user. Expired keys are treated as absent (no conflict).
    pub async fn check_conflict(
        &self,
        user_id: Uuid,
        key: &str,
        new_hash: &str,
    ) -> Result<Option<IdempotencyConflict>, ApiError> {
        let result = sqlx::query!(
            r#"
            SELECT key, request_hash, created_at 
            FROM idempotency_keys 
            WHERE key = $1 
            AND user_id = $2
            AND expires_at > NOW()
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            key,
            user_id
        )
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        if let Some(existing) = result {
            if existing.request_hash != new_hash {
                return Ok(Some(IdempotencyConflict {
                    key: key.to_string(),
                    original_hash: existing.request_hash,
                    new_hash: new_hash.to_string(),
                    original_timestamp: existing.created_at,
                    conflict_type: ConflictType::PayloadMismatch,
                }));
            }
        }

        Ok(None)
    }

    /// Reconcile a replay against newer server state.
    ///
    /// Returns `Ok(None)` when the replay is safe to apply (matching hash and
    /// not expired). Returns `Ok(Some(conflict))` when the replay must be
    /// surfaced to the client rather than silently overwriting newer state.
    pub async fn reconcile_replay(
        &self,
        user_id: Uuid,
        key: &str,
        new_hash: &str,
        strategy: ConflictStrategy,
    ) -> Result<Option<IdempotencyConflict>, ApiError> {
        let existing = sqlx::query!(
            r#"
            SELECT key, request_hash, created_at, expires_at, conflict_strategy
            FROM idempotency_keys 
            WHERE key = $1 
            AND user_id = $2
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            key,
            user_id
        )
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        let Some(existing) = existing else {
            return Ok(None);
        };

        // Expired commands must not be replayed.
        if existing.expires_at <= Utc::now() {
            return Ok(Some(IdempotencyConflict {
                key: key.to_string(),
                original_hash: existing.request_hash,
                new_hash: new_hash.to_string(),
                original_timestamp: existing.created_at,
                conflict_type: ConflictType::Expired,
            }));
        }

        if existing.request_hash == new_hash {
            return Ok(None);
        }

        // Hash mismatch: never silently overwrite. Surface the conflict
        // regardless of strategy; the strategy only affects how the client
        // should resolve it.
        let _ = strategy;
        Ok(Some(IdempotencyConflict {
            key: key.to_string(),
            original_hash: existing.request_hash,
            new_hash: new_hash.to_string(),
            original_timestamp: existing.created_at,
            conflict_type: ConflictType::PayloadMismatch,
        }))
    }

    /// Purge all queued commands for a user (logout / account switch).
    pub async fn purge_user_keys(&self, user_id: Uuid) -> Result<i64, ApiError> {
        let result = sqlx::query!(
            "DELETE FROM idempotency_keys WHERE user_id = $1",
            user_id
        )
        .execute(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(result.rows_affected() as i64)
    }

    /// Mark an idempotency key as used, scoped to the user.
    pub async fn mark_key_used(&self, user_id: Uuid, key: &str) -> Result<(), ApiError> {
        sqlx::query!(
            "UPDATE idempotency_keys SET used_at = NOW() WHERE key = $1 AND user_id = $2",
            key,
            user_id
        )
        .execute(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(())
    }

    /// Clean up expired idempotency keys
    pub async fn cleanup_expired_keys(&self) -> Result<i64, ApiError> {
        let result = sqlx::query!(
            "DELETE FROM idempotency_keys WHERE expires_at <= NOW()"
        )
        .execute(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(result.rows_affected() as i64)
    }

    /// Get statistics about idempotency key usage
    pub async fn get_usage_stats(&self) -> Result<IdempotencyStats, ApiError> {
        let total_keys = sqlx::query_scalar!(
            "SELECT COUNT(*) as count FROM idempotency_keys"
        )
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        let active_keys = sqlx::query_scalar!(
            "SELECT COUNT(*) as count FROM idempotency_keys WHERE expires_at > NOW()"
        )
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        let used_keys = sqlx::query_scalar!(
            "SELECT COUNT(*) as count FROM idempotency_keys WHERE used_at IS NOT NULL"
        )
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        let conflicts_detected = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) as count 
            FROM idempotency_keys 
            WHERE created_at > NOW() - INTERVAL '24 hours'
            GROUP BY key 
            HAVING COUNT(*) > 1
            "#
        )
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(IdempotencyStats {
            total_keys: total_keys.unwrap_or(0),
            active_keys: active_keys.unwrap_or(0),
            used_keys: used_keys.unwrap_or(0),
            conflicts_detected: conflicts_detected.unwrap_or(0),
        })
    }

    /// Get idempotency keys for a specific user (for debugging/admin)
    pub async fn get_user_keys(&self, user_id: Uuid, limit: i64, offset: i64) -> Result<Vec<IdempotencyKey>, ApiError> {
        let keys = sqlx::query_as!(
            IdempotencyKey,
            r#"
            SELECT 
                ik.id, ik.key, ik.request_hash, ik.response_status, ik.response_headers, ik.response_body,
                ik.created_at, ik.expires_at, ik.used_at
            FROM idempotency_keys ik
            JOIN request_logs rl ON ik.key = rl.idempotency_key
            WHERE rl.user_id = $1
            ORDER BY ik.created_at DESC
            LIMIT $2 OFFSET $3
            "#,
            user_id,
            limit,
            offset
        )
        .fetch_all(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(keys)
    }

    /// Invalidate an idempotency key (admin function), scoped to the user.
    pub async fn invalidate_key(&self, user_id: Uuid, key: &str) -> Result<(), ApiError> {
        sqlx::query!(
            "DELETE FROM idempotency_keys WHERE key = $1 AND user_id = $2",
            key,
            user_id
        )
        .execute(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(())
    }

    /// Update idempotency policy configuration
    pub async fn update_policy(&self, policy: IdempotencyPolicy) -> Result<(), ApiError> {
        // Update route configurations
        for route in &policy.enabled_routes {
            sqlx::query!(
                r#"
                INSERT INTO idempotency_configs (id, route_pattern, enabled, ttl_seconds, max_response_size_kb, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (route_pattern) 
                DO UPDATE SET 
                enabled = EXCLUDED.enabled,
                ttl_seconds = EXCLUDED.ttl_seconds,
                max_response_size_kb = EXCLUDED.max_response_size_kb,
                updated_at = EXCLUDED.updated_at
                "#,
                Uuid::new_v4(),
                route,
                true,
                policy.default_ttl_seconds,
                policy.max_response_size_kb as i32,
                Utc::now(),
                Utc::now()
            )
            .execute(&self.db_pool)
            .await
            .map_err(|e| ApiError::database_error(e))?;
        }

        Ok(())
    }

    /// Get route-specific configuration
    pub async fn get_route_config(&self, route_pattern: &str) -> Result<Option<IdempotencyConfig>, ApiError> {
        let config = sqlx::query_as!(
            IdempotencyConfig,
            "SELECT * FROM idempotency_configs WHERE route_pattern = $1",
            route_pattern
        )
        .fetch_optional(&self.db_pool)
        .await
        .map_err(|e| ApiError::database_error(e))?;

        Ok(config)
    }

    /// Generate a unique idempotency key (for client convenience)
    pub fn generate_key() -> String {
        format!("idemp_{}", Uuid::new_v4().to_string().replace("-", ""))
    }

    /// Build a scoped key descriptor for an offline command, enforcing the
    /// policy TTL so callers cannot extend expiry beyond the policy.
    pub fn scoped_key(
        &self,
        user_id: Uuid,
        key: String,
        request_hash: String,
        conflict_strategy: ConflictStrategy,
    ) -> Result<ScopedIdempotencyKey, ApiError> {
        Self::validate_key_format(&key)?;
        Ok(ScopedIdempotencyKey {
            key,
            user_id,
            request_hash,
            conflict_strategy,
            expires_at: Utc::now()
                + chrono::Duration::seconds(self.policy.default_ttl_seconds as i64),
        })
    }

    /// Validate idempotency key format
    pub fn validate_key_format(key: &str) -> Result<(), ApiError> {
        if key.len() < 8 {
            return Err(ApiError::bad_request("Idempotency key must be at least 8 characters"));
        }

        if key.len() > 255 {
            return Err(ApiError::bad_request("Idempotency key must be less than 255 characters"));
        }

        // Check for valid characters (alphanumeric, hyphens, underscores)
        if !key.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err(ApiError::bad_request("Idempotency key can only contain alphanumeric characters, hyphens, and underscores"));
        }

        Ok(())
    }
}

impl ConflictStrategy {
    pub fn to_string(&self) -> String {
        match self {
            ConflictStrategy::Reject => "reject".to_string(),
            ConflictStrategy::SurfaceConflict => "surface_conflict".to_string(),
            ConflictStrategy::MatchOriginal => "match_original".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyStats {
    pub total_keys: i64,
    pub active_keys: i64,
    pub used_keys: i64,
    pub conflicts_detected: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyKeyRequest {
    pub route_pattern: Option<String>,
    pub ttl_seconds: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyKeyResponse {
    pub key: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub ttl_seconds: i32,
}

impl IdempotencyKeyResponse {
    pub fn new(key: String, ttl_seconds: i32) -> Self {
        Self {
            key,
            expires_at: Utc::now() + chrono::Duration::seconds(ttl_seconds as i64),
            ttl_seconds,
        }
    }
}
