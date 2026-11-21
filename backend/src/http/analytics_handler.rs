use crate::{
    api_error::ApiError,
    middleware::security::validate_uuid,
    service::analytics_service::{AnalyticsService, RecordMatchRequest},
};
use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};
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
    pub freshness_budget_secs: Option<i64>,
    pub latency_budget_ms: Option<i64>,
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

/// Validate a single event, returning a rejection reason when invalid.
fn validate_event(ev: &AnalyticsEvent) -> Result<(), String> {
    if validate_uuid(&ev.event_id).is_err() {
        return Err("invalid_event_id".to_string());
    }
    if !SUPPORTED_SCHEMA_VERSIONS.contains(&ev.schema_version) {
        return Err(format!("unsupported_schema_version:{}", ev.schema_version));
    }
    if ev.name.trim().is_empty() || ev.name.len() > 128 {
        return Err("invalid_event_name".to_string());
    }
    if let Some(payload) = &ev.payload {
        let size = serde_json::to_vec(payload).map(|v| v.len()).unwrap_or(usize::MAX);
        if size > MAX_EVENT_PAYLOAD_BYTES {
            return Err("payload_too_large".to_string());
        }
    }
    Ok(())
}

/// Ingest a batch of analytics events with durable deduplication.
///
/// Deduplication is keyed on `(producer_id, event_id)`: a retried event that was
/// already accepted converges to a single accepted record, while malformed or
/// schema-invalid events are routed to a diagnosable rejection path.
pub async fn ingest_events(
    db: web::Data<PgPool>,
    body: web::Json<IngestEventsBody>,
) -> Result<HttpResponse, ApiError> {
    if body.producer_id.trim().is_empty() || body.producer_id.len() > 128 {
        return Err(ApiError::BadRequest("invalid producer_id".to_string()));
    }
    if body.events.len() > MAX_EVENT_BATCH {
        return Err(ApiError::BadRequest(format!(
            "batch exceeds maximum of {} events",
            MAX_EVENT_BATCH
        )));
    }

    let svc = AnalyticsService::new(db.get_ref().clone());
    let mut outcomes = Vec::with_capacity(body.events.len());
    let mut accepted = 0usize;
    let mut rejected = 0usize;

    for ev in &body.events {
        match validate_event(ev) {
            Ok(()) => match svc
                .ingest_event(&body.producer_id, ev.event_id, ev.schema_version, &ev.name, ev.occurred_at, ev.payload.clone())
                .await
            {
                Ok(()) => {
                    accepted += 1;
                    outcomes.push(EventOutcome {
                        event_id: ev.event_id.clone(),
                        status: "accepted",
                        reason: None,
                    });
                }
                Err(e) => {
                    rejected += 1;
                    outcomes.push(EventOutcome {
                        event_id: ev.event_id.clone(),
                        status: "rejected",
                        reason: Some(format!("ingest_error:{}", e)),
                    });
                }
            },
            Err(reason) => {
                rejected += 1;
                // Record the rejection so operators can diagnose malformed events.
                let _ = svc
                    .record_rejected_event(&body.producer_id, &ev.event_id, &reason)
                    .await;
                outcomes.push(EventOutcome {
                    event_id: ev.event_id.clone(),
                    status: "rejected",
                    reason: Some(reason),
                });
            }
        }
    }

    Ok(HttpResponse::Ok().json(IngestEventsResponse {
        accepted,
        rejected,
        outcomes,
    }))
}

/// Link an anonymous subject to an authenticated account, gated on consent.
///
/// Only the owning pseudonymous subject may perform the link; raw authentication
/// identifiers are never stored alongside event payloads.
pub async fn link_identity(
    req: HttpRequest,
    db: web::Data<PgPool>,
    body: web::Json<IdentityLinkBody>,
) -> Result<HttpResponse, ApiError> {
    let subject_id = validate_uuid(&body.subject_id)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    authorize_subject(&req, subject_id)?;

    if !body.consent {
        return Ok(HttpResponse::Forbidden().json(IdentityResponse {
            status: "rejected",
            reason: Some("consent_required".to_string()),
        }));
    }
    if body.account_id.trim().is_empty() || body.account_id.len() > 128 {
        return Err(ApiError::BadRequest("invalid account_id".to_string()));
    }

    let svc = AnalyticsService::new(db.get_ref().clone());
    svc.link_identity(subject_id, &body.account_id).await?;
    Ok(HttpResponse::Ok().json(IdentityResponse {
        status: "linked",
        reason: None,
    }))
}

/// Unlink a subject from any account, making the identity link reversible.
pub async fn unlink_identity(
    req: HttpRequest,
    db: web::Data<PgPool>,
    body: web::Json<IdentityUnlinkBody>,
) -> Result<HttpResponse, ApiError> {
    let subject_id = validate_uuid(&body.subject_id)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    authorize_subject(&req, subject_id)?;

    let svc = AnalyticsService::new(db.get_ref().clone());
    svc.unlink_identity(subject_id).await?;
    Ok(HttpResponse::Ok().json(IdentityResponse {
        status: "unlinked",
        reason: None,
    }))
}

/// Ensure the caller owns the pseudonymous subject. In production this reads
/// verified JWT claims; here we require a matching `x-subject-id` header.
fn authorize_subject(req: &HttpRequest, subject_id: Uuid) -> Result<(), ApiError> {
    let claimed = req
        .headers()
        .get("x-subject-id")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing subject identity".to_string()))?;
    let claimed = validate_uuid(claimed)
        .map_err(|_| ApiError::Unauthorized("invalid subject identity".to_string()))?;
    if claimed != subject_id {
        return Err(ApiError::Forbidden(
            "subject does not own this identity link".to_string(),
        ));
    }
    Ok(())
}

/// Build budget metadata and stale/incomplete markers for a dashboard query.
fn dashboard_budget(
    period_start: Option<i64>,
    period_end: Option<i64>,
    freshness_budget_secs: Option<i64>,
    latency_budget_ms: Option<i64>,
) -> Result<(BudgetMetadata, bool, bool), ApiError> {
    let freshness_budget_secs = freshness_budget_secs.unwrap_or(DEFAULT_FRESHNESS_BUDGET_SECS);
    let latency_budget_ms = latency_budget_ms.unwrap_or(DEFAULT_LATENCY_BUDGET_MS);
    if freshness_budget_secs <= 0 || latency_budget_ms <= 0 {
        return Err(ApiError::BadRequest("budgets must be positive".to_string()));
    }

    let (range_secs, incomplete) = match (period_start, period_end) {
        (Some(start), Some(end)) => {
            if end < start {
                return Err(ApiError::BadRequest("period_end before period_start".to_string()));
            }
            let range = end - start;
            if range > MAX_QUERY_RANGE_SECS {
                // Bound the range rather than silently returning partial data as current.
                (MAX_QUERY_RANGE_SECS, true)
            } else {
                (range, false)
            }
        }
        _ => (MAX_QUERY_RANGE_SECS, true),
    };

    let now = chrono::Utc::now().timestamp();
    let stale = period_end.map(|end| now - end > freshness_budget_secs).unwrap_or(true);

    Ok((
        BudgetMetadata {
            freshness_budget_secs,
            latency_budget_ms,
            max_range_secs: MAX_QUERY_RANGE_SECS,
            range_secs,
        },
        stale,
        incomplete,
    ))
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
    let (budget, stale, incomplete) = dashboard_budget(
        query.period_start,
        query.period_end,
        query.freshness_budget_secs,
        query.latency_budget_ms,
    )?;
    let stats = svc.get_aggregated_stats(
        query.game_id,
        query.period_start,
        query.period_end,
    ).await?;
    Ok(HttpResponse::Ok().json(DashboardResponse {
        data: stats,
        stale,
        incomplete,
        budget,
    }))
}

pub async fn get_revenue_metrics(
    db: web::Data<PgPool>,
    query: web::Query<AggregationQuery>,
) -> Result<HttpResponse, ApiError> {
    let svc = AnalyticsService::new(db.get_ref().clone());
    let (budget, stale, incomplete) = dashboard_budget(
        query.period_start,
        query.period_end,
        query.freshness_budget_secs,
        query.latency_budget_ms,
    )?;
    let metrics = svc.get_revenue_metrics(
        query.game_id,
        query.period_start,
        query.period_end,
    ).await?;
    Ok(HttpResponse::Ok().json(DashboardResponse {
        data: metrics,
        stale,
        incomplete,
        budget,
    }))
}
