//! Audit service for sensitive operations (Issue #946)
//!
//! The trail itself — the `audit_logs` table, its hash chain and its
//! append-only rules — was built in Issue #863, and the database writes a row
//! for every change to a sensitive table. That covers *data modifications*.
//!
//! Two things it cannot cover, which this service adds:
//!
//!   1. **Admin actions that are not row changes.** Reading a player's payment
//!      history, exporting a user's data, forcing a match result, impersonating
//!      an account — these leave no `UPDATE` behind, so a trigger never fires.
//!      They are exactly the operations an audit trail exists to record.
//!   2. **Attribution.** A trigger knows which row changed but not who asked
//!      for it; the connection is the same service account either way. The
//!      actor has to be declared by the application, per transaction.
//!
//! It also owns the retention policy: entries are kept for seven years.
//!
//! # Why writes go through one type
//!
//! Every write here sets `source = 'application'`, so the trail distinguishes
//! what the database observed from what the application claimed. Nothing else
//! in the codebase should `INSERT INTO audit_logs` directly.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use tracing::{debug, error, warn};
use uuid::Uuid;

/// How long audit entries are retained before they may be archived.
///
/// Seven years, expressed in days. Calendar arithmetic over that span is the
/// database's job — `retained_until` is computed as an INTERVAL in SQL — but
/// the constant is here so the policy is greppable from the service that
/// enforces it.
pub const RETENTION_YEARS: i64 = 7;
const RETENTION_DAYS: i64 = RETENTION_YEARS * 365;

/// Upper bound on rows returned by one query.
///
/// The trail only grows, so an unbounded read is a way to exhaust the
/// connection pool — accidentally or otherwise.
const MAX_PAGE_SIZE: i64 = 500;
const DEFAULT_PAGE_SIZE: i64 = 50;

#[derive(Error, Debug)]
pub enum AuditError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Invalid audit request: {0}")]
    Invalid(String),
}

/// Actions the service records.
///
/// A closed set rather than a free-form string: an audit trail is only
/// queryable if the same operation is named the same way every time, and
/// `"user_ban"` vs `"ban_user"` vs `"banned"` is how that stops being true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// Privileged read of another user's data.
    AdminView,
    /// Bulk export of personal or financial data.
    AdminExport,
    /// Role, permission or feature-flag change.
    AdminGrant,
    AdminRevoke,
    /// Account state changes made on a user's behalf.
    AdminBan,
    AdminUnban,
    /// Acting as another user.
    AdminImpersonate,
    /// Manual override of a system-computed outcome (match result, payout).
    AdminOverride,
    /// Configuration change to the platform itself.
    AdminConfigChange,
    /// Explicit application-level record of a data modification, for paths
    /// where the change does not land in an audited table.
    DataModification,
    /// A participant was assigned to an experiment cohort (Issue #255).
    ///
    /// Records the assignment version and cohort, never the exposure — that
    /// is a separate action logged at presentation time.
    ExperimentAssignment,
    /// A participant was presented with the assigned experience (Issue #255).
    ///
    /// Logged at presentation, not assignment, so the trail distinguishes
    /// "was bucketed" from "actually saw it".
    ExperimentExposure,
    /// An experiment was stopped by a guardrail breach (Issue #256).
    ///
    /// Records which guardrail tripped, the observed value and the threshold
    /// that was defined before the experiment started.
    ExperimentGuardrailStop,
    /// A leaderboard snapshot was generated (Issue #257).
    ///
    /// Records the snapshot version and generation time so a served page can
    /// be traced back to the exact ranking it was built from.
    LeaderboardSnapshotGenerated,
    /// A privacy deletion job was requested (Issue #258).
    PrivacyDeletionRequested,
    /// A dependent system acknowledged (or failed) a deletion (Issue #258).
    ///
    /// One entry per system per attempt; the payload carries the system name
    /// and retry status but never the deleted values.
    PrivacyDeletionAck,
    /// A privacy deletion job was retried after a failed acknowledgement.
    PrivacyDeletionRetry,
    /// A privacy deletion job completed across all required systems.
    PrivacyDeletionCompleted,
    /// A documented legal-retention exception was applied to a deletion.
    PrivacyDeletionRetentionException,
}

impl AuditAction {
    /// Stored form. Kept under the `action VARCHAR(50)` the table declares.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdminView => "admin_view",
            Self::AdminExport => "admin_export",
            Self::AdminGrant => "admin_grant",
            Self::AdminRevoke => "admin_revoke",
            Self::AdminBan => "admin_ban",
            Self::AdminUnban => "admin_unban",
            Self::AdminImpersonate => "admin_impersonate",
            Self::AdminOverride => "admin_override",
            Self::AdminConfigChange => "admin_config_change",
            Self::DataModification => "data_modification",
            Self::ExperimentAssignment => "experiment_assignment",
            Self::ExperimentExposure => "experiment_exposure",
            Self::ExperimentGuardrailStop => "experiment_guardrail_stop",
            Self::LeaderboardSnapshotGenerated => "leaderboard_snapshot_generated",
            Self::PrivacyDeletionRequested => "privacy_deletion_requested",
            Self::PrivacyDeletionAck => "privacy_deletion_ack",
            Self::PrivacyDeletionRetry => "privacy_deletion_retry",
            Self::PrivacyDeletionCompleted => "privacy_deletion_completed",
            Self::PrivacyDeletionRetentionException => "privacy_deletion_retention_exception",
        }
    }

    /// Whether entries of this action may carry personal data that must be
    /// redacted once the subject's deletion completes (Issue #258).
    ///
    /// The privacy-deletion lifecycle actions are deliberately excluded: they
    /// are the evidence that the deletion happened, and they are written with
    /// identifiers already stripped (see [`redact_deleted_identifiers`]).
    pub fn carries_subject_data(self) -> bool {
        !matches!(
            self,
            Self::PrivacyDeletionRequested
                | Self::PrivacyDeletionAck
                | Self::PrivacyDeletionRetry
                | Self::PrivacyDeletionCompleted
                | Self::PrivacyDeletionRetentionException
        )
    }
}

/// Placeholder written in place of an identifier belonging to a deleted user.
pub const REDACTED_PLACEHOLDER: &str = "[redacted]";

/// Keys whose values are identifiers of the audited subject and must not
/// survive a privacy deletion (Issue #258).
///
/// Matched case-insensitively against object keys at any depth. The list is
/// intentionally about *identity*, not about every field: an audit entry that
/// says "a ban was lifted" is still useful after the subject is gone, but one
/// that names their email address is a leak.
const REDACTABLE_KEYS: &[&str] = &[
    "email",
    "email_address",
    "username",
    "display_name",
    "phone",
    "phone_number",
    "ip_address",
    "user_agent",
    "full_name",
    "address",
    "date_of_birth",
    "external_id",
    "stripe_customer_id",
    "device_id",
];

/// Recursively replace values under identity-bearing keys with a placeholder.
///
/// Used when a privacy deletion completes: the audit trail must remain
/// verifiable and queryable, but must no longer contain the deleted subject's
/// identifiers. Returns the redacted value and the number of fields replaced
/// so the caller can record that redaction happened without logging the
/// values themselves.
pub fn redact_deleted_identifiers(value: &JsonValue) -> (JsonValue, usize) {
    fn walk(value: &JsonValue, redacted: &mut usize) -> JsonValue {
        match value {
            JsonValue::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (key, child) in map {
                    let is_identity = REDACTABLE_KEYS
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(key));
                    if is_identity && !child.is_null() {
                        *redacted += 1;
                        out.insert(
                            key.clone(),
                            JsonValue::String(REDACTED_PLACEHOLDER.to_string()),
                        );
                    } else {
                        out.insert(key.clone(), walk(child, redacted));
                    }
                }
                JsonValue::Object(out)
            }
            JsonValue::Array(items) => {
                JsonValue::Array(items.iter().map(|item| walk(item, redacted)).collect())
            }
            other => other.clone(),
        }
    }

    let mut redacted = 0usize;
    let out = walk(value, &mut redacted);
    (out, redacted)
}

/// One entry to be written.
#[derive(Debug, Clone)]
pub struct AuditEntryInput {
    /// The administrator or system account performing the action.
    pub actor_id: Option<Uuid>,
    pub action: AuditAction,
    /// Table or domain object the action touched, e.g. `"users"`.
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    /// State before the change, where there was one.
    pub old_values: Option<JsonValue>,
    /// State after the change, or the parameters of a read/export.
    pub new_values: Option<JsonValue>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

impl AuditEntryInput {
    pub fn new(action: AuditAction, resource_type: impl Into<String>) -> Self {
        Self {
            actor_id: None,
            action,
            resource_type: resource_type.into(),
            resource_id: None,
            old_values: None,
            new_values: None,
            ip_address: None,
            user_agent: None,
        }
    }

    pub fn actor(mut self, actor_id: Uuid) -> Self {
        self.actor_id = Some(actor_id);
        self
    }

    pub fn resource(mut self, resource_id: Uuid) -> Self {
        self.resource_id = Some(resource_id);
        self
    }

    pub fn before(mut self, old_values: JsonValue) -> Self {
        self.old_values = Some(old_values);
        self
    }

    pub fn after(mut self, new_values: JsonValue) -> Self {
        self.new_values = Some(new_values);
        self
    }

    pub fn request_context(
        mut self,
        ip_address: Option<String>,
        user_agent: Option<String>,
    ) -> Self {
        self.ip_address = ip_address;
        self.user_agent = user_agent;
        self
    }
}

/// A stored entry.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AuditRecord {
    pub id: Uuid,
    pub sequence_number: i64,
    pub user_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub old_values: Option<JsonValue>,
    pub new_values: Option<JsonValue>,
    pub source: String,
    pub entry_hash: Option<String>,
    pub previous_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Filters for reading the trail.
///
/// All optional and all narrowing: an investigation starts from one known fact
/// — a user, a resource, a date — and widens from there.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuditFilter {
    pub user_id: Option<Uuid>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<Uuid>,
    pub source: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Outcome of a chain verification.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChainStatus {
    pub ok: bool,
    pub checked_rows: i64,
    pub first_bad_sequence: Option<i64>,
    pub reason: String,
}

/// Result of an archive sweep.
#[derive(Debug, Clone, Serialize)]
pub struct RetentionSweep {
    pub archived: i64,
    pub cutoff: DateTime<Utc>,
}

#[derive(Clone)]
pub struct AuditService {
    pool: PgPool,
}

impl AuditService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ───────────────────────────────────────────
    // Writing
    // ───────────────────────────────────────────

    /// Record an admin action.
    ///
    /// # Failure handling
    ///
    /// Returns the error rather than swallowing it. Callers doing something
    /// irreversible should write the entry *before* the action and treat a
    /// failure as a reason not to proceed: an action that happened with no
    /// record of it is worse than an action that did not happen.
    pub async fn log(&self, entry: AuditEntryInput) -> Result<Uuid, AuditError> {
        if entry.resource_type.trim().is_empty() {
            return Err(AuditError::Invalid("resource_type is required".into()));
        }

        let id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO audit_logs (
                user_id, action, resource_type, resource_id,
                old_values, new_values, ip_address, user_agent,
                source, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7::inet, $8, 'application', NOW())
            RETURNING id
            "#,
        )
        .bind(entry.actor_id)
        .bind(entry.action.as_str())
        .bind(&entry.resource_type)
        .bind(entry.resource_id)
        .bind(&entry.old_values)
        .bind(&entry.new_values)
        .bind(entry.ip_address.as_deref())
        .bind(entry.user_agent.as_deref())
        .fetch_one(&self.pool)
        .await
        .inspect_err(|e| {
            error!(
                action = entry.action.as_str(),
                resource_type = %entry.resource_type,
                "failed to write audit entry: {e}"
            );
        })?;

        debug!(
            audit_id = %id,
            action = entry.action.as_str(),
            "audit entry recorded"
        );

        Ok(id)
    }

    /// Record an entry inside a caller's transaction.
    ///
    /// Use this when the audit entry and the change it describes must either
    /// both land or neither does — a committed change with no entry is a gap
    /// in the trail, and an entry for a rolled-back change is a false record.
    pub async fn log_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        entry: AuditEntryInput,
    ) -> Result<Uuid, AuditError> {
        let id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO audit_logs (
                user_id, action, resource_type, resource_id,
                old_values, new_values, ip_address, user_agent,
                source, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7::inet, $8, 'application', NOW())
            RETURNING id
            "#,
        )
        .bind(entry.actor_id)
        .bind(entry.action.as_str())
        .bind(&entry.resource_type)
        .bind(entry.resource_id)
        .bind(&entry.old_values)
        .bind(&entry.new_values)
        .bind(entry.ip_address.as_deref())
        .bind(entry.user_agent.as_deref())
        .fetch_one(&mut **tx)
        .await?;

        Ok(id)
    }

    /// Declare who the current transaction is acting as.
    ///
    /// The `audit_row_change` trigger reads `audit.actor_id` and falls back to
    /// the row's own `user_id` when it is unset — which attributes an admin's
    /// edit to the user whose row was edited. Calling this at the start of a
    /// privileged transaction is what makes trigger-written rows say who
    /// actually did it.
    ///
    /// `set_config(..., true)` scopes the value to the transaction, so it
    /// cannot leak to the next request that borrows the same pooled connection.
    pub async fn set_transaction_actor(
        tx: &mut Transaction<'_, Postgres>,
        actor_id: Uuid,
    ) -> Result<(), AuditError> {
        sqlx::query("SELECT set_config('audit.actor_id', $1, true)")
            .bind(actor_id.to_string())
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    // ───────────────────────────────────────────
    // Reading
    // ───────────────────────────────────────────

    /// Query the trail by user, date range and/or resource.
    ///
    /// The filters are bound as parameters against one prepared statement
    /// rather than concatenated per combination: fewer cached plans, and no
    /// string building anywhere near an admin-privileged query.
    pub async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditRecord>, AuditError> {
        if let (Some(from), Some(to)) = (filter.from, filter.to) {
            if from > to {
                return Err(AuditError::Invalid("`from` is after `to`".into()));
            }
        }

        let limit = filter
            .limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let offset = filter.offset.unwrap_or(0).max(0);

        let records = sqlx::query_as::<_, AuditRecord>(
            r#"
            SELECT id, sequence_number, user_id, action, resource_type, resource_id,
                   old_values, new_values, source, entry_hash, previous_hash, created_at
            FROM audit_logs
            WHERE ($1::uuid IS NULL OR user_id = $1)
              AND ($2::text IS NULL OR action = $2)
              AND ($3::text IS NULL OR resource_type = $3)
              AND ($4::uuid IS NULL OR resource_id = $4)
              AND ($5::text IS NULL OR source = $5)
              AND ($6::timestamptz IS NULL OR created_at >= $6)
              AND ($7::timestamptz IS NULL OR created_at <= $7)
            ORDER BY sequence_number DESC
            LIMIT $8 OFFSET $9
            "#,
        )
        .bind(filter.user_id)
        .bind(filter.action.as_deref())
        .bind(filter.resource_type.as_deref())
        .bind(filter.resource_id)
        .bind(filter.source.as_deref())
        .bind(filter.from)
        .bind(filter.to)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Count matching entries, for paging.
    pub async fn count(&self, filter: &AuditFilter) -> Result<i64, AuditError> {
        let total: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM audit_logs
            WHERE ($1::uuid IS NULL OR user_id = $1)
              AND ($2::text IS NULL OR action = $2)
              AND ($3::text IS NULL OR resource_type = $3)
              AND ($4::uuid IS NULL OR resource_id = $4)
              AND ($5::text IS NULL OR source = $5)
              AND ($6::timestamptz IS NULL OR created_at >= $6)
              AND ($7::timestamptz IS NULL OR created_at <= $7)
            "#,
        )
        .bind(filter.user_id)
        .bind(filter.action.as_deref())
        .bind(filter.resource_type.as_deref())
        .bind(filter.resource_id)
        .bind(filter.source.as_deref())
        .bind(filter.from)
        .bind(filter.to)
        .fetch_one(&self.pool)
        .await?;

        Ok(total)
    }

    /// Full history for one resource, oldest first — the order the changes
    /// actually happened in, which is how a reviewer reads a timeline.
    pub async fn resource_history(
        &self,
        resource_type: &str,
        resource_id: Uuid,
    ) -> Result<Vec<AuditRecord>, AuditError> {
        let records = sqlx::query_as::<_, AuditRecord>(
            r#"
            SELECT id, sequence_number, user_id, action, resource_type, resource_id,
                   old_values, new_values, source, entry_hash, previous_hash, created_at
            FROM audit_logs
            WHERE resource_type = $1 AND resource_id = $2
            ORDER BY sequence_number ASC
            LIMIT $3
            "#,
        )
        .bind(resource_type)
        .bind(resource_id)
        .bind(MAX_PAGE_SIZE)
        .fetch_all(&self.pool)
        .await?;

        Ok(records)
    }

    /// Everything one actor did in a window.
    pub async fn actor_activity(
        &self,
        actor_id: Uuid,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<AuditRecord>, AuditError> {
        self.query(&AuditFilter {
            user_id: Some(actor_id),
            from: Some(from),
            to: Some(to),
            limit: Some(MAX_PAGE_SIZE),
            ..Default::default()
        })
        .await
    }

    // ───────────────────────────────────────────
    // Integrity and retention
    // ───────────────────────────────────────────

    /// Recompute the hash chain and report the first break.
    ///
    /// Worth running on a schedule rather than only on demand: the value of a
    /// tamper-evident log is in noticing, and nobody notices a chain they
    /// never check.
    pub async fn verify_chain(
        &self,
        from_sequence: Option<i64>,
        to_sequence: Option<i64>,
    ) -> Result<ChainStatus, AuditError> {
        let status = sqlx::query_as::<_, ChainStatus>(
            "SELECT ok, checked_rows, first_bad_sequence, reason
             FROM verify_audit_chain($1, $2)",
        )
        .bind(from_sequence.unwrap_or(0))
        .bind(to_sequence)
        .fetch_one(&self.pool)
        .await?;

        if !status.ok {
            error!(
                first_bad_sequence = ?status.first_bad_sequence,
                "audit chain verification failed: {}",
                status.reason
            );
        }

        Ok(status)
    }

    /// The date before which entries are past their retention period.
    pub fn retention_cutoff() -> DateTime<Utc> {
        Utc::now() - Duration::days(RETENTION_DAYS)
    }

    /// Move entries past their seven-year retention into the archive table.
    ///
    /// Not a delete. `audit_logs` carries rules rejecting UPDATE and DELETE,
    /// and the hash chain is only verifiable while the rows it covers are
    /// contiguous — removing the oldest entries would break verification of
    /// everything after them. The archive keeps the rows, and their hashes,
    /// outside the hot table.
    ///
    /// Safe to call repeatedly; already-archived entries are skipped.
    pub async fn archive_past_retention(&self) -> Result<RetentionSweep, AuditError> {
        let cutoff = Self::retention_cutoff();

        let archived: i64 = sqlx::query_scalar("SELECT archive_audit_logs_past_retention($1)")
            .bind(cutoff)
            .fetch_one(&self.pool)
            .await?;

        if archived > 0 {
            warn!(
                archived,
                cutoff = %cutoff,
                "archived audit entries past the {RETENTION_YEARS}-year retention period"
            );
        }

        Ok(RetentionSweep { archived, cutoff })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_names_fit_the_column_and_are_stable() {
        let actions = [
            AuditAction::AdminView,
            AuditAction::AdminExport,
            AuditAction::AdminGrant,
            AuditAction::AdminRevoke,
            AuditAction::AdminBan,
            AuditAction::AdminUnban,
            AuditAction::AdminImpersonate,
            AuditAction::AdminOverride,
            AuditAction::AdminConfigChange,
            AuditAction::DataModification,
            AuditAction::ExperimentAssignment,
            AuditAction::ExperimentExposure,
            AuditAction::ExperimentGuardrailStop,
            AuditAction::LeaderboardSnapshotGenerated,
            AuditAction::PrivacyDeletionRequested,
            AuditAction::PrivacyDeletionAck,
            AuditAction::PrivacyDeletionRetry,
            AuditAction::PrivacyDeletionCompleted,
            AuditAction::PrivacyDeletionRetentionException,
        ];

        for action in actions {
            // `action` is VARCHAR(50); an overlong name would be rejected at
            // INSERT, which is the worst moment to find out.
            assert!(action.as_str().len() <= 50);
            assert!(!action.as_str().is_empty());
        }
    }

    #[test]
    fn builder_carries_every_field() {
        let actor = Uuid::new_v4();
        let resource = Uuid::new_v4();

        let entry = AuditEntryInput::new(AuditAction::AdminBan, "users")
            .actor(actor)
            .resource(resource)
            .before(serde_json::json!({ "banned": false }))
            .after(serde_json::json!({ "banned": true }))
            .request_context(Some("203.0.113.7".into()), Some("curl/8".into()));

        assert_eq!(entry.actor_id, Some(actor));
        assert_eq!(entry.resource_id, Some(resource));
        assert_eq!(entry.action.as_str(), "admin_ban");
        assert_eq!(entry.old_values, Some(serde_json::json!({"banned": false})));
        assert_eq!(entry.new_values, Some(serde_json::json!({"banned": true})));
        assert_eq!(entry.ip_address.as_deref(), Some("203.0.113.7"));
    }

    #[test]
    fn retention_cutoff_is_seven_years_back() {
        let cutoff = AuditService::retention_cutoff();
        let years_back = (Utc::now() - cutoff).num_days() / 365;
        assert_eq!(years_back, RETENTION_YEARS);
    }

    #[test]
    fn privacy_lifecycle_actions_do_not_carry_subject_data() {
        assert!(!AuditAction::PrivacyDeletionRequested.carries_subject_data());
        assert!(!AuditAction::PrivacyDeletionAck.carries_subject_data());
        assert!(!AuditAction::PrivacyDeletionRetry.carries_subject_data());
        assert!(!AuditAction::PrivacyDeletionCompleted.carries_subject_data());
        assert!(!AuditAction::PrivacyDeletionRetentionException.carries_subject_data());

        // Ordinary actions may carry identifiers and are therefore subject to
        // redaction once a deletion completes.
        assert!(AuditAction::AdminView.carries_subject_data());
        assert!(AuditAction::ExperimentExposure.carries_subject_data());
    }

    #[test]
    fn redaction_strips_identity_keys_at_any_depth() {
        let value = serde_json::json!({
            "email": "player@example.com",
            "nested": {
                "Username": "player_one",
                "ip_address": "203.0.113.7",
                "cohort": "treatment",
            },
            "list": [
                { "phone": "+15550100" },
                { "note": "kept" },
            ],
            "count": 3,
        });

        let (redacted, count) = redact_deleted_identifiers(&value);

        assert_eq!(count, 4);
        assert_eq!(redacted["email"], REDACTED_PLACEHOLDER);
        assert_eq!(redacted["nested"]["Username"], REDACTED_PLACEHOLDER);
        assert_eq!(redacted["nested"]["ip_address"], REDACTED_PLACEHOLDER);
        assert_eq!(redacted["list"][0]["phone"], REDACTED_PLACEHOLDER);
        // Non-identity fields survive so the trail stays useful.
        assert_eq!(redacted["nested"]["cohort"], "treatment");
        assert_eq!(redacted["list"][1]["note"], "kept");
        assert_eq!(redacted["count"], 3);
    }

    #[test]
    fn redaction_leaves_null_identifiers_alone() {
        let value = serde_json::json!({ "email": null, "username": "x" });
        let (redacted, count) = redact_deleted_identifiers(&value);
        assert_eq!(count, 1);
        assert!(redacted["email"].is_null());
        assert_eq!(redacted["username"], REDACTED_PLACEHOLDER);
    }
}
