use actix::{Actor, ActorContext, AsyncContext, Handler, Message, StreamHandler};
use actix_web::{web, Error, HttpRequest, HttpResponse};
use actix_web_actors::ws;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// WebSocket message types
///
/// # Snapshot / event-stream merge contract
///
/// Every outbound event carries a monotonically increasing `sequence` (a
/// per-match stream watermark). Clients bootstrap from an HTTP snapshot which
/// returns the `sequence` the snapshot was taken at, then apply only events
/// with `sequence > snapshot.sequence` in strictly increasing order.
///
/// On (re)subscribe the client supplies `last_seen` — the highest sequence it
/// has already applied (or the snapshot watermark). The server uses it to:
///   * replay events newer than `last_seen` when they are still retained, or
///   * emit `StreamGap` when the requested window has expired, telling the
///     client to discard local state and fetch a fresh snapshot.
///
/// Clients MUST reject events whose `sequence` is `<= last_seen` (stale /
/// duplicate) and MUST NOT apply optimistic updates twice when a replayed
/// event confirms them.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Subscribe to match updates.
    ///
    /// `last_seen` is the highest sequence already applied by the client
    /// (typically the snapshot watermark). `None` means "start from now".
    Subscribe {
        match_id: Uuid,
        #[serde(default)]
        last_seen: Option<u64>,
    },
    /// Acknowledged subscription with the current stream watermark.
    Subscribed {
        match_id: Uuid,
        /// Current server-side watermark for this match stream.
        sequence: u64,
        /// True when the server replayed events after `last_seen`.
        replayed: bool,
    },
    /// Signals that the client's `last_seen` is older than the retained
    /// window; the client must fetch a replacement snapshot.
    StreamGap {
        match_id: Uuid,
        /// Oldest sequence still retained by the server.
        oldest_retained: u64,
        /// Current server-side watermark.
        sequence: u64,
    },
    /// Unsubscribe from match updates
    Unsubscribe {
        match_id: Uuid,
    },
    /// Match state changed
    MatchStateChanged {
        match_id: Uuid,
        /// Stream sequence for this event (see merge contract above).
        sequence: u64,
        from_state: String,
        to_state: String,
        timestamp: String,
    },
    /// Match created
    MatchCreated {
        match_id: Uuid,
        sequence: u64,
        on_chain_match_id: String,
        player_a: String,
        player_b: String,
    },
    /// Match started
    MatchStarted {
        match_id: Uuid,
        sequence: u64,
        started_at: String,
    },
    /// Match completed
    MatchCompleted {
        match_id: Uuid,
        sequence: u64,
        winner: String,
        completed_at: String,
    },
    /// Match disputed
    MatchDisputed {
        match_id: Uuid,
        sequence: u64,
        actor: String,
        reason: String,
    },
    /// Match finalized
    MatchFinalized {
        match_id: Uuid,
        sequence: u64,
        finalized_at: String,
    },
    /// Error message
    Error {
        message: String,
    },
    /// Ping/Pong for keepalive
    Ping,
    Pong,
}

/// Per-match subscription state tracked by the actor.
#[derive(Debug, Clone)]
struct Subscription {
    /// Highest sequence the client has acknowledged/applied.
    last_seen: u64,
}

/// WebSocket actor for match updates
pub struct MatchWebSocket {
    /// Unique session ID
    id: Uuid,
    /// Last heartbeat time
    hb: Instant,
    /// Subscribed match IDs with their last-seen watermark
    subscriptions: Vec<(Uuid, Subscription)>,
}

impl MatchWebSocket {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            hb: Instant::now(),
            subscriptions: Vec::new(),
        }
    }

    /// Look up the subscription state for a match, if any.
    fn subscription(&self, match_id: Uuid) -> Option<&Subscription> {
        self.subscriptions
            .iter()
            .find(|(id, _)| *id == match_id)
            .map(|(_, sub)| sub)
    }

    /// Send heartbeat to client
    fn hb(&self, ctx: &mut <Self as Actor>::Context) {
        ctx.run_interval(Duration::from_secs(5), |act, ctx| {
            // Check if client is still connected
            if Instant::now().duration_since(act.hb) > Duration::from_secs(10) {
                warn!(session_id = %act.id, "WebSocket heartbeat timeout, disconnecting");
                ctx.stop();
                return;
            }

            let ping = serde_json::to_string(&WsMessage::Ping).unwrap();
            ctx.text(ping);
        });
    }

    /// Handle subscription request.
    ///
    /// `last_seen` is the client's applied watermark (snapshot sequence or the
    /// highest event sequence it has processed). The server acknowledges with
    /// `Subscribed` carrying the current watermark. Replay of retained events
    /// and `StreamGap` signalling are emitted by the broadcast layer, which
    /// owns the retained event buffer; here we only record the watermark and
    /// acknowledge so the client can detect gaps on the next event.
    fn handle_subscribe(
        &mut self,
        match_id: Uuid,
        last_seen: Option<u64>,
        ctx: &mut <Self as Actor>::Context,
    ) {
        let watermark = last_seen.unwrap_or(0);
        if let Some((_, sub)) = self.subscriptions.iter_mut().find(|(id, _)| *id == match_id) {
            // Re-subscribe: keep the highest watermark the client has applied.
            sub.last_seen = sub.last_seen.max(watermark);
        } else {
            self.subscriptions.push((
                match_id,
                Subscription {
                    last_seen: watermark,
                },
            ));
        }
        let current = self
            .subscription(match_id)
            .map(|s| s.last_seen)
            .unwrap_or(watermark);
        info!(
            session_id = %self.id,
            match_id = %match_id,
            last_seen = current,
            "Subscribed to match updates"
        );
        let ack = WsMessage::Subscribed {
            match_id,
            sequence: current,
            replayed: false,
        };
        if let Ok(json) = serde_json::to_string(&ack) {
            ctx.text(json);
        }
    }

    /// Handle unsubscription request
    fn handle_unsubscribe(&mut self, match_id: Uuid) {
        self.subscriptions.retain(|(id, _)| *id != match_id);
        info!(
            session_id = %self.id,
            match_id = %match_id,
            "Unsubscribed from match updates"
        );
    }

    /// Extract the stream sequence from an outbound event, if it carries one.
    fn event_sequence(message: &WsMessage) -> Option<u64> {
        match message {
            WsMessage::MatchStateChanged { sequence, .. }
            | WsMessage::MatchCreated { sequence, .. }
            | WsMessage::MatchStarted { sequence, .. }
            | WsMessage::MatchCompleted { sequence, .. }
            | WsMessage::MatchDisputed { sequence, .. }
            | WsMessage::MatchFinalized { sequence, .. } => Some(*sequence),
            _ => None,
        }
    }

    /// Handle incoming WebSocket message
    fn handle_message(&mut self, msg: &str, ctx: &mut <Self as Actor>::Context) {
        match serde_json::from_str::<WsMessage>(msg) {
            Ok(ws_msg) => match ws_msg {
                WsMessage::Subscribe { match_id, last_seen } => {
                    self.handle_subscribe(match_id, last_seen, ctx);
                }
                WsMessage::Unsubscribe { match_id } => {
                    self.handle_unsubscribe(match_id);
                }
                WsMessage::Ping => {
                    let pong = serde_json::to_string(&WsMessage::Pong).unwrap();
                    ctx.text(pong);
                }
                WsMessage::Pong => {
                    self.hb = Instant::now();
                }
                _ => {
                    warn!(
                        session_id = %self.id,
                        "Received unexpected message type"
                    );
                }
            },
            Err(e) => {
                error!(
                    session_id = %self.id,
                    error = %e,
                    "Failed to parse WebSocket message"
                );
                let error_msg = WsMessage::Error {
                    message: "Invalid message format".to_string(),
                };
                ctx.text(serde_json::to_string(&error_msg).unwrap());
            }
        }
    }

    /// Broadcast message to this session if subscribed to match.
    ///
    /// Enforces the merge contract: events with `sequence <= last_seen` are
    /// stale/duplicate and dropped; events with a gap (`sequence >
    /// last_seen + 1`) trigger a `StreamGap` so the client refetches a
    /// snapshot instead of applying an out-of-order event.
    pub fn broadcast_if_subscribed(
        &mut self,
        match_id: Uuid,
        message: &WsMessage,
        ctx: &mut <Self as Actor>::Context,
    ) {
        let Some(seq) = Self::event_sequence(message) else {
            // Non-sequenced control messages (ping/pong/errors) pass through.
            if self.subscription(match_id).is_some() {
                if let Ok(json) = serde_json::to_string(message) {
                    ctx.text(json);
                }
            }
            return;
        };

        let Some(sub) = self.subscription(match_id).cloned() else {
            return;
        };

        if seq <= sub.last_seen {
            debug!(
                session_id = %self.id,
                match_id = %match_id,
                sequence = seq,
                last_seen = sub.last_seen,
                "Dropping stale/duplicate event"
            );
            return;
        }

        if seq > sub.last_seen + 1 {
            warn!(
                session_id = %self.id,
                match_id = %match_id,
                sequence = seq,
                last_seen = sub.last_seen,
                "Detected stream gap, requesting snapshot replacement"
            );
            let gap = WsMessage::StreamGap {
                match_id,
                oldest_retained: sub.last_seen + 1,
                sequence: seq,
            };
            if let Ok(json) = serde_json::to_string(&gap) {
                ctx.text(json);
            }
            return;
        }

        debug!(
            session_id = %self.id,
            match_id = %match_id,
            sequence = seq,
            "Broadcasting message to subscribed client"
        );
        if let Ok(json) = serde_json::to_string(message) {
            ctx.text(json);
        }
        if let Some((_, sub)) = self.subscriptions.iter_mut().find(|(id, _)| *id == match_id) {
            sub.last_seen = seq;
        }
    }
}

impl Actor for MatchWebSocket {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!(session_id = %self.id, "WebSocket connection established");
        self.hb(ctx);
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!(
            session_id = %self.id,
            subscription_count = self.subscriptions.len(),
            "WebSocket connection closed — releasing subscriptions"
        );
        // Explicitly free the subscription list so any heap allocation is
        // released immediately rather than waiting for the actor to be dropped.
        self.subscriptions.clear();
        self.subscriptions.shrink_to_fit();
    }
}

/// Handle incoming WebSocket messages (text)
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for MatchWebSocket {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => {
                self.hb = Instant::now();
                ctx.pong(&msg);
            }
            Ok(ws::Message::Pong(_)) => {
                self.hb = Instant::now();
            }
            Ok(ws::Message::Text(text)) => {
                self.handle_message(&text, ctx);
            }
            Ok(ws::Message::Binary(_)) => {
                warn!(session_id = %self.id, "Binary messages not supported");
            }
            Ok(ws::Message::Close(reason)) => {
                info!(
                    session_id = %self.id,
                    reason = ?reason,
                    "Client initiated close"
                );
                ctx.stop();
            }
            _ => (),
        }
    }
}

/// Actix message for broadcasting to all sessions
#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct BroadcastMessage {
    pub match_id: Uuid,
    pub message: WsMessage,
}

impl Handler<BroadcastMessage> for MatchWebSocket {
    type Result = ();

    fn handle(&mut self, msg: BroadcastMessage, ctx: &mut Self::Context) {
        self.broadcast_if_subscribed(msg.match_id, &msg.message, ctx);
    }
}

impl Handler<BroadcastMessage> for MatchWebSocket {
    type Result = ();

    fn handle(&mut self, msg: BroadcastMessage, ctx: &mut Self::Context) {
        self.broadcast_if_subscribed(msg.match_id, &msg.message, ctx);
    }
}

/// WebSocket endpoint handler
/// WS /ws/matches/:id
pub async fn match_websocket(
    req: HttpRequest,
    stream: web::Payload,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, Error> {
    let match_id = path.into_inner();

    info!(match_id = %match_id, "New WebSocket connection request");

    let ws = MatchWebSocket::new();

    let resp = ws::start(ws, &req, stream)?;

    Ok(resp)
}

/// Configure WebSocket routes
pub fn configure_ws_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/ws/matches/{id}", web::get().to(match_websocket));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_message_serialization() {
        let msg = WsMessage::Subscribe {
            match_id: Uuid::new_v4(),
            last_seen: Some(42),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("subscribe"));
        assert!(json.contains("last_seen"));

        let deserialized: WsMessage = serde_json::from_str(&json).unwrap();
        match deserialized {
            WsMessage::Subscribe { last_seen, .. } => assert_eq!(last_seen, Some(42)),
            _ => panic!("Expected Subscribe message"),
        }
    }

    #[test]
    fn test_subscribe_without_last_seen_defaults() {
        let json = format!(r#"{{"type":"subscribe","match_id":"{}"}}"#, Uuid::new_v4());
        let deserialized: WsMessage = serde_json::from_str(&json).unwrap();
        match deserialized {
            WsMessage::Subscribe { last_seen, .. } => assert_eq!(last_seen, None),
            _ => panic!("Expected Subscribe message"),
        }
    }

    #[test]
    fn test_match_state_changed_serialization() {
        let msg = WsMessage::MatchStateChanged {
            match_id: Uuid::new_v4(),
            sequence: 7,
            from_state: "CREATED".to_string(),
            to_state: "STARTED".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("match_state_changed"));
        assert!(json.contains("CREATED"));
        assert!(json.contains("STARTED"));
        assert!(json.contains("\"sequence\":7"));
    }

    #[test]
    fn test_event_sequence_extraction() {
        let match_id = Uuid::new_v4();
        let msg = WsMessage::MatchStarted {
            match_id,
            sequence: 3,
            started_at: "2024-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(MatchWebSocket::event_sequence(&msg), Some(3));
        assert_eq!(MatchWebSocket::event_sequence(&WsMessage::Ping), None);
    }

    #[test]
    fn test_stream_gap_serialization() {
        let msg = WsMessage::StreamGap {
            match_id: Uuid::new_v4(),
            oldest_retained: 5,
            sequence: 9,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("stream_gap"));
        assert!(json.contains("oldest_retained"));
    }

    #[test]
    fn test_error_message() {
        let msg = WsMessage::Error {
            message: "Test error".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("Test error"));
    }
}
