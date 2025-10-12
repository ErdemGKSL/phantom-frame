pub mod cache;
pub mod config;
pub mod control;
pub mod proxy;

use axum::Router;
use cache::{CacheStore, RefreshTrigger};
use proxy::ProxyState;
use std::sync::Arc;

/// The main library interface for using phantom-frame as a library
/// Returns a proxy handler function and a refresh trigger
pub fn create_proxy(proxy_url: String) -> (Router, RefreshTrigger) {
    let refresh_trigger = RefreshTrigger::new();
    let cache = CacheStore::new(refresh_trigger.clone());

    let proxy_state = Arc::new(ProxyState::new(cache, proxy_url));

    let app = Router::new()
        .fallback(proxy::proxy_handler)
        .with_state(proxy_state);

    (app, refresh_trigger)
}

/// Create a proxy handler with an existing refresh trigger
pub fn create_proxy_with_trigger(proxy_url: String, refresh_trigger: RefreshTrigger) -> Router {
    let cache = CacheStore::new(refresh_trigger);
    let proxy_state = Arc::new(ProxyState::new(cache, proxy_url));

    Router::new()
        .fallback(proxy::proxy_handler)
        .with_state(proxy_state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_proxy() {
        let (_app, trigger) = create_proxy("http://localhost:8080".to_string());
        trigger.trigger();
        // Just ensure it compiles and runs without panic
    }
}
