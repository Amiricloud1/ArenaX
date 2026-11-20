use crate::{
    api_error::ApiError,
    middleware::security::validate_uuid,
    service::analytics_service::{AnalyticsService, RecordMatchRequest},
};
use actix_web::{web, HttpResponse};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Current consent policy version. Bump when the consent text/scope changes so
/// that stale consent records are treated as not-granted.
pub const ANALYTICS_CONSENT_VERSION: i32 = 1;

/// Versioned consent record supplied by the client (or resolved from the
/// authenticated subject). `granted == false` means the user has withdrawn
/// consent; any record whose `version` is stale is treated as not granted.
#[derive(Deserialize, Clone, Debug)]
pub struct ConsentRecord {
    pub version: i32,
    pub granted: bool,
}

impl ConsentRecord {
    /// Consent is only effective when it is explicitly granted *and* matches
    /// the current policy version.
    pub fn is_effective(&self) -> bool {
        self.granted && self.version == ANALYTICS_CONSENT_VERSION
    }
}

#[derive(Deserialize)]
pub struct RecordMatchBody {
    pub game_id: i32,
    pub match_id: String,
    pub duration_secs: i64,
    pub wager_amount: i64,
    pub reward_amount: i64,
    pub player_count: i32,
    /// Optional consent record. When absent the event is treated as
    /// non-consented and suppressed.
    pub consent: Option<ConsentRecord>,
}

#[derive(Deserialize)]
pub struct PlayerBehaviourBody {
    pub user_id: String,
    pub game_id: i32,
    pub won: bool,
    pub session_secs: i64,
    /// Optional consent record. When absent the event is treated as
    /// non-consented and suppressed.
    pub consent: Option<ConsentRecord>,
}

/// Resolve whether non-essential analytics may be recorded for this request.
///
/// Consent is authoritative and versioned: a missing record, a withdrawn
/// record, or a stale version all result in suppression. This is the single
/// gate used by every producer path so withdrawal halts future events.
fn consent_allows_analytics(consent: &Option<ConsentRecord>) -> bool {
    consent.as_ref().map(ConsentRecord::is_effective).unwrap_or(false)
}

/// Response returned when a non-essential analytics write is suppressed
/// because consent is absent, withdrawn, or stale.
fn consent_suppressed_response() -> HttpResponse {
    HttpResponse::Accepted().json(serde_json::json!({
        "status": "suppressed",
        "reason": "analytics_consent_not_granted",
        "consent_version": ANALYTICS_CONSENT_VERSION,
    }))
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

pub async fn record_match(
    db: web::Data<PgPool>,
    body: web::Json<RecordMatchBody>,
) -> Result<HttpResponse, ApiError> {
    // Consent gate: non-essential analytics are suppressed unless the caller
    // presents an effective (granted + current version) consent record.
    // Financial/private state is unaffected and remains server-authoritative.
    if !consent_allows_analytics(&body.consent) {
        return Ok(consent_suppressed_response());
    }

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
    // Consent gate: player-linked behaviour analytics are non-essential and
    // must be suppressed on withdrawal or stale consent.
    if !consent_allows_analytics(&body.consent) {
        return Ok(consent_suppressed_response());
    }

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
