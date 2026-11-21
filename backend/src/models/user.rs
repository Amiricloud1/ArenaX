use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub phone_number: Option<String>,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub country_code: Option<String>,
    pub is_verified: bool,
    pub is_active: bool,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub skill_score: Option<i32>,
    pub fair_play_score: Option<i32>,
    pub reputation_last_updated: Option<DateTime<Utc>>,
    pub anticheat_flags_count: Option<i32>,
    pub is_bad_actor: Option<bool>,
    pub is_beta_tester: bool,
    // Extended fields (may not be in all DB versions)
    pub password_hash: Option<String>,
    pub profile_image_url: Option<String>,
    pub reputation_score: Option<i32>,
    pub stellar_account_id: Option<Uuid>,
    pub stellar_public_key: Option<String>,
    pub total_earnings: Option<rust_decimal::Decimal>,
    pub is_banned: Option<bool>,
    pub banned_until: Option<DateTime<Utc>>,
    pub device_fingerprint: Option<String>,
    /// Soft-delete marker (GDPR/NDPR erasure). Set at account deletion;
    /// PII fields are anonymised and login is refused.
    pub deleted_at: Option<DateTime<Utc>>,
    // Avatar pipeline state (privacy-safe upload/transformation).
    /// Stable processing state of the most recent avatar upload attempt.
    pub avatar_processing_state: Option<AvatarProcessingState>,
    /// Bounded derivative URLs generated from the last successful upload.
    /// The previous avatar (`avatar_url`) is only replaced once these are ready.
    pub avatar_thumbnail_url: Option<String>,
    pub avatar_medium_url: Option<String>,
    /// User-visible, recoverable error from the last failed upload attempt.
    pub avatar_processing_error: Option<String>,
    /// Timestamp of the last avatar state transition (upload/scan/transform).
    pub avatar_updated_at: Option<DateTime<Utc>>,
}

/// Stable, persisted processing state for the avatar pipeline.
///
/// Stored as a lowercase string in the database so the value is stable across
/// deploys and readable by operators. The previous avatar remains intact for
/// every state other than `Ready`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarProcessingState {
    /// Upload accepted; awaiting scan/transformation.
    Pending,
    /// Malware/content scan in progress.
    Scanning,
    /// Scan passed; generating bounded derivatives.
    Transforming,
    /// Derivatives ready; safe to render in public UI.
    Ready,
    /// Scan or transformation failed; previous avatar preserved.
    Failed,
}

impl AvatarProcessingState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AvatarProcessingState::Pending => "pending",
            AvatarProcessingState::Scanning => "scanning",
            AvatarProcessingState::Transforming => "transforming",
            AvatarProcessingState::Ready => "ready",
            AvatarProcessingState::Failed => "failed",
        }
    }

    /// Parse a persisted state string. Unknown values are treated as `Failed`
    /// so an unexpected state never exposes an unprocessed original.
    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "pending" => AvatarProcessingState::Pending,
            "scanning" => AvatarProcessingState::Scanning,
            "transforming" => AvatarProcessingState::Transforming,
            "ready" => AvatarProcessingState::Ready,
            _ => AvatarProcessingState::Failed,
        }
    }

    /// Whether the pipeline has settled (no further transitions expected).
    pub fn is_terminal(&self) -> bool {
        matches!(self, AvatarProcessingState::Ready | AvatarProcessingState::Failed)
    }

    /// Whether derivatives from this state may be rendered publicly.
    pub fn is_renderable(&self) -> bool {
        matches!(self, AvatarProcessingState::Ready)
    }
}

impl std::fmt::Display for AvatarProcessingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for AvatarProcessingState {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(AvatarProcessingState::Pending),
            "scanning" => Ok(AvatarProcessingState::Scanning),
            "transforming" => Ok(AvatarProcessingState::Transforming),
            "ready" => Ok(AvatarProcessingState::Ready),
            "failed" => Ok(AvatarProcessingState::Failed),
            _ => Err(()),
        }
    }
}

impl User {
    /// Encrypt all sensitive PII fields (email, phone_number, device_fingerprint) in place.
    pub fn encrypt_pii(&mut self, actor_id: Option<&str>) -> Result<(), crate::security::EncryptionError> {
        let protector = &crate::security::encryption::GLOBAL_PROTECTOR;
        if let Some(ref phone) = self.phone_number {
            if !phone.starts_with("enc:") {
                self.phone_number = Some(protector.encrypt_str(phone, "user.phone_number", actor_id)?);
            }
        }
        if let Some(ref email) = self.email {
            if !email.starts_with("enc:") {
                self.email = Some(protector.encrypt_str(email, "user.email", actor_id)?);
            }
        }
        if let Some(ref fp) = self.device_fingerprint {
            if !fp.starts_with("enc:") {
                self.device_fingerprint = Some(protector.encrypt_str(fp, "user.device_fingerprint", actor_id)?);
            }
        }
        Ok(())
    }

    /// Decrypt all sensitive PII fields in place.
    pub fn decrypt_pii(&mut self, actor_id: Option<&str>) -> Result<(), crate::security::EncryptionError> {
        let protector = &crate::security::encryption::GLOBAL_PROTECTOR;
        if let Some(ref phone) = self.phone_number {
            if phone.starts_with("enc:") {
                self.phone_number = Some(protector.decrypt_str(phone, "user.phone_number", actor_id)?);
            }
        }
        if let Some(ref email) = self.email {
            if email.starts_with("enc:") {
                self.email = Some(protector.decrypt_str(email, "user.email", actor_id)?);
            }
        }
        if let Some(ref fp) = self.device_fingerprint {
            if fp.starts_with("enc:") {
                self.device_fingerprint = Some(protector.decrypt_str(fp, "user.device_fingerprint", actor_id)?);
            }
        }
        Ok(())
    }

    /// Re-encrypt PII fields to the latest active key version.
    pub fn reencrypt_pii(&mut self, actor_id: Option<&str>) -> Result<(), crate::security::EncryptionError> {
        let protector = &crate::security::encryption::GLOBAL_PROTECTOR;
        if let Some(ref phone) = self.phone_number {
            if phone.starts_with("enc:") {
                self.phone_number = Some(protector.reencrypt(phone, "user.phone_number", actor_id)?);
            }
        }
        if let Some(ref email) = self.email {
            if email.starts_with("enc:") {
                self.email = Some(protector.reencrypt(email, "user.email", actor_id)?);
            }
        }
        if let Some(ref fp) = self.device_fingerprint {
            if fp.starts_with("enc:") {
                self.device_fingerprint = Some(protector.reencrypt(fp, "user.device_fingerprint", actor_id)?);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: Option<String>,
    pub phone_number: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub refresh_token: String,
    pub user: UserProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_verified: bool,
    pub created_at: DateTime<Utc>,
    // Reputation fields
    pub skill_score: Option<i32>,
    pub fair_play_score: Option<i32>,
    pub is_bad_actor: Option<bool>,
    // Avatar pipeline state exposed to the client so it can render a stable
    // processing indicator and a recoverable error without leaking originals.
    pub avatar_processing_state: Option<AvatarProcessingState>,
    pub avatar_thumbnail_url: Option<String>,
    pub avatar_medium_url: Option<String>,
    pub avatar_processing_error: Option<String>,
}
