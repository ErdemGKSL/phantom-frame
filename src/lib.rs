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
    
    /// Enable WebSocket and protocol upgrade support (default: true)
    /// When enabled, requests with Connection: Upgrade headers will bypass
    /// the cache and establish a direct bidirectional TCP tunnel
    pub enable_websocket: bool,
    
    /// Only allow GET requests, reject all others (default: false)
    /// When true, only GET requests are processed; POST, PUT, DELETE, etc. return 405 Method Not Allowed
    /// Useful for static site prerendering where mutations shouldn't be allowed
    pub forward_get_only: bool,
    
    /// Custom cache key generator
    /// Takes request info and returns a cache key
    /// Default: method + path + query string
    pub cache_key_fn: Arc<dyn Fn(&RequestInfo) -> String + Send + Sync>,
    /// Capacity for special 404 cache. When 0, 404 caching is disabled.
    pub cache_404_capacity: usize,

    /// When true, treat a response containing the meta tag `<meta name="phantom-404" content="true">` as a 404
    /// This is an optional performance-affecting fallback to detect framework-generated 404 pages.
    pub use_404_meta: bool,
}

impl CreateProxyConfig {
    /// Create a new config with default settings
    pub fn new(proxy_url: String) -> Self {
        Self {
            proxy_url,
            include_paths: vec![],
            exclude_paths: vec![],
            enable_websocket: true,
            forward_get_only: false,
            cache_key_fn: Arc::new(|req_info| {
                if req_info.query.is_empty() {
                    format!("{}:{}", req_info.method, req_info.path)
                } else {
                    format!("{}:{}?{}", req_info.method, req_info.path, req_info.query)
                }
            }),
            cache_404_capacity: 100,
            use_404_meta: false,
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
    
    /// Enable or disable WebSocket and protocol upgrade support
    pub fn with_websocket_enabled(mut self, enabled: bool) -> Self {
        self.enable_websocket = enabled;
        self
    }
    
    /// Only allow GET requests, reject all others
    pub fn with_forward_get_only(mut self, enabled: bool) -> Self {
        self.forward_get_only = enabled;
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

    /// Set 404 cache capacity. When 0, 404 caching is disabled.
    pub fn with_cache_404_capacity(mut self, capacity: usize) -> Self {
        self.cache_404_capacity = capacity;
        self
    }

    /// Treat pages that include the special meta tag as 404 pages
    pub fn with_use_404_meta(mut self, enabled: bool) -> Self {
        self.use_404_meta = enabled;
        self
    }
}

/// The main library interface for using phantom-frame as a library
/// Returns a proxy handler function and a refresh trigger
pub fn create_proxy(config: CreateProxyConfig) -> (Router, RefreshTrigger) {
    let refresh_trigger = RefreshTrigger::new();
    let cache = CacheStore::new(refresh_trigger.clone(), config.cache_404_capacity);

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
    let cache = CacheStore::new(refresh_trigger, 100);
    
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
                    tracing::debug!("Cache refresh triggered: clearing all entries");
                    cache.clear().await;
                }
                Ok(cache::RefreshMessage::Pattern(pattern)) => {
                    tracing::debug!("Cache refresh triggered: clearing entries matching pattern '{}'", pattern);
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
