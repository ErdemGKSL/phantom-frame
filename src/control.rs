use crate::cache::CacheHandle;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct ControlState {
    /// Named server handles — (server_name, handle) pairs.
    handles: Vec<(String, CacheHandle)>,
    auth_token: Option<String>,
}

impl ControlState {
    pub fn new(handles: Vec<(String, CacheHandle)>, auth_token: Option<String>) -> Self {
        Self { handles, auth_token }
    }

    /// Return handles matching `server` (if provided) or all handles.
    /// Returns `Err` when a name was given but no server matched.
    fn resolve_handles(
        &self,
        server: Option<&str>,
    ) -> Result<Vec<&CacheHandle>, (StatusCode, String)> {
        match server {
            None => Ok(self.handles.iter().map(|(_, h)| h).collect()),
            Some(name) => {
                let matched: Vec<&CacheHandle> = self
                    .handles
                    .iter()
                    .filter(|(n, _)| n == name)
                    .map(|(_, h)| h)
                    .collect();
                if matched.is_empty() {
                    Err((
                        StatusCode::NOT_FOUND,
                        format!("No server named '{}' found", name),
                    ))
                } else {
                    Ok(matched)
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct PatternBody {
    pattern: String,
    /// Optional: only invalidate this named server's cache.
    server: Option<String>,
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
    /// Optional: only operate on this named server.
    /// When omitted, the operation is broadcast to all servers.
    server: Option<String>,
}

/// Returns `Err(UNAUTHORIZED)` when the request lacks a valid Bearer token.
fn check_auth(state: &ControlState, headers: &HeaderMap) -> Result<(), StatusCode> {
    if let Some(required_token) = &state.auth_token {
        let auth_header = headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok());
        let expected = format!("Bearer {}", required_token);
        if auth_header != Some(expected.as_str()) {
            tracing::warn!("Unauthorized control endpoint attempt");
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(())
}

/// POST /invalidate_all — invalidate every cached entry across all servers.
async fn invalidate_all_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    check_auth(&state, &headers)?;

    for (_, handle) in &state.handles {
        handle.invalidate_all();
    }
    tracing::info!(
        "invalidate_all triggered via control endpoint ({} server(s))",
        state.handles.len()
    );
    Ok((StatusCode::OK, "Cache invalidated"))
}

/// POST /invalidate — invalidate entries matching a wildcard pattern.
///
/// Body: `{ "pattern": "/api/*" }` or `{ "pattern": "/api/*", "server": "frontend" }`
async fn invalidate_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PatternBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    let handles = state.resolve_handles(body.server.as_deref())?;
    for handle in handles {
        handle.invalidate(&body.pattern);
    }
    tracing::info!(
        "invalidate('{}') triggered via control endpoint (server={:?})",
        body.pattern,
        body.server
    );
    Ok((StatusCode::OK, "Pattern invalidation triggered".to_string()))
}

/// POST /add_snapshot — fetch a path from upstream, cache it, and track it.
///
/// Only available when the proxy is running in `PreGenerate` mode.
/// Body: `{ "path": "/about" }` or `{ "path": "/about", "server": "frontend" }`
async fn add_snapshot_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    let handles = state.resolve_handles(body.server.as_deref())?;
    for handle in handles {
        handle
            .add_snapshot(&body.path)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!(
        "add_snapshot('{}') triggered via control endpoint (server={:?})",
        body.path, body.server
    );
    Ok((StatusCode::OK, "Snapshot added".to_string()))
}

/// POST /refresh_snapshot — re-fetch a cached snapshot path from upstream.
///
/// Only available when the proxy is running in `PreGenerate` mode.
/// Body: `{ "path": "/about" }` or `{ "path": "/about", "server": "frontend" }`
async fn refresh_snapshot_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    let handles = state.resolve_handles(body.server.as_deref())?;
    for handle in handles {
        handle
            .refresh_snapshot(&body.path)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!(
        "refresh_snapshot('{}') triggered via control endpoint (server={:?})",
        body.path, body.server
    );
    Ok((StatusCode::OK, "Snapshot refreshed".to_string()))
}

/// POST /remove_snapshot — remove a path from the cache and snapshot list.
///
/// Only available when the proxy is running in `PreGenerate` mode.
/// Body: `{ "path": "/about" }` or `{ "path": "/about", "server": "frontend" }`
async fn remove_snapshot_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    let handles = state.resolve_handles(body.server.as_deref())?;
    for handle in handles {
        handle
            .remove_snapshot(&body.path)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!(
        "remove_snapshot('{}') triggered via control endpoint (server={:?})",
        body.path, body.server
    );
    Ok((StatusCode::OK, "Snapshot removed".to_string()))
}

/// POST /refresh_all_snapshots — re-fetch every tracked snapshot from upstream.
///
/// Only available when the proxy is running in `PreGenerate` mode.
/// Optional body: `{ "server": "frontend" }` to target a specific server.
async fn refresh_all_snapshots_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    let server_filter = body
        .as_ref()
        .and_then(|Json(v)| v.get("server"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let handles = state.resolve_handles(server_filter.as_deref())?;
    for handle in handles {
        handle
            .refresh_all_snapshots()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!(
        "refresh_all_snapshots triggered via control endpoint (server={:?})",
        server_filter
    );
    Ok((StatusCode::OK, "All snapshots refreshed".to_string()))
}

/// Create the control server router.
///
/// `handles` contains one `(server_name, CacheHandle)` pair per named proxy server.
pub fn create_control_router(
    handles: Vec<(String, CacheHandle)>,
    auth_token: Option<String>,
) -> Router {
    let state = Arc::new(ControlState::new(handles, auth_token));

    Router::new()
        .route("/invalidate_all", post(invalidate_all_handler))
        .route("/invalidate", post(invalidate_handler))
        .route("/add_snapshot", post(add_snapshot_handler))
        .route("/refresh_snapshot", post(refresh_snapshot_handler))
        .route("/remove_snapshot", post(remove_snapshot_handler))
        .route("/refresh_all_snapshots", post(refresh_all_snapshots_handler))
        .with_state(state)
}
