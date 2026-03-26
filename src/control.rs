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
    handles: Vec<CacheHandle>,
    auth_token: Option<String>,
}

impl ControlState {
    pub fn new(handles: Vec<CacheHandle>, auth_token: Option<String>) -> Self {
        Self { handles, auth_token }
    }
}

#[derive(Deserialize)]
struct PatternBody {
    pattern: String,
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
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

    for handle in &state.handles {
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
/// Body: `{ "pattern": "/api/*" }`
async fn invalidate_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PatternBody>,
) -> Result<impl IntoResponse, StatusCode> {
    check_auth(&state, &headers)?;

    for handle in &state.handles {
        handle.invalidate(&body.pattern);
    }
    tracing::info!(
        "invalidate('{}') triggered via control endpoint ({} server(s))",
        body.pattern,
        state.handles.len()
    );
    Ok((StatusCode::OK, "Pattern invalidation triggered"))
}

/// POST /add_snapshot — fetch a path from upstream, cache it, and track it.
///
/// Only available when the proxy is running in `PreGenerate` mode.
/// Body: `{ "path": "/about" }`
async fn add_snapshot_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    for handle in &state.handles {
        handle
            .add_snapshot(&body.path)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!("add_snapshot('{}') triggered via control endpoint", body.path);
    Ok((StatusCode::OK, "Snapshot added".to_string()))
}

/// POST /refresh_snapshot — re-fetch a cached snapshot path from upstream.
///
/// Only available when the proxy is running in `PreGenerate` mode.
/// Body: `{ "path": "/about" }`
async fn refresh_snapshot_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    for handle in &state.handles {
        handle
            .refresh_snapshot(&body.path)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!(
        "refresh_snapshot('{}') triggered via control endpoint",
        body.path
    );
    Ok((StatusCode::OK, "Snapshot refreshed".to_string()))
}

/// POST /remove_snapshot — remove a path from the cache and snapshot list.
///
/// Only available when the proxy is running in `PreGenerate` mode.
/// Body: `{ "path": "/about" }`
async fn remove_snapshot_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
    Json(body): Json<PathBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    for handle in &state.handles {
        handle
            .remove_snapshot(&body.path)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!(
        "remove_snapshot('{}') triggered via control endpoint",
        body.path
    );
    Ok((StatusCode::OK, "Snapshot removed".to_string()))
}

/// POST /refresh_all_snapshots — re-fetch every tracked snapshot from upstream.
///
/// Only available when the proxy is running in `PreGenerate` mode.
async fn refresh_all_snapshots_handler(
    State(state): State<Arc<ControlState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    check_auth(&state, &headers).map_err(|s| (s, String::new()))?;

    for handle in &state.handles {
        handle
            .refresh_all_snapshots()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    tracing::info!(
        "refresh_all_snapshots triggered via control endpoint ({} server(s))",
        state.handles.len()
    );
    Ok((StatusCode::OK, "All snapshots refreshed".to_string()))
}

/// Create the control server router.
///
/// `handles` contains one [`CacheHandle`] per named proxy server.
pub fn create_control_router(handles: Vec<CacheHandle>, auth_token: Option<String>) -> Router {
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
