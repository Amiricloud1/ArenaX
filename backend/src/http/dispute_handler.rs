//! Dispute resolution HTTP handlers (Issue #909).
//!
//! Any authenticated user can open a ticket, add evidence to their own
//! ticket, and read tickets they opened. Assigning, escalating, resolving,
//! and rejecting are moderator/admin actions.

use actix_web::{web, HttpRequest, HttpResponse};
use sqlx::PgPool;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::middleware::ClaimsExt;
use crate::models::{
    AddEvidenceRequest, AssignDisputeRequest, CreateTicketRequest, EscalateDisputeRequest,
    ListDisputesQuery, PaginationParams, RejectDisputeRequest, ResolveDisputeRequest,
    ReviewDisputeAppealRequest, SubmitDisputeAppealRequest,
};
use crate::service::DisputeService;

/// Require the caller to hold the `admin` or `moderator` role.
///
/// Mirrors `tournament_handler::require_admin`'s shape; disputes are handled
/// by moderators day-to-day, with admins as the final escalation tier, so
/// both roles pass here and the escalation *level* (not the caller's role)
/// is what actually gates how far a ticket has progressed.
fn require_staff(req: &HttpRequest) -> Result<Uuid, ApiError> {
    let claims = req
        .claims()
        .ok_or_else(|| ApiError::unauthorized("Authentication required"))?;
    let is_staff = claims.roles.contains(&"admin".to_string())
        || claims.roles.contains(&"moderator".to_string());
    if !is_staff {
        return Err(ApiError::forbidden("Moderator or admin access required"));
    }
    req.user_id()
        .ok_or_else(|| ApiError::unauthorized("Authentication required"))
}

fn require_user(req: &HttpRequest) -> Result<Uuid, ApiError> {
    req.user_id()
        .ok_or_else(|| ApiError::unauthorized("Authentication required"))
}

/// POST /api/disputes - Open a new dispute ticket.
pub async fn create_dispute(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    body: web::Json<CreateTicketRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = require_user(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let dispute = service.create_dispute(user_id, body.into_inner()).await?;

    Ok(HttpResponse::Created().json(serde_json::json!({
        "data": dispute,
        "ticketId": dispute.ticket_id(),
    })))
}

/// GET /api/disputes/:id - Full detail: dispute + history + evidence.
///
/// Open to the ticket's own opener or to staff -- not to arbitrary users.
pub async fn get_dispute(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let user_id = require_user(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());
    let dispute_id = path.into_inner();

    let dispute = service.get_dispute(dispute_id).await?;
    let claims = req.claims();
    let is_staff = claims
        .as_ref()
        .map(|c| {
            c.roles.contains(&"admin".to_string()) || c.roles.contains(&"moderator".to_string())
        })
        .unwrap_or(false);
    if dispute.opened_by != user_id && !is_staff {
        return Err(ApiError::forbidden("You may only view your own disputes"));
    }

    let detail = service.get_dispute_detail(dispute_id).await?;
    audit_dispute_access(
        pool.get_ref(),
        user_id,
        dispute_id,
        "DISPUTE_EVIDENCE_METADATA_ACCESSED",
        "evidence metadata viewed",
    )
    .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": detail })))
}

/// GET /api/disputes - List disputes (staff only; use `/api/disputes/mine`
/// for a user's own tickets).
pub async fn list_disputes(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    query: web::Query<ListDisputesQuery>,
    pagination: web::Query<PaginationParams>,
) -> Result<HttpResponse, ApiError> {
    require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let limit = pagination.resolved_limit();
    let offset = pagination.sql_offset();
    let (disputes, total) = service.list_disputes(&query, limit, offset).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "data": disputes,
        "total": total,
        "page": pagination.resolved_page(),
        "limit": limit,
    })))
}

/// GET /api/disputes/mine - Disputes the caller opened.
pub async fn list_my_disputes(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    pagination: web::Query<PaginationParams>,
) -> Result<HttpResponse, ApiError> {
    let user_id = require_user(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let filter = ListDisputesQuery {
        status: None,
        category: None,
        assigned_to: None,
        opened_by: Some(user_id),
    };
    let limit = pagination.resolved_limit();
    let offset = pagination.sql_offset();
    let (disputes, total) = service.list_disputes(&filter, limit, offset).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "data": disputes,
        "total": total,
        "page": pagination.resolved_page(),
        "limit": limit,
    })))
}

/// POST /api/disputes/:id/evidence - Attach evidence metadata.
///
/// Open to the ticket's own opener or to staff.
pub async fn add_evidence(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: web::Json<AddEvidenceRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = require_user(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());
    let dispute_id = path.into_inner();

    let dispute = service.get_dispute(dispute_id).await?;
    let claims = req.claims();
    let is_staff = claims
        .as_ref()
        .map(|c| {
            c.roles.contains(&"admin".to_string()) || c.roles.contains(&"moderator".to_string())
        })
        .unwrap_or(false);
    if dispute.opened_by != user_id && !is_staff {
        return Err(ApiError::forbidden(
            "You may only add evidence to your own disputes",
        ));
    }

    let evidence = service
        .add_evidence(dispute_id, user_id, body.into_inner())
        .await?;

    Ok(HttpResponse::Created().json(serde_json::json!({ "data": evidence })))
}

/// GET /api/disputes/:id/evidence - List evidence for a dispute.
pub async fn list_evidence(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let user_id = require_user(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());
    let dispute_id = path.into_inner();

    let dispute = service.get_dispute(dispute_id).await?;
    let claims = req.claims();
    let is_staff = claims
        .as_ref()
        .map(|c| {
            c.roles.contains(&"admin".to_string()) || c.roles.contains(&"moderator".to_string())
        })
        .unwrap_or(false);
    if dispute.opened_by != user_id && !is_staff {
        return Err(ApiError::forbidden(
            "You may only view evidence on your own disputes",
        ));
    }

    let evidence = service.list_evidence(dispute_id).await?;
    audit_dispute_access(
        pool.get_ref(),
        user_id,
        dispute_id,
        "DISPUTE_EVIDENCE_METADATA_ACCESSED",
        "evidence metadata viewed",
    )
    .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": evidence })))
}

async fn audit_dispute_access(
    pool: &PgPool,
    actor_id: Uuid,
    dispute_id: Uuid,
    action: &str,
    details: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (user_id, action, resource_type, resource_id, details)
        VALUES ($1, $2, 'dispute', $3, $4)
        "#,
    )
    .bind(actor_id)
    .bind(action)
    .bind(dispute_id)
    .bind(details)
    .execute(pool)
    .await
    .map_err(ApiError::database_error)?;
    Ok(())
}

/// GET /api/disputes/:id/history - Resolution history (staff only).
pub async fn get_history(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let history = service.get_history(path.into_inner()).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": history })))
}

/// POST /api/disputes/:id/assign - Assign a dispute to a staff member.
pub async fn assign_dispute(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: web::Json<AssignDisputeRequest>,
) -> Result<HttpResponse, ApiError> {
    let actor_id = require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let dispute = service
        .assign_dispute(path.into_inner(), body.assigned_to, actor_id)
        .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": dispute })))
}

/// POST /api/disputes/:id/review - Move a dispute from `open` to
/// `under_review`.
pub async fn start_review(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let actor_id = require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let dispute = service.start_review(path.into_inner(), actor_id).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": dispute })))
}

/// POST /api/disputes/:id/escalate - Escalate to the next tier.
pub async fn escalate_dispute(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: web::Json<EscalateDisputeRequest>,
) -> Result<HttpResponse, ApiError> {
    let actor_id = require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let dispute = service
        .escalate_dispute(path.into_inner(), actor_id, body.into_inner().note)
        .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": dispute })))
}

/// POST /api/disputes/:id/resolve - Resolve a dispute (notifies the opener).
pub async fn resolve_dispute(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: web::Json<ResolveDisputeRequest>,
) -> Result<HttpResponse, ApiError> {
    let actor_id = require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let dispute = service
        .resolve_dispute(path.into_inner(), actor_id, body.into_inner().resolution)
        .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": dispute })))
}

/// POST /api/disputes/:id/reject - Reject a dispute (notifies the opener).
pub async fn reject_dispute(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: web::Json<RejectDisputeRequest>,
) -> Result<HttpResponse, ApiError> {
    let actor_id = require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());

    let dispute = service
        .reject_dispute(path.into_inner(), actor_id, body.into_inner().reason)
        .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": dispute })))
}

/// POST /api/disputes/:id/appeal - Submit one appeal against a versioned decision.
pub async fn submit_dispute_appeal(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: web::Json<SubmitDisputeAppealRequest>,
) -> Result<HttpResponse, ApiError> {
    let user_id = require_user(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());
    let appeal = service
        .submit_appeal(path.into_inner(), user_id, body.into_inner())
        .await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "data": appeal })))
}

/// GET /api/disputes/:id/appeal - Read the caller's appeal or staff-visible case appeal.
pub async fn get_dispute_appeal(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, ApiError> {
    let user_id = require_user(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());
    let dispute_id = path.into_inner();
    let dispute = service.get_dispute(dispute_id).await?;
    let staff_access = req.claims().is_some_and(|claims| {
        claims.roles.iter().any(|role| role == "admin" || role == "moderator")
    });
    if dispute.opened_by != user_id && !staff_access {
        return Err(ApiError::forbidden("You may only view your own case appeal"));
    }
    let appeal = service.get_appeal(dispute_id).await?;
    audit_dispute_access(
        pool.get_ref(),
        user_id,
        dispute_id,
        "DISPUTE_APPEAL_ACCESSED",
        "case appeal viewed",
    )
    .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": appeal })))
}

/// GET /api/disputes/appeals - Independent-review queue.
pub async fn list_pending_appeals(
    req: HttpRequest,
    pool: web::Data<PgPool>,
) -> Result<HttpResponse, ApiError> {
    require_staff(&req)?;
    let staff_id = require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());
    let appeals = service.pending_appeals(100).await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (user_id, action, resource_type, details)
        VALUES ($1, 'DISPUTE_APPEAL_QUEUE_ACCESSED', 'dispute_appeal', 'pending appeal queue viewed')
        "#,
    )
    .bind(staff_id)
    .execute(pool.get_ref())
    .await
    .map_err(ApiError::database_error)?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": appeals })))
}

/// POST /api/disputes/appeals/:id/review - Review an appeal independently.
pub async fn review_dispute_appeal(
    req: HttpRequest,
    pool: web::Data<PgPool>,
    path: web::Path<Uuid>,
    body: web::Json<ReviewDisputeAppealRequest>,
) -> Result<HttpResponse, ApiError> {
    let reviewer_id = require_staff(&req)?;
    let service = DisputeService::new(pool.get_ref().clone());
    let appeal = service
        .review_appeal(
            path.into_inner(),
            reviewer_id,
            body.accept,
            &body.decision_reason,
        )
        .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": appeal })))
}

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/disputes")
            .route("/appeals", web::get().to(list_pending_appeals))
            .route("/appeals/{id}/review", web::post().to(review_dispute_appeal))
            .route("", web::post().to(create_dispute))
            .route("", web::get().to(list_disputes))
            .route("/mine", web::get().to(list_my_disputes))
            .route("/{id}", web::get().to(get_dispute))
            .route("/{id}/evidence", web::post().to(add_evidence))
            .route("/{id}/evidence", web::get().to(list_evidence))
            .route("/{id}/history", web::get().to(get_history))
            .route("/{id}/appeal", web::post().to(submit_dispute_appeal))
            .route("/{id}/appeal", web::get().to(get_dispute_appeal))
            .route("/{id}/assign", web::post().to(assign_dispute))
            .route("/{id}/review", web::post().to(start_review))
            .route("/{id}/escalate", web::post().to(escalate_dispute))
            .route("/{id}/resolve", web::post().to(resolve_dispute))
            .route("/{id}/reject", web::post().to(reject_dispute)),
    );
}