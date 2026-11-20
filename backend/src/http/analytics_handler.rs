use crate::{
    api_error::ApiError,
    middleware::security::validate_uuid,
    service::analytics_service::{AnalyticsService, RecordMatchRequest},
};
use actix_web::{web, HttpRequest, HttpResponse};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RecordMatchBody {
    pub game_id: i32,
    pub match_id: String,
    pub duration_secs: i64,
    pub wager_amount: i64,
    pub reward_amount: i64,
    pub player_count: i32,
}

#[derive(Deserialize)]
pub struct PlayerBehaviourBody {
    pub user_id: String,
    pub game_id: i32,
    pub won: bool,
    pub session_secs: i64,
}

#[derive(Deserialize)]
pub struct PlayerInsightsQuery {
    pub game_id: i32,
}

#[derive(Deserialize)]
pub struct AggregationQuery {
    pub period_start: Option<i64>,
    pub period_end: Option<i64>,
    pub game_id: Option<i32>,
}

#[derive(Deserialize)]
pub struct ExposureBody {
    pub experiment_key: String,
    pub variant: String,
    pub assignment_version: i32,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub surface: Option<String>,
}

#[derive(Deserialize)]
pub struct ExperimentStatusQuery {
    pub analysis_window_secs: Option<i64>,
}

pub async fn record_match(
    db: web::Data<PgPool>,
    body: web::Json<RecordMatchBody>,
) -> Result<HttpResponse, ApiError> {
    let match_id = validate_uuid(&body.match_id)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let svc = AnalyticsService::new(db.get_ref().clone());
    svc.record_match(&RecordMatchRequest {
        game_id: body.game_id,
        match_id,
        duration_secs: body.duration_secs,
        wager_amount: body.wager_amount,
        reward_amount: body.reward_amount,
        player_count: body.player_count,
    }).await?;

    Ok(HttpResponse::NoContent().finish())
}

pub async fn record_player_behaviour(
    db: web::Data<PgPool>,
    body: web::Json<PlayerBehaviourBody>,
) -> Result<HttpResponse, ApiError> {
    let user_id = validate_uuid(&body.user_id)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let svc = AnalyticsService::new(db.get_ref().clone());
    svc.record_player_behaviour(user_id, body.game_id, body.won, body.session_secs).await?;
    Ok(HttpResponse::NoContent().finish())
}

pub async fn get_game_metrics(
    db: web::Data<PgPool>,
    path: web::Path<i32>,
) -> Result<HttpResponse, ApiError> {
    let svc = AnalyticsService::new(db.get_ref().clone());
    match svc.get_game_metrics(path.into_inner()).await? {
        Some(m) => Ok(HttpResponse::Ok().json(m)),
        None => Ok(HttpResponse::NotFound().json(serde_json::json!({"error": "no metrics found"}))),
    }
}

pub async fn get_platform_metrics(db: web::Data<PgPool>) -> Result<HttpResponse, ApiError> {
    let svc = AnalyticsService::new(db.get_ref().clone());
    Ok(HttpResponse::Ok().json(svc.get_platform_metrics().await?))
}

pub async fn get_player_insights(
    db: web::Data<PgPool>,
    path: web::Path<String>,
    query: web::Query<PlayerInsightsQuery>,
    // In production this comes from JWT claims; simplified here
) -> Result<HttpResponse, ApiError> {
    let user_id = validate_uuid(&path.into_inner())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let svc = AnalyticsService::new(db.get_ref().clone());
    // requesting_user_id == target for self-service; admin check omitted for brevity
    match svc.get_player_insights(user_id, user_id, false, query.game_id).await? {
        Some(i) => Ok(HttpResponse::Ok().json(i)),
        None => Ok(HttpResponse::NotFound().json(serde_json::json!({"error": "no data"}))),
    }
}

pub async fn get_aggregated_stats(
    db: web::Data<PgPool>,
    query: web::Query<AggregationQuery>,
) -> Result<HttpResponse, ApiError> {
    let svc = AnalyticsService::new(db.get_ref().clone());
    let stats = svc.get_aggregated_stats(
        query.game_id,
        query.period_start,
        query.period_end,
    ).await?;
    Ok(HttpResponse::Ok().json(stats))
}

pub async fn get_revenue_metrics(
    db: web::Data<PgPool>,
    query: web::Query<AggregationQuery>,
) -> Result<HttpResponse, ApiError> {
    let svc = AnalyticsService::new(db.get_ref().clone());
    let metrics = svc.get_revenue_metrics(
        query.game_id,
        query.period_start,
        query.period_end,
    ).await?;
    Ok(HttpResponse::Ok().json(metrics))
}

/// Logs an experiment exposure at presentation time.
///
/// The caller must be authenticated; the authenticated user id is taken from
/// the request extensions (populated by the auth middleware). A body-supplied
/// `user_id` is only honoured when it matches the authenticated principal, so
/// exposures cannot be forged on behalf of other participants.
pub async fn record_exposure(
    req: HttpRequest,
    db: web::Data<PgPool>,
    body: web::Json<ExposureBody>,
) -> Result<HttpResponse, ApiError> {
    let authenticated_user = req
        .extensions()
        .get::<crate::middleware::security::AuthenticatedUser>()
        .map(|u| u.user_id)
        .ok_or_else(|| ApiError::Unauthorized("authentication required".to_string()))?;

    if body.experiment_key.trim().is_empty() {
        return Err(ApiError::BadRequest("experiment_key is required".to_string()));
    }
    if body.variant.trim().is_empty() {
        return Err(ApiError::BadRequest("variant is required".to_string()));
    }
    if body.assignment_version < 1 {
        return Err(ApiError::BadRequest(
            "assignment_version must be >= 1".to_string(),
        ));
    }

    let user_id = match body.user_id.as_deref() {
        Some(raw) => {
            let parsed = validate_uuid(raw)
                .map_err(|e| ApiError::BadRequest(e.to_string()))?;
            if parsed != authenticated_user {
                return Err(ApiError::Forbidden(
                    "cannot log exposure for another user".to_string(),
                ));
            }
            parsed
        }
        None => authenticated_user,
    };

    let session_id = match body.session_id.as_deref() {
        Some(raw) => Some(
            validate_uuid(raw).map_err(|e| ApiError::BadRequest(e.to_string()))?,
        ),
        None => None,
    };

    let svc = AnalyticsService::new(db.get_ref().clone());
    svc.record_exposure(
        &body.experiment_key,
        &body.variant,
        body.assignment_version,
        user_id,
        session_id,
        body.surface.as_deref(),
    )
    .await?;

    Ok(HttpResponse::NoContent().finish())
}

/// Returns the guardrail dashboard status for an experiment.
///
/// Admin-only: the authenticated principal must carry the admin role.
pub async fn get_experiment_status(
    req: HttpRequest,
    db: web::Data<PgPool>,
    path: web::Path<String>,
    query: web::Query<ExperimentStatusQuery>,
) -> Result<HttpResponse, ApiError> {
    let authenticated_user = req
        .extensions()
        .get::<crate::middleware::security::AuthenticatedUser>()
        .ok_or_else(|| ApiError::Unauthorized("authentication required".to_string()))?;

    if !authenticated_user.is_admin {
        return Err(ApiError::Forbidden(
            "admin role required".to_string(),
        ));
    }

    let experiment_key = path.into_inner();
    if experiment_key.trim().is_empty() {
        return Err(ApiError::BadRequest("experiment_key is required".to_string()));
    }

    if let Some(window) = query.analysis_window_secs {
        if window <= 0 {
            return Err(ApiError::BadRequest(
                "analysis_window_secs must be positive".to_string(),
            ));
        }
    }

    let svc = AnalyticsService::new(db.get_ref().clone());
    match svc
        .get_experiment_status(&experiment_key, query.analysis_window_secs)
        .await?
    {
        Some(status) => Ok(HttpResponse::Ok().json(status)),
        None => Ok(HttpResponse::NotFound()
            .json(serde_json::json!({"error": "experiment not found"}))),
    }
}
