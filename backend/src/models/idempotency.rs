use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct IdempotencyKey {
    pub id: Uuid,
    pub key: String,
    pub request_hash: String,
    pub response_status: i16,
    pub response_headers: Option<serde_json::Value>,
    pub response_body: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct IdempotencyConfig {
    pub id: Uuid,
    pub route_pattern: String,
    pub enabled: bool,
    pub ttl_seconds: i32,
    pub max_response_size_kb: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyResponse {
    pub status: i16,
    pub headers: Option<serde_json::Value>,
    pub body: Option<serde_json::Value>,
    pub is_cached: bool,
    pub cached_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyConflict {
    pub key: String,
    pub original_hash: String,
    pub new_hash: String,
    pub original_timestamp: DateTime<Utc>,
    pub conflict_type: ConflictType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictType {
    PayloadMismatch,
    MethodMismatch,
    RouteMismatch,
    UserMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyPolicy {
    /// Apply idempotency to ALL POST and PUT methods globally
    pub enabled: bool,
    pub enabled_routes: Vec<String>,
    /// Header name for the idempotency key (default: "Idempotency-Key")
    pub key_header_name: String,
    /// TTL in seconds for cached responses (default: 86400 = 24 hours)
    pub ttl_seconds: u64,
    pub default_ttl_seconds: i32,
    /// Max response body size in KB to cache (default: 1024 = 1MB)
    pub max_response_size_kb: u32,
    /// Status code returned on idempotency key conflict (default: 422)
    pub conflict_status_code: u16,
}

impl Default for IdempotencyPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            enabled_routes: vec![
                "/api/payments/create".to_string(),
                "/api/payments/refund".to_string(),
                "/api/wallets/deposit".to_string(),
                "/api/wallets/withdraw".to_string(),
                "/api/tournaments/join".to_string(),
                "/api/matchmaking/join".to_string(),
            ],
            key_header_name: "Idempotency-Key".to_string(),
            ttl_seconds: 86400, // 24 hours
            default_ttl_seconds: 86400,
            max_response_size_kb: 1024, // 1MB
            conflict_status_code: 422,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    pub request_hash: String,
    pub status: u16,
    pub headers: serde_json::Value,
    pub body: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl CachedResponse {
    pub fn new(
        request_hash: String,
        status: u16,
        headers: serde_json::Value,
        body: serde_json::Value,
    ) -> Self {
        Self {
            request_hash,
            status,
            headers,
            body,
            created_at: Utc::now(),
        }
    }

    pub fn size_bytes(&self) -> usize {
        let serialized = serde_json::to_string(&self).unwrap_or_default();
        serialized.len()
    }

    pub fn size_kb(&self) -> u32 {
        (self.size_bytes() / 1024) as u32
    }
}

/// Metrics for idempotency middleware tracking cache performance and errors.
#[derive(Debug, Clone)]
pub struct IdempotencyMetrics {
    /// Total cache hits (duplicate request with same key)
    pub hits: Arc<AtomicU64>,
    /// Total cache misses (new request key)
    pub misses: Arc<AtomicU64>,
    /// Total idempotency key conflicts (same key, different request data)
    pub conflicts: Arc<AtomicU64>,
    /// Total Redis errors (Redis unavailable, etc.)
    pub redis_errors: Arc<AtomicU64>,
}

impl IdempotencyMetrics {
    pub fn new() -> Self {
        Self {
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            conflicts: Arc::new(AtomicU64::new(0)),
            redis_errors: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn increment_hits(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_misses(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_conflicts(&self) {
        self.conflicts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_redis_errors(&self) {
        self.redis_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn get_misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    pub fn get_conflicts(&self) -> u64 {
        self.conflicts.load(Ordering::Relaxed)
    }

    pub fn get_redis_errors(&self) -> u64 {
        self.redis_errors.load(Ordering::Relaxed)
    }

    /// Calculate hit rate as percentage (hits / (hits + misses))
    pub fn hit_rate(&self) -> f64 {
        let hits = self.get_hits();
        let misses = self.get_misses();
        let total = hits + misses;

        if total == 0 {
            0.0
        } else {
            (hits as f64 / total as f64) * 100.0
        }
    }

    /// Get snapshot of all metrics for reporting
    pub fn snapshot(&self) -> IdempotencyMetricsSnapshot {
        IdempotencyMetricsSnapshot {
            hits: self.get_hits(),
            misses: self.get_misses(),
            conflicts: self.get_conflicts(),
            redis_errors: self.get_redis_errors(),
            hit_rate: self.hit_rate(),
        }
    }
}

impl Default for IdempotencyMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Point-in-time snapshot of idempotency metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyMetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub conflicts: u64,
    pub redis_errors: u64,
    pub hit_rate: f64,
}

/// Outcome recorded at the analytics ingestion boundary for a single event.
///
/// Every producer assigns a stable `event_id`; the ingestion boundary persists
/// exactly one outcome per `event_id` so retries converge to a single accepted
/// event and malformed payloads land in a diagnosable rejection path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsIngestionOutcome {
    /// Event passed validation and was durably accepted.
    Accepted,
    /// Event was rejected; see `AnalyticsRejectionReason`.
    Rejected,
}

/// Why an analytics event was rejected at the ingestion boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsRejectionReason {
    /// Payload could not be parsed (invalid JSON, wrong types, etc.).
    MalformedPayload,
    /// Payload parsed but failed schema validation.
    SchemaInvalid,
    /// Event schema version is not supported by the ingestion boundary.
    UnsupportedSchemaVersion,
    /// Event ID was missing or not a stable identifier.
    InvalidEventId,
    /// Producer is not authorized to submit analytics events.
    Unauthorized,
    /// Event timestamp was outside the accepted window.
    StaleTimestamp,
}

impl AnalyticsRejectionReason {
    /// Stable machine-readable code used for diagnostics and metrics.
    pub fn code(&self) -> &'static str {
        match self {
            AnalyticsRejectionReason::MalformedPayload => "malformed_payload",
            AnalyticsRejectionReason::SchemaInvalid => "schema_invalid",
            AnalyticsRejectionReason::UnsupportedSchemaVersion => "unsupported_schema_version",
            AnalyticsRejectionReason::InvalidEventId => "invalid_event_id",
            AnalyticsRejectionReason::Unauthorized => "unauthorized",
            AnalyticsRejectionReason::StaleTimestamp => "stale_timestamp",
        }
    }
}

/// Durable record of an analytics ingestion decision keyed by event identity.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AnalyticsIngestionRecord {
    pub id: Uuid,
    /// Producer-assigned stable event identifier.
    pub event_id: String,
    /// Producer identifier (e.g. client instance or service name).
    pub producer_id: String,
    /// Schema version of the submitted event payload.
    pub schema_version: i32,
    /// Hash of the canonical payload, used to detect conflicting retries.
    pub payload_hash: String,
    pub outcome: AnalyticsIngestionOutcome,
    /// Present only when `outcome` is `Rejected`.
    pub rejection_reason: Option<AnalyticsRejectionReason>,
    /// Human-readable diagnostic detail for rejected events.
    pub rejection_detail: Option<String>,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

impl AnalyticsIngestionRecord {
    /// Build an accepted record for a validated event.
    pub fn accepted(
        event_id: String,
        producer_id: String,
        schema_version: i32,
        payload_hash: String,
        ttl_seconds: i64,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            event_id,
            producer_id,
            schema_version,
            payload_hash,
            outcome: AnalyticsIngestionOutcome::Accepted,
            rejection_reason: None,
            rejection_detail: None,
            received_at: now,
            processed_at: Some(now),
            expires_at: now + chrono::Duration::seconds(ttl_seconds),
        }
    }

    /// Build a rejected record for a malformed or schema-invalid event.
    pub fn rejected(
        event_id: String,
        producer_id: String,
        schema_version: i32,
        payload_hash: String,
        reason: AnalyticsRejectionReason,
        detail: Option<String>,
        ttl_seconds: i64,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            event_id,
            producer_id,
            schema_version,
            payload_hash,
            outcome: AnalyticsIngestionOutcome::Rejected,
            rejection_reason: Some(reason),
            rejection_detail: detail,
            received_at: now,
            processed_at: Some(now),
            expires_at: now + chrono::Duration::seconds(ttl_seconds),
        }
    }

    pub fn is_accepted(&self) -> bool {
        self.outcome == AnalyticsIngestionOutcome::Accepted
    }

    pub fn is_rejected(&self) -> bool {
        self.outcome == AnalyticsIngestionOutcome::Rejected
    }
}

/// Result of attempting to record an analytics event at the ingestion boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsIngestionResult {
    /// First time this event ID was seen and it was accepted.
    Accepted(AnalyticsIngestionRecord),
    /// First time this event ID was seen and it was rejected.
    Rejected(AnalyticsIngestionRecord),
    /// Event ID already recorded; the stored outcome is returned unchanged.
    Duplicate(AnalyticsIngestionRecord),
    /// Same event ID arrived with a different payload hash.
    Conflict {
        event_id: String,
        original_hash: String,
        new_hash: String,
    },
}

impl AnalyticsIngestionResult {
    /// The stored record, if any, for replay/inspection.
    pub fn record(&self) -> Option<&AnalyticsIngestionRecord> {
        match self {
            AnalyticsIngestionResult::Accepted(record)
            | AnalyticsIngestionResult::Rejected(record)
            | AnalyticsIngestionResult::Duplicate(record) => Some(record),
            AnalyticsIngestionResult::Conflict { .. } => None,
        }
    }

    /// Whether the event is durably accepted (including duplicate replays).
    pub fn is_accepted(&self) -> bool {
        matches!(
            self,
            AnalyticsIngestionResult::Accepted(_) | AnalyticsIngestionResult::Duplicate(_)
        ) && self
            .record()
            .map(|record| record.is_accepted())
            .unwrap_or(false)
    }
}
