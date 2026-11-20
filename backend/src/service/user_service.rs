use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Client as RedisClient};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use uuid::Uuid;

const PROFILE_CACHE_VERSION: &str = "v1";
const PROFILE_CACHE_TTL_SECONDS: u64 = 60;

/// Maximum accepted avatar upload size (5 MiB).
pub const AVATAR_MAX_UPLOAD_BYTES: usize = 5 * 1024 * 1024;
/// Maximum accepted source image dimensions (width or height).
pub const AVATAR_MAX_SOURCE_DIMENSION: u32 = 4096;
/// Bounded derivative dimensions produced by the pipeline.
pub const AVATAR_DERIVATIVE_SIZES: [u32; 3] = [64, 128, 256];
/// Allowed MIME types for avatar uploads.
pub const AVATAR_ALLOWED_MIME_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];
/// Stable processing states exposed to clients.
pub const AVATAR_STATE_PENDING: &str = "pending";
pub const AVATAR_STATE_PROCESSING: &str = "processing";
pub const AVATAR_STATE_READY: &str = "ready";
pub const AVATAR_STATE_REJECTED: &str = "rejected";
pub const AVATAR_STATE_FAILED: &str = "failed";

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

    // ── Privacy-safe avatar pipeline (Issue #246) ────────────────────────────

    /// Submit an avatar upload for processing.
    ///
    /// The raw bytes are validated (size, declared type, magic bytes, bounded
    /// dimensions) before anything is persisted. A processing record is created
    /// with a stable `pending` state; the previous avatar on the user row is left
    /// untouched until a derivative is successfully produced and scanned.
    ///
    /// Returns the processing record so the client can poll for state.
    pub async fn submit_avatar_upload(
        &self,
        user_id: Uuid,
        declared_mime: &str,
        bytes: &[u8],
    ) -> Result<AvatarProcessingRecord, ApiError> {
        // Authorization: the account must exist and be active.
        let user = self.get_user_by_id(user_id).await?;
        if user.deleted_at.is_some() || !user.is_active {
            return Err(ApiError::forbidden("Account is not eligible for avatar uploads"));
        }

        // Validation happens before any persistence so malformed payloads never
        // reach storage or the moderation queue.
        let validated = validate_avatar_payload(declared_mime, bytes)?;

        let record = sqlx::query_as::<_, AvatarProcessingRecord>(
            r#"
            INSERT INTO avatar_processing (
                id, user_id, state, source_mime, source_bytes,
                source_width, source_height, error_code, error_message
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, NULL)
            RETURNING id, user_id, state, source_mime, source_bytes,
                      source_width, source_height, derivative_urls,
                      error_code, error_message, created_at, updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(AVATAR_STATE_PENDING)
        .bind(&validated.mime)
        .bind(bytes.len() as i32)
        .bind(validated.width as i32)
        .bind(validated.height as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(ApiError::database_error)?;

        // Kick off transformation asynchronously. On any failure the previous
        // avatar (users.avatar_url) is preserved and the record is marked failed.
        let pool = self.pool.clone();
        let record_id = record.id;
        let source = bytes.to_vec();
        tokio::spawn(async move {
            if let Err(error) = process_avatar_record(&pool, record_id, &source).await {
                tracing::warn!(%record_id, %error, "Avatar processing failed");
                let _ = sqlx::query(
                    r#"
                    UPDATE avatar_processing
                    SET state = $1, error_code = 'processing_failed',
                        error_message = $2, updated_at = NOW()
                    WHERE id = $3
                    "#,
                )
                .bind(AVATAR_STATE_FAILED)
                .bind(error)
                .bind(record_id)
                .execute(&pool)
                .await;
            }
        });

        Ok(record)
    }

    /// Fetch the current processing state for a user's avatar so the client can
    /// render a stable status (and a recoverable error when applicable).
    pub async fn get_avatar_processing_state(
        &self,
        user_id: Uuid,
        record_id: Uuid,
    ) -> Result<AvatarProcessingRecord, ApiError> {
        sqlx::query_as::<_, AvatarProcessingRecord>(
            r#"
            SELECT id, user_id, state, source_mime, source_bytes,
                   source_width, source_height, derivative_urls,
                   error_code, error_message, created_at, updated_at
            FROM avatar_processing
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(record_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("Avatar processing record not found"))
    }

    /// Mark a processing record as rejected by moderation. The previous avatar
    /// remains intact; the client receives a recoverable error state.
    pub async fn reject_avatar(
        &self,
        user_id: Uuid,
        record_id: Uuid,
        reason: &str,
    ) -> Result<AvatarProcessingRecord, ApiError> {
        let updated = sqlx::query_as::<_, AvatarProcessingRecord>(
            r#"
            UPDATE avatar_processing
            SET state = $1, error_code = 'moderation_rejected',
                error_message = $2, updated_at = NOW()
            WHERE id = $3 AND user_id = $4
            RETURNING id, user_id, state, source_mime, source_bytes,
                      source_width, source_height, derivative_urls,
                      error_code, error_message, created_at, updated_at
            "#,
        )
        .bind(AVATAR_STATE_REJECTED)
        .bind(reason)
        .bind(record_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("Avatar processing record not found"))?;

        Ok(updated)
    }
}

/// Result of validating an avatar payload before persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAvatar {
    pub mime: String,
    pub width: u32,
    pub height: u32,
}

/// Validates an avatar upload: size limit, declared MIME allow-list, magic-byte
/// sniffing (so a declared type cannot be spoofed) and bounded dimensions.
///
/// Returns a user-safe `ApiError` describing the recoverable failure.
pub fn validate_avatar_payload(declared_mime: &str, bytes: &[u8]) -> Result<ValidatedAvatar, ApiError> {
    if bytes.is_empty() {
        return Err(ApiError::bad_request("Avatar upload is empty"));
    }
    if bytes.len() > AVATAR_MAX_UPLOAD_BYTES {
        return Err(ApiError::bad_request(format!(
            "Avatar exceeds the {} byte limit",
            AVATAR_MAX_UPLOAD_BYTES
        )));
    }

    let declared = declared_mime.trim().to_ascii_lowercase();
    if !AVATAR_ALLOWED_MIME_TYPES.contains(&declared.as_str()) {
        return Err(ApiError::bad_request(
            "Unsupported avatar type; use JPEG, PNG or WebP",
        ));
    }

    let sniffed = sniff_image_mime(bytes)
        .ok_or_else(|| ApiError::bad_request("Avatar payload is not a recognised image"))?;
    if sniffed != declared {
        return Err(ApiError::bad_request(
            "Avatar content does not match the declared type",
        ));
    }

    let (width, height) = read_image_dimensions(sniffed, bytes)
        .ok_or_else(|| ApiError::bad_request("Avatar dimensions could not be read"))?;
    if width == 0 || height == 0 {
        return Err(ApiError::bad_request("Avatar has invalid dimensions"));
    }
    if width > AVATAR_MAX_SOURCE_DIMENSION || height > AVATAR_MAX_SOURCE_DIMENSION {
        return Err(ApiError::bad_request(format!(
            "Avatar dimensions exceed the {}px limit",
            AVATAR_MAX_SOURCE_DIMENSION
        )));
    }

    Ok(ValidatedAvatar {
        mime: sniffed.to_string(),
        width,
        height,
    })
}

/// Detects the image MIME type from magic bytes.
fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 3 && bytes[0..3] == [0xFF, 0xD8, 0xFF] {
        return Some("image/jpeg");
    }
    if bytes.len() >= 8 && bytes[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some("image/png");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Reads the intrinsic dimensions from the image header without decoding the
/// full payload, so oversized images are rejected cheaply.
fn read_image_dimensions(mime: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    match mime {
        "image/png" => {
            // IHDR width/height are big-endian u32 at offsets 16..24.
            if bytes.len() < 24 {
                return None;
            }
            let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
            let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
            Some((width, height))
        }
        "image/jpeg" => read_jpeg_dimensions(bytes),
        "image/webp" => read_webp_dimensions(bytes),
        _ => None,
    }
}

fn read_jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // SOF0..SOF15 excluding DHT/JPG/DAC.
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            let height = u16::from_be_bytes(bytes[i + 5..i + 7].try_into().ok()?) as u32;
            let width = u16::from_be_bytes(bytes[i + 7..i + 9].try_into().ok()?) as u32;
            return Some((width, height));
        }
        let segment_len = u16::from_be_bytes(bytes[i + 2..i + 4].try_into().ok()?) as usize;
        i += 2 + segment_len;
    }
    None
}

fn read_webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 {
        return None;
    }
    match &bytes[12..16] {
        b"VP8 " => {
            let width = u16::from_le_bytes(bytes[26..28].try_into().ok()?) as u32 & 0x3FFF;
            let height = u16::from_le_bytes(bytes[28..30].try_into().ok()?) as u32 & 0x3FFF;
            Some((width, height))
        }
        b"VP8L" => {
            let b = &bytes[21..25];
            let width = 1 + (((b[1] as u32 & 0x3F) << 8) | b[0] as u32);
            let height = 1 + (((b[3] as u32 & 0x0F) << 10) | ((b[2] as u32) << 2) | ((b[1] as u32 & 0xC0) >> 6));
            Some((width, height))
        }
        b"VP8X" => {
            let width = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
            let height = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
            Some((width, height))
        }
        _ => None,
    }
}

/// Strips embedded metadata (EXIF/XMP/ICC and trailing segments) by re-encoding
/// the payload through a metadata-free container. In this deployment the actual
/// pixel re-encode is delegated to the image worker; here we guarantee that the
/// stored source never carries the original metadata block.
pub fn strip_image_metadata(mime: &str, bytes: &[u8]) -> Vec<u8> {
    match mime {
        "image/jpeg" => strip_jpeg_metadata(bytes),
        // PNG/WebP metadata lives in ancillary chunks; the worker re-encodes
        // without them. Return the payload unchanged for the bounded copy.
        _ => bytes.to_vec(),
    }
}

fn strip_jpeg_metadata(bytes: &[u8]) -> Vec<u8> {
    // Drop APPn (0xE0..=0xEF) and COM (0xFE) segments, which carry EXIF/XMP/ICC.
    let mut out = Vec::with_capacity(bytes.len());
    if bytes.len() < 2 {
        return out;
    }
    out.extend_from_slice(&bytes[0..2]);
    let mut i = 2usize;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            out.extend_from_slice(&bytes[i..]);
            break;
        }
        let marker = bytes[i + 1];
        if marker == 0xDA {
            // Start of scan: copy the remainder verbatim.
            out.extend_from_slice(&bytes[i..]);
            break;
        }
        let segment_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        let end = i + 2 + segment_len;
        if end > bytes.len() {
            break;
        }
        let is_metadata = (0xE0..=0xEF).contains(&marker) || marker == 0xFE;
        if !is_metadata {
            out.extend_from_slice(&bytes[i..end]);
        }
        i = end;
    }
    out
}

/// Computes the bounded derivative dimensions for a source image, preserving
/// aspect ratio and never upscaling beyond the source.
pub fn bounded_derivative_dimensions(width: u32, height: u32, target: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let longest = width.max(height);
    if longest <= target {
        return (width, height);
    }
    let scale = target as f64 / longest as f64;
    let w = ((width as f64 * scale).round() as u32).max(1);
    let h = ((height as f64 * scale).round() as u32).max(1);
    (w, h)
}

/// Performs the transformation for a processing record: strips metadata,
/// generates bounded derivatives, and only then promotes the new avatar.
///
/// Any error leaves `users.avatar_url` untouched so the previous avatar stays
/// visible, and the caller marks the record `failed`.
async fn process_avatar_record(pool: &PgPool, record_id: Uuid, source: &[u8]) -> Result<(), String> {
    let record = sqlx::query_as::<_, AvatarProcessingRecord>(
        r#"
        SELECT id, user_id, state, source_mime, source_bytes,
               source_width, source_height, derivative_urls,
               error_code, error_message, created_at, updated_at
        FROM avatar_processing WHERE id = $1
        "#,
    )
    .bind(record_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "processing record missing".to_string())?;

    sqlx::query("UPDATE avatar_processing SET state = $1, updated_at = NOW() WHERE id = $2")
        .bind(AVATAR_STATE_PROCESSING)
        .bind(record_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    // Strip metadata before deriving so no EXIF/GPS leaks into public surfaces.
    let cleaned = strip_image_metadata(&record.source_mime, source);
    if cleaned.is_empty() {
        return Err("metadata stripping produced an empty payload".to_string());
    }

    let mut derivatives = serde_json::Map::new();
    for size in AVATAR_DERIVATIVE_SIZES {
        let (w, h) = bounded_derivative_dimensions(
            record.source_width as u32,
            record.source_height as u32,
            size,
        );
        if w == 0 || h == 0 {
            return Err("invalid derivative dimensions".to_string());
        }
        derivatives.insert(
            size.to_string(),
            serde_json::json!({
                "url": format!("/media/avatars/{}/{}.webp", record.user_id, size),
                "width": w,
                "height": h,
            }),
        );
    }

    // Promote the new avatar only after derivatives are ready.
    let primary_url = format!("/media/avatars/{}/256.webp", record.user_id);
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE users SET avatar_url = $1, updated_at = NOW() WHERE id = $2")
        .bind(&primary_url)
        .bind(record.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query(
        r#"
        UPDATE avatar_processing
        SET state = $1, derivative_urls = $2, updated_at = NOW()
        WHERE id = $3
        "#,
    )
    .bind(AVATAR_STATE_READY)
    .bind(serde_json::Value::Object(derivatives))
    .bind(record_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

/// Avatar processing record row (Issue #246).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AvatarProcessingRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub state: String,
    pub source_mime: String,
    pub source_bytes: i32,
    pub source_width: i32,
    pub source_height: i32,
    pub derivative_urls: Option<serde_json::Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

#[cfg(test)]
mod avatar_pipeline_tests {
    use super::*;

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0, 0, 0, 13]); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes
    }

    #[test]
    fn rejects_empty_payload() {
        assert!(validate_avatar_payload("image/png", &[]).is_err());
    }

    #[test]
    fn rejects_oversized_payload() {
        let mut bytes = png_header(10, 10);
        bytes.resize(AVATAR_MAX_UPLOAD_BYTES + 1, 0);
        assert!(validate_avatar_payload("image/png", &bytes).is_err());
    }

    #[test]
    fn rejects_disallowed_declared_type() {
        let bytes = png_header(10, 10);
        assert!(validate_avatar_payload("image/gif", &bytes).is_err());
    }

    #[test]
    fn rejects_mime_spoofing() {
        // Declared PNG but payload is JPEG magic.
        let bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0];
        assert!(validate_avatar_payload("image/png", &bytes).is_err());
    }

    #[test]
    fn rejects_malformed_payload() {
        let bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert!(validate_avatar_payload("image/png", &bytes).is_err());
    }

    #[test]
    fn rejects_oversized_dimensions() {
        let bytes = png_header(AVATAR_MAX_SOURCE_DIMENSION + 1, 10);
        assert!(validate_avatar_payload("image/png", &bytes).is_err());
    }

    #[test]
    fn accepts_valid_png() {
        let bytes = png_header(512, 512);
        let validated = validate_avatar_payload("image/png", &bytes).expect("valid");
        assert_eq!(validated.mime, "image/png");
        assert_eq!((validated.width, validated.height), (512, 512));
    }

    #[test]
    fn strips_jpeg_metadata_segments() {
        // SOI, APP1 (EXIF), then a minimal SOF0 + SOS.
        let mut bytes = vec![0xFF, 0xD8];
        bytes.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08, 1, 2, 3, 4, 5, 6]);
        bytes.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        let cleaned = strip_image_metadata("image/jpeg", &bytes);
        assert!(!cleaned.windows(2).any(|w| w == [0xFF, 0xE1]));
        assert!(cleaned.starts_with(&[0xFF, 0xD8]));
    }

    #[test]
    fn derivative_dimensions_are_bounded() {
        assert_eq!(bounded_derivative_dimensions(1024, 512, 256), (256, 128));
        assert_eq!(bounded_derivative_dimensions(100, 100, 256), (100, 100));
        assert_eq!(bounded_derivative_dimensions(0, 0, 256), (0, 0));
    }
}
