//! Cache observability endpoint (Issue #910).
//!
//! Hit rate is the number that tells you whether the cache is earning its
//! keep. A cache with a 5% hit rate is pure overhead plus a staleness risk,
//! and without this endpoint there is no way to notice that from the outside.

use actix_web::{web, HttpRequest, HttpResponse, Result};
use serde::Serialize;

use crate::api_error::ApiError;
use crate::auth::middleware::ClaimsExt;
use crate::middleware::cache::{CacheMetricsSnapshot, ResponseCache};
use crate::privacy::deletion::PurgeAckMetricsSnapshot;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheStatsResponse {
    hits: u64,
    stale_hits: u64,
    misses: u64,
    errors: u64,
    /// Fraction in `0.0..=1.0`. Stale hits count as hits — they still spared
    /// the database a query, which is what the rate measures.
    hit_rate: f64,
    /// Leaderboard snapshot responses served from a bounded-staleness cache
    /// entry (Issue #257). These are observable so operators can see when
    /// rankings are being served from a snapshot older than the write path.
    snapshot_stale_served: u64,
    /// Leaderboard snapshot responses that exceeded the staleness bound and
    /// were revalidated before being served.
    snapshot_revalidated: u64,
    /// Privacy deletion cache purges acknowledged by this process (Issue #258).
    privacy_purge_acked: u64,
    /// Privacy deletion cache purges that failed and are awaiting retry.
    privacy_purge_pending_retry: u64,
    /// Privacy deletion cache purges that exhausted retries and are surfaced
    /// for operator intervention.
    privacy_purge_failed: u64,
}

impl From<CacheMetricsSnapshot> for CacheStatsResponse {
    fn from(snapshot: CacheMetricsSnapshot) -> Self {
        Self {
            hits: snapshot.hits,
            stale_hits: snapshot.stale_hits,
            misses: snapshot.misses,
            errors: snapshot.errors,
            hit_rate: snapshot.hit_rate(),
            snapshot_stale_served: snapshot.snapshot_stale_served,
            snapshot_revalidated: snapshot.snapshot_revalidated,
            privacy_purge_acked: snapshot.privacy_purge_acked,
            privacy_purge_pending_retry: snapshot.privacy_purge_pending_retry,
            privacy_purge_failed: snapshot.privacy_purge_failed,
        }
    }
}

impl CacheStatsResponse {
    /// Merge the privacy-deletion purge acknowledgement counters into the
    /// cache stats payload. Kept separate so the cache middleware does not
    /// need to depend on the privacy module.
    fn with_purge_acks(mut self, acks: PurgeAckMetricsSnapshot) -> Self {
        self.privacy_purge_acked = acks.acked;
        self.privacy_purge_pending_retry = acks.pending_retry;
        self.privacy_purge_failed = acks.failed;
        self
    }
}

/// GET /api/cache/stats — hit/miss counters for this process.
///
/// Per-process, not cluster-wide: the counters live in the instance that
/// served the reads. Scraping every instance and summing is the caller's job,
/// which is what a metrics collector does anyway.
pub async fn cache_stats(
    req: HttpRequest,
    cache: web::Data<ResponseCache>,
    purge_acks: web::Data<PurgeAckMetricsSnapshot>,
) -> Result<HttpResponse, ApiError> {
    // Admin-only: the counters leak a rough picture of traffic shape.
    let is_admin = req
        .claims()
        .map(|c| c.roles.iter().any(|r| r == "admin"))
        .unwrap_or(false);

    if !is_admin {
        return Err(ApiError::Forbidden);
    }

    let stats: CacheStatsResponse = CacheStatsResponse::from(cache.metrics())
        .with_purge_acks(purge_acks.get_ref().clone());
    Ok(HttpResponse::Ok().json(serde_json::json!({ "data": stats })))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::scope("/api/cache").route("/stats", web::get().to(cache_stats)));
}
