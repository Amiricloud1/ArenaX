use crate::api_error::ApiError;
use crate::auth::Claims;
use crate::db::DbPool;
use crate::models::idempotency::*;
use crate::service::idempotency_service::{IdempotencyService, IdempotencyKeyResponse, IdempotencyKeyRequest};
use crate::service::operator_session_service::OperatorSessionService;
use actix_web::{web, HttpResponse, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Generate a new idempotency key for client use
pub async fn generate_key(
    db_pool: web::Data<DbPool>,
    query: web::Query<IdempotencyKeyRequest>,
) -> Result<HttpResponse> {
    let idempotency_service = IdempotencyService::default(db_pool.as_ref().clone());
    
    let ttl = query.ttl_seconds.unwrap_or(86400); // Default 24 hours
    let key = IdempotencyService::generate_key();
    
    let response = IdempotencyKeyResponse::new(key.clone(), ttl);
    
    // Store the key with placeholder data (will be updated when actually used)
    let placeholder_response = CachedResponse::new(
        "placeholder".to_string(),
        200,
        serde_json::json!({}),
        serde_json::json!({}),
    );
    
    if let Err(e) = idempotency_service.store_key(
        key.clone(),
        "placeholder".to_string(),
        placeholder_response,
    ).await {
        // Log error but still return the key to the client
        tracing::error!("Failed to pre-store idempotency key: {:?}", e);
    }
    
    Ok(HttpResponse::Ok().json(response))
}

/// Get idempotency statistics (admin only)
pub async fn get_stats(
    db_pool: web::Data<DbPool>,
    claims: web::ReqData<Claims>,
) -> Result<HttpResponse> {
    // Check if user is admin (implement based on your auth system)
    if !is_admin(&claims) {
        return Err(ApiError::forbidden("Admin access required").into());
    }
    
    let idempotency_service = IdempotencyService::default(db_pool.as_ref().clone());
    let stats = idempotency_service.get_usage_stats().await?;
    
    Ok(HttpResponse::Ok().json(stats))
}

/// Get user's idempotency keys (for debugging)
pub async fn get_user_keys(
    db_pool: web::Data<DbPool>,
    claims: web::ReqData<Claims>,
    query: web::Query<PaginationQuery>,
) -> Result<HttpResponse> {
    let idempotency_service = IdempotencyService::default(db_pool.as_ref().clone());
    let limit = query.limit.unwrap_or(50).min(100); // Max 100
    let offset = query.offset.unwrap_or(0);
    
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::unauthorized("Invalid user ID in token"))?;
    let keys = idempotency_service
        .get_user_keys(user_id, limit, offset)
        .await?;
    
    Ok(HttpResponse::Ok().json(keys))
}

/// Invalidate an idempotency key (admin only)
pub async fn invalidate_key(
    db_pool: web::Data<DbPool>,
    claims: web::ReqData<Claims>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    if !is_admin(&claims) {
        return Err(ApiError::forbidden("Admin access required").into());
    }
    
    let key = path.into_inner();
    let idempotency_service = IdempotencyService::default(db_pool.as_ref().clone());
    
    idempotency_service.invalidate_key(&key).await?;
    
    Ok(HttpResponse::NoContent().finish())
}

/// Clean up expired keys (admin only)
pub async fn cleanup_expired(
    db_pool: web::Data<DbPool>,
    claims: web::ReqData<Claims>,
) -> Result<HttpResponse> {
    if !is_admin(&claims) {
        return Err(ApiError::forbidden("Admin access required").into());
    }
    
    let idempotency_service = IdempotencyService::default(db_pool.as_ref().clone());
    let cleaned_count = idempotency_service.cleanup_expired_keys().await?;
    
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "cleaned_keys": cleaned_count,
        "message": "Expired keys cleaned up successfully"
    })))
}

/// Update idempotency configuration (admin only)
pub async fn update_config(
    db_pool: web::Data<DbPool>,
    claims: web::ReqData<Claims>,
    config: web::Json<IdempotencyPolicy>,
) -> Result<HttpResponse> {
    if !is_admin(&claims) {
        return Err(ApiError::forbidden("Admin access required").into());
    }
    
    let idempotency_service = IdempotencyService::default(db_pool.as_ref().clone());
    idempotency_service.update_policy(config.into_inner()).await?;
    
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "message": "Idempotency configuration updated successfully"
    })))
}

/// Get route-specific configuration
pub async fn get_route_config(
    db_pool: web::Data<DbPool>,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let route_pattern = path.into_inner();
    let idempotency_service = IdempotencyService::default(db_pool.as_ref().clone());
    
    let config = idempotency_service.get_route_config(&route_pattern).await?;
    
    match config {
        Some(config) => Ok(HttpResponse::Ok().json(config)),
        None => Err(ApiError::not_found("Route configuration not found").into()),
    }
}

/// Validate idempotency key format
pub async fn validate_key(
    body: web::Json<ValidateKeyRequest>,
) -> Result<HttpResponse> {
    let key = &body.key;
    
    match IdempotencyService::validate_key_format(key) {
        Ok(()) => Ok(HttpResponse::Ok().json(serde_json::json!({
            "valid": true,
            "message": "Idempotency key format is valid"
        }))),
        Err(e) => Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "valid": false,
            "error": e.to_string()
        }))),
    }
}

/// Helper function to check if user is admin
fn is_admin(claims: &Claims) -> bool {
    claims.roles.contains(&"admin".to_string())
}

#[derive(Debug, Deserialize)]
pub struct PaginationQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ValidateKeyRequest {
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct IdempotencyInfo {
    pub enabled: bool,
    pub key_header: String,
    pub default_ttl: i32,
    pub max_response_size_kb: i32,
    pub enabled_routes: Vec<String>,
}

/// Get idempotency framework information
pub async fn get_framework_info() -> Result<HttpResponse> {
    let policy = IdempotencyPolicy::default();
    
    let info = IdempotencyInfo {
        enabled: true,
        key_header: policy.key_header_name,
        default_ttl: policy.default_ttl_seconds,
        max_response_size_kb: policy.max_response_size_kb as i32,
        enabled_routes: policy.enabled_routes,
    };
    
    Ok(HttpResponse::Ok().json(info))
}

/// Test endpoint for idempotency (for testing purposes)
pub async fn test_idempotency(
    _db_pool: web::Data<DbPool>,
    body: web::Json<serde_json::Value>,
) -> Result<HttpResponse> {
    let response_data = serde_json::json!({
        "message": "Test endpoint",
        "received_data": body,
        "timestamp": chrono::Utc::now(),
        "request_id": Uuid::new_v4()
    });
    
    Ok(HttpResponse::Ok().json(response_data))
}

/// Outcome of a queue-drain submission attempt for a queued privileged action.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum DrainOutcome {
    /// The action was submitted exactly once.
    Submitted,
    /// The action was rejected because the operator session was invalid at submission time.
    Rejected { reason: String },
    /// The action was a duplicate delivery of an already-submitted action.
    Duplicate,
}

/// A queued privileged action awaiting submission.
#[derive(Debug, Clone)]
pub struct QueuedOperatorAction {
    pub idempotency_key: String,
    pub session_id: Uuid,
    pub actor: String,
    pub purpose: String,
    pub scope: String,
    pub approval_reference: String,
}

/// Re-check operator session validity immediately before submission.
///
/// This is the last line of defense: even if a session was valid when the action
/// was queued, a revocation or expiry that occurred while the action sat in the
/// queue must prevent submission. Signing keys are never read here; the session
/// store only holds session metadata.
pub async fn check_session_before_submission(
    session_service: &OperatorSessionService,
    action: &QueuedOperatorAction,
    required_scope: &str,
) -> std::result::Result<(), String> {
    let session = session_service
        .get_session(action.session_id)
        .await
        .map_err(|e| format!("session lookup failed: {e}"))?
        .ok_or_else(|| "operator session not found".to_string())?;

    if session.revoked {
        return Err("operator session revoked".to_string());
    }
    if session.is_expired(chrono::Utc::now()) {
        return Err("operator session expired".to_string());
    }
    if session.actor != action.actor {
        return Err("operator session actor mismatch".to_string());
    }
    if session.purpose != action.purpose {
        return Err("operator session purpose mismatch".to_string());
    }
    if session.scope != required_scope || action.scope != required_scope {
        return Err("operator session scope mismatch".to_string());
    }
    if session.approval_reference != action.approval_reference {
        return Err("operator session approval reference mismatch".to_string());
    }
    Ok(())
}

/// Drain a single queued privileged action, re-checking the operator session
/// immediately before submission and recording duplicate-delivery outcomes.
///
/// The idempotency store is consulted first so that a duplicate delivery of an
/// already-submitted action is recorded as `Duplicate` rather than re-submitted.
pub async fn drain_queued_action(
    idempotency_service: &IdempotencyService,
    session_service: &OperatorSessionService,
    action: &QueuedOperatorAction,
    required_scope: &str,
) -> DrainOutcome {
    // Duplicate-delivery check: if the key already has a non-placeholder cached
    // response, this is a duplicate delivery of an already-submitted action.
    match idempotency_service.get_cached_response(&action.idempotency_key).await {
        Ok(Some(cached)) if cached.status_code != 200 || cached.response_body != serde_json::json!({}) => {
            tracing::info!(
                idempotency_key = %action.idempotency_key,
                session_id = %action.session_id,
                "duplicate delivery of already-submitted operator action"
            );
            return DrainOutcome::Duplicate;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(
                idempotency_key = %action.idempotency_key,
                error = ?e,
                "failed to check idempotency store before submission"
            );
            return DrainOutcome::Rejected {
                reason: format!("idempotency lookup failed: {e}"),
            };
        }
    }

    // Re-check session validity immediately before submission.
    if let Err(reason) = check_session_before_submission(session_service, action, required_scope).await {
        tracing::warn!(
            idempotency_key = %action.idempotency_key,
            session_id = %action.session_id,
            actor = %action.actor,
            reason = %reason,
            "rejected queued operator action: session invalid at submission time"
        );
        return DrainOutcome::Rejected { reason };
    }

    // Record the submission outcome so subsequent duplicate deliveries are detected.
    let cached = CachedResponse::new(
        action.idempotency_key.clone(),
        200,
        serde_json::json!({ "submitted": true }),
        serde_json::json!({}),
    );
    if let Err(e) = idempotency_service
        .store_key(action.idempotency_key.clone(), action.actor.clone(), cached)
        .await
    {
        tracing::error!(
            idempotency_key = %action.idempotency_key,
            error = ?e,
            "failed to record submission outcome"
        );
        return DrainOutcome::Rejected {
            reason: format!("failed to record submission outcome: {e}"),
        };
    }

    DrainOutcome::Submitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::operator_session_service::OperatorSession;

    fn action(session_id: Uuid, scope: &str) -> QueuedOperatorAction {
        QueuedOperatorAction {
            idempotency_key: "op-key-1".to_string(),
            session_id,
            actor: "operator-1".to_string(),
            purpose: "settlement".to_string(),
            scope: scope.to_string(),
            approval_reference: "approval-42".to_string(),
        }
    }

    fn session(session_id: Uuid, scope: &str) -> OperatorSession {
        OperatorSession {
            id: session_id,
            actor: "operator-1".to_string(),
            purpose: "settlement".to_string(),
            scope: scope.to_string(),
            approval_reference: "approval-42".to_string(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
            revoked: false,
        }
    }

    #[tokio::test]
    async fn expiry_during_queueing_blocks_submission() {
        let session_id = Uuid::new_v4();
        let store = OperatorSessionService::in_memory();
        let mut s = session(session_id, "settlement:submit");
        s.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        store.insert_session(s).await.unwrap();

        let idem = IdempotencyService::in_memory();
        let outcome = drain_queued_action(&idem, &store, &action(session_id, "settlement:submit"), "settlement:submit").await;
        assert!(matches!(outcome, DrainOutcome::Rejected { .. }));
    }

    #[tokio::test]
    async fn revocation_race_blocks_submission() {
        let session_id = Uuid::new_v4();
        let store = OperatorSessionService::in_memory();
        store.insert_session(session(session_id, "settlement:submit")).await.unwrap();

        // Revocation happens after queueing but before drain.
        store.revoke_session(session_id).await.unwrap();

        let idem = IdempotencyService::in_memory();
        let outcome = drain_queued_action(&idem, &store, &action(session_id, "settlement:submit"), "settlement:submit").await;
        assert!(matches!(outcome, DrainOutcome::Rejected { .. }));
    }

    #[tokio::test]
    async fn wrong_scope_action_is_rejected() {
        let session_id = Uuid::new_v4();
        let store = OperatorSessionService::in_memory();
        store.insert_session(session(session_id, "settlement:submit")).await.unwrap();

        let idem = IdempotencyService::in_memory();
        let outcome = drain_queued_action(&idem, &store, &action(session_id, "settlement:submit"), "settlement:approve").await;
        assert!(matches!(outcome, DrainOutcome::Rejected { .. }));
    }

    #[tokio::test]
    async fn duplicate_delivery_is_recorded() {
        let session_id = Uuid::new_v4();
        let store = OperatorSessionService::in_memory();
        store.insert_session(session(session_id, "settlement:submit")).await.unwrap();

        let idem = IdempotencyService::in_memory();
        let first = drain_queued_action(&idem, &store, &action(session_id, "settlement:submit"), "settlement:submit").await;
        assert_eq!(first, DrainOutcome::Submitted);

        let second = drain_queued_action(&idem, &store, &action(session_id, "settlement:submit"), "settlement:submit").await;
        assert_eq!(second, DrainOutcome::Duplicate);
    }

    #[tokio::test]
    async fn retry_after_rejection_succeeds_once_session_valid() {
        let session_id = Uuid::new_v4();
        let store = OperatorSessionService::in_memory();
        let mut s = session(session_id, "settlement:submit");
        s.revoked = true;
        store.insert_session(s).await.unwrap();

        let idem = IdempotencyService::in_memory();
        let rejected = drain_queued_action(&idem, &store, &action(session_id, "settlement:submit"), "settlement:submit").await;
        assert!(matches!(rejected, DrainOutcome::Rejected { .. }));

        // A fresh, valid session allows the retry to succeed.
        let new_session_id = Uuid::new_v4();
        store.insert_session(session(new_session_id, "settlement:submit")).await.unwrap();
        let retried = drain_queued_action(&idem, &store, &action(new_session_id, "settlement:submit"), "settlement:submit").await;
        assert_eq!(retried, DrainOutcome::Submitted);
    }
}
