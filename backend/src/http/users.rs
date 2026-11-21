use actix_web::{web, HttpRequest, HttpResponse, Result};
use redis::Client as RedisClient;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::middleware::ClaimsExt;
use crate::service::avatar::{
    AvatarError, AvatarPipeline, AvatarProcessingState, AvatarUpload,
};
use crate::service::UserService;
use crate::service::privacy_deletion::PrivacyDeletionService;

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub username: Option<String>,
    pub avatar_url: Option<String>,
    pub display_name: Option<String>,
    pub bio: Option<String>,
}

/// GET /api/users/{id}
/// Get public user profile by ID
pub async fn get_user_profile(
    pool: web::Data<sqlx::PgPool>,
    redis: web::Data<Arc<RedisClient>>,
    user_id: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let service = UserService::new(pool.get_ref().clone()).with_redis(redis.get_ref().clone());
    let profile = service.get_user_profile(*user_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": profile
    })))
}

/// GET /api/users/me
/// Get authenticated user's own profile
pub async fn get_current_user_profile(
    pool: web::Data<sqlx::PgPool>,
    redis: web::Data<Arc<RedisClient>>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let service = UserService::new(pool.get_ref().clone()).with_redis(redis.get_ref().clone());
    let user = service.get_current_user_profile(user_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": user
    })))
}

/// PUT /api/users/me
/// Update authenticated user's profile
pub async fn update_user_profile(
    pool: web::Data<sqlx::PgPool>,
    redis: web::Data<Arc<RedisClient>>,
    req: HttpRequest,
    body: web::Json<UpdateProfileRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let service = UserService::new(pool.get_ref().clone()).with_redis(redis.get_ref().clone());
    let updated_user = service
        .update_user_profile(
            user_id,
            body.username.clone(),
            body.avatar_url.clone(),
            body.display_name.clone(),
            body.bio.clone(),
        )
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": updated_user,
        "message": "Profile updated successfully"
    })))
}

/// POST /api/users/me/avatar
///
/// Accepts a multipart upload of a profile image. The pipeline enforces
/// size/type limits, strips embedded metadata (EXIF/GPS/etc.), generates
/// bounded derivatives and records a stable processing/moderation state.
///
/// On any failure the previously stored avatar is left untouched and a
/// recoverable, user-visible error is returned.
pub async fn upload_avatar(
    pool: web::Data<sqlx::PgPool>,
    redis: web::Data<Arc<RedisClient>>,
    req: HttpRequest,
    payload: web::Payload,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let upload = AvatarUpload::from_multipart(payload).await.map_err(|e| {
        // Malformed/oversized payloads are recoverable: the client can retry
        // with a valid image and the existing avatar remains in place.
        e.into_api_error()
    })?;

    let service = UserService::new(pool.get_ref().clone()).with_redis(redis.get_ref().clone());

    // Authorization: only the authenticated owner may replace their avatar.
    // `update_avatar` re-checks ownership against the persisted record.
    let pipeline = AvatarPipeline::new();
    let processed = pipeline.process(upload).await.map_err(|e| e.into_api_error())?;

    let result = service
        .update_avatar(user_id, processed)
        .await
        .map_err(|e| e)?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": {
            "avatar_url": result.avatar_url,
            "processing_state": result.processing_state,
            "moderation_state": result.moderation_state,
        },
        "message": "Avatar updated successfully"
    })))
}

/// GET /api/users/me/avatar/status
///
/// Returns the stable processing/moderation state of the caller's avatar so
/// the client can poll while a transformation or scan is in flight.
pub async fn get_avatar_status(
    pool: web::Data<sqlx::PgPool>,
    redis: web::Data<Arc<RedisClient>>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let service = UserService::new(pool.get_ref().clone()).with_redis(redis.get_ref().clone());
    let status = service.get_avatar_status(user_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": {
            "avatar_url": status.avatar_url,
            "processing_state": status.processing_state,
            "moderation_state": status.moderation_state,
            "error": status.error,
        }
    })))
}

/// GET /api/users/{id}/stats
/// Get user stats including win/loss record and Elo history
pub async fn get_user_stats(
    pool: web::Data<sqlx::PgPool>,
    redis: web::Data<Arc<RedisClient>>,
    user_id: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let service = UserService::new(pool.get_ref().clone()).with_redis(redis.get_ref().clone());
    let stats = service.get_user_stats(*user_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": stats
    })))
}

/// DELETE /api/users/me
///
/// Soft-deletes the authenticated user's account (GDPR/NDPR right to erasure):
/// PII is anonymised, `deleted_at` is set, the account is deactivated and
/// login is refused. The Stellar public key is retained for the on-chain audit
/// trail.
pub async fn delete_account(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let service = UserService::new(pool.get_ref().clone());
    service.soft_delete_user(user_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "message": "Account deletion scheduled. Your personal data has been anonymised (right to erasure)."
    })))
}

/// POST /api/users/me/delete
///
/// Initiates a tracked privacy-deletion job across every dependent system
/// (primary database, caches, analytics projections, search indexes).
/// Returns the job id so the caller can poll per-system acknowledgement and
/// retry status. Completion is only reported once all required systems
/// confirm deletion (or a documented legal-retention exception applies).
pub async fn initiate_privacy_deletion(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let service = PrivacyDeletionService::new(pool.get_ref().clone());
    let job = service.initiate_deletion(user_id).await?;

    Ok(HttpResponse::Accepted().json(serde_json::json!({
        "success": true,
        "message": "Privacy deletion initiated. Poll the job status for per-system acknowledgement.",
        "data": job
    })))
}

/// GET /api/users/me/delete/{job_id}
///
/// Returns the status of a privacy-deletion job, including per-system
/// acknowledgement and retry status. Accessible to the job owner or an admin.
/// Deleted values are never echoed back in the response.
pub async fn get_privacy_deletion_job(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
    job_id: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;
    let is_admin = req.is_admin();

    let service = PrivacyDeletionService::new(pool.get_ref().clone());
    let job = service
        .get_deletion_job(job_id.into_inner(), user_id, is_admin)
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": job
    })))
}

/// POST /api/users/me/delete/{job_id}/retry
///
/// Admin-only: retries the dependent systems that have not yet acknowledged
/// deletion for the given job. Returns the refreshed per-system status.
pub async fn retry_privacy_deletion(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
    job_id: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;
    if !req.is_admin() {
        return Err(ApiError::forbidden("Admin privileges required"));
    }

    let service = PrivacyDeletionService::new(pool.get_ref().clone());
    let job = service
        .retry_deletion(job_id.into_inner(), user_id)
        .await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "message": "Retry requested for pending systems.",
        "data": job
    })))
}

/// GET /api/users/me/export
///
/// Queues an asynchronous data-export job. The archive is prepared in the
/// background and delivered via email link when ready (GDPR/NDPR data
/// portability; not a synchronous download).
pub async fn export_user_data(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let service = UserService::new(pool.get_ref().clone());
    let job = service.queue_data_export(user_id).await?;

    Ok(HttpResponse::Accepted().json(serde_json::json!({
        "success": true,
        "message": "Data export queued. You will receive a secure download link by email once it is ready.",
        "data": job
    })))
}

/// GET /api/users/me/export/{job_id}
///
/// Returns the status (and the retrieval link, once ready) of a queued data
/// export job.
pub async fn get_export_job(
    pool: web::Data<sqlx::PgPool>,
    req: HttpRequest,
    job_id: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let service = UserService::new(pool.get_ref().clone());
    let job = service.get_export_job(job_id.into_inner(), user_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "data": job
    })))
}

/// Configure user routes.
///
/// Intended to be called via `.configure(...)` inside an existing `/api`
/// scope.  Opens a `/users` sub-scope — **not** `/api/users` — so paths
/// resolve to `/api/users/…` without a duplicate `/api` prefix.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/users")
            .route("/{id}", web::get().to(get_user_profile))
            .route("/me", web::get().to(get_current_user_profile))
            .route("/me", web::put().to(update_user_profile))
            .route("/me", web::delete().to(delete_account))
            .route("/me/delete", web::post().to(initiate_privacy_deletion))
            .route("/me/delete/{job_id}", web::get().to(get_privacy_deletion_job))
            .route(
                "/me/delete/{job_id}/retry",
                web::post().to(retry_privacy_deletion),
            )
            .route("/me/export", web::get().to(export_user_data))
            .route("/me/export/{job_id}", web::get().to(get_export_job))
            .route("/me/avatar", web::post().to(upload_avatar))
            .route("/me/avatar/status", web::get().to(get_avatar_status))
            .route("/{id}/stats", web::get().to(get_user_stats)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_update_profile_request_deserialization() {
        let json = r#"{"username":"new_username","avatar_url":"https://example.com/avatar.jpg"}"#;
        let req: UpdateProfileRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, Some("new_username".to_string()));
        assert_eq!(req.avatar_url, Some("https://example.com/avatar.jpg".to_string()));
    }

    #[test]
    fn test_update_profile_request_partial() {
        let json = r#"{"display_name":"John Doe"}"#;
        let req: UpdateProfileRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.display_name, Some("John Doe".to_string()));
        assert_eq!(req.username, None);
    }

    #[test]
    fn test_avatar_error_is_recoverable_and_user_visible() {
        // Oversized / malformed payloads must surface a recoverable error
        // (client can retry) rather than a hard failure.
        let err = AvatarError::TooLarge { max_bytes: 5 * 1024 * 1024 };
        let api = err.into_api_error();
        assert!(api.is_recoverable());
        assert!(!api.message().is_empty());
    }

    #[test]
    fn test_avatar_processing_state_is_stable() {
        // The state enum serialises to stable, client-consumable strings.
        assert_eq!(
            serde_json::to_string(&AvatarProcessingState::Processing).unwrap(),
            "\"processing\""
        );
        assert_eq!(
            serde_json::to_string(&AvatarProcessingState::Ready).unwrap(),
            "\"ready\""
        );
        assert_eq!(
            serde_json::to_string(&AvatarProcessingState::Failed).unwrap(),
            "\"failed\""
        );
    }
}
