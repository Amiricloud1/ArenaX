use soroban_sdk::contracterror;

/// Stable, client-facing classification for a contract error.
///
/// The numeric value is part of the public API: clients and backend adapters
/// must treat it as immutable. New classifications may be appended, but
/// existing values must never be renumbered or reused.
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ErrorRecovery {
    /// Transient condition; the caller may retry the same operation.
    Retry = 0,
    /// Permanent condition; the caller must not retry without changing input
    /// or state. Requires operator/manual intervention to resolve.
    ManualRecovery = 1,
    /// The operation was rejected for authorization reasons; retrying with the
    /// same credentials will not succeed. Escalate to the operator.
    Authorization = 2,
    /// The operation conflicts with existing state (duplicate delivery or an
    /// already-applied action). Treat as idempotent success where the prior
    /// effect is observable; otherwise escalate.
    Conflict = 3,
}

/// Public error catalog entry: stable code, safe message, and recovery class.
///
/// `code` mirrors the `AntiCheatError` discriminant so clients can switch on a
/// single stable integer. `message` is safe to surface to end users and must
/// never contain signing material, secrets, or internal addresses.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ErrorMapping {
    pub code: u32,
    pub message: &'static str,
    pub recovery: ErrorRecovery,
}

/// Mapping table for every public `AntiCheatError` variant.
///
/// This table is the canonical source of truth and is mirrored by the backend
/// error catalog. Contract tests assert that every variant has an entry and
/// that codes are unique and stable.
pub const ERROR_MAPPINGS: &[ErrorMapping] = &[
    // ── Initialization ──────────────────────────────────────────────────────
    ErrorMapping { code: 1, message: "contract already initialized", recovery: ErrorRecovery::Conflict },
    ErrorMapping { code: 2, message: "contract not initialized", recovery: ErrorRecovery::ManualRecovery },
    // ── Authorization ───────────────────────────────────────────────────────
    ErrorMapping { code: 3, message: "caller is not authorized", recovery: ErrorRecovery::Authorization },
    ErrorMapping { code: 4, message: "caller is not a registered oracle", recovery: ErrorRecovery::Authorization },
    ErrorMapping { code: 5, message: "operation requires admin", recovery: ErrorRecovery::Authorization },
    // ── Feed management ─────────────────────────────────────────────────────
    ErrorMapping { code: 6, message: "feed not found", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 7, message: "feed already exists", recovery: ErrorRecovery::Conflict },
    ErrorMapping { code: 8, message: "feed is not active", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 9, message: "feed value out of range", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 10, message: "submission rate limited", recovery: ErrorRecovery::Retry },
    ErrorMapping { code: 11, message: "feed has been revoked", recovery: ErrorRecovery::ManualRecovery },
    // ── Detection rules ─────────────────────────────────────────────────────
    ErrorMapping { code: 12, message: "rule not found", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 13, message: "rule already exists", recovery: ErrorRecovery::Conflict },
    ErrorMapping { code: 14, message: "invalid rule type", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 15, message: "invalid threshold", recovery: ErrorRecovery::ManualRecovery },
    // ── Alerts ──────────────────────────────────────────────────────────────
    ErrorMapping { code: 16, message: "alert not found", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 17, message: "alert already closed", recovery: ErrorRecovery::Conflict },
    ErrorMapping { code: 18, message: "alert is not open", recovery: ErrorRecovery::Conflict },
    // ── Oracle config / validation ──────────────────────────────────────────
    ErrorMapping { code: 19, message: "invalid severity", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 20, message: "invalid confidence", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 21, message: "invalid feed type", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 22, message: "invalid oracle state", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 23, message: "consensus not reached", recovery: ErrorRecovery::Retry },
    ErrorMapping { code: 24, message: "reading is stale", recovery: ErrorRecovery::Retry },
    // ── Governance ──────────────────────────────────────────────────────────
    ErrorMapping { code: 25, message: "proposal not found", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 26, message: "proposal is not active", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 27, message: "proposal has expired", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 28, message: "already voted", recovery: ErrorRecovery::Conflict },
    ErrorMapping { code: 29, message: "quorum not reached", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 30, message: "invalid proposal type", recovery: ErrorRecovery::ManualRecovery },
    // ── System ──────────────────────────────────────────────────────────────
    ErrorMapping { code: 31, message: "contract is paused", recovery: ErrorRecovery::Retry },
    ErrorMapping { code: 32, message: "reputation not set", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 33, message: "arithmetic overflow", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 34, message: "attestation already used", recovery: ErrorRecovery::Conflict },
    ErrorMapping { code: 35, message: "attestation has expired", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 36, message: "invalid attestation window", recovery: ErrorRecovery::ManualRecovery },
    ErrorMapping { code: 37, message: "invalid attestation domain", recovery: ErrorRecovery::ManualRecovery },
];

/// Look up the public mapping for a raw contract error code.
///
/// Returns `None` for unknown codes so callers can keep unknown errors
/// distinguishable from known ones and log them with transaction context.
pub fn lookup_error(code: u32) -> Option<ErrorMapping> {
    let mut i = 0;
    while i < ERROR_MAPPINGS.len() {
        if ERROR_MAPPINGS[i].code == code {
            return Some(ERROR_MAPPINGS[i]);
        }
        i += 1;
    }
    None
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AntiCheatError {
    // ── Initialization ──────────────────────────────────────────────────────
    AlreadyInitialized = 1,
    NotInitialized = 2,

    // ── Authorization ───────────────────────────────────────────────────────
    Unauthorized = 3,
    /// Caller is not a registered oracle.
    OracleNotAuthorized = 4,
    /// Admin-only operation called by non-admin.
    AdminOnly = 5,

    // ── Feed management ─────────────────────────────────────────────────────
    FeedNotFound = 6,
    FeedAlreadyExists = 7,
    FeedNotActive = 8,
    /// Submitted value is outside the feed's [min_value, max_value] range.
    FeedValueOutOfRange = 9,
    /// Oracle is submitting too frequently (below min_submit_interval).
    SubmitRateLimited = 10,
    /// Feed has been revoked and cannot accept new readings.
    FeedRevoked = 11,

    // ── Detection rules ─────────────────────────────────────────────────────
    RuleNotFound = 12,
    RuleAlreadyExists = 13,
    InvalidRuleType = 14,
    InvalidThreshold = 15,

    // ── Alerts ──────────────────────────────────────────────────────────────
    AlertNotFound = 16,
    AlertAlreadyClosed = 17,
    AlertNotOpen = 18,

    // ── Oracle config / validation ──────────────────────────────────────────
    InvalidSeverity = 19,
    /// confidence value outside 0-100.
    InvalidConfidence = 20,
    InvalidFeedType = 21,
    InvalidOracleState = 22,
    /// Consensus check: too few oracles agree on the submitted value.
    ConsensusFailed = 23,
    /// Reading is too old relative to current ledger time.
    StaleReading = 24,

    // ── Governance ──────────────────────────────────────────────────────────
    ProposalNotFound = 25,
    ProposalNotActive = 26,
    ProposalExpired = 27,
    AlreadyVoted = 28,
    QuorumNotReached = 29,
    InvalidProposalType = 30,

    // ── System ──────────────────────────────────────────────────────────────
    EmergencyPaused = 31,
    ReputationNotSet = 32,
    ArithmeticOverflow = 33,
    AttestationAlreadyUsed = 34,
    AttestationExpired = 35,
    InvalidAttestationWindow = 36,
    InvalidAttestationDomain = 37,
}

impl AntiCheatError {
    /// Stable public code for this error (mirrors the discriminant).
    pub fn code(self) -> u32 {
        self as u32
    }

    /// Public mapping (safe message + recovery class) for this error.
    ///
    /// Every variant has an entry in `ERROR_MAPPINGS`; this is asserted by
    /// contract tests.
    pub fn mapping(self) -> ErrorMapping {
        match lookup_error(self.code()) {
            Some(m) => m,
            // Unreachable: tests assert full coverage. Fall back to a safe,
            // non-leaking message rather than panicking.
            None => ErrorMapping {
                code: self.code(),
                message: "unknown contract error",
                recovery: ErrorRecovery::ManualRecovery,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every public variant, in discriminant order.
    const ALL_VARIANTS: &[AntiCheatError] = &[
        AntiCheatError::AlreadyInitialized,
        AntiCheatError::NotInitialized,
        AntiCheatError::Unauthorized,
        AntiCheatError::OracleNotAuthorized,
        AntiCheatError::AdminOnly,
        AntiCheatError::FeedNotFound,
        AntiCheatError::FeedAlreadyExists,
        AntiCheatError::FeedNotActive,
        AntiCheatError::FeedValueOutOfRange,
        AntiCheatError::SubmitRateLimited,
        AntiCheatError::FeedRevoked,
        AntiCheatError::RuleNotFound,
        AntiCheatError::RuleAlreadyExists,
        AntiCheatError::InvalidRuleType,
        AntiCheatError::InvalidThreshold,
        AntiCheatError::AlertNotFound,
        AntiCheatError::AlertAlreadyClosed,
        AntiCheatError::AlertNotOpen,
        AntiCheatError::InvalidSeverity,
        AntiCheatError::InvalidConfidence,
        AntiCheatError::InvalidFeedType,
        AntiCheatError::InvalidOracleState,
        AntiCheatError::ConsensusFailed,
        AntiCheatError::StaleReading,
        AntiCheatError::ProposalNotFound,
        AntiCheatError::ProposalNotActive,
        AntiCheatError::ProposalExpired,
        AntiCheatError::AlreadyVoted,
        AntiCheatError::QuorumNotReached,
        AntiCheatError::InvalidProposalType,
        AntiCheatError::EmergencyPaused,
        AntiCheatError::ReputationNotSet,
        AntiCheatError::ArithmeticOverflow,
        AntiCheatError::AttestationAlreadyUsed,
        AntiCheatError::AttestationExpired,
        AntiCheatError::InvalidAttestationWindow,
        AntiCheatError::InvalidAttestationDomain,
    ];

    #[test]
    fn every_variant_has_a_mapping() {
        for variant in ALL_VARIANTS {
            let mapping = lookup_error(variant.code());
            assert!(mapping.is_some(), "missing mapping for code {}", variant.code());
            let mapping = mapping.unwrap();
            assert_eq!(mapping.code, variant.code());
            assert!(!mapping.message.is_empty());
        }
    }

    #[test]
    fn mapping_table_covers_exactly_the_public_variants() {
        assert_eq!(ERROR_MAPPINGS.len(), ALL_VARIANTS.len());
        for (i, mapping) in ERROR_MAPPINGS.iter().enumerate() {
            assert_eq!(mapping.code, ALL_VARIANTS[i].code());
        }
    }

    #[test]
    fn codes_are_unique_and_stable() {
        for (i, a) in ERROR_MAPPINGS.iter().enumerate() {
            for b in ERROR_MAPPINGS.iter().skip(i + 1) {
                assert_ne!(a.code, b.code, "duplicate code {}", a.code);
            }
        }
        // Spot-check stability of the public surface.
        assert_eq!(AntiCheatError::Unauthorized.code(), 3);
        assert_eq!(AntiCheatError::InvalidAttestationDomain.code(), 37);
    }

    #[test]
    fn unknown_error_is_distinguishable() {
        assert!(lookup_error(0).is_none());
        assert!(lookup_error(9999).is_none());
    }

    #[test]
    fn retry_and_conflict_classifications_are_present() {
        assert_eq!(
            AntiCheatError::SubmitRateLimited.mapping().recovery,
            ErrorRecovery::Retry
        );
        assert_eq!(
            AntiCheatError::AlreadyVoted.mapping().recovery,
            ErrorRecovery::Conflict
        );
        assert_eq!(
            AntiCheatError::Unauthorized.mapping().recovery,
            ErrorRecovery::Authorization
        );
    }
}
