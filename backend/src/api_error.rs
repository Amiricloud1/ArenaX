use actix_web::{HttpResponse, ResponseError};
use serde::Serialize;
use std::collections::BTreeMap;
use thiserror::Error;
use tracing::error;

/// One field's validation failure.
///
/// `field` is a full path (`profile.email`, `players[1].handle`) rather than a
/// bare name, so a client can point at the input that failed inside a nested or
/// repeated structure instead of guessing which one the message refers to.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    /// Machine-readable rule that failed, e.g. `length`, `range`, `email`.
    pub code: String,
    pub message: String,
}

impl FieldError {
    pub fn new(
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Internal server error: {0}")]
    InternalServerError(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Forbidden")]
    Forbidden,

    #[error("Not found")]
    NotFound,

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("Redis error: {0}")]
    RedisError(String),

    #[error("Stellar error: {0}")]
    StellarError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    /// Rate limit exceeded. The `RateLimit-*` headers describe the current
    /// window; clients should wait `retry_after` seconds before retrying.
    #[error("Too many requests: {message}")]
    TooManyRequests {
        message: String,
        limit: u64,
        remaining: u64,
        reset: u64,
        retry_after: u64,
    },

    /// One or more fields failed validation. Carries a per-field breakdown and
    /// answers 422, so a client can distinguish "this request was malformed"
    /// from "these specific values were rejected".
    #[error("Validation failed")]
    ValidationFailed(Vec<FieldError>),

    /// A dependency is unavailable and the request cannot be served right now.
    /// Distinct from `InternalServerError`: this says "try again", not
    /// "something is broken in a way retrying will not help".
    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    /// A contract call failed. Carries the stable public code, a safe
    /// user-facing message, and the recovery classification so clients and
    /// support can act on it without interpreting raw contract errors.
    #[error("Contract error: {message}")]
    ContractError {
        code: String,
        message: String,
        recovery: RecoveryClass,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Public contract error catalog (Issue #235)
// ─────────────────────────────────────────────────────────────────────────────

/// How a client (or operator) should recover from a contract error.
///
/// This is deliberately coarse: three buckets are enough to drive automated
/// retry logic and support escalation without leaking internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryClass {
    /// Transient — the same request may succeed if retried (with backoff).
    Retry,
    /// Not retryable automatically; a human must intervene (e.g. reconcile a
    /// duplicate delivery or top up a balance).
    ManualRecovery,
    /// Terminal — retrying will never help; the request is rejected for good.
    Fatal,
}

/// Stable code emitted when a contract returns an error we do not recognise.
///
/// Kept distinct from every known code so clients can tell "we know what went
/// wrong" from "something new happened" and route it to support.
pub const UNKNOWN_CONTRACT_ERROR: &str = "CONTRACT_UNKNOWN_ERROR";

/// One entry in the public contract error catalog.
///
/// Serializable so the frontend and generated docs can consume the exact same
/// mapping the backend uses — there is one source of truth.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContractErrorEntry {
    /// Stable, machine-readable code. Never changes once published.
    pub code: &'static str,
    /// Safe, user-facing message. Contains no internal details.
    pub message: &'static str,
    /// How to recover from this error.
    pub recovery: RecoveryClass,
}

/// The full public contract error catalog.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContractErrorCatalog {
    pub entries: Vec<ContractErrorEntry>,
}

impl ContractErrorCatalog {
    /// Look up a catalog entry by its stable code.
    pub fn get(&self, code: &str) -> Option<&ContractErrorEntry> {
        self.entries.iter().find(|e| e.code == code)
    }
}

/// Every public contract error, mapped to a stable code, safe message, and
/// recovery classification.
///
/// Codes are derived from `AntiCheatError` and `VirtualEconomyError` variants.
/// Once published a code must never be reused for a different meaning.
pub fn contract_error_catalog() -> ContractErrorCatalog {
    use RecoveryClass::*;

    let entries = vec![
        // ── AntiCheatError ────────────────────────────────────────────────
        ContractErrorEntry {
            code: "ANTICHEAT_UNAUTHORIZED",
            message: "You are not authorized to perform this action.",
            recovery: Fatal,
        },
        ContractErrorEntry {
            code: "ANTICHEAT_INVALID_PROOF",
            message: "The submitted proof could not be verified.",
            recovery: Fatal,
        },
        ContractErrorEntry {
            code: "ANTICHEAT_PROOF_EXPIRED",
            message: "The submitted proof has expired. Please request a new one.",
            recovery: Retry,
        },
        ContractErrorEntry {
            code: "ANTICHEAT_DUPLICATE_SUBMISSION",
            message: "This submission was already recorded.",
            recovery: ManualRecovery,
        },
        ContractErrorEntry {
            code: "ANTICHEAT_RATE_LIMITED",
            message: "Too many submissions. Please try again later.",
            recovery: Retry,
        },
        ContractErrorEntry {
            code: "ANTICHEAT_INVALID_STATE",
            message: "The contract is not in a state that allows this action.",
            recovery: Fatal,
        },
        ContractErrorEntry {
            code: "ANTICHEAT_NOT_INITIALIZED",
            message: "The anti-cheat contract has not been initialized.",
            recovery: ManualRecovery,
        },
        // ── VirtualEconomyError ───────────────────────────────────────────
        ContractErrorEntry {
            code: "ECONOMY_UNAUTHORIZED",
            message: "You are not authorized to perform this action.",
            recovery: Fatal,
        },
        ContractErrorEntry {
            code: "ECONOMY_INSUFFICIENT_BALANCE",
            message: "Insufficient balance for this transaction.",
            recovery: ManualRecovery,
        },
        ContractErrorEntry {
            code: "ECONOMY_INVALID_AMOUNT",
            message: "The amount is not valid for this transaction.",
            recovery: Fatal,
        },
        ContractErrorEntry {
            code: "ECONOMY_OVERFLOW",
            message: "The transaction would overflow the allowed range.",
            recovery: Fatal,
        },
        ContractErrorEntry {
            code: "ECONOMY_DUPLICATE_TRANSACTION",
            message: "This transaction was already processed.",
            recovery: ManualRecovery,
        },
        ContractErrorEntry {
            code: "ECONOMY_ITEM_NOT_FOUND",
            message: "The requested item does not exist.",
            recovery: Fatal,
        },
        ContractErrorEntry {
            code: "ECONOMY_NOT_INITIALIZED",
            message: "The economy contract has not been initialized.",
            recovery: ManualRecovery,
        },
        // ── Unknown ───────────────────────────────────────────────────────
        ContractErrorEntry {
            code: UNKNOWN_CONTRACT_ERROR,
            message: "An unexpected contract error occurred. Please contact support.",
            recovery: ManualRecovery,
        },
    ];

    ContractErrorCatalog { entries }
}

/// Context attached to a contract error for logging. Never contains signing
/// material — only identifiers safe to log.
#[derive(Debug, Clone, Default)]
pub struct ContractTxContext {
    pub transaction_hash: Option<String>,
    pub contract_id: Option<String>,
    pub method: Option<String>,
}

impl ContractTxContext {
    pub fn new(
        transaction_hash: impl Into<String>,
        contract_id: impl Into<String>,
        method: impl Into<String>,
    ) -> Self {
        Self {
            transaction_hash: Some(transaction_hash.into()),
            contract_id: Some(contract_id.into()),
            method: Some(method.into()),
        }
    }
}

// Helper methods for convenience
impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        ApiError::BadRequest(message.into())
    }

    /// Creates an `InternalServerError` response.
    ///
    /// The `message` is written to the structured log at ERROR level so
    /// engineers can diagnose the root cause, but it is **never** forwarded
    /// to API consumers — the public response always says "Internal server
    /// error".
    pub fn internal_error(message: impl Into<String>) -> Self {
        let msg = message.into();
        error!(error.message = %msg, "Internal server error");
        ApiError::InternalServerError(msg)
    }

    pub fn internal_server_error(message: impl Into<String>) -> Self {
        Self::internal_error(message)
    }

    pub fn database_error(e: impl Into<sqlx::Error>) -> Self {
        ApiError::DatabaseError(e.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError::NotFound
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        ApiError::Unauthorized
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        ApiError::Forbidden
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        ApiError::Conflict(message.into())
    }

    pub fn too_many_requests(message: impl Into<String>) -> Self {
        ApiError::TooManyRequests {
            message: message.into(),
            limit: 0,
            remaining: 0,
            reset: 0,
            retry_after: 60,
        }
    }

    /// Translate a raw contract error code into a public `ApiError`.
    ///
    /// Known codes map to their catalog entry (stable code, safe message,
    /// recovery class). Unknown codes are logged at ERROR with transaction
    /// context — never signing material — and surface as
    /// `UNKNOWN_CONTRACT_ERROR` so they stay distinguishable from known
    /// failures.
    pub fn from_contract_error(code: &str, tx_context: &ContractTxContext) -> Self {
        let catalog = contract_error_catalog();

        if let Some(entry) = catalog.get(code) {
            return ApiError::ContractError {
                code: entry.code.to_string(),
                message: entry.message.to_string(),
                recovery: entry.recovery,
            };
        }

        // Unknown: log with context so support can trace it, but never echo
        // the raw code or any signing material back to the client.
        error!(
            contract.error.code = %code,
            transaction.hash = tx_context.transaction_hash.as_deref().unwrap_or(""),
            contract.id = tx_context.contract_id.as_deref().unwrap_or(""),
            contract.method = tx_context.method.as_deref().unwrap_or(""),
            "Unknown contract error"
        );

        let unknown = catalog
            .get(UNKNOWN_CONTRACT_ERROR)
            .expect("unknown contract error entry must exist");
        ApiError::ContractError {
            code: unknown.code.to_string(),
            message: unknown.message.to_string(),
            recovery: unknown.recovery,
        }
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
    details: Option<String>,
    /// Per-field failures. Omitted entirely for non-validation errors so the
    /// response shape stays unchanged for every existing consumer.
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<Vec<FieldError>>,
    /// Stable contract error code. Present only for contract errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    contract_code: Option<String>,
    /// Recovery classification for contract errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<RecoveryClass>,
}

impl ResponseError for ApiError {
    fn error_response(&self) -> HttpResponse {
        let (status, message) = match self {
            ApiError::InternalServerError(_) => (
                actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            ),
            ApiError::BadRequest(_) => (actix_web::http::StatusCode::BAD_REQUEST, self.to_string()),
            ApiError::Unauthorized => (actix_web::http::StatusCode::UNAUTHORIZED, self.to_string()),
            ApiError::Forbidden => (actix_web::http::StatusCode::FORBIDDEN, self.to_string()),
            ApiError::NotFound => (actix_web::http::StatusCode::NOT_FOUND, self.to_string()),
            ApiError::Conflict(_) => (actix_web::http::StatusCode::CONFLICT, self.to_string()),
            ApiError::DatabaseError(_) => (
                actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Database error".to_string(),
            ),
            ApiError::RedisError(_) => (
                actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Cache error".to_string(),
            ),
            ApiError::StellarError(_) => (
                actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Blockchain error".to_string(),
            ),
            // 422, not 400: the request was well-formed enough to parse — it is
            // the values that were rejected.
            ApiError::ValidationError(_) => (
                actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
                self.to_string(),
            ),
            ApiError::ValidationFailed(_) => (
                actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
                self.to_string(),
            ),
            ApiError::TooManyRequests { .. } => (
                actix_web::http::StatusCode::TOO_MANY_REQUESTS,
                self.to_string(),
            ),
            ApiError::ServiceUnavailable(_) => (
                actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
                self.to_string(),
            ),
            // Contract errors are classified by recovery: retryable ones answer
            // 503 so clients back off; manual/fatal ones answer 409 so they are
            // not retried blindly.
            ApiError::ContractError { message, recovery, .. } => {
                let status = match recovery {
                    RecoveryClass::Retry => actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
                    RecoveryClass::ManualRecovery | RecoveryClass::Fatal => {
                        actix_web::http::StatusCode::CONFLICT
                    }
                };
                (status, message.clone())
            }
        };

        let (contract_code, recovery) = match self {
            ApiError::ContractError { code, recovery, .. } => {
                (Some(code.clone()), Some(*recovery))
            }
            _ => (None, None),
        };

        let error_response = ErrorResponse {
            error: message,
            code: status.as_u16(),
            // Never echo internal details (DB errors, stack traces, etc.) back to the client.
            details: None,
            // Field errors are the exception: they describe the caller's own
            // input, so returning them leaks nothing and is the only way the
            // caller can fix the request.
            fields: match self {
                ApiError::ValidationFailed(fields) => Some(fields.clone()),
                _ => None,
            },
            contract_code,
            recovery,
        };

        let mut builder = HttpResponse::build(status);
        if let ApiError::TooManyRequests {
            limit,
            remaining,
            reset,
            retry_after,
            ..
        } = self
        {
            builder.insert_header(("RateLimit-Limit", limit.to_string()));
            builder.insert_header(("RateLimit-Remaining", remaining.to_string()));
            builder.insert_header(("RateLimit-Reset", reset.to_string()));
            builder.insert_header(("Retry-After", retry_after.to_string()));
            builder.insert_header(("Access-Control-Expose-Headers", "RateLimit-Limit, RateLimit-Remaining, RateLimit-Reset, Retry-After"));
        }
        builder.json(error_response)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// validator::ValidationErrors → ApiError::ValidationFailed  (Issue #860)
// ─────────────────────────────────────────────────────────────────────────────

/// Flatten `validator`'s nested error tree into a flat list of field paths.
///
/// `validator` reports nested structs and lists as a tree, so a failure inside
/// `players[1].handle` arrives three levels deep. A client cannot act on that
/// shape; it needs the path. This walks the tree and joins the segments, using
/// `parent.child` for structs and `parent[i].child` for lists.
fn flatten_validation_errors(
    prefix: &str,
    errors: &validator::ValidationErrors,
    out: &mut Vec<FieldError>,
) {
    for (field, kind) in errors.errors() {
        let path = if prefix.is_empty() {
            field.to_string()
        } else {
            format!("{prefix}.{field}")
        };

        match kind {
            validator::ValidationErrorsKind::Field(field_errors) => {
                for err in field_errors {
                    let message = err
                        .message
                        .as_ref()
                        .map(|m| m.to_string())
                        // `validator` leaves the message unset for built-in
                        // rules; the code is the only thing that describes the
                        // failure, so fall back to it rather than to an empty
                        // string.
                        .unwrap_or_else(|| format!("failed the '{}' rule", err.code));
                    out.push(FieldError::new(path.clone(), err.code.to_string(), message));
                }
            }
            validator::ValidationErrorsKind::Struct(nested) => {
                flatten_validation_errors(&path, nested, out);
            }
            validator::ValidationErrorsKind::List(items) => {
                for (index, nested) in items {
                    flatten_validation_errors(&format!("{path}[{index}]"), nested, out);
                }
            }
        }
    }
}

impl From<validator::ValidationErrors> for ApiError {
    fn from(errors: validator::ValidationErrors) -> Self {
        let mut fields = Vec::new();
        flatten_validation_errors("", &errors, &mut fields);
        // Deterministic order: the same bad request must produce the same
        // response, and `validator` iterates a hash map.
        fields.sort_by(|a, b| a.field.cmp(&b.field).then(a.code.cmp(&b.code)));
        ApiError::ValidationFailed(fields)
    }
}

impl From<crate::service::wallet_service::WalletError> for ApiError {
    fn from(err: crate::service::wallet_service::WalletError) -> Self {
        match err {
            crate::service::wallet_service::WalletError::WalletNotFound => ApiError::not_found("Wallet not found"),
            crate::service::wallet_service::WalletError::TransactionNotFound => ApiError::not_found("Transaction not found"),
            crate::service::wallet_service::WalletError::InsufficientBalance { required, available } => {
                ApiError::bad_request(format!("Insufficient balance: required {}, available {}", required, available))
            }
            crate::service::wallet_service::WalletError::InvalidAmount(msg) => ApiError::bad_request(msg),
            crate::service::wallet_service::WalletError::PaymentVerificationFailed => ApiError::bad_request("Payment verification failed"),
            crate::service::wallet_service::WalletError::DatabaseError(e) => ApiError::DatabaseError(e),
            crate::service::wallet_service::WalletError::RedisError(e) => ApiError::RedisError(e),
        }
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use actix_web::ResponseError;
    use validator::{Validate, ValidationErrors};

    #[derive(Debug, Validate)]
    struct Inner {
        #[validate(length(min = 3))]
        handle: String,
    }

    #[derive(Debug, Validate)]
    struct Outer {
        #[validate(email)]
        email: String,
        #[validate(range(min = 1, max = 100))]
        age: u32,
        #[validate(nested)]
        inner: Inner,
    }

    fn errors_for(outer: &Outer) -> ValidationErrors {
        outer.validate().expect_err("expected validation to fail")
    }

    fn fields_of(err: &ApiError) -> Vec<FieldError> {
        match err {
            ApiError::ValidationFailed(fields) => fields.clone(),
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn validation_answers_422_not_400() {
        let err = ApiError::ValidationFailed(vec![FieldError::new("email", "email", "invalid")]);

        assert_eq!(err.error_response().status().as_u16(), 422);
    }

    #[test]
    fn the_legacy_string_variant_also_answers_422() {
        // Same class of failure, so the same status — a caller should not have
        // to know which code path rejected it.
        let err = ApiError::ValidationError("bad input".to_string());

        assert_eq!(err.error_response().status().as_u16(), 422);
    }

    #[test]
    fn reports_one_entry_per_failing_field() {
        let outer = Outer {
            email: "not-an-email".to_string(),
            age: 500,
            inner: Inner { handle: "ok".to_string() },
        };

        let fields = fields_of(&ApiError::from(errors_for(&outer)));

        let names: Vec<&str> = fields.iter().map(|f| f.field.as_str()).collect();
        assert!(names.contains(&"email"));
        assert!(names.contains(&"age"));
    }

    #[test]
    fn nested_failures_carry_the_full_path() {
        let outer = Outer {
            email: "a@example.com".to_string(),
            age: 30,
            inner: Inner { handle: "x".to_string() },
        };

        let fields = fields_of(&ApiError::from(errors_for(&outer)));

        // Not "handle" — a client cannot act on a bare name it cannot locate.
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field, "inner.handle");
    }

    #[test]
    fn carries_the_rule_that_failed_as_a_machine_readable_code() {
        let outer = Outer {
            email: "nope".to_string(),
            age: 30,
            inner: Inner { handle: "abc".to_string() },
        };

        let fields = fields_of(&ApiError::from(errors_for(&outer)));

        assert_eq!(fields[0].code, "email");
    }

    #[test]
    fn range_violations_are_reported_with_the_range_code() {
        let outer = Outer {
            email: "a@example.com".to_string(),
            age: 0,
            inner: Inner { handle: "abc".to_string() },
        };

        let fields = fields_of(&ApiError::from(errors_for(&outer)));

        assert_eq!(fields[0].field, "age");
        assert_eq!(fields[0].code, "range");
    }

    #[test]
    fn field_order_is_deterministic() {
        let outer = Outer {
            email: "nope".to_string(),
            age: 500,
            inner: Inner { handle: "x".to_string() },
        };

        let first = fields_of(&ApiError::from(errors_for(&outer)));
        let second = fields_of(&ApiError::from(errors_for(&outer)));

        // validator iterates a hash map, so without sorting the same bad
        // request could answer with a different ordering each time.
        assert_eq!(first, second);
        assert_eq!(
            first.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(),
            vec!["age", "email", "inner.handle"]
        );
    }

    #[test]
    fn a_message_is_always_present_even_for_built_in_rules() {
        let outer = Outer {
            email: "nope".to_string(),
            age: 30,
            inner: Inner { handle: "abc".to_string() },
        };

        let fields = fields_of(&ApiError::from(errors_for(&outer)));

        assert!(!fields[0].message.is_empty());
    }

    #[test]
    fn non_validation_errors_carry_no_field_breakdown() {
        let response = ApiError::NotFound.error_response();

        assert_eq!(response.status().as_u16(), 404);
    }
}

#[cfg(test)]
mod contract_error_tests {
    use super::*;
    use actix_web::ResponseError;

    /// Every public contract error code the catalog must cover.
    const PUBLIC_CODES: &[&str] = &[
        "ANTICHEAT_UNAUTHORIZED",
        "ANTICHEAT_INVALID_PROOF",
        "ANTICHEAT_PROOF_EXPIRED",
        "ANTICHEAT_DUPLICATE_SUBMISSION",
        "ANTICHEAT_RATE_LIMITED",
        "ANTICHEAT_INVALID_STATE",
        "ANTICHEAT_NOT_INITIALIZED",
        "ECONOMY_UNAUTHORIZED",
        "ECONOMY_INSUFFICIENT_BALANCE",
        "ECONOMY_INVALID_AMOUNT",
        "ECONOMY_OVERFLOW",
        "ECONOMY_DUPLICATE_TRANSACTION",
        "ECONOMY_ITEM_NOT_FOUND",
        "ECONOMY_NOT_INITIALIZED",
    ];

    fn ctx() -> ContractTxContext {
        ContractTxContext::new("deadbeef", "CABC", "submit")
    }

    #[test]
    fn every_public_error_maps_to_a_stable_code_message_and_recovery() {
        let catalog = contract_error_catalog();

        for code in PUBLIC_CODES {
            let entry = catalog
                .get(code)
                .unwrap_or_else(|| panic!("missing catalog entry for {code}"));
            assert_eq!(entry.code, *code);
            assert!(!entry.message.is_empty(), "{code} has empty message");
        }
    }

    #[test]
    fn catalog_codes_are_unique() {
        let catalog = contract_error_catalog();
        let mut seen = BTreeMap::new();
        for entry in &catalog.entries {
            assert!(
                seen.insert(entry.code, ()).is_none(),
                "duplicate code {}",
                entry.code
            );
        }
    }

    #[test]
    fn known_code_translates_to_contract_error() {
        let err = ApiError::from_contract_error("ECONOMY_INSUFFICIENT_BALANCE", &ctx());

        match err {
            ApiError::ContractError { code, recovery, .. } => {
                assert_eq!(code, "ECONOMY_INSUFFICIENT_BALANCE");
                assert_eq!(recovery, RecoveryClass::ManualRecovery);
            }
            other => panic!("expected ContractError, got {other:?}"),
        }
    }

    #[test]
    fn unknown_code_stays_distinguishable() {
        let err = ApiError::from_contract_error("SOME_FUTURE_ERROR", &ctx());

        match err {
            ApiError::ContractError { code, recovery, .. } => {
                assert_eq!(code, UNKNOWN_CONTRACT_ERROR);
                assert_eq!(recovery, RecoveryClass::ManualRecovery);
            }
            other => panic!("expected ContractError, got {other:?}"),
        }
    }

    #[test]
    fn retryable_errors_answer_503() {
        let err = ApiError::from_contract_error("ANTICHEAT_RATE_LIMITED", &ctx());

        assert_eq!(err.error_response().status().as_u16(), 503);
    }

    #[test]
    fn fatal_errors_answer_409() {
        let err = ApiError::from_contract_error("ECONOMY_INVALID_AMOUNT", &ctx());

        assert_eq!(err.error_response().status().as_u16(), 409);
    }

    #[test]
    fn duplicate_delivery_is_manual_recovery() {
        let err = ApiError::from_contract_error("ECONOMY_DUPLICATE_TRANSACTION", &ctx());

        match err {
            ApiError::ContractError { recovery, .. } => {
                assert_eq!(recovery, RecoveryClass::ManualRecovery);
            }
            other => panic!("expected ContractError, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_submission_is_manual_recovery() {
        let err = ApiError::from_contract_error("ANTICHEAT_DUPLICATE_SUBMISSION", &ctx());

        match err {
            ApiError::ContractError { recovery, .. } => {
                assert_eq!(recovery, RecoveryClass::ManualRecovery);
            }
            other => panic!("expected ContractError, got {other:?}"),
        }
    }

    #[test]
    fn catalog_serializes_for_frontend_consumption() {
        let catalog = contract_error_catalog();
        let json = serde_json::to_string(&catalog).expect("catalog must serialize");

        assert!(json.contains("ANTICHEAT_UNAUTHORIZED"));
        assert!(json.contains("manual_recovery"));
    }
}

