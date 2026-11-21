use crate::models::notification::{Notification, NotificationKind, NotificationPreference};
use crate::models::security_event::{SecurityEvent, SecurityEventKind};
use std::collections::HashSet;
use std::sync::Mutex;

/// Base URL used to build the safe account activity link. Never includes
/// user-supplied data so the link cannot be tampered with.
const ACCOUNT_ACTIVITY_URL: &str = "https://app.example.com/account/activity";
const ACCOUNT_LOCK_URL: &str = "https://app.example.com/account/lock";
const ACCOUNT_REPORT_URL: &str = "https://app.example.com/account/report";

/// Tracks already-delivered security events so retries are idempotent.
pub struct NotificationService {
    delivered: Mutex<HashSet<String>>,
}

impl NotificationService {
    pub fn new() -> Self {
        Self {
            delivered: Mutex::new(HashSet::new()),
        }
    }

    /// Deliver a security notification for the given event.
    ///
    /// Security notifications are always delivered regardless of marketing
    /// preferences, contain only redacted content, and are idempotent per
    /// event id so retries do not produce duplicates.
    pub fn notify_security_event(
        &self,
        event: &SecurityEvent,
        prefs: &NotificationPreference,
    ) -> Result<Option<Notification>, NotificationError> {
        // Security notifications are mandatory; marketing opt-out must not
        // suppress them. We only check that the account is not fully locked
        // out of all notifications.
        if prefs.suppress_all_notifications {
            return Ok(None);
        }

        let dedupe_key = event.dedupe_key();
        {
            let mut delivered = self
                .delivered
                .lock()
                .map_err(|_| NotificationError::Internal)?;
            if !delivered.insert(dedupe_key) {
                // Already delivered; retry is a no-op.
                return Ok(None);
            }
        }

        let notification = self.build_security_notification(event);
        Ok(Some(notification))
    }

    fn build_security_notification(&self, event: &SecurityEvent) -> Notification {
        let (title, body) = match event.kind {
            SecurityEventKind::NewDevice => (
                "New device signed in",
                "A new device accessed your account. If this was not you, lock your account now.",
            ),
            SecurityEventKind::CredentialChange => (
                "Credentials changed",
                "Your account credentials were changed. If this was not you, lock your account now.",
            ),
            SecurityEventKind::PayoutDestinationChange => (
                "Payout destination changed",
                "A payout destination was updated. If this was not you, lock your account now.",
            ),
        };

        Notification {
            kind: NotificationKind::Security,
            title: title.to_string(),
            body: body.to_string(),
            // Safe, static links only. No email, wallet, IP, or transaction
            // data is embedded in the preview or the link.
            activity_url: ACCOUNT_ACTIVITY_URL.to_string(),
            actions: vec![
                NotificationAction {
                    label: "Review activity".to_string(),
                    url: ACCOUNT_ACTIVITY_URL.to_string(),
                },
                NotificationAction {
                    label: "Lock account".to_string(),
                    url: ACCOUNT_LOCK_URL.to_string(),
                },
                NotificationAction {
                    label: "Report activity".to_string(),
                    url: ACCOUNT_REPORT_URL.to_string(),
                },
            ],
        }
    }
}

impl Default for NotificationService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub enum NotificationError {
    Internal,
}

pub struct NotificationAction {
    pub label: String,
    pub url: String,
}
