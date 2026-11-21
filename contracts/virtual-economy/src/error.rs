use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VirtualEconomyError {
    // General errors
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    EmergencyPaused = 4,
    /// An arithmetic operation would overflow.
    Overflow = 5,
    /// The provided address is invalid (e.g. the contract's own address).
    InvalidAddress = 6,

    // Currency errors
    InvalidAmount = 10,
    InsufficientBalance = 11,
    SupplyLimitExceeded = 12,

    // NFT errors
    TokenNotFound = 20,
    TokenAlreadyExists = 21,
    NotOwner = 22,

    // Marketplace errors
    InvalidPrice = 30,
    OrderNotFound = 31,
    OrderNotActive = 32,
    OrderExpired = 33,

    // Validation errors
    InvalidMetadata = 40,
    InvalidConfig = 41,

    // Royalty & Licensing errors (merged with InvalidConfig to stay within 50-case XDR limit)
    RoyaltyTooHigh = 50,

    // Dynamic pricing errors
    AuctionNotFound = 60,
    AuctionNotActive = 61,
    AuctionEnded = 62,
    InvalidAuctionParams = 63,

    DropNotFound = 70,
    DropInactive = 71,
    DropSupplyExceeded = 72,
    InvalidCurveParams = 73,

    // Price oracle errors
    OracleNotConfigured = 80,
    InvalidOracleConfig = 81,
    OraclePriceStale = 82,
    OraclePriceVarianceTooHigh = 83,
    OracleUpdateTooFrequent = 84,
    InvalidAssetPair = 85,
    OracleInvalidPrice = 86,

    // NFT staking errors
    NftStakingNotConfigured = 90,
    NftCurrentlyStaked = 91,
    NftNotStaked = 92,
    NftLockPeriodNotMet = 93,
    NftStakingPaused = 94,

    // Liquidity pool / AMM
    PoolNotFound = 100,
    PoolAlreadyExists = 101,
    InsufficientLiquidity = 102,
    SlippageExceeded = 103,
    InvariantViolation = 104,

    // Referral errors
    ReferralNotConfigured = 110,
    ReferralNotFound = 111,
    ReferralAlreadyRegistered = 112,
    InvalidReferral = 113,
    ReferralCooldown = 114,
    NothingToClaim = 115,
    // NFT collections (#913) and trading rebates (#916) reuse InvalidConfig
    // and TokenNotFound / ReferralCooldown above — this contracterror enum
    // is capped at 50 cases by the XDR spec (ScSpecUdtErrorEnumV0), and it
    // was already at that cap, so new features must reuse existing cases
    // rather than add new ones.
}

/// Retry / manual-recovery classification for a public contract error.
///
/// Mirrored by the backend error catalog (`backend/src/errors/contract.ts`).
/// Keep the two in sync: the backend catalog is keyed by the numeric code.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ErrorRecovery {
    /// The caller may safely retry the same operation (possibly after a
    /// short delay). No state was mutated by the failed call.
    Retry,
    /// The caller must change inputs or state before retrying; retrying the
    /// identical call will fail again.
    ManualRecovery,
    /// The failure is final for this operation; do not retry. Escalate to
    /// support if the caller believes the outcome is wrong.
    Final,
}

impl VirtualEconomyError {
    /// Stable, user-readable message for this error.
    ///
    /// Messages are safe to surface to clients: they never include signing
    /// material, addresses, or internal state.
    pub fn safe_message(self) -> &'static str {
        match self {
            Self::AlreadyInitialized => "Contract is already initialized.",
            Self::NotInitialized => "Contract is not initialized.",
            Self::Unauthorized => "Caller is not authorized for this operation.",
            Self::EmergencyPaused => "Contract is paused for emergency maintenance.",
            Self::Overflow => "Arithmetic overflow.",
            Self::InvalidAddress => "Address is not valid for this operation.",
            Self::InvalidAmount => "Amount is not valid.",
            Self::InsufficientBalance => "Insufficient balance.",
            Self::SupplyLimitExceeded => "Supply limit exceeded.",
            Self::TokenNotFound => "Token not found.",
            Self::TokenAlreadyExists => "Token already exists.",
            Self::NotOwner => "Caller does not own this token.",
            Self::InvalidPrice => "Price is not valid.",
            Self::OrderNotFound => "Order not found.",
            Self::OrderNotActive => "Order is not active.",
            Self::OrderExpired => "Order has expired.",
            Self::InvalidMetadata => "Metadata is not valid.",
            Self::InvalidConfig => "Configuration is not valid.",
            Self::RoyaltyTooHigh => "Royalty exceeds the maximum allowed.",
            Self::AuctionNotFound => "Auction not found.",
            Self::AuctionNotActive => "Auction is not active.",
            Self::AuctionEnded => "Auction has ended.",
            Self::InvalidAuctionParams => "Auction parameters are not valid.",
            Self::DropNotFound => "Drop not found.",
            Self::DropInactive => "Drop is not active.",
            Self::DropSupplyExceeded => "Drop supply exceeded.",
            Self::InvalidCurveParams => "Pricing curve parameters are not valid.",
            Self::OracleNotConfigured => "Price oracle is not configured.",
            Self::InvalidOracleConfig => "Price oracle configuration is not valid.",
            Self::OraclePriceStale => "Oracle price is stale.",
            Self::OraclePriceVarianceTooHigh => "Oracle price variance is too high.",
            Self::OracleUpdateTooFrequent => "Oracle update attempted too frequently.",
            Self::InvalidAssetPair => "Asset pair is not valid.",
            Self::OracleInvalidPrice => "Oracle returned an invalid price.",
            Self::NftStakingNotConfigured => "NFT staking is not configured.",
            Self::NftCurrentlyStaked => "NFT is currently staked.",
            Self::NftNotStaked => "NFT is not staked.",
            Self::NftLockPeriodNotMet => "NFT lock period has not been met.",
            Self::NftStakingPaused => "NFT staking is paused.",
            Self::PoolNotFound => "Liquidity pool not found.",
            Self::PoolAlreadyExists => "Liquidity pool already exists.",
            Self::InsufficientLiquidity => "Insufficient liquidity.",
            Self::SlippageExceeded => "Slippage tolerance exceeded.",
            Self::InvariantViolation => "Pool invariant violated.",
            Self::ReferralNotConfigured => "Referral program is not configured.",
            Self::ReferralNotFound => "Referral not found.",
            Self::ReferralAlreadyRegistered => "Referral is already registered.",
            Self::InvalidReferral => "Referral is not valid.",
            Self::ReferralCooldown => "Referral cooldown is still active.",
            Self::NothingToClaim => "Nothing to claim.",
        }
    }

    /// Retry / manual-recovery classification for this error.
    pub fn recovery(self) -> ErrorRecovery {
        match self {
            // Transient or caller-fixable conditions: safe to retry.
            Self::EmergencyPaused
            | Self::OraclePriceStale
            | Self::OracleUpdateTooFrequent
            | Self::ReferralCooldown
            | Self::SlippageExceeded => ErrorRecovery::Retry,

            // Caller must change inputs or state first.
            Self::NotInitialized
            | Self::InvalidAddress
            | Self::InvalidAmount
            | Self::InsufficientBalance
            | Self::SupplyLimitExceeded
            | Self::InvalidPrice
            | Self::OrderNotActive
            | Self::OrderExpired
            | Self::InvalidMetadata
            | Self::InvalidConfig
            | Self::RoyaltyTooHigh
            | Self::AuctionNotActive
            | Self::AuctionEnded
            | Self::InvalidAuctionParams
            | Self::DropInactive
            | Self::DropSupplyExceeded
            | Self::InvalidCurveParams
            | Self::OracleNotConfigured
            | Self::InvalidOracleConfig
            | Self::OraclePriceVarianceTooHigh
            | Self::InvalidAssetPair
            | Self::OracleInvalidPrice
            | Self::NftStakingNotConfigured
            | Self::NftCurrentlyStaked
            | Self::NftNotStaked
            | Self::NftLockPeriodNotMet
            | Self::NftStakingPaused
            | Self::InsufficientLiquidity
            | Self::ReferralNotConfigured
            | Self::ReferralAlreadyRegistered
            | Self::InvalidReferral
            | Self::NothingToClaim => ErrorRecovery::ManualRecovery,

            // Final: authorization, existence, and invariant failures.
            Self::AlreadyInitialized
            | Self::Unauthorized
            | Self::Overflow
            | Self::TokenNotFound
            | Self::TokenAlreadyExists
            | Self::NotOwner
            | Self::OrderNotFound
            | Self::AuctionNotFound
            | Self::DropNotFound
            | Self::PoolNotFound
            | Self::PoolAlreadyExists
            | Self::InvariantViolation
            | Self::ReferralNotFound => ErrorRecovery::Final,
        }
    }

    /// Re-entrant marketplace call (#1056).
    ///
    /// Alias of [`Self::InvalidMetadata`]. Trades never return that case for
    /// metadata, and the contracterror enum cannot grow past 50 variants.
    #[allow(non_upper_case_globals)]
    pub const Reentrancy: Self = Self::InvalidMetadata;

    /// Fewer than two non-stale oracle sources were available (#1053).
    ///
    /// Alias of [`Self::OraclePriceStale`] for the same 50-variant cap.
    /// Legacy primary/fallback resolution still returns `OraclePriceStale`
    /// when no multi-oracle sources are registered.
    #[allow(non_upper_case_globals)]
    pub const OracleInsufficientSources: Self = Self::OraclePriceStale;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every public variant, in declaration order. Kept explicit so adding a
    /// variant without a mapping fails the build.
    const ALL_VARIANTS: &[VirtualEconomyError] = &[
        VirtualEconomyError::AlreadyInitialized,
        VirtualEconomyError::NotInitialized,
        VirtualEconomyError::Unauthorized,
        VirtualEconomyError::EmergencyPaused,
        VirtualEconomyError::Overflow,
        VirtualEconomyError::InvalidAddress,
        VirtualEconomyError::InvalidAmount,
        VirtualEconomyError::InsufficientBalance,
        VirtualEconomyError::SupplyLimitExceeded,
        VirtualEconomyError::TokenNotFound,
        VirtualEconomyError::TokenAlreadyExists,
        VirtualEconomyError::NotOwner,
        VirtualEconomyError::InvalidPrice,
        VirtualEconomyError::OrderNotFound,
        VirtualEconomyError::OrderNotActive,
        VirtualEconomyError::OrderExpired,
        VirtualEconomyError::InvalidMetadata,
        VirtualEconomyError::InvalidConfig,
        VirtualEconomyError::RoyaltyTooHigh,
        VirtualEconomyError::AuctionNotFound,
        VirtualEconomyError::AuctionNotActive,
        VirtualEconomyError::AuctionEnded,
        VirtualEconomyError::InvalidAuctionParams,
        VirtualEconomyError::DropNotFound,
        VirtualEconomyError::DropInactive,
        VirtualEconomyError::DropSupplyExceeded,
        VirtualEconomyError::InvalidCurveParams,
        VirtualEconomyError::OracleNotConfigured,
        VirtualEconomyError::InvalidOracleConfig,
        VirtualEconomyError::OraclePriceStale,
        VirtualEconomyError::OraclePriceVarianceTooHigh,
        VirtualEconomyError::OracleUpdateTooFrequent,
        VirtualEconomyError::InvalidAssetPair,
        VirtualEconomyError::OracleInvalidPrice,
        VirtualEconomyError::NftStakingNotConfigured,
        VirtualEconomyError::NftCurrentlyStaked,
        VirtualEconomyError::NftNotStaked,
        VirtualEconomyError::NftLockPeriodNotMet,
        VirtualEconomyError::NftStakingPaused,
        VirtualEconomyError::PoolNotFound,
        VirtualEconomyError::PoolAlreadyExists,
        VirtualEconomyError::InsufficientLiquidity,
        VirtualEconomyError::SlippageExceeded,
        VirtualEconomyError::InvariantViolation,
        VirtualEconomyError::ReferralNotConfigured,
        VirtualEconomyError::ReferralNotFound,
        VirtualEconomyError::ReferralAlreadyRegistered,
        VirtualEconomyError::InvalidReferral,
        VirtualEconomyError::ReferralCooldown,
        VirtualEconomyError::NothingToClaim,
    ];

    #[test]
    fn every_variant_has_a_safe_message() {
        for err in ALL_VARIANTS {
            let msg = err.safe_message();
            assert!(!msg.is_empty(), "missing message for {:?}", err);
            assert!(
                msg.chars().all(|c| c.is_ascii()),
                "message must be ASCII for {:?}",
                err
            );
        }
    }

    #[test]
    fn every_variant_has_a_recovery_classification() {
        for err in ALL_VARIANTS {
            // Exhaustive match: adding a variant without classifying it fails
            // to compile in `recovery`.
            let _ = err.recovery();
        }
    }

    #[test]
    fn codes_are_stable_and_unique() {
        let mut codes: Vec<u32> = ALL_VARIANTS.iter().map(|e| *e as u32).collect();
        let len_before = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), len_before, "duplicate error codes");
    }

    #[test]
    fn aliases_share_their_canonical_mapping() {
        assert_eq!(
            VirtualEconomyError::Reentrancy.safe_message(),
            VirtualEconomyError::InvalidMetadata.safe_message()
        );
        assert_eq!(
            VirtualEconomyError::OracleInsufficientSources.recovery(),
            VirtualEconomyError::OraclePriceStale.recovery()
        );
    }
}
