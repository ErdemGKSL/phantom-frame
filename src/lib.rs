pub mod cache;
pub mod config;
pub mod control;
pub mod path_matcher;
pub mod proxy;

use axum::{extract::Extension, Router};
use cache::{CacheStore, RefreshTrigger};
use proxy::ProxyState;
use std::sync::Arc;

/// Information about an incoming request for cache key generation
#[derive(Clone, Debug)]
pub struct RequestInfo<'a> {
    /// HTTP method (e.g., "GET", "POST", "PUT", "DELETE")
    pub method: &'a str,
    /// Request path (e.g., "/api/users")
    pub path: &'a str,
    /// Query string (e.g., "id=123&sort=asc")
    pub query: &'a str,
    /// Request headers (for custom cache key logic based on headers)
    pub headers: &'a axum::http::HeaderMap,
}

/// Configuration for creating a proxy
#[derive(Clone)]
pub struct CreateProxyConfig {
    /// The backend URL to proxy requests to
    pub proxy_url: String,
    
    /// Paths to include in caching (empty means include all)
    /// Supports wildcards and method prefixes: "/api/*", "POST /api/*", "GET /*/users", etc.
    pub include_paths: Vec<String>,
    
    /// Paths to exclude from caching (empty means exclude none)
    /// Supports wildcards and method prefixes: "/admin/*", "POST *", "PUT /api/*", etc.
    /// Exclude overrides include
    pub exclude_paths: Vec<String>,
    
    /// Custom cache key generator
    /// Takes request info and returns a cache key
    /// Default: method + path + query string
    pub cache_key_fn: Arc<dyn Fn(&RequestInfo) -> String + Send + Sync>,
}

impl CreateProxyConfig {
    /// Create a new config with default settings
    pub fn new(proxy_url: String) -> Self {
        Self {
            proxy_url,
            include_paths: vec![],
            exclude_paths: vec![],
            cache_key_fn: Arc::new(|req_info| {
                if req_info.query.is_empty() {
                    format!("{}:{}", req_info.method, req_info.path)
                } else {
                    format!("{}:{}?{}", req_info.method, req_info.path, req_info.query)
                }
            }),
        }
    }
    
    /// Set include paths
    pub fn with_include_paths(mut self, paths: Vec<String>) -> Self {
        self.include_paths = paths;
        self
    }
    
    /// Set exclude paths
    pub fn with_exclude_paths(mut self, paths: Vec<String>) -> Self {
        self.exclude_paths = paths;
        self
    }
    
    /// Set custom cache key function
    pub fn with_cache_key_fn<F>(mut self, f: F) -> Self
    where
        F: Fn(&RequestInfo) -> String + Send + Sync + 'static,
    {
        self.cache_key_fn = Arc::new(f);
        self
    }
}

/// The main library interface for using phantom-frame as a library
/// Returns a proxy handler function and a refresh trigger
pub fn create_proxy(config: CreateProxyConfig) -> (Router, RefreshTrigger) {
    let refresh_trigger = RefreshTrigger::new();
    let cache = CacheStore::new(refresh_trigger.clone());

    // Spawn background task to listen for refresh events
    spawn_refresh_listener(cache.clone());

    let proxy_state = Arc::new(ProxyState::new(cache, config));

    let app = Router::new()
        .fallback(proxy::proxy_handler)
        .layer(Extension(proxy_state));

    (app, refresh_trigger)
}

/// Create a proxy handler with an existing refresh trigger
pub fn create_proxy_with_trigger(config: CreateProxyConfig, refresh_trigger: RefreshTrigger) -> Router {
    let cache = CacheStore::new(refresh_trigger);
    
    // Spawn background task to listen for refresh events
    spawn_refresh_listener(cache.clone());

    let proxy_state = Arc::new(ProxyState::new(cache, config));

    Router::new()
        .fallback(proxy::proxy_handler)
        .layer(Extension(proxy_state))
}

/// Spawn a background task to listen for refresh events
fn spawn_refresh_listener(cache: CacheStore) {
    let mut receiver = cache.refresh_trigger().subscribe();
    
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(cache::RefreshMessage::All) => {
                    tracing::info!("Cache refresh triggered: clearing all entries");
                    cache.clear().await;
                }
                Ok(cache::RefreshMessage::Pattern(pattern)) => {
                    tracing::info!("Cache refresh triggered: clearing entries matching pattern '{}'", pattern);
                    cache.clear_by_pattern(&pattern).await;
                }
                Err(e) => {
                    tracing::error!("Refresh trigger channel error: {}", e);
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_proxy() {
        let config = CreateProxyConfig::new("http://localhost:8080".to_string());
        let (_app, trigger) = create_proxy(config);
        trigger.trigger();
        trigger.trigger_by_key_match("GET:/api/*");
        // Just ensure it compiles and runs without panic
    }
}
