use crate::models::{Notification, NotificationKind, SecurityEvent, SecurityEventKind};
use crate::services::notifications::{NotificationService, NotificationError};
use std::collections::HashSet;
use std::sync::Mutex;

/// Minimal-content security notification builder.
///
/// Security notifications are always delivered regardless of marketing
/// preferences, contain only redacted details, and carry a safe account
/// activity link plus an explicit report/lock action.
pub struct SecurityNotificationService {
    notifications: NotificationService,
    /// Tracks already-delivered event ids so retries are idempotent.
    delivered: Mutex<HashSet<String>>,
}

impl SecurityNotificationService {
    pub fn new(notifications: NotificationService) -> Self {
        Self {
            notifications,
            delivered: Mutex::new(HashSet::new()),
        }
    }

    /// Deliver a security notification for the given event.
    ///
    /// Returns `Ok(false)` when the event was already delivered (idempotent
    /// retry), `Ok(true)` when a new notification was sent.
    pub fn notify(&self, event: &SecurityEvent) -> Result<bool, NotificationError> {
        // Idempotency: duplicate events must not produce duplicate notifications.
        {
            let mut delivered = self.delivered.lock().unwrap();
            if !delivered.insert(event.id.clone()) {
                return Ok(false);
            }
        }

        let notification = self.build_notification(event);

        // Security notifications bypass marketing preferences: they are
        // required and must never be suppressed by disabled marketing.
        match self.notifications.send_security(&notification) {
            Ok(()) => Ok(true),
            Err(err) => {
                // Allow retry on failure by releasing the idempotency marker.
                self.delivered.lock().unwrap().remove(&event.id);
                Err(err)
            }
        }
    }

    fn build_notification(&self, event: &SecurityEvent) -> Notification {
        let (title, body) = match event.kind {
            SecurityEventKind::NewDevice => (
                "New device signed in".to_string(),
                format!(
                    "A new device signed in to your account{}. If this was not you, lock your account now.",
                    Self::device_hint(&event.device)
                ),
            ),
            SecurityEventKind::CredentialChange => (
                "Account credentials changed".to_string(),
                "Your password or sign-in credentials were changed. If this was not you, lock your account now."
                    .to_string(),
            ),
            SecurityEventKind::PayoutDestinationChange => (
                "Payout destination changed".to_string(),
                format!(
                    "A payout destination was updated{}. If this was not you, lock your account now.",
                    Self::wallet_hint(&event.wallet)
                ),
            ),
        };

        Notification {
            kind: NotificationKind::Security,
            title,
            body,
            // Safe, non-sensitive account activity link.
            action_url: Some("/account/activity".to_string()),
            // Explicit supported report/lock action.
            action_label: Some("Review activity & lock account".to_string()),
            // Never include raw sensitive data in previews.
            preview: Some("Security alert for your account. Review activity.".to_string()),
        }
    }

    /// Redact a device identifier to a short, non-identifying hint.
    fn device_hint(device: &Option<String>) -> String {
        match device.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            Some(device) => format!(" from a device ending in {}", Self::mask_tail(device)),
            None => String::new(),
        }
    }

    /// Redact a wallet address to a short, non-identifying hint.
    fn wallet_hint(wallet: &Option<String>) -> String {
        match wallet.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
            Some(wallet) => format!(" to a destination ending in {}", Self::mask_tail(wallet)),
            None => String::new(),
        }
    }

    /// Return only the last 4 characters of a sensitive value.
    fn mask_tail(value: &str) -> String {
        let chars: Vec<char> = value.chars().collect();
        let start = chars.len().saturating_sub(4);
        let tail: String = chars[start..].iter().collect();
        format!("…{}", tail)
    }
}
