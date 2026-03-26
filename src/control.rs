use crate::cache::CacheHandle;
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
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

/// Handler for POST /refresh-cache endpoint
async fn refresh_cache_handler(
    State(state): State<Arc<ControlState>>,
    req: Request<Body>,
) -> Result<impl IntoResponse, StatusCode> {
    // Check authorization if auth_token is set
    if let Some(required_token) = &state.auth_token {
        let auth_header = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok());

        let expected = format!("Bearer {}", required_token);

        if auth_header != Some(expected.as_str()) {
            tracing::warn!("Unauthorized refresh-cache attempt");
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    // Trigger cache invalidation on all registered server caches
    for handle in &state.handles {
        handle.invalidate_all();
    }
    tracing::info!(
        "Cache invalidation triggered via control endpoint ({} server(s))",
        state.handles.len()
    );

    Ok((StatusCode::OK, "Cache refresh triggered"))
}

/// Create the control server router.
///
/// `handles` contains one [`CacheHandle`] per named proxy server.
/// A single `/refresh-cache` call invalidates all of them.
pub fn create_control_router(handles: Vec<CacheHandle>, auth_token: Option<String>) -> Router {
    let state = Arc::new(ControlState::new(handles, auth_token));

    Router::new()
        .route("/refresh-cache", post(refresh_cache_handler))
        .with_state(state)
}
