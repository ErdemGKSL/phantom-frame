use crate::cache::RefreshTrigger;
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
    refresh_trigger: RefreshTrigger,
    auth_token: Option<String>,
}

impl ControlState {
    pub fn new(refresh_trigger: RefreshTrigger, auth_token: Option<String>) -> Self {
        Self {
            refresh_trigger,
            auth_token,
        }
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

    // Trigger cache refresh
    state.refresh_trigger.trigger();
    tracing::info!("Cache refresh triggered via control endpoint");

    Ok((StatusCode::OK, "Cache refresh triggered"))
}

/// Create the control server router
pub fn create_control_router(
    refresh_trigger: RefreshTrigger,
    auth_token: Option<String>,
) -> Router {
    let state = Arc::new(ControlState::new(refresh_trigger, auth_token));

    Router::new()
        .route("/refresh-cache", post(refresh_cache_handler))
        .with_state(state)
}
