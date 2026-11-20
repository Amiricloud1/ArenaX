use crate::auth::jwt_service::Claims;
use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    error::ErrorForbidden,
    Error, HttpMessage,
};
use chrono::{DateTime, Utc};
use futures::future::LocalBoxFuture;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    future::{ready, Ready},
    sync::{Arc, RwLock},
};
use tracing::{info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 0. Privileged Operator Sessions
// ---------------------------------------------------------------------------

/// A bounded, revocable authorization grant for privileged operator actions.
///
/// Sessions bind the acting operator, the purpose of the action, the scope of
/// what may be performed, an expiry, and the approval reference that authorized
/// the session. Signing keys are deliberately NOT part of this structure; they
/// remain in the external signer and are never stored alongside the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorSession {
    pub session_id: String,
    pub actor: String,
    pub purpose: String,
    pub scope: Permission,
    pub expires_at: DateTime<Utc>,
    pub approval_reference: String,
    pub revoked: bool,
}

impl OperatorSession {
    pub fn new(
        actor: &str,
        purpose: &str,
        scope: Permission,
        expires_at: DateTime<Utc>,
        approval_reference: &str,
    ) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            actor: actor.to_string(),
            purpose: purpose.to_string(),
            scope,
            expires_at,
            approval_reference: approval_reference.to_string(),
            revoked: false,
        }
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.revoked && !self.is_expired(now)
    }
}

/// Reason a privileged session check failed. Recorded verbatim in audit logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionRejection {
    Missing,
    Revoked,
    Expired,
    WrongActor,
    WrongScope,
}

impl SessionRejection {
    pub fn as_reason(&self) -> &'static str {
        match self {
            SessionRejection::Missing => "operator session missing",
            SessionRejection::Revoked => "operator session revoked",
            SessionRejection::Expired => "operator session expired",
            SessionRejection::WrongActor => "operator session actor mismatch",
            SessionRejection::WrongScope => "operator session scope mismatch",
        }
    }
}

/// In-memory store of privileged operator sessions.
///
/// Only session metadata is stored here. Signing keys live outside this store
/// and are never persisted alongside session state.
#[derive(Debug, Clone, Default)]
pub struct OperatorSessionStore {
    sessions: Arc<RwLock<HashMap<String, OperatorSession>>>,
}

impl OperatorSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn insert(&self, session: OperatorSession) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.insert(session.session_id.clone(), session);
        }
    }

    pub fn get(&self, session_id: &str) -> Option<OperatorSession> {
        self.sessions
            .read()
            .ok()
            .and_then(|s| s.get(session_id).cloned())
    }

    /// Revokes a session. Returns true if a session was found and revoked.
    pub fn revoke(&self, session_id: &str) -> bool {
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(session) = sessions.get_mut(session_id) {
                session.revoked = true;
                return true;
            }
        }
        false
    }

    /// Validates a session against the actor and required scope at `now`.
    ///
    /// This is the single canonical check used both immediately before
    /// submission and again at queue-drain time, so a session revoked or
    /// expired while an action was queued cannot be submitted.
    pub fn validate(
        &self,
        session_id: &str,
        actor: &str,
        required: &Permission,
        now: DateTime<Utc>,
    ) -> Result<OperatorSession, SessionRejection> {
        let session = self
            .get(session_id)
            .ok_or(SessionRejection::Missing)?;

        if session.revoked {
            return Err(SessionRejection::Revoked);
        }
        if session.is_expired(now) {
            return Err(SessionRejection::Expired);
        }
        if session.actor != actor {
            return Err(SessionRejection::WrongActor);
        }
        if !session.scope.matches(required) {
            return Err(SessionRejection::WrongScope);
        }

        Ok(session)
    }
}

/// A privileged action that has been queued but not yet submitted.
///
/// The session is re-validated at drain time so revocation or expiry that
/// happens while the action waits in the queue prevents submission.
#[derive(Debug, Clone)]
pub struct QueuedOperatorAction {
    pub action_id: String,
    pub session_id: String,
    pub actor: String,
    pub required: Permission,
    pub payload: String,
}

/// Outcome of draining a queued operator action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainOutcome {
    Submitted,
    Rejected(SessionRejection),
}

/// Drains queued privileged actions, re-checking the session immediately before
/// submission. Rejections are recorded in the audit log.
pub fn drain_queued_action(
    store: &OperatorSessionStore,
    audit_logger: &PermissionAuditLogger,
    action: &QueuedOperatorAction,
    now: DateTime<Utc>,
) -> DrainOutcome {
    match store.validate(&action.session_id, &action.actor, &action.required, now) {
        Ok(_) => DrainOutcome::Submitted,
        Err(rejection) => {
            audit_logger.log_decision(
                Some(&action.actor),
                None,
                &action.required.action,
                &action.required.resource_type,
                action.required.resource_id.as_deref(),
                AuditDecision::Denied,
                &format!(
                    "queued action {} rejected at drain: {}",
                    action.action_id,
                    rejection.as_reason()
                ),
            );
            DrainOutcome::Rejected(rejection)
        }
    }
}

// ---------------------------------------------------------------------------
// 1. Fine-Grained Permission System
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission {
    pub action: String,          // e.g. "read", "write", "delete", "admin", "*"
    pub resource_type: String,   // e.g. "tournament", "match", "user", "staking", "*"
    pub resource_id: Option<String>, // e.g. Some("t-123") or None (all)
}

impl Permission {
    pub fn new(action: &str, resource_type: &str, resource_id: Option<&str>) -> Self {
        Self {
            action: action.to_string(),
            resource_type: resource_type.to_string(),
            resource_id: resource_id.map(|s| s.to_string()),
        }
    }

    /// Parse permission string formatted as "resource_type:action" or "resource_type:action:resource_id" or "*".
    pub fn parse(s: &str) -> Self {
        let parts: Vec<&str> = s.split(':').collect();
        match parts.as_slice() {
            ["*"] => Permission::new("*", "*", None),
            [res, act] => Permission::new(act, res, None),
            [res, act, id] => Permission::new(act, res, if *id == "*" { None } else { Some(id) }),
            _ => Permission::new(s, "*", None),
        }
    }

    /// Checks if this permission grants access to a required permission.
    /// Supports wildcard matching:
    /// - '*' action matches any action
    /// - '*' resource_type matches any resource_type
    /// - None or '*' resource_id in holding permission matches any resource_id requested
    pub fn matches(&self, required: &Permission) -> bool {
        let action_matches = self.action == "*" || self.action == required.action;
        let resource_type_matches =
            self.resource_type == "*" || self.resource_type == required.resource_type;

        let resource_id_matches = match (&self.resource_id, &required.resource_id) {
            (None, _) => true, // holds global permission for resource type
            (Some(holding_id), _) if holding_id == "*" => true,
            (Some(holding_id), Some(req_id)) => holding_id == req_id,
            (Some(_), None) => true,
        };

        action_matches && resource_type_matches && resource_id_matches
    }
}

// ---------------------------------------------------------------------------
// 2. Resource-Level Access Control Engine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct AccessControlEngine {
    role_hierarchy: RoleHierarchy,
    role_templates: RoleTemplateRegistry,
    audit_logger: PermissionAuditLogger,
}

impl AccessControlEngine {
    pub fn new() -> Self {
        Self {
            role_hierarchy: RoleHierarchy::default(),
            role_templates: RoleTemplateRegistry::default(),
            audit_logger: PermissionAuditLogger::default(),
        }
    }

    pub fn with_components(
        role_hierarchy: RoleHierarchy,
        role_templates: RoleTemplateRegistry,
        audit_logger: PermissionAuditLogger,
    ) -> Self {
        Self {
            role_hierarchy,
            role_templates,
            audit_logger,
        }
    }

    pub fn role_hierarchy(&self) -> &RoleHierarchy {
        &self.role_hierarchy
    }

    pub fn role_templates(&self) -> &RoleTemplateRegistry {
        &self.role_templates
    }

    pub fn audit_logger(&self) -> &PermissionAuditLogger {
        &self.audit_logger
    }

    /// Evaluates if user with given roles and explicit permissions has access to the requested permission.
    pub fn check_access(
        &self,
        user_id: Option<&str>,
        user_roles: &[String],
        explicit_permissions: &[Permission],
        required: &Permission,
    ) -> bool {
        // Collect all permissions including role inheritance and role templates
        let mut available_permissions = explicit_permissions.to_vec();

        // Expand user roles using role hierarchy
        let expanded_roles = self.role_hierarchy.expand_roles(user_roles);

        // Fetch permissions from role templates for all expanded roles
        for role in &expanded_roles {
            if let Some(template) = self.role_templates.get_template(role) {
                available_permissions.extend(template.permissions.clone());
            }
        }

        // Check if any available permission matches the required permission
        let granted = available_permissions.iter().any(|perm| perm.matches(required));

        let primary_role = user_roles
            .first()
            .cloned()
            .unwrap_or_else(|| "guest".to_string());
        let reason = if granted {
            "Permission granted by matching policy".to_string()
        } else {
            format!("Missing required permission: {:?}", required)
        };

        // Audit log the authorization decision
        self.audit_logger.log_decision(
            user_id,
            Some(&primary_role),
            &required.action,
            &required.resource_type,
            required.resource_id.as_deref(),
            if granted {
                AuditDecision::Granted
            } else {
                AuditDecision::Denied
            },
            &reason,
        );

        granted
    }
}

// ---------------------------------------------------------------------------
// 3. Permission Inheritance (Role Hierarchy)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RoleHierarchy {
    // Maps role -> list of parent roles it inherits from
    parents: HashMap<String, Vec<String>>,
}

impl Default for RoleHierarchy {
    fn default() -> Self {
        let mut hierarchy = Self {
            parents: HashMap::new(),
        };

        // Default hierarchy: SuperAdmin > Admin > Moderator > User > Guest
        hierarchy.add_inheritance("super_admin", "admin");
        hierarchy.add_inheritance("admin", "moderator");
        hierarchy.add_inheritance("moderator", "user");
        hierarchy.add_inheritance("tournament_organizer", "user");
        hierarchy.add_inheritance("user", "guest");

        hierarchy
    }
}

impl RoleHierarchy {
    pub fn new() -> Self {
        Self {
            parents: HashMap::new(),
        }
    }

    /// Adds inheritance relationship: `child_role` inherits all rights of `parent_role`.
    pub fn add_inheritance(&mut self, child_role: &str, parent_role: &str) {
        self.parents
            .entry(child_role.to_string())
            .or_default()
            .push(parent_role.to_string());
    }

    /// Expand given user roles to include all inherited parent roles recursively.
    pub fn expand_roles(&self, roles: &[String]) -> Vec<String> {
        let mut expanded = HashSet::new();
        let mut stack: Vec<String> = roles.to_vec();

        while let Some(role) = stack.pop() {
            if expanded.insert(role.clone()) {
                if let Some(parent_roles) = self.parents.get(&role) {
                    for parent in parent_roles {
                        if !expanded.contains(parent) {
                            stack.push(parent.clone());
                        }
                    }
                }
            }
        }

        expanded.into_iter().collect()
    }
}

// ---------------------------------------------------------------------------
// 4. Role Templates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleTemplate {
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub parent_roles: Vec<String>,
}

impl RoleTemplate {
    pub fn new(
        name: &str,
        description: &str,
        permissions: Vec<Permission>,
        parent_roles: Vec<String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            permissions,
            parent_roles,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoleTemplateRegistry {
    templates: HashMap<String, RoleTemplate>,
}

impl Default for RoleTemplateRegistry {
    fn default() -> Self {
        let mut registry = Self {
            templates: HashMap::new(),
        };

        // Built-in role templates
        registry.register_template(RoleTemplate::new(
            "super_admin",
            "Super Administrator with total system control",
            vec![Permission::new("*", "*", None)],
            vec!["admin".to_string()],
        ));

        registry.register_template(RoleTemplate::new(
            "admin",
            "Administrator managing system resources",
            vec![
                Permission::new("*", "user", None),
                Permission::new("*", "tournament", None),
                Permission::new("*", "match", None),
                Permission::new("*", "staking", None),
                Permission::new("*", "governance", None),
            ],
            vec!["moderator".to_string()],
        ));

        registry.register_template(RoleTemplate::new(
            "tournament_organizer",
            "Organizer capable of creating and managing tournaments and matches",
            vec![
                Permission::new("create", "tournament", None),
                Permission::new("update", "tournament", None),
                Permission::new("delete", "tournament", None),
                Permission::new("manage", "match", None),
                Permission::new("read", "tournament", None),
            ],
            vec!["user".to_string()],
        ));

        registry.register_template(RoleTemplate::new(
            "moderator",
            "Content and match moderator",
            vec![
                Permission::new("read", "user", None),
                Permission::new("flag", "user", None),
                Permission::new("read", "tournament", None),
                Permission::new("update", "tournament", None),
                Permission::new("read", "match", None),
                Permission::new("moderate", "match", None),
            ],
            vec!["user".to_string()],
        ));

        registry.register_template(RoleTemplate::new(
            "user",
            "Standard registered platform player",
            vec![
                Permission::new("read", "tournament", None),
                Permission::new("join", "tournament", None),
                Permission::new("read", "match", None),
                Permission::new("play", "match", None),
                Permission::new("read", "user", None),
            ],
            vec!["guest".to_string()],
        ));

        registry.register_template(RoleTemplate::new(
            "guest",
            "Unauthenticated guest user",
            vec![
                Permission::new("read", "tournament", None),
                Permission::new("read", "match", None),
            ],
            vec![],
        ));

        registry
    }
}

impl RoleTemplateRegistry {
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
        }
    }

    pub fn register_template(&mut self, template: RoleTemplate) {
        self.templates.insert(template.name.clone(), template);
    }

    pub fn get_template(&self, name: &str) -> Option<&RoleTemplate> {
        self.templates.get(name)
    }

    pub fn list_templates(&self) -> Vec<&RoleTemplate> {
        self.templates.values().collect()
    }
}

// ---------------------------------------------------------------------------
// 5. Permission Audit Log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditDecision {
    Granted,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub user_id: Option<String>,
    pub role: Option<String>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub decision: AuditDecision,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct PermissionAuditLogger {
    logs: Arc<RwLock<Vec<AuditLogEntry>>>,
    max_capacity: usize,
}

impl Default for PermissionAuditLogger {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl PermissionAuditLogger {
    pub fn new(max_capacity: usize) -> Self {
        Self {
            logs: Arc::new(RwLock::new(Vec::new())),
            max_capacity,
        }
    }

    pub fn log_decision(
        &self,
        user_id: Option<&str>,
        role: Option<&str>,
        action: &str,
        resource_type: &str,
        resource_id: Option<&str>,
        decision: AuditDecision,
        reason: &str,
    ) {
        let entry = AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            user_id: user_id.map(|s| s.to_string()),
            role: role.map(|s| s.to_string()),
            action: action.to_string(),
            resource_type: resource_type.to_string(),
            resource_id: resource_id.map(|s| s.to_string()),
            decision: decision.clone(),
            reason: reason.to_string(),
        };

        match decision {
            AuditDecision::Granted => {
                info!(
                    user_id = ?entry.user_id,
                    action = %entry.action,
                    resource_type = %entry.resource_type,
                    resource_id = ?entry.resource_id,
                    "Access GRANTED"
                );
            }
            AuditDecision::Denied => {
                warn!(
                    user_id = ?entry.user_id,
                    action = %entry.action,
                    resource_type = %entry.resource_type,
                    resource_id = ?entry.resource_id,
                    reason = %entry.reason,
                    "Access DENIED"
                );
            }
        }

        if let Ok(mut logs) = self.logs.write() {
            if logs.len() >= self.max_capacity {
                logs.remove(0);
            }
            logs.push(entry);
        }
    }

    pub fn get_entries(&self) -> Vec<AuditLogEntry> {
        self.logs.read().map(|l| l.clone()).unwrap_or_default()
    }

    pub fn get_entries_for_user(&self, user_id: &str) -> Vec<AuditLogEntry> {
        self.logs
            .read()
            .map(|l| {
                l.iter()
                    .filter(|e| e.user_id.as_deref() == Some(user_id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get_denied_entries(&self) -> Vec<AuditLogEntry> {
        self.logs
            .read()
            .map(|l| {
                l.iter()
                    .filter(|e| e.decision == AuditDecision::Denied)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut logs) = self.logs.write() {
            logs.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Actix-web Middleware Integration
// ---------------------------------------------------------------------------

pub struct AuthorizationMiddleware {
    engine: Arc<AccessControlEngine>,
    required_permission: Permission,
    operator_sessions: Option<Arc<OperatorSessionStore>>,
}

impl AuthorizationMiddleware {
    pub fn new(engine: Arc<AccessControlEngine>, required_permission: Permission) -> Self {
        Self {
            engine,
            required_permission,
            operator_sessions: None,
        }
    }

    /// Requires a valid privileged operator session in addition to the
    /// permission check. The session is validated immediately before the
    /// request is forwarded for submission.
    pub fn with_operator_sessions(
        engine: Arc<AccessControlEngine>,
        required_permission: Permission,
        operator_sessions: Arc<OperatorSessionStore>,
    ) -> Self {
        Self {
            engine,
            required_permission,
            operator_sessions: Some(operator_sessions),
        }
    }
}

impl<S, B> Transform<S, ServiceRequest> for AuthorizationMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = AuthorizationMiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(AuthorizationMiddlewareService {
            service: Arc::new(service),
            engine: self.engine.clone(),
            required_permission: self.required_permission.clone(),
            operator_sessions: self.operator_sessions.clone(),
        }))
    }
}

pub struct AuthorizationMiddlewareService<S> {
    service: Arc<S>,
    engine: Arc<AccessControlEngine>,
    required_permission: Permission,
    operator_sessions: Option<Arc<OperatorSessionStore>>,
}

impl<S, B> Service<ServiceRequest> for AuthorizationMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = self.service.clone();
        let engine = self.engine.clone();
        let required = self.required_permission.clone();
        let operator_sessions = self.operator_sessions.clone();

        Box::pin(async move {
            let claims_opt = req.extensions().get::<Claims>().cloned();

            let (user_id, roles) = match claims_opt {
                Some(claims) => (Some(claims.sub), claims.roles),
                None => (None, vec!["guest".to_string()]),
            };

            let granted = engine.check_access(user_id.as_deref(), &roles, &[], &required);

            if !granted {
                return Err(ErrorForbidden("Insufficient permissions"));
            }

            // Privileged operator actions additionally require a bounded,
            // revocable session checked immediately before submission.
            if let Some(store) = operator_sessions {
                let actor = match user_id.as_deref() {
                    Some(actor) => actor.to_string(),
                    None => {
                        engine.audit_logger().log_decision(
                            None,
                            None,
                            &required.action,
                            &required.resource_type,
                            required.resource_id.as_deref(),
                            AuditDecision::Denied,
                            SessionRejection::Missing.as_reason(),
                        );
                        return Err(ErrorForbidden("Operator session required"));
                    }
                };

                let session_id = req
                    .headers()
                    .get("x-operator-session")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());

                let session_id = match session_id {
                    Some(id) => id,
                    None => {
                        engine.audit_logger().log_decision(
                            Some(&actor),
                            None,
                            &required.action,
                            &required.resource_type,
                            required.resource_id.as_deref(),
                            AuditDecision::Denied,
                            SessionRejection::Missing.as_reason(),
                        );
                        return Err(ErrorForbidden("Operator session required"));
                    }
                };

                if let Err(rejection) =
                    store.validate(&session_id, &actor, &required, Utc::now())
                {
                    engine.audit_logger().log_decision(
                        Some(&actor),
                        None,
                        &required.action,
                        &required.resource_type,
                        required.resource_id.as_deref(),
                        AuditDecision::Denied,
                        rejection.as_reason(),
                    );
                    return Err(ErrorForbidden("Operator session rejected"));
                }
            }

            service.call(req).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fine_grained_permission_matching() {
        let perm_global = Permission::new("read", "tournament", None);
        let perm_specific = Permission::new("read", "tournament", Some("t-100"));
        let req_specific = Permission::new("read", "tournament", Some("t-100"));
        let req_other = Permission::new("read", "tournament", Some("t-200"));
        let req_write = Permission::new("write", "tournament", Some("t-100"));

        assert!(perm_global.matches(&req_specific));
        assert!(perm_global.matches(&req_other));
        assert!(!perm_global.matches(&req_write));

        assert!(perm_specific.matches(&req_specific));
        assert!(!perm_specific.matches(&req_other));

        let perm_wildcard = Permission::parse("*");
        assert!(perm_wildcard.matches(&req_specific));
        assert!(perm_wildcard.matches(&req_write));
    }

    #[test]
    fn test_permission_inheritance() {
        let hierarchy = RoleHierarchy::default();
        let expanded = hierarchy.expand_roles(&["super_admin".to_string()]);

        assert!(expanded.contains(&"super_admin".to_string()));
        assert!(expanded.contains(&"admin".to_string()));
        assert!(expanded.contains(&"moderator".to_string()));
        assert!(expanded.contains(&"user".to_string()));
        assert!(expanded.contains(&"guest".to_string()));
    }

    #[test]
    fn test_role_templates_and_access_engine() {
        let engine = AccessControlEngine::new();

        // Guest trying to read tournament -> GRANTED
        let req_read_tournament = Permission::new("read", "tournament", Some("t-1"));
        assert!(engine.check_access(
            Some("u-guest"),
            &["guest".to_string()],
            &[],
            &req_read_tournament
        ));

        // Guest trying to create tournament -> DENIED
        let req_create_tournament = Permission::new("create", "tournament", None);
        assert!(!engine.check_access(
            Some("u-guest"),
            &["guest".to_string()],
            &[],
            &req_create_tournament
        ));

        // Tournament organizer creating tournament -> GRANTED
        assert!(engine.check_access(
            Some("u-organizer"),
            &["tournament_organizer".to_string()],
            &[],
            &req_create_tournament
        ));
    }

    #[test]
    fn test_permission_audit_log() {
        let engine = AccessControlEngine::new();
        let req = Permission::new("delete", "user", Some("u-99"));

        engine.check_access(
            Some("u-test"),
            &["guest".to_string()],
            &[],
            &req,
        );

        let denied_logs = engine.audit_logger().get_denied_entries();
        assert_eq!(denied_logs.len(), 1);
        assert_eq!(denied_logs[0].user_id.as_deref(), Some("u-test"));
        assert_eq!(denied_logs[0].action, "delete");
        assert_eq!(denied_logs[0].resource_type, "user");
        assert_eq!(denied_logs[0].decision, AuditDecision::Denied);
    }

    fn operator_session(
        store: &OperatorSessionStore,
        actor: &str,
        scope: Permission,
        expires_at: DateTime<Utc>,
    ) -> String {
        let session = OperatorSession::new(
            actor,
            "settlement",
            scope,
            expires_at,
            "approval-123",
        );
        let id = session.session_id.clone();
        store.insert(session);
        id
    }

    #[test]
    fn test_session_binds_actor_scope_expiry_approval() {
        let store = OperatorSessionStore::new();
        let scope = Permission::new("submit", "settlement", None);
        let id = operator_session(&store, "op-1", scope.clone(), Utc::now() + chrono::Duration::minutes(5));

        let session = store.get(&id).unwrap();
        assert_eq!(session.actor, "op-1");
        assert_eq!(session.purpose, "settlement");
        assert_eq!(session.approval_reference, "approval-123");
        assert_eq!(session.scope, scope);

        assert!(store.validate(&id, "op-1", &scope, Utc::now()).is_ok());
    }

    #[test]
    fn test_expiry_during_queueing_blocks_submission() {
        let store = OperatorSessionStore::new();
        let audit = PermissionAuditLogger::default();
        let required = Permission::new("submit", "settlement", None);
        let expires = Utc::now() + chrono::Duration::seconds(1);
        let id = operator_session(&store, "op-1", required.clone(), expires);

        let action = QueuedOperatorAction {
            action_id: "a-1".to_string(),
            session_id: id,
            actor: "op-1".to_string(),
            required: required.clone(),
            payload: "{}".to_string(),
        };

        // Valid while queued.
        assert_eq!(
            drain_queued_action(&store, &audit, &action, Utc::now()),
            DrainOutcome::Submitted
        );

        // Expired by drain time.
        let later = expires + chrono::Duration::seconds(1);
        assert_eq!(
            drain_queued_action(&store, &audit, &action, later),
            DrainOutcome::Rejected(SessionRejection::Expired)
        );

        let denied = audit.get_denied_entries();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].user_id.as_deref(), Some("op-1"));
        assert!(denied[0].reason.contains("expired"));
    }

    #[test]
    fn test_revocation_race_prevents_queued_submission() {
        let store = OperatorSessionStore::new();
        let audit = PermissionAuditLogger::default();
        let required = Permission::new("submit", "settlement", None);
        let id = operator_session(&store, "op-1", required.clone(), Utc::now() + chrono::Duration::minutes(5));

        let action = QueuedOperatorAction {
            action_id: "a-2".to_string(),
            session_id: id.clone(),
            actor: "op-1".to_string(),
            required: required.clone(),
            payload: "{}".to_string(),
        };

        // Revoked after queueing but before drain.
        assert!(store.revoke(&id));
        assert_eq!(
            drain_queued_action(&store, &audit, &action, Utc::now()),
            DrainOutcome::Rejected(SessionRejection::Revoked)
        );

        // Duplicate delivery of the same action is also rejected.
        assert_eq!(
            drain_queued_action(&store, &audit, &action, Utc::now()),
            DrainOutcome::Rejected(SessionRejection::Revoked)
        );

        let denied = audit.get_denied_entries();
        assert_eq!(denied.len(), 2);
        assert!(denied.iter().all(|e| e.reason.contains("revoked")));
    }

    #[test]
    fn test_wrong_scope_action_rejected_and_audited() {
        let store = OperatorSessionStore::new();
        let audit = PermissionAuditLogger::default();
        let granted_scope = Permission::new("submit", "settlement", None);
        let id = operator_session(&store, "op-1", granted_scope, Utc::now() + chrono::Duration::minutes(5));

        let wrong = Permission::new("submit", "treasury", None);
        let action = QueuedOperatorAction {
            action_id: "a-3".to_string(),
            session_id: id,
            actor: "op-1".to_string(),
            required: wrong.clone(),
            payload: "{}".to_string(),
        };

        assert_eq!(
            drain_queued_action(&store, &audit, &action, Utc::now()),
            DrainOutcome::Rejected(SessionRejection::WrongScope)
        );

        let denied = audit.get_denied_entries();
        assert_eq!(denied.len(), 1);
        assert_eq!(denied[0].resource_type, "treasury");
        assert!(denied[0].reason.contains("scope"));
    }

    #[test]
    fn test_wrong_actor_rejected() {
        let store = OperatorSessionStore::new();
        let required = Permission::new("submit", "settlement", None);
        let id = operator_session(&store, "op-1", required.clone(), Utc::now() + chrono::Duration::minutes(5));

        assert_eq!(
            store.validate(&id, "op-2", &required, Utc::now()),
            Err(SessionRejection::WrongActor)
        );
    }

    #[test]
    fn test_missing_session_rejected() {
        let store = OperatorSessionStore::new();
        let required = Permission::new("submit", "settlement", None);
        assert_eq!(
            store.validate("does-not-exist", "op-1", &required, Utc::now()),
            Err(SessionRejection::Missing)
        );
    }
}
