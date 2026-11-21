//! Dispute resolution service (Issue #909).
//!
//! Ticketed, escalatable disputes with an append-only resolution history and
//! evidence attachments. Deliberately built as a standalone module rather
//! than extending `match_service.rs`'s existing `match_disputes` handling --
//! see `models::dispute`'s module doc comment for why.
//!
//! Every query here uses the `sqlx::query_as::<_, T>()` / `sqlx::query()`
//! runtime-checked form rather than the `sqlx::query!`/`query_as!` compile-time
//! macros. The compile-time macros need a live, migrated database reachable
//! at build time to even compile, and several existing files in this crate
//! (e.g. match_service.rs) already fail that check today due to unrelated,
//! pre-existing schema drift. The runtime-checked form only needs `FromRow`
//! to compile, matching the convention already used in
//! `notification_handler.rs` and `social_service.rs`, and keeps this new
//! module buildable independent of that pre-existing breakage.

use crate::api_error::ApiError;
use crate::models::{
    AddEvidenceRequest, CreateTicketRequest, Dispute, DisputeAppeal, DisputeDetail,
    DisputeEvidence, DisputeHistoryEntry, EscalationLevel, ListDisputesQuery,
    SubmitDisputeAppealRequest, TicketStatus,
};
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub struct DisputeService {
    db_pool: PgPool,
}

impl DisputeService {
    pub fn new(db_pool: PgPool) -> Self {
        Self { db_pool }
    }

    // ─── Create ─────────────────────────────────────────────────────────────

    /// Open a new dispute ticket. Always starts at status `open`, escalation
    /// level 1 (front-line support), and records the opening as the first
    /// history entry.
    pub async fn create_dispute(
        &self,
        opened_by: Uuid,
        request: CreateTicketRequest,
    ) -> Result<Dispute, ApiError> {
        if request.subject.trim().is_empty() {
            return Err(ApiError::bad_request("subject must not be empty"));
        }
        if request.description.trim().is_empty() {
            return Err(ApiError::bad_request("description must not be empty"));
        }

        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let dedupe_key = format!(
            "{}:{}:{}:{}",
            opened_by,
            request.match_id.map_or_else(|| "none".to_string(), |id| id.to_string()),
            request.category.trim().to_lowercase(),
            request.subject.trim().to_lowercase()
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(dedupe_key)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::database_error)?;
        let duplicate_id: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT id FROM disputes
            WHERE opened_by = $1
              AND match_id IS NOT DISTINCT FROM $2
              AND lower(trim(category)) = lower(trim($3))
              AND lower(trim(subject)) = lower(trim($4))
              AND status IN ('open', 'under_review', 'escalated')
            LIMIT 1
            "#,
        )
        .bind(opened_by)
        .bind(request.match_id)
        .bind(request.category.trim())
        .bind(request.subject.trim())
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;
        if let Some(duplicate_id) = duplicate_id {
            return Err(ApiError::conflict(format!(
                "A matching active report already exists ({duplicate_id})"
            )));
        }

        let dispute = sqlx::query_as::<_, Dispute>(
            r#"
            INSERT INTO disputes (opened_by, match_id, category, subject, description)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(opened_by)
        .bind(request.match_id)
        .bind(&request.category)
        .bind(&request.subject)
        .bind(&request.description)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        sqlx::query(
            r#"
            INSERT INTO dispute_history (dispute_id, actor_id, action, to_status, to_escalation_level)
            VALUES ($1, $2, 'opened', $3, $4)
            "#,
        )
        .bind(dispute.id)
        .bind(opened_by)
        .bind(TicketStatus::Open.as_str())
        .bind(EscalationLevel::FrontLine.as_i16())
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        tx.commit().await.map_err(ApiError::database_error)?;

        Ok(dispute)
    }

    // ─── Read ───────────────────────────────────────────────────────────────

    pub async fn get_dispute(&self, dispute_id: Uuid) -> Result<Dispute, ApiError> {
        sqlx::query_as::<_, Dispute>("SELECT * FROM disputes WHERE id = $1")
            .bind(dispute_id)
            .fetch_optional(&self.db_pool)
            .await
            .map_err(ApiError::database_error)?
            .ok_or_else(|| ApiError::not_found("dispute not found"))
    }

    /// Full detail view: the dispute plus its complete history and evidence,
    /// oldest first -- what a support agent or the ticket's own opener would
    /// want when opening the ticket.
    pub async fn get_dispute_detail(&self, dispute_id: Uuid) -> Result<DisputeDetail, ApiError> {
        let dispute = self.get_dispute(dispute_id).await?;
        let history = self.get_history(dispute_id).await?;
        let evidence = self.list_evidence(dispute_id).await?;

        Ok(DisputeDetail {
            ticket_id: dispute.ticket_id(),
            dispute,
            history,
            evidence,
        })
    }

    pub async fn list_disputes(
        &self,
        filter: &ListDisputesQuery,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Dispute>, i64), ApiError> {
        let disputes = sqlx::query_as::<_, Dispute>(
            r#"
            SELECT * FROM disputes
            WHERE ($1::text IS NULL OR status = $1)
              AND ($2::text IS NULL OR category = $2)
              AND ($3::uuid IS NULL OR assigned_to = $3)
              AND ($4::uuid IS NULL OR opened_by = $4)
            ORDER BY created_at DESC
            LIMIT $5 OFFSET $6
            "#,
        )
        .bind(&filter.status)
        .bind(&filter.category)
        .bind(filter.assigned_to)
        .bind(filter.opened_by)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db_pool)
        .await
        .map_err(ApiError::database_error)?;

        let total: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM disputes
            WHERE ($1::text IS NULL OR status = $1)
              AND ($2::text IS NULL OR category = $2)
              AND ($3::uuid IS NULL OR assigned_to = $3)
              AND ($4::uuid IS NULL OR opened_by = $4)
            "#,
        )
        .bind(&filter.status)
        .bind(&filter.category)
        .bind(filter.assigned_to)
        .bind(filter.opened_by)
        .fetch_one(&self.db_pool)
        .await
        .map_err(ApiError::database_error)?;

        Ok((disputes, total))
    }

    pub async fn get_history(
        &self,
        dispute_id: Uuid,
    ) -> Result<Vec<DisputeHistoryEntry>, ApiError> {
        sqlx::query_as::<_, DisputeHistoryEntry>(
            "SELECT * FROM dispute_history WHERE dispute_id = $1 ORDER BY created_at ASC",
        )
        .bind(dispute_id)
        .fetch_all(&self.db_pool)
        .await
        .map_err(ApiError::database_error)
    }

    pub async fn submit_appeal(
        &self,
        dispute_id: Uuid,
        appellant_id: Uuid,
        request: SubmitDisputeAppealRequest,
    ) -> Result<DisputeAppeal, ApiError> {
        let appeal_text = request.appeal_text.trim();
        if appeal_text.is_empty() || appeal_text.len() > 2000 {
            return Err(ApiError::bad_request(
                "appeal_text must contain 1 to 2000 characters",
            ));
        }
        if request.evidence_refs.len() > 5
            || request
                .evidence_refs
                .iter()
                .any(|reference| reference.trim().is_empty() || reference.len() > 500)
        {
            return Err(ApiError::bad_request(
                "an appeal may include at most five non-empty evidence references of 500 characters or fewer",
            ));
        }

        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let dispute = sqlx::query_as::<_, Dispute>(
            "SELECT * FROM disputes WHERE id = $1 FOR UPDATE",
        )
        .bind(dispute_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("dispute not found"))?;

        if dispute.opened_by != appellant_id {
            return Err(ApiError::forbidden("You may only appeal your own dispute"));
        }
        if !matches!(
            dispute.status(),
            TicketStatus::Resolved | TicketStatus::Rejected
        ) {
            return Err(ApiError::conflict(
                "only a resolved or rejected case may be appealed",
            ));
        }
        if request.decision_version != dispute.decision_version {
            return Err(ApiError::conflict(
                "the appeal references a stale decision version",
            ));
        }
        let deadline = dispute
            .resolved_at
            .ok_or_else(|| ApiError::conflict("the case has no finalized decision"))?
            + Duration::days(30);
        if deadline <= Utc::now() {
            return Err(ApiError::conflict("the appeal deadline has passed"));
        }
        let already_appealed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM dispute_appeals WHERE dispute_id = $1)",
        )
        .bind(dispute_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;
        if already_appealed {
            return Err(ApiError::conflict(
                "only one appeal may be submitted for a case decision",
            ));
        }

        let appeal = sqlx::query_as::<_, DisputeAppeal>(
            r#"
            INSERT INTO dispute_appeals
                (dispute_id, appellant_id, decision_version, appeal_text, evidence_refs, deadline_at)
            SELECT $1, $2, $3, $4, $5, $6
            WHERE $6 > NOW()
            RETURNING *
            "#,
        )
        .bind(dispute_id)
        .bind(appellant_id)
        .bind(request.decision_version)
        .bind(appeal_text)
        .bind(serde_json::json!(request.evidence_refs))
        .bind(deadline)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::conflict("the appeal deadline has passed"))?;

        sqlx::query(
            r#"
            INSERT INTO dispute_history (dispute_id, actor_id, action, note)
            VALUES ($1, $2, 'appeal_submitted', 'Appeal submitted')
            "#,
        )
        .bind(dispute_id)
        .bind(appellant_id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;
        tx.commit().await.map_err(ApiError::database_error)?;
        Ok(appeal)
    }

    pub async fn pending_appeals(&self, limit: i64) -> Result<Vec<DisputeAppeal>, ApiError> {
        sqlx::query_as::<_, DisputeAppeal>(
            r#"
            SELECT * FROM dispute_appeals
            WHERE status = 'pending'
            ORDER BY submitted_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db_pool)
        .await
        .map_err(ApiError::database_error)
    }

    pub async fn get_appeal(&self, dispute_id: Uuid) -> Result<DisputeAppeal, ApiError> {
        sqlx::query_as::<_, DisputeAppeal>(
            "SELECT * FROM dispute_appeals WHERE dispute_id = $1",
        )
        .bind(dispute_id)
        .fetch_optional(&self.db_pool)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("appeal not found"))
    }

    pub async fn review_appeal(
        &self,
        appeal_id: Uuid,
        reviewer_id: Uuid,
        accept: bool,
        decision_reason: &str,
    ) -> Result<DisputeAppeal, ApiError> {
        let reason = decision_reason.trim();
        if reason.is_empty() || reason.len() > 1000 {
            return Err(ApiError::bad_request(
                "decision_reason must contain 1 to 1000 characters",
            ));
        }

        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let appeal = sqlx::query_as::<_, DisputeAppeal>(
            "SELECT * FROM dispute_appeals WHERE id = $1 FOR UPDATE",
        )
        .bind(appeal_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("appeal not found"))?;
        if appeal.status != "pending" {
            return Err(ApiError::conflict("appeal is no longer reviewable"));
        }

        let dispute = sqlx::query_as::<_, Dispute>(
            "SELECT * FROM disputes WHERE id = $1 FOR UPDATE",
        )
        .bind(appeal.dispute_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;
        if dispute.resolved_by == Some(reviewer_id) || dispute.opened_by == reviewer_id {
            return Err(ApiError::forbidden(
                "The original decision maker and case opener cannot review this appeal",
            ));
        }
        if dispute.decision_version != appeal.decision_version {
            return Err(ApiError::conflict(
                "the case decision changed after this appeal was submitted",
            ));
        }

        let status = if accept { "accepted" } else { "rejected" };
        let reviewed = sqlx::query_as::<_, DisputeAppeal>(
            r#"
            UPDATE dispute_appeals
            SET status = $2, reviewer_id = $3, decision_reason = $4, reviewed_at = NOW()
            WHERE id = $1 AND status = 'pending'
            RETURNING *
            "#,
        )
        .bind(appeal_id)
        .bind(status)
        .bind(reviewer_id)
        .bind(reason)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::conflict("appeal is no longer reviewable"))?;

        sqlx::query(
            r#"
            INSERT INTO dispute_history (dispute_id, actor_id, action, note)
            VALUES ($1, $2, 'appeal_reviewed', $3)
            "#,
        )
        .bind(dispute.id)
        .bind(reviewer_id)
        .bind(format!("Appeal {status}: {reason}"))
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;
        sqlx::query(
            r#"
            INSERT INTO notifications (user_id, type, title, message, link)
            VALUES ($1, 'dispute_appeal', $2, $3, $4)
            "#,
        )
        .bind(appeal.appellant_id)
        .bind(if accept {
            "Your appeal was accepted"
        } else {
            "Your appeal was reviewed"
        })
        .bind(reason)
        .bind(format!("/disputes/{}", dispute.id))
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        tx.commit().await.map_err(ApiError::database_error)?;
        Ok(reviewed)
    }

    pub async fn list_evidence(&self, dispute_id: Uuid) -> Result<Vec<DisputeEvidence>, ApiError> {
        sqlx::query_as::<_, DisputeEvidence>(
            "SELECT * FROM dispute_evidence WHERE dispute_id = $1 ORDER BY created_at ASC",
        )
        .bind(dispute_id)
        .fetch_all(&self.db_pool)
        .await
        .map_err(ApiError::database_error)
    }

    // ─── Evidence ───────────────────────────────────────────────────────────

    /// Attach evidence to a dispute. Records metadata for a file already
    /// uploaded elsewhere -- see the module doc comment on why this doesn't
    /// handle the upload itself.
    pub async fn add_evidence(
        &self,
        dispute_id: Uuid,
        uploaded_by: Uuid,
        request: AddEvidenceRequest,
    ) -> Result<DisputeEvidence, ApiError> {
        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let dispute = sqlx::query_as::<_, Dispute>(
            "SELECT * FROM disputes WHERE id = $1 FOR UPDATE",
        )
        .bind(dispute_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("dispute not found"))?;
        if dispute.status().is_terminal() {
            return Err(ApiError::conflict(
                "cannot add evidence to a closed dispute",
            ));
        }
        if request.file_url.trim().is_empty() || request.file_name.trim().is_empty() {
            return Err(ApiError::bad_request("file_url and file_name are required"));
        }

        let evidence = sqlx::query_as::<_, DisputeEvidence>(
            r#"
            INSERT INTO dispute_evidence
                (dispute_id, uploaded_by, file_url, file_name, content_type, size_bytes, description)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(dispute_id)
        .bind(uploaded_by)
        .bind(&request.file_url)
        .bind(&request.file_name)
        .bind(&request.content_type)
        .bind(request.size_bytes)
        .bind(&request.description)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        sqlx::query(
            r#"
            INSERT INTO dispute_history (dispute_id, actor_id, action, note)
            VALUES ($1, $2, 'evidence_added', $3)
            "#,
        )
        .bind(dispute_id)
        .bind(uploaded_by)
        .bind(format!("added evidence: {}", request.file_name))
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        tx.commit().await.map_err(ApiError::database_error)?;

        Ok(evidence)
    }

    // ─── Assignment ─────────────────────────────────────────────────────────

    pub async fn assign_dispute(
        &self,
        dispute_id: Uuid,
        assigned_to: Uuid,
        actor_id: Uuid,
    ) -> Result<Dispute, ApiError> {
        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let dispute = sqlx::query_as::<_, Dispute>(
            "SELECT * FROM disputes WHERE id = $1 FOR UPDATE",
        )
        .bind(dispute_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("dispute not found"))?;
        if dispute.status().is_terminal() {
            return Err(ApiError::conflict("cannot reassign a closed dispute"));
        }
        if assigned_to == dispute.opened_by {
            return Err(ApiError::bad_request(
                "the case opener cannot be assigned as its reviewer",
            ));
        }
        let assigned_role: Option<String> =
            sqlx::query_scalar("SELECT role FROM users WHERE id = $1")
                .bind(assigned_to)
                .fetch_optional(&mut *tx)
                .await
                .map_err(ApiError::database_error)?;
        if !matches!(assigned_role.as_deref(), Some("moderator" | "admin")) {
            return Err(ApiError::bad_request(
                "cases may only be assigned to moderators or administrators",
            ));
        }

        let updated = sqlx::query_as::<_, Dispute>(
            "UPDATE disputes SET assigned_to = $1 WHERE id = $2 RETURNING *",
        )
        .bind(assigned_to)
        .bind(dispute_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        sqlx::query(
            r#"
            INSERT INTO dispute_history (dispute_id, actor_id, action, note)
            VALUES ($1, $2, 'assigned', $3)
            "#,
        )
        .bind(dispute_id)
        .bind(actor_id)
        .bind(format!("assigned to {assigned_to}"))
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        tx.commit().await.map_err(ApiError::database_error)?;

        Ok(updated)
    }

    // ─── Escalation ─────────────────────────────────────────────────────────

    /// Move a dispute to `under_review` if it's still `open` -- the first
    /// step of actually working a ticket, distinct from escalating it to a
    /// higher tier.
    pub async fn start_review(
        &self,
        dispute_id: Uuid,
        actor_id: Uuid,
    ) -> Result<Dispute, ApiError> {
        self.transition_status(dispute_id, actor_id, TicketStatus::UnderReview, None)
            .await
    }

    /// Escalate a dispute to the next tier (front-line -> senior -> admin).
    /// Escalating a dispute already at the top tier is a no-op that still
    /// records a history entry, so a repeated escalation attempt is visible
    /// in the trail without being treated as an error.
    pub async fn escalate_dispute(
        &self,
        dispute_id: Uuid,
        actor_id: Uuid,
        note: Option<String>,
    ) -> Result<Dispute, ApiError> {
        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let dispute = sqlx::query_as::<_, Dispute>(
            "SELECT * FROM disputes WHERE id = $1 FOR UPDATE",
        )
        .bind(dispute_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("dispute not found"))?;
        if dispute.status().is_terminal() {
            return Err(ApiError::conflict("cannot escalate a closed dispute"));
        }

        let from_level = dispute.escalation();
        let to_level = from_level.next();

        let updated = sqlx::query_as::<_, Dispute>(
            r#"
            UPDATE disputes
            SET escalation_level = $1, status = 'escalated'
            WHERE id = $2
            RETURNING *
            "#,
        )
        .bind(to_level.as_i16())
        .bind(dispute_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        sqlx::query(
            r#"
            INSERT INTO dispute_history
                (dispute_id, actor_id, action, from_status, to_status,
                 from_escalation_level, to_escalation_level, note)
            VALUES ($1, $2, 'escalated', $3, $4, $5, $6, $7)
            "#,
        )
        .bind(dispute_id)
        .bind(actor_id)
        .bind(dispute.status().as_str())
        .bind(TicketStatus::Escalated.as_str())
        .bind(from_level.as_i16())
        .bind(to_level.as_i16())
        .bind(&note)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        tx.commit().await.map_err(ApiError::database_error)?;

        Ok(updated)
    }

    // ─── Resolution ─────────────────────────────────────────────────────────

    /// Resolve a dispute, recording the resolution text, and notify the
    /// person who opened it.
    pub async fn resolve_dispute(
        &self,
        dispute_id: Uuid,
        resolved_by: Uuid,
        resolution: String,
    ) -> Result<Dispute, ApiError> {
        self.finalize(dispute_id, resolved_by, TicketStatus::Resolved, resolution)
            .await
    }

    /// Reject a dispute (closed without the claim being upheld), recording
    /// the reason as the resolution text, and notify the opener.
    pub async fn reject_dispute(
        &self,
        dispute_id: Uuid,
        resolved_by: Uuid,
        reason: String,
    ) -> Result<Dispute, ApiError> {
        self.finalize(dispute_id, resolved_by, TicketStatus::Rejected, reason)
            .await
    }

    async fn finalize(
        &self,
        dispute_id: Uuid,
        resolved_by: Uuid,
        to_status: TicketStatus,
        resolution: String,
    ) -> Result<Dispute, ApiError> {
        if resolution.trim().is_empty() {
            return Err(ApiError::bad_request("resolution must not be empty"));
        }

        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let dispute = sqlx::query_as::<_, Dispute>(
            "SELECT * FROM disputes WHERE id = $1 FOR UPDATE",
        )
        .bind(dispute_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("dispute not found"))?;
        if dispute.status().is_terminal() {
            return Err(ApiError::conflict("dispute is already closed"));
        }
        if dispute.opened_by == resolved_by
            || dispute
                .assigned_to
                .is_some_and(|assigned_to| assigned_to != resolved_by)
        {
            return Err(ApiError::forbidden(
                "Only the assigned reviewer may finalize a case, and the opener cannot review it",
            ));
        }

        let updated = sqlx::query_as::<_, Dispute>(
            r#"
            UPDATE disputes
            SET status = $1, resolution = $2, resolved_by = $3, resolved_at = NOW(),
                decision_version = decision_version + 1
            WHERE id = $4
            RETURNING *
            "#,
        )
        .bind(to_status.as_str())
        .bind(&resolution)
        .bind(resolved_by)
        .bind(dispute_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        let action = if to_status == TicketStatus::Resolved {
            "resolved"
        } else {
            "rejected"
        };

        sqlx::query(
            r#"
            INSERT INTO dispute_history
                (dispute_id, actor_id, action, from_status, to_status, note)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(dispute_id)
        .bind(resolved_by)
        .bind(action)
        .bind(dispute.status().as_str())
        .bind(to_status.as_str())
        .bind(&resolution)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        // Notification on resolution: reuses the existing `notifications`
        // table (see notification_handler.rs) rather than a bespoke
        // dispute-only mechanism, so it shows up wherever a user already
        // checks their notifications.
        let title = if to_status == TicketStatus::Resolved {
            "Your dispute has been resolved"
        } else {
            "Your dispute has been rejected"
        };
        sqlx::query(
            r#"
            INSERT INTO notifications (user_id, type, title, message, link)
            VALUES ($1, 'dispute_resolution', $2, $3, $4)
            "#,
        )
        .bind(dispute.opened_by)
        .bind(title)
        .bind(&resolution)
        .bind(format!("/disputes/{dispute_id}"))
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        tx.commit().await.map_err(ApiError::database_error)?;

        Ok(updated)
    }

    /// Shared status-only transition (used by `start_review`; resolution and
    /// escalation have their own richer transitions above).
    async fn transition_status(
        &self,
        dispute_id: Uuid,
        actor_id: Uuid,
        to_status: TicketStatus,
        note: Option<String>,
    ) -> Result<Dispute, ApiError> {
        let mut tx = self
            .db_pool
            .begin()
            .await
            .map_err(ApiError::database_error)?;
        let dispute = sqlx::query_as::<_, Dispute>(
            "SELECT * FROM disputes WHERE id = $1 FOR UPDATE",
        )
        .bind(dispute_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::database_error)?
        .ok_or_else(|| ApiError::not_found("dispute not found"))?;
        if dispute.status() != TicketStatus::Open {
            return Err(ApiError::conflict(
                "only an open dispute can enter review",
            ));
        }

        let updated = sqlx::query_as::<_, Dispute>(
            "UPDATE disputes SET status = $1 WHERE id = $2 RETURNING *",
        )
        .bind(to_status.as_str())
        .bind(dispute_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        sqlx::query(
            r#"
            INSERT INTO dispute_history
                (dispute_id, actor_id, action, from_status, to_status, note)
            VALUES ($1, $2, 'status_changed', $3, $4, $5)
            "#,
        )
        .bind(dispute_id)
        .bind(actor_id)
        .bind(dispute.status().as_str())
        .bind(to_status.as_str())
        .bind(&note)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::database_error)?;

        tx.commit().await.map_err(ApiError::database_error)?;

        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escalation_level_progresses_and_caps_at_admin() {
        assert_eq!(EscalationLevel::FrontLine.next(), EscalationLevel::Senior);
        assert_eq!(EscalationLevel::Senior.next(), EscalationLevel::Admin);
        assert_eq!(EscalationLevel::Admin.next(), EscalationLevel::Admin);
    }

    #[test]
    fn escalation_level_round_trips_through_i16() {
        for level in [
            EscalationLevel::FrontLine,
            EscalationLevel::Senior,
            EscalationLevel::Admin,
        ] {
            assert_eq!(EscalationLevel::try_from(level.as_i16()).unwrap(), level);
        }
        assert!(EscalationLevel::try_from(0).is_err());
        assert!(EscalationLevel::try_from(4).is_err());
    }

    #[test]
    fn ticket_status_round_trips_through_str() {
        for status in [
            TicketStatus::Open,
            TicketStatus::UnderReview,
            TicketStatus::Escalated,
            TicketStatus::Resolved,
            TicketStatus::Rejected,
            TicketStatus::Closed,
        ] {
            let parsed: TicketStatus = status.as_str().parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn terminal_statuses_are_resolved_rejected_closed_only() {
        assert!(!TicketStatus::Open.is_terminal());
        assert!(!TicketStatus::UnderReview.is_terminal());
        assert!(!TicketStatus::Escalated.is_terminal());
        assert!(TicketStatus::Resolved.is_terminal());
        assert!(TicketStatus::Rejected.is_terminal());
        assert!(TicketStatus::Closed.is_terminal());
    }
}