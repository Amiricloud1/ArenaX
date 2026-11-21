use crate::api_error::ApiError;
use crate::auth::jwt_service::{JwtConfig, JwtService, TokenPair};
use crate::auth::middleware::ClaimsExt;
use crate::models::user::{AuthResponse, CreateUserRequest, LoginRequest};
use crate::service::auth_service::{ActiveSession, AuthService};
use actix_web::cookie::{Cookie, SameSite};
use actix_web::{web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

// ── Cookie names ──────────────────────────────────────────────────────────────

pub const ACCESS_TOKEN_COOKIE: &str = "auth_token";
pub const REFRESH_TOKEN_COOKIE: &str = "auth_refresh_token";

// ── Cookie builders ───────────────────────────────────────────────────────────

/// Build the `auth_token` (access) cookie.
///
/// - `HttpOnly` — not readable by JavaScript.
/// - `Secure`   — only sent over HTTPS (browsers ignore on localhost).
/// - `SameSite=Strict` — never sent on cross-site requests.
/// - `Path=/` — available to the frontend middleware for protected page checks.
fn build_access_cookie(token: &str, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build(ACCESS_TOKEN_COOKIE, token.to_owned())
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(max_age_secs))
        .finish()
}

/// Build the `auth_refresh_token` cookie.
///
/// Scoped to `/api/auth/refresh` so it is only sent for token refresh
/// requests, minimising the attack surface.
fn build_refresh_cookie(token: &str, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build(REFRESH_TOKEN_COOKIE, token.to_owned())
        .path("/api/auth/refresh")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(max_age_secs))
        .finish()
}

/// Build expired (clearing) versions of both cookies.
fn clear_access_cookie() -> Cookie<'static> {
    Cookie::build(ACCESS_TOKEN_COOKIE, "")
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(0))
        .finish()
}

fn clear_refresh_cookie() -> Cookie<'static> {
    Cookie::build(REFRESH_TOKEN_COOKIE, "")
        .path("/api/auth/refresh")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(0))
        .finish()
}

// ── Helper: attach cookie pair to any response builder ───────────────────────

/// Attach the access + refresh httpOnly cookies to `response_builder` and
/// return the finalised `HttpResponse`.
fn response_with_auth_cookies(
    mut builder: actix_web::HttpResponseBuilder,
    tokens: &TokenPair,
    refresh_max_age: i64,
) -> HttpResponse {
    builder
        .cookie(build_access_cookie(&tokens.access_token, tokens.expires_in))
        .cookie(build_refresh_cookie(&tokens.refresh_token, refresh_max_age))
        .finish()
}

// ── Return-destination validation ─────────────────────────────────────────────

/// Query parameters that must never be carried across an authentication
/// redirect, even when the destination path itself is approved.
const SECRET_QUERY_PARAMS: &[&str] = &[
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "auth_token",
    "password",
    "secret",
    "api_key",
    "apikey",
    "code",
    "session",
    "session_id",
    "sig",
    "signature",
];

/// Maximum accepted length of a raw return destination. Anything longer is
/// rejected outright to avoid pathological parsing / header abuse.
const MAX_RETURN_TO_LEN: usize = 2048;

/// Top-level path segments that are approved as post-authentication
/// destinations. Anything else falls back to the default landing page.
const APPROVED_PATH_PREFIXES: &[&str] = &[
    "tournaments",
    "matches",
    "dashboard",
    "profile",
    "settings",
    "notifications",
    "leaderboard",
    "wallet",
];

/// Locales the client supports for locale-prefixed routes (e.g. `/fr/tournaments/1`).
const SUPPORTED_LOCALES: &[&str] = &["en", "es", "fr", "de", "pt", "ja", "zh"];

/// Where the user is sent when no valid destination is supplied.
pub const DEFAULT_RETURN_TO: &str = "/dashboard";

/// Validate and normalise a client-supplied return destination.
///
/// Returns `Ok(Some(path))` for a safe, approved local path, `Ok(None)` when
/// the destination is absent, and `Err(())` when the destination is present
/// but unsafe (external URL, protocol-relative, scheme, encoded bypass, etc.).
///
/// Only local absolute paths are accepted. Query parameters are preserved
/// except for a denylist of secret-bearing keys. Fragments are dropped.
pub fn validate_return_to(raw: &str) -> Result<Option<String>, ()> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > MAX_RETURN_TO_LEN {
        return Err(());
    }

    // Reject control characters and whitespace that could smuggle headers.
    if raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(());
    }

    // Reject backslashes: browsers treat `\` like `/` in some contexts, which
    // enables `/\evil.com` style open redirects.
    if raw.contains('\\') {
        return Err(());
    }

    // Must be a local absolute path.
    if !raw.starts_with('/') {
        return Err(());
    }

    // Reject protocol-relative URLs (`//evil.com`) and their encoded forms.
    let lowered = raw.to_ascii_lowercase();
    if lowered.starts_with("//") || lowered.starts_with("/%2f") || lowered.starts_with("/%5c") {
        return Err(());
    }

    // Reject any scheme smuggled into the path (e.g. `/redirect?to=https://...`
    // is fine, but `/https://evil.com` is not).
    if lowered.contains("://") {
        return Err(());
    }

    // Split off the fragment (dropped) and query string.
    let without_fragment = raw.split('#').next().unwrap_or("");
    let (path, query) = match without_fragment.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (without_fragment, None),
    };

    // Normalise path segments: reject traversal and empty/`.` segments.
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." {
            return Err(());
        }
        // Reject percent-encoded traversal / separators inside segments.
        let seg_lower = segment.to_ascii_lowercase();
        if seg_lower.contains("%2e") || seg_lower.contains("%2f") || seg_lower.contains("%5c") {
            return Err(());
        }
        segments.push(segment);
    }

    if segments.is_empty() {
        return Ok(None);
    }

    // Allow an optional leading locale segment.
    let mut prefix_index = 0;
    if SUPPORTED_LOCALES.contains(&segments[0].to_ascii_lowercase().as_str()) {
        prefix_index = 1;
    }

    let approved = segments
        .get(prefix_index)
        .map(|s| APPROVED_PATH_PREFIXES.contains(&s.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if !approved {
        return Err(());
    }

    // Rebuild the path from validated segments.
    let mut normalised = String::with_capacity(path.len());
    for segment in &segments {
        normalised.push('/');
        normalised.push_str(segment);
    }

    // Filter query parameters, dropping secret-bearing keys.
    if let Some(query) = query {
        let mut kept: Vec<String> = Vec::new();
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let key = pair.split('=').next().unwrap_or("");
            let key_lower = key.to_ascii_lowercase();
            if SECRET_QUERY_PARAMS.contains(&key_lower.as_str()) {
                continue;
            }
            kept.push(pair.to_string());
        }
        if !kept.is_empty() {
            normalised.push('?');
            normalised.push_str(&kept.join("&"));
        }
    }

    Ok(Some(normalised))
}

/// Resolve a raw return destination to a safe path, falling back to the
/// default landing page when absent or invalid.
pub fn resolve_return_to(raw: Option<&str>) -> String {
    match raw {
        Some(value) => match validate_return_to(value) {
            Ok(Some(path)) => path,
            Ok(None) => DEFAULT_RETURN_TO.to_string(),
            Err(()) => {
                warn!(return_to = %value, "Rejected unsafe return destination");
                DEFAULT_RETURN_TO.to_string()
            }
        },
        None => DEFAULT_RETURN_TO.to_string(),
    }
}

// ── Request / response DTOs ───────────────────────────────────────────────────

/// Refresh token request — accepted from JSON body as a fallback, but the
/// preferred path is the `auth_refresh_token` cookie set at login.
#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    pub refresh_token: Option<String>,
}

/// Change password request
#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// Sessions response
#[derive(Debug, Serialize)]
pub struct SessionsResponse {
    pub sessions: Vec<ActiveSession>,
    pub total: usize,
}

/// Response body returned to the client on login / register.
///
/// Tokens are delivered as httpOnly cookies; this body carries only the user
/// profile plus `expires_in` so the frontend knows when to expect a 401.
#[derive(Debug, Serialize)]
pub struct AuthSuccessResponse {
    pub user: crate::models::user::UserProfile,
    /// Access token TTL in seconds — the frontend uses this to proactively
    /// refresh before expiry without ever touching the token value itself.
    pub expires_in: i64,
    /// Validated local path the client should navigate to after auth.
    /// Always present; falls back to `DEFAULT_RETURN_TO`.
    pub return_to: String,
}

/// Short-lived token for WebSocket authentication handshakes.
#[derive(Debug, Serialize)]
pub struct WsTokenResponse {
    /// A signed JWT valid for 60 seconds, used only to authenticate the
    /// initial WebSocket message.  It must NOT be stored — request a fresh
    /// one immediately before opening a socket.
    pub ws_token: String,
    pub expires_in: i64,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// POST /api/auth/register
pub async fn register(
    auth_service: web::Data<AuthService>,
    jwt_service: web::Data<Arc<JwtService>>,
    req: HttpRequest,
    request: web::Json<CreateUserRequest>,
) -> Result<impl Responder, ApiError> {
    info!(
        username = %request.username,
        email = %request.email.as_deref().unwrap_or("(none)"),
        "Registration request received"
    );

    // Capture the intended destination before consuming the request body.
    let return_to = resolve_return_to(extract_return_to(&req));

    let response: AuthResponse = auth_service.register(request.into_inner()).await?;

    // Retrieve the refresh TTL from the JwtService config so the cookie
    // max-age matches the token lifetime exactly.
    let refresh_max_age = jwt_service
        .refresh_token_expiry_secs();

    let body = AuthSuccessResponse {
        user: response.user,
        expires_in: {
            // We don't have direct access to the access TTL here, but the
            // TokenPair carries it — reconstruct a temporary pair just for
            // the expires_in field.
            jwt_service.access_token_expiry_secs()
        },
        return_to,
    };

    let tokens = TokenPair {
        access_token: response.token,
        refresh_token: response.refresh_token,
        expires_in: body.expires_in,
        token_type: "Bearer".to_string(),
    };

    let mut builder = HttpResponse::Created();
    builder
        .cookie(build_access_cookie(&tokens.access_token, tokens.expires_in))
        .cookie(build_refresh_cookie(&tokens.refresh_token, refresh_max_age));
    Ok(builder.json(body))
}

/// POST /api/auth/login
pub async fn login(
    auth_service: web::Data<AuthService>,
    jwt_service: web::Data<Arc<JwtService>>,
    req: HttpRequest,
    request: web::Json<LoginRequest>,
) -> Result<impl Responder, ApiError> {
    info!(email = %request.email, "Login request received");

    // Capture the intended destination before consuming the request body.
    let return_to = resolve_return_to(extract_return_to(&req));

    let response: AuthResponse = auth_service.login(request.into_inner()).await?;

    let refresh_max_age = jwt_service.refresh_token_expiry_secs();
    let access_expires_in = jwt_service.access_token_expiry_secs();

    let body = AuthSuccessResponse {
        user: response.user,
        expires_in: access_expires_in,
        return_to,
    };

    let tokens = TokenPair {
        access_token: response.token,
        refresh_token: response.refresh_token,
        expires_in: access_expires_in,
        token_type: "Bearer".to_string(),
    };

    let mut builder = HttpResponse::Ok();
    builder
        .cookie(build_access_cookie(&tokens.access_token, tokens.expires_in))
        .cookie(build_refresh_cookie(&tokens.refresh_token, refresh_max_age));
    Ok(builder.json(body))
}

/// Extract the raw return destination from the query string or the
/// `X-Return-To` header. The query string takes precedence.
fn extract_return_to(req: &HttpRequest) -> Option<&str> {
    if let Some(value) = req.query_string().split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key == "return_to" || key == "redirect" {
            Some(value)
        } else {
            None
        }
    }) {
        return Some(value);
    }
    req.headers()
        .get("X-Return-To")
        .and_then(|h| h.to_str().ok())
}

/// POST /api/auth/refresh
///
/// Reads the refresh token from the `auth_refresh_token` cookie (preferred)
/// or from a JSON body field `refresh_token` (legacy / mobile clients).
pub async fn refresh_token(
    auth_service: web::Data<AuthService>,
    jwt_service: web::Data<Arc<JwtService>>,
    req: HttpRequest,
    body: web::Json<RefreshTokenRequest>,
) -> Result<impl Responder, ApiError> {
    info!("Token refresh request received");

    // Prefer the httpOnly cookie; fall back to the JSON body field.
    let refresh_token_value = req
        .cookie(REFRESH_TOKEN_COOKIE)
        .map(|c| c.value().to_owned())
        .or_else(|| body.refresh_token.clone())
        .ok_or_else(|| ApiError::bad_request("No refresh token provided"))?;

    let token_pair = auth_service.refresh_token(&refresh_token_value).await?;

    let refresh_max_age = jwt_service.refresh_token_expiry_secs();

    let mut builder = HttpResponse::Ok();
    builder
        .cookie(build_access_cookie(&token_pair.access_token, token_pair.expires_in))
        .cookie(build_refresh_cookie(&token_pair.refresh_token, refresh_max_age));
    Ok(builder.json(serde_json::json!({ "expires_in": token_pair.expires_in })))
}

/// GET /api/auth/oauth/callback
///
/// Completes an OAuth login and returns the validated post-auth destination
/// so the client can restore it exactly once. The destination is validated
/// with the same rules as password login/registration.
pub async fn oauth_callback(req: HttpRequest) -> Result<impl Responder, ApiError> {
    // The OAuth exchange itself is handled by the provider adapter; this
    // endpoint only resolves the return destination for the client.
    let return_to = resolve_return_to(extract_return_to(&req));
    Ok(HttpResponse::Ok().json(serde_json::json!({ "return_to": return_to })))
}

/// POST /api/auth/logout
///
/// Blacklists the current access token (extracted from cookie or Authorization
/// header) and clears both auth cookies.
pub async fn logout(
    auth_service: web::Data<AuthService>,
    req: HttpRequest,
) -> Result<impl Responder, ApiError> {
    // Accept token from cookie or Authorization header.
    let token = req
        .cookie(ACCESS_TOKEN_COOKIE)
        .map(|c| c.value().to_owned())
        .or_else(|| {
            req.headers()
                .get("Authorization")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(str::to_owned)
        })
        .ok_or_else(|| ApiError::bad_request("No auth token found"))?;

    auth_service.logout(&token).await?;

    info!("User logged out successfully");

    Ok(HttpResponse::Ok()
        .cookie(clear_access_cookie())
        .cookie(clear_refresh_cookie())
        .json(serde_json::json!({ "message": "Logged out successfully" })))
}

/// GET /api/auth/me
pub async fn get_current_user(
    auth_service: web::Data<AuthService>,
    req: HttpRequest,
) -> Result<impl Responder, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let user = auth_service.get_user(user_id).await?;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "id": user.id,
        "username": user.username,
        "email": user.email,
        "is_verified": user.is_verified,
        "created_at": user.created_at,
    })))
}

/// POST /api/auth/change-password
pub async fn change_password(
    auth_service: web::Data<AuthService>,
    req: HttpRequest,
    request: web::Json<ChangePasswordRequest>,
) -> Result<impl Responder, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    auth_service
        .change_password(user_id, &request.old_password, &request.new_password)
        .await?;

    info!(user_id = %user_id, "Password changed successfully");

    // Clear cookies — the user must log in again after a password change.
    Ok(HttpResponse::Ok()
        .cookie(clear_access_cookie())
        .cookie(clear_refresh_cookie())
        .json(serde_json::json!({
            "message": "Password changed successfully. All sessions have been revoked."
        })))
}

/// POST /api/auth/revoke-sessions
pub async fn revoke_all_sessions(
    auth_service: web::Data<AuthService>,
    req: HttpRequest,
) -> Result<impl Responder, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let count = auth_service.revoke_all_sessions(user_id).await?;

    info!(user_id = %user_id, count = count, "Sessions revoked");

    Ok(HttpResponse::Ok()
        .cookie(clear_access_cookie())
        .cookie(clear_refresh_cookie())
        .json(serde_json::json!({
            "message": format!("{} session(s) revoked successfully", count),
            "count": count
        })))
}

/// GET /api/auth/sessions
pub async fn get_sessions(
    auth_service: web::Data<AuthService>,
    req: HttpRequest,
) -> Result<impl Responder, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let mut sessions = auth_service.get_sessions(user_id).await?;
    let current_session_id = req.claims().map(|claims| claims.session_id);
    for session in &mut sessions {
        session.is_current = current_session_id.as_deref() == Some(session.session_id.as_str());
    }
    let total = sessions.len();

    Ok(HttpResponse::Ok().json(SessionsResponse { sessions, total }))
}

/// DELETE /api/auth/sessions/{session_id}
pub async fn revoke_session(
    auth_service: web::Data<AuthService>,
    req: HttpRequest,
    session_id: web::Path<String>,
) -> Result<impl Responder, ApiError> {
    let user_id = req
        .user_id()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;
    auth_service.revoke_session(user_id, &session_id).await?;
    Ok(HttpResponse::NoContent().finish())
}

/// GET /api/auth/analytics  (admin only)
pub async fn get_analytics(
    _auth_service: web::Data<AuthService>,
    req: HttpRequest,
) -> Result<impl Responder, ApiError> {
    let claims = req
        .claims()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    if !claims.roles.contains(&"admin".to_string()) {
        return Err(ApiError::forbidden("Admin access required"));
    }

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "total_generated": 0,
        "total_validated": 0,
        "total_refreshed": 0,
        "total_blacklisted": 0,
        "active_sessions": 0
    })))
}

/// GET /api/auth/ws-token
///
/// Issues a short-lived (60-second) signed access token for authenticating
/// the initial WebSocket handshake message.  The token is NOT stored in a
/// cookie — it is returned in the JSON body and used immediately.
///
/// Requires: valid `auth_token` cookie (i.e. the user must already be logged in).
pub async fn get_ws_token(
    jwt_service: web::Data<Arc<JwtService>>,
    req: HttpRequest,
) -> Result<impl Responder, ApiError> {
    let claims = req
        .claims()
        .ok_or_else(|| ApiError::unauthorized("User not authenticated"))?;

    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::internal_error("Invalid user ID in token"))?;

    let ws_token = jwt_service
        .generate_ws_token(user_id, claims.roles)
        .await
        .map_err(|e| ApiError::internal_error(format!("WS token generation failed: {}", e)))?;

    Ok(HttpResponse::Ok().json(WsTokenResponse {
        ws_token,
        expires_in: 60,
    }))
}

/// Configure authentication routes.
pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/auth")
            .route("/register", web::post().to(register))
            .route("/login", web::post().to(login))
            .route("/oauth/callback", web::get().to(oauth_callback))
            .route("/refresh", web::post().to(refresh_token))
            .route("/logout", web::post().to(logout))
            .route("/me", web::get().to(get_current_user))
            .route("/change-password", web::post().to(change_password))
            .route("/revoke-sessions", web::post().to(revoke_all_sessions))
            .route("/sessions", web::get().to(get_sessions))
            .route("/sessions/{session_id}", web::delete().to(revoke_session))
            .route("/analytics", web::get().to(get_analytics))
            .route("/ws-token", web::get().to(get_ws_token)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_request_optional_field() {
        // Fully absent field
        let json = r#"{}"#;
        let req: RefreshTokenRequest = serde_json::from_str(json).unwrap();
        assert!(req.refresh_token.is_none());

        // Present field
        let json = r#"{"refresh_token":"test_token"}"#;
        let req: RefreshTokenRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.refresh_token.as_deref(), Some("test_token"));
    }

    #[test]
    fn test_change_password_request() {
        let json = r#"{"old_password":"old123","new_password":"new456"}"#;
        let req: ChangePasswordRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.old_password, "old123");
        assert_eq!(req.new_password, "new456");
    }

    #[test]
    fn accepts_approved_local_paths() {
        assert_eq!(
            validate_return_to("/tournaments/42").unwrap().as_deref(),
            Some("/tournaments/42")
        );
        assert_eq!(
            validate_return_to("/matches/abc-123").unwrap().as_deref(),
            Some("/matches/abc-123")
        );
    }

    #[test]
    fn accepts_locale_prefixed_routes() {
        assert_eq!(
            validate_return_to("/fr/tournaments/42").unwrap().as_deref(),
            Some("/fr/tournaments/42")
        );
        assert_eq!(
            validate_return_to("/ja/matches/9").unwrap().as_deref(),
            Some("/ja/matches/9")
        );
    }

    #[test]
    fn rejects_open_redirect_attempts() {
        for raw in [
            "https://evil.com",
            "//evil.com",
            "/\\evil.com",
            "/%2f%2fevil.com",
            "/%5cevil.com",
            "javascript:alert(1)",
            "/tournaments/../../etc/passwd",
            "/tournaments/%2e%2e/secret",
            " /tournaments/1",
            "/tournaments/1\r\nSet-Cookie: x=y",
        ] {
            assert!(validate_return_to(raw).is_err(), "should reject: {raw}");
        }
    }

    #[test]
    fn rejects_unapproved_paths() {
        assert!(validate_return_to("/admin/secret").is_err());
        assert!(validate_return_to("/api/auth/me").is_err());
    }

    #[test]
    fn strips_secret_query_params() {
        let result = validate_return_to("/tournaments/1?tab=bracket&token=abc&access_token=xyz")
            .unwrap()
            .unwrap();
        assert_eq!(result, "/tournaments/1?tab=bracket");
    }

    #[test]
    fn preserves_non_secret_query_params() {
        let result = validate_return_to("/matches/1?round=2&view=live")
            .unwrap()
            .unwrap();
        assert_eq!(result, "/matches/1?round=2&view=live");
    }

    #[test]
    fn drops_fragments() {
        let result = validate_return_to("/tournaments/1#section").unwrap().unwrap();
        assert_eq!(result, "/tournaments/1");
    }

    #[test]
    fn empty_and_root_fall_back_to_default() {
        assert_eq!(validate_return_to("").unwrap(), None);
        assert_eq!(validate_return_to("/").unwrap(), None);
        assert_eq!(resolve_return_to(None), DEFAULT_RETURN_TO);
        assert_eq!(resolve_return_to(Some("")), DEFAULT_RETURN_TO);
        assert_eq!(resolve_return_to(Some("/")), DEFAULT_RETURN_TO);
    }

    #[test]
    fn unsafe_destination_falls_back_to_default() {
        assert_eq!(resolve_return_to(Some("https://evil.com")), DEFAULT_RETURN_TO);
        assert_eq!(resolve_return_to(Some("//evil.com")), DEFAULT_RETURN_TO);
    }

    #[test]
    fn rejects_overlong_destination() {
        let raw = format!("/tournaments/{}", "a".repeat(MAX_RETURN_TO_LEN));
        assert!(validate_return_to(&raw).is_err());
    }
}
