//! Append-only, tamper-evident audit log for privileged operator actions.
//!
//! Records are chained by hash so any mutation of history is detectable, and
//! sensitive fields (credentials, private evidence) are redacted before storage.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Roles allowed to perform privileged operator actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRole {
    Admin,
    Moderator,
    Support,
}

impl OperatorRole {
    /// Whether this role may perform the given sensitive action.
    pub fn can_perform(&self, action: &SensitiveAction) -> bool {
        match self {
            OperatorRole::Admin => true,
            OperatorRole::Moderator => matches!(
                action,
                SensitiveAction::Moderation | SensitiveAction::DisputeResolution
            ),
            // Support is read-only: may search the audit log but never act.
            OperatorRole::Support => false,
        }
    }

    /// Whether this role may read/search the audit log.
    pub fn can_read_audit(&self) -> bool {
        matches!(self, OperatorRole::Admin | OperatorRole::Moderator | OperatorRole::Support)
    }
}

/// Categories of privileged actions that must be attributable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitiveAction {
    Moderation,
    DisputeResolution,
    AccountSuspension,
    FeatureFlag,
    Payout,
}

impl SensitiveAction {
    /// Policy: these actions always require a human-supplied reason.
    pub fn requires_reason(&self) -> bool {
        matches!(
            self,
            SensitiveAction::AccountSuspension | SensitiveAction::Payout | SensitiveAction::DisputeResolution
        )
    }
}

/// Authenticated operator identity attached to every record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub role: OperatorRole,
}

/// A request to record a privileged action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRequest {
    pub actor: Actor,
    pub action: SensitiveAction,
    pub target: String,
    pub reason: Option<String>,
    #[serde(default)]
    pub before: Option<serde_json::Value>,
    #[serde(default)]
    pub after: Option<serde_json::Value>,
}

/// Errors surfaced when an action is denied or the log is tampered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditError {
    Unauthenticated,
    Forbidden,
    ReasonRequired,
    IntegrityViolation { index: usize },
}

/// Why a privileged operator session was rejected at the submission boundary.
///
/// These reasons are recorded in the audit log so that rejected requests remain
/// attributable and tamper-evident, even though the action never executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRejectionReason {
    /// The session had expired by the time the action reached submission.
    Expired,
    /// The session was revoked (possibly while the action was queued).
    Revoked,
    /// The session's scope does not cover the requested action/target.
    WrongScope,
    /// The session's bound actor does not match the requesting actor.
    ActorMismatch,
    /// The session's purpose does not permit the requested action.
    WrongPurpose,
    /// The session is missing a required approval reference.
    MissingApproval,
    /// The session could not be found in the session store.
    UnknownSession,
}

impl SessionRejectionReason {
    /// Stable machine-readable code for audit and API responses.
    pub fn code(&self) -> &'static str {
        match self {
            SessionRejectionReason::Expired => "session_expired",
            SessionRejectionReason::Revoked => "session_revoked",
            SessionRejectionReason::WrongScope => "session_wrong_scope",
            SessionRejectionReason::ActorMismatch => "session_actor_mismatch",
            SessionRejectionReason::WrongPurpose => "session_wrong_purpose",
            SessionRejectionReason::MissingApproval => "session_missing_approval",
            SessionRejectionReason::UnknownSession => "session_unknown",
        }
    }
}

/// A stored, immutable record of a rejected privileged request.
///
/// Rejected requests are chained into the same hash chain as accepted actions so
/// that attempts to act outside a valid session are themselves auditable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedActionRecord {
    pub seq: u64,
    pub actor_id: String,
    pub actor_role: OperatorRole,
    pub action: SensitiveAction,
    pub target: String,
    /// Session identifier the request claimed to act under, if any.
    pub session_id: Option<String>,
    pub reason: SessionRejectionReason,
    pub prev_hash: String,
    pub hash: String,
}

/// A stored, immutable audit record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub seq: u64,
    pub actor_id: String,
    pub actor_role: OperatorRole,
    pub action: SensitiveAction,
    pub target: String,
    pub reason: Option<String>,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub prev_hash: String,
    pub hash: String,
}

/// Append-only audit log backed by a hash chain.
#[derive(Debug, Default)]
pub struct AuditLog {
    records: Vec<AuditRecord>,
    rejections: Vec<RejectedActionRecord>,
}

const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Keys whose values must never be persisted in the audit log.
const REDACTED_KEYS: &[&str] = &[
    "password",
    "password_hash",
    "token",
    "access_token",
    "refresh_token",
    "secret",
    "api_key",
    "credential",
    "private_evidence",
    "evidence",
    "ssn",
    "card_number",
];

fn redact_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                if REDACTED_KEYS.iter().any(|r| k.eq_ignore_ascii_case(r)) {
                    out.insert(k.clone(), serde_json::Value::String("[REDACTED]".into()));
                } else {
                    out.insert(k.clone(), redact_value(v));
                }
            }
            serde_json::Value::Object(out.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact_value).collect())
        }
        other => other.clone(),
    }
}

fn compute_hash(record: &AuditRecord) -> String {
    let mut hasher = Sha256::new();
    hasher.update(record.seq.to_le_bytes());
    hasher.update(record.actor_id.as_bytes());
    hasher.update(format!("{:?}", record.actor_role).as_bytes());
    hasher.update(format!("{:?}", record.action).as_bytes());
    hasher.update(record.target.as_bytes());
    hasher.update(record.reason.as_deref().unwrap_or("").as_bytes());
    hasher.update(record.before.as_ref().map(|v| v.to_string()).unwrap_or_default().as_bytes());
    hasher.update(record.after.as_ref().map(|v| v.to_string()).unwrap_or_default().as_bytes());
    hasher.update(record.prev_hash.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn compute_rejection_hash(record: &RejectedActionRecord) -> String {
    let mut hasher = Sha256::new();
    // Domain-separate rejection hashes from accepted-action hashes.
    hasher.update(b"rejected_action");
    hasher.update(record.seq.to_le_bytes());
    hasher.update(record.actor_id.as_bytes());
    hasher.update(format!("{:?}", record.actor_role).as_bytes());
    hasher.update(format!("{:?}", record.action).as_bytes());
    hasher.update(record.target.as_bytes());
    hasher.update(record.session_id.as_deref().unwrap_or("").as_bytes());
    hasher.update(record.reason.code().as_bytes());
    hasher.update(record.prev_hash.as_bytes());
    format!("{:x}", hasher.finalize())
}

impl AuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a privileged action, enforcing role and reason policy.
    pub fn record(&mut self, req: AuditRequest) -> Result<&AuditRecord, AuditError> {
        if req.actor.id.trim().is_empty() {
            return Err(AuditError::Unauthenticated);
        }
        if !req.actor.role.can_perform(&req.action) {
            return Err(AuditError::Forbidden);
        }
        let reason = req.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
        if req.action.requires_reason() && reason.is_none() {
            return Err(AuditError::ReasonRequired);
        }

        let seq = self.records.len() as u64;
        let prev_hash = self
            .records
            .last()
            .map(|r| r.hash.clone())
            .unwrap_or_else(|| GENESIS_HASH.to_string());

        let mut record = AuditRecord {
            seq,
            actor_id: req.actor.id,
            actor_role: req.actor.role,
            action: req.action,
            target: req.target,
            reason: reason.map(str::to_string),
            before: req.before.as_ref().map(redact_value),
            after: req.after.as_ref().map(redact_value),
            prev_hash,
            hash: String::new(),
        };
        record.hash = compute_hash(&record);
        self.records.push(record);
        Ok(self.records.last().expect("just pushed"))
    }

    /// Record a rejected privileged request so the denial is attributable.
    ///
    /// Rejections share the hash chain with accepted actions: the `prev_hash`
    /// links to the most recent entry of either kind, so removing or reordering
    /// a rejection breaks [`AuditLog::verify_integrity`].
    pub fn record_rejection(
        &mut self,
        actor: &Actor,
        action: SensitiveAction,
        target: &str,
        session_id: Option<&str>,
        reason: SessionRejectionReason,
    ) -> Result<&RejectedActionRecord, AuditError> {
        if actor.id.trim().is_empty() {
            return Err(AuditError::Unauthenticated);
        }

        let seq = (self.records.len() + self.rejections.len()) as u64;
        let prev_hash = self
            .rejections
            .last()
            .map(|r| r.hash.clone())
            .or_else(|| self.records.last().map(|r| r.hash.clone()))
            .unwrap_or_else(|| GENESIS_HASH.to_string());

        let mut record = RejectedActionRecord {
            seq,
            actor_id: actor.id.clone(),
            actor_role: actor.role,
            action,
            target: target.to_string(),
            session_id: session_id.map(str::to_string),
            reason,
            prev_hash,
            hash: String::new(),
        };
        record.hash = compute_rejection_hash(&record);
        self.rejections.push(record);
        Ok(self.rejections.last().expect("just pushed"))
    }

    /// Read-only search over the audit log. Never mutates records.
    pub fn search(&self, actor: &Actor, query: &str) -> Result<Vec<&AuditRecord>, AuditError> {
        if actor.id.trim().is_empty() {
            return Err(AuditError::Unauthenticated);
        }
        if !actor.role.can_read_audit() {
            return Err(AuditError::Forbidden);
        }
        let q = query.to_lowercase();
        Ok(self
            .records
            .iter()
            .filter(|r| {
                q.is_empty()
                    || r.actor_id.to_lowercase().contains(&q)
                    || r.target.to_lowercase().contains(&q)
                    || r.reason.as_deref().map(|s| s.to_lowercase().contains(&q)).unwrap_or(false)
            })
            .collect())
    }

    /// Verify the hash chain is intact; detects tampering of any record.
    ///
    /// Accepted actions and rejections form a single interleaved chain ordered
    /// by `seq`, so neither kind can be silently dropped or reordered.
    pub fn verify_integrity(&self) -> Result<(), AuditError> {
        let mut expected_prev = GENESIS_HASH.to_string();
        let mut next_seq: u64 = 0;

        let mut actions = self.records.iter().peekable();
        let mut rejections = self.rejections.iter().peekable();

        loop {
            let take_action = match (actions.peek(), rejections.peek()) {
                (Some(a), Some(r)) => a.seq <= r.seq,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };

            let index = next_seq as usize;
            if take_action {
                let record = actions.next().expect("peeked");
                if record.seq != next_seq
                    || record.prev_hash != expected_prev
                    || compute_hash(record) != record.hash
                {
                    return Err(AuditError::IntegrityViolation { index });
                }
                expected_prev = record.hash.clone();
            } else {
                let record = rejections.next().expect("peeked");
                if record.seq != next_seq
                    || record.prev_hash != expected_prev
                    || compute_rejection_hash(record) != record.hash
                {
                    return Err(AuditError::IntegrityViolation { index });
                }
                expected_prev = record.hash.clone();
            }
            next_seq += 1;
        }

        Ok(())
    }

    pub fn records(&self) -> &[AuditRecord] {
        &self.records
    }

    /// Immutable view of rejected privileged requests.
    pub fn rejections(&self) -> &[RejectedActionRecord] {
        &self.rejections
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(role: OperatorRole) -> Actor {
        Actor { id: "op-1".into(), role }
    }

    fn req(role: OperatorRole, action: SensitiveAction, reason: Option<&str>) -> AuditRequest {
        AuditRequest {
            actor: actor(role),
            action,
            target: "user-42".into(),
            reason: reason.map(str::to_string),
            before: None,
            after: None,
        }
    }

    #[test]
    fn denies_support_actions() {
        let mut log = AuditLog::new();
        assert_eq!(
            log.record(req(OperatorRole::Support, SensitiveAction::Moderation, Some("x"))),
            Err(AuditError::Forbidden)
        );
    }

    #[test]
    fn requires_reason_for_suspension() {
        let mut log = AuditLog::new();
        assert_eq!(
            log.record(req(OperatorRole::Admin, SensitiveAction::AccountSuspension, None)),
            Err(AuditError::ReasonRequired)
        );
    }

    #[test]
    fn records_and_redacts() {
        let mut log = AuditLog::new();
        let mut r = req(OperatorRole::Admin, SensitiveAction::Payout, Some("refund"));
        r.after = Some(serde_json::json!({ "amount": 10, "token": "secret" }));
        let rec = log.record(r).unwrap().clone();
        assert_eq!(rec.after.unwrap()["token"], "[REDACTED]");
        assert!(log.verify_integrity().is_ok());
    }

    #[test]
    fn detects_tampering() {
        let mut log = AuditLog::new();
        log.record(req(OperatorRole::Admin, SensitiveAction::FeatureFlag, None)).unwrap();
        log.records[0].target = "tampered".into();
        assert_eq!(log.verify_integrity(), Err(AuditError::IntegrityViolation { index: 0 }));
    }

    #[test]
    fn support_can_search() {
        let mut log = AuditLog::new();
        log.record(req(OperatorRole::Admin, SensitiveAction::Moderation, Some("spam"))).unwrap();
        let hits = log.search(&actor(OperatorRole::Support), "spam").unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn records_rejection_with_reason_and_session() {
        let mut log = AuditLog::new();
        let rec = log
            .record_rejection(
                &actor(OperatorRole::Admin),
                SensitiveAction::Payout,
                "user-42",
                Some("sess-1"),
                SessionRejectionReason::Expired,
            )
            .unwrap()
            .clone();
        assert_eq!(rec.reason, SessionRejectionReason::Expired);
        assert_eq!(rec.session_id.as_deref(), Some("sess-1"));
        assert_eq!(rec.seq, 0);
        assert!(log.verify_integrity().is_ok());
    }

    #[test]
    fn rejection_requires_actor() {
        let mut log = AuditLog::new();
        let anon = Actor { id: "  ".into(), role: OperatorRole::Admin };
        assert_eq!(
            log.record_rejection(
                &anon,
                SensitiveAction::Moderation,
                "user-42",
                None,
                SessionRejectionReason::Revoked,
            ),
            Err(AuditError::Unauthenticated)
        );
        assert!(log.rejections().is_empty());
    }

    #[test]
    fn interleaved_chain_detects_tampering_of_rejection() {
        let mut log = AuditLog::new();
        log.record(req(OperatorRole::Admin, SensitiveAction::FeatureFlag, None)).unwrap();
        log.record_rejection(
            &actor(OperatorRole::Admin),
            SensitiveAction::Payout,
            "user-42",
            Some("sess-1"),
            SessionRejectionReason::Revoked,
        )
        .unwrap();
        log.record(req(OperatorRole::Admin, SensitiveAction::Moderation, Some("ok"))).unwrap();
        assert!(log.verify_integrity().is_ok());

        // Mutating a rejection's reason must break the chain.
        log.rejections[0].reason = SessionRejectionReason::WrongScope;
        assert!(log.verify_integrity().is_err());
    }

    #[test]
    fn dropped_rejection_breaks_chain() {
        let mut log = AuditLog::new();
        log.record_rejection(
            &actor(OperatorRole::Admin),
            SensitiveAction::Payout,
            "user-42",
            None,
            SessionRejectionReason::WrongScope,
        )
        .unwrap();
        log.record(req(OperatorRole::Admin, SensitiveAction::Moderation, Some("ok"))).unwrap();
        assert!(log.verify_integrity().is_ok());

        log.rejections.clear();
        assert!(log.verify_integrity().is_err());
    }

    #[test]
    fn rejection_reasons_have_distinct_codes() {
        let reasons = [
            SessionRejectionReason::Expired,
            SessionRejectionReason::Revoked,
            SessionRejectionReason::WrongScope,
            SessionRejectionReason::ActorMismatch,
            SessionRejectionReason::WrongPurpose,
            SessionRejectionReason::MissingApproval,
            SessionRejectionReason::UnknownSession,
        ];
        let mut codes: Vec<&str> = reasons.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), reasons.len());
    }
}
