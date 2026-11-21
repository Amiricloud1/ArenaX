use actix_web::{web, HttpRequest, HttpResponse, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use std::sync::Arc;

use crate::api_error::ApiError;
use crate::auth::middleware::ClaimsExt;
use crate::middleware::cache::ResponseCache;
use crate::models::{CloseSeasonRequest, EloHistoryQuery, PaginatedResponse, PaginationParams};
use crate::realtime::leaderboard_broadcaster::LeaderboardBroadcaster;
use crate::service::LeaderboardService;

fn require_admin(req: &HttpRequest) -> Result<(), ApiError> {
    let claims = req
        .claims()
        .ok_or_else(|| ApiError::unauthorized("Authentication required"))?;
    if claims.roles.contains(&"admin".to_string()) {
        Ok(())
    } else {
        Err(ApiError::forbidden("ROLE_ADMIN required"))
    }
}

/// Maximum age (in seconds) a snapshot may reach before it is reported as
/// stale. Bounded so clients can reason about freshness (Issue #257).
const SNAPSHOT_MAX_STALENESS_SECS: i64 = 60;

/// Query parameters shared by snapshot-backed leaderboard reads.
///
/// `snapshot_id` pins the read to a specific immutable snapshot so pagination
/// across pages never observes users moving between pages. When omitted the
/// newest snapshot is used and its version is echoed back for the client to
/// pin subsequent pages.
#[derive(Debug, Deserialize)]
pub struct SnapshotQuery {
    #[serde(flatten)]
    pub pagination: PaginationParams,
    pub snapshot_id: Option<Uuid>,
}

/// Freshness metadata attached to every snapshot-backed leaderboard response.
#[derive(Debug, Serialize)]
pub struct SnapshotMeta {
    /// Immutable snapshot identifier; pass back as `snapshot_id` to paginate.
    pub snapshot_id: Uuid,
    /// Monotonic version of the snapshot for the category.
    pub version: i64,
    /// When the snapshot was materialised.
    pub snapshot_time: DateTime<Utc>,
    /// Age of the snapshot in seconds at response time.
    pub age_seconds: i64,
    /// Upper bound on how stale a snapshot may be before it is considered
    /// expired; observable so clients can decide to refetch.
    pub max_staleness_seconds: i64,
    /// True when `age_seconds` exceeds `max_staleness_seconds`.
    pub stale: bool,
}

/// Paginated leaderboard payload carrying snapshot metadata.
#[derive(Debug, Serialize)]
pub struct SnapshotPaginatedResponse<T> {
    pub total: i64,
    pub page: i64,
    pub limit: i64,
    pub snapshot: SnapshotMeta,
    pub data: Vec<T>,
}

fn snapshot_meta(snapshot_id: Uuid, version: i64, snapshot_time: DateTime<Utc>) -> SnapshotMeta {
    let age_seconds = (Utc::now() - snapshot_time).num_seconds().max(0);
    SnapshotMeta {
        snapshot_id,
        version,
        snapshot_time,
        age_seconds,
        max_staleness_seconds: SNAPSHOT_MAX_STALENESS_SECS,
        stale: age_seconds > SNAPSHOT_MAX_STALENESS_SECS,
    }
}

/// Builds the service with whatever optional infrastructure is registered.
///
/// The cache and the broadcaster are both `Option` on the service, so a
/// deployment without Redis still serves leaderboards — it just reads through
/// to Postgres and pushes nothing. Centralised here so a handler cannot
/// accidentally construct a bare service and silently lose caching
/// (Issue #910) or real-time deltas (Issue #900).
fn leaderboard_service(
    pool: &web::Data<PgPool>,
    cache: Option<&web::Data<ResponseCache>>,
    broadcaster: Option<&web::Data<Arc<LeaderboardBroadcaster>>>,
) -> LeaderboardService {
    let mut service = LeaderboardService::new(pool.get_ref().clone());

    if let Some(cache) = cache {
        service = service.with_cache(cache.get_ref().clone());
    }
    if let Some(broadcaster) = broadcaster {
        service = service.with_broadcaster(broadcaster.get_ref().clone());
    }

    service
}

/// GET /api/v1/leaderboards/:category
///
/// Serves from a versioned, freshness-aware snapshot. Pass `snapshot_id` to
/// pin pagination to a single immutable snapshot; otherwise the newest
/// snapshot is used and its id is returned for the client to pin.
pub async fn get_leaderboard(
    pool: web::Data<PgPool>,
    cache: Option<web::Data<ResponseCache>>,
    category: web::Path<String>,
    query: web::Query<SnapshotQuery>,
) -> Result<HttpResponse, ApiError> {
    let service = leaderboard_service(&pool, cache.as_ref(), None);
    let limit = query.pagination.resolved_limit();
    let offset = query.pagination.sql_offset();

    let leaderboard = service
        .get_leaderboard_snapshot(&category, query.snapshot_id, limit, offset)
        .await?;

    Ok(HttpResponse::Ok().json(SnapshotPaginatedResponse {
        total: leaderboard.total_count,
        page: query.pagination.resolved_page(),
        limit,
        snapshot: snapshot_meta(
            leaderboard.snapshot_id,
            leaderboard.version,
            leaderboard.snapshot_time,
        ),
        data: leaderboard.entries,
    }))
}

/// GET /api/v1/leaderboards/:category/season/:season
pub async fn get_seasonal_leaderboard(
    pool: web::Data<PgPool>,
    path: web::Path<(String, String)>,
    query: web::Query<SnapshotQuery>,
) -> Result<HttpResponse, ApiError> {
    let (category, season) = path.into_inner();
    let service = LeaderboardService::new(pool.get_ref().clone());
    let limit = query.pagination.resolved_limit();
    let offset = query.pagination.sql_offset();

    let leaderboard = service
        .get_seasonal_leaderboard_snapshot(&category, &season, query.snapshot_id, limit, offset)
        .await?;

    Ok(HttpResponse::Ok().json(SnapshotPaginatedResponse {
        total: leaderboard.total_participants,
        page: query.pagination.resolved_page(),
        limit,
        snapshot: snapshot_meta(
            leaderboard.snapshot_id,
            leaderboard.version,
            leaderboard.snapshot_time,
        ),
        data: leaderboard.entries,
    }))
}

/// GET /api/v1/leaderboards/:category/player/:player_id
pub async fn get_player_rank(
    pool: web::Data<PgPool>,
    path: web::Path<(String, Uuid)>,
) -> Result<HttpResponse, ApiError> {
    let (category, player_id) = path.into_inner();
    let service = LeaderboardService::new(pool.get_ref().clone());

    let player_rank = service.get_player_rank(&category, player_id).await?;

    Ok(HttpResponse::Ok().json(player_rank))
}

/// GET /api/v1/leaderboards/:category/history/:player_id
pub async fn get_rank_history(
    pool: web::Data<PgPool>,
    path: web::Path<(String, Uuid)>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> Result<HttpResponse, ApiError> {
    let (category, player_id) = path.into_inner();
    let service = LeaderboardService::new(pool.get_ref().clone());
    let days = query
        .get("days")
        .and_then(|d| d.parse::<i64>().ok())
        .unwrap_or(30);

    let history = service
        .get_rank_history(player_id, &category, days)
        .await?;

    Ok(HttpResponse::Ok().json(history))
}

/// GET /api/v1/leaderboards/:category/player/:player_id/elo-history
///
/// Returns daily ELO snapshots, progression chart data, highest/lowest records,
/// and a volatility metric for the player, scoped to an optional date range
/// (`start_date` / `end_date` query params, defaulting to the last 30 days).
pub async fn get_elo_history(
    pool: web::Data<PgPool>,
    path: web::Path<(String, Uuid)>,
    query: web::Query<EloHistoryQuery>,
) -> Result<HttpResponse, ApiError> {
    let (category, player_id) = path.into_inner();
    let service = LeaderboardService::new(pool.get_ref().clone());

    let end_date = query.end_date.unwrap_or_else(Utc::now);
    let start_date = query.start_date.unwrap_or_else(|| end_date - Duration::days(30));

    let history = service
        .get_elo_history(player_id, &category, start_date, end_date)
        .await?;

    Ok(HttpResponse::Ok().json(history))
}

/// POST /api/v1/leaderboards/:category/refresh — admin-only (ROLE_ADMIN).
///
/// Materialises a new immutable snapshot for the category and invalidates any
/// cached pages. Returns the new snapshot's version/time so operators can
/// observe the refresh.
pub async fn refresh_leaderboard(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    cache: Option<web::Data<ResponseCache>>,
    broadcaster: Option<web::Data<Arc<LeaderboardBroadcaster>>>,
    category: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    require_admin(&req)?;
    let service = leaderboard_service(&pool, cache.as_ref(), broadcaster.as_ref());

    let snapshot = service.refresh_leaderboard_snapshot(&category).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "message": format!("Leaderboard for {} refreshed successfully", category),
        "snapshot": snapshot_meta(snapshot.snapshot_id, snapshot.version, snapshot.snapshot_time),
    })))
}

/// GET /api/v1/leaderboards/:category/stats
pub async fn get_leaderboard_stats(
    pool: web::Data<PgPool>,
    category: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    let service = LeaderboardService::new(pool.get_ref().clone());

    let stats = service.get_leaderboard_stats(&category).await?;

    Ok(HttpResponse::Ok().json(stats))
}

// ─── Season close (#1075) ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SeasonHistoryQuery {
    pub season_id: Uuid,
}

/// POST /api/admin/seasons/{id}/close — admin-only (ROLE_ADMIN).
pub async fn close_season(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: Option<web::Json<CloseSeasonRequest>>,
) -> Result<HttpResponse, ApiError> {
    require_admin(&req)?;
    let season_id = path.into_inner();
    let top_n = body.and_then(|b| b.top_n).unwrap_or(100);

    let service = LeaderboardService::new(pool.get_ref().clone());
    let result = service.close_season(season_id, top_n).await?;

    Ok(HttpResponse::Ok().json(result))
}

/// GET /api/leaderboard/history?season_id=X
pub async fn get_season_history(
    pool: web::Data<PgPool>,
    query: web::Query<SeasonHistoryQuery>,
) -> Result<HttpResponse, ApiError> {
    let service = LeaderboardService::new(pool.get_ref().clone());
    let snapshots = service.get_season_history(query.season_id).await?;

    Ok(HttpResponse::Ok().json(snapshots))
}

/// POST /api/admin/leaderboards/:category/snapshot — admin-only (ROLE_ADMIN).
///
/// Explicitly triggers a snapshot refresh for a category. Distinct from the
/// public refresh route so operators have a stable, authorised entry point.
pub async fn admin_refresh_snapshot(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    cache: Option<web::Data<ResponseCache>>,
    broadcaster: Option<web::Data<Arc<LeaderboardBroadcaster>>>,
    category: web::Path<String>,
) -> Result<HttpResponse, ApiError> {
    require_admin(&req)?;
    let service = leaderboard_service(&pool, cache.as_ref(), broadcaster.as_ref());

    let snapshot = service.refresh_leaderboard_snapshot(&category).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "snapshot": snapshot_meta(snapshot.snapshot_id, snapshot.version, snapshot.snapshot_time),
    })))
}

/// Registers this module's season-close endpoints. Call inside the `/api`
/// scope alongside the other `.configure(...)` handlers.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/admin/seasons").route("/{id}/close", web::post().to(close_season)),
    )
    .service(
        web::scope("/admin/leaderboards")
            .route("/{category}/snapshot", web::post().to(admin_refresh_snapshot)),
    )
    .service(
        web::scope("/leaderboard").route("/history", web::get().to(get_season_history)),
    );
}
