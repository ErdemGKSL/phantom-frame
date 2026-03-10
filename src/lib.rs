pub mod cache;
pub mod compression;
pub mod config;
pub mod control;
pub mod path_matcher;
pub mod proxy;

use axum::{extract::Extension, Router};
use cache::{CacheStore, RefreshTrigger};
use proxy::ProxyState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Controls which backend responses are eligible for caching.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheStrategy {
    /// Cache every response that passes the existing path and method filters.
    #[default]
    All,
    /// Disable caching entirely, including 404 cache entries.
    None,
    /// Cache HTML documents only.
    OnlyHtml,
    /// Cache everything except image responses.
    NoImages,
    /// Cache image responses only.
    OnlyImages,
    /// Cache non-HTML static/application assets.
    OnlyAssets,
}

impl CacheStrategy {
    /// Check whether a response with the given content type can be cached.
    pub fn allows_content_type(&self, content_type: Option<&str>) -> bool {
        let content_type = content_type
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_ascii_lowercase());

        match self {
            Self::All => true,
            Self::None => false,
            Self::OnlyHtml => content_type
                .as_deref()
                .is_some_and(|value| value == "text/html" || value == "application/xhtml+xml"),
            Self::NoImages => !content_type
                .as_deref()
                .is_some_and(|value| value.starts_with("image/")),
            Self::OnlyImages => content_type
                .as_deref()
                .is_some_and(|value| value.starts_with("image/")),
            Self::OnlyAssets => content_type.as_deref().is_some_and(|value| {
                value.starts_with("image/")
                    || value.starts_with("font/")
                    || value == "text/css"
                    || value == "text/javascript"
                    || value == "application/javascript"
                    || value == "application/x-javascript"
                    || value == "application/json"
                    || value == "application/manifest+json"
                    || value == "application/wasm"
                    || value == "application/xml"
                    || value == "text/xml"
            }),
        }
    }
}

impl std::fmt::Display for CacheStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::All => "all",
            Self::None => "none",
            Self::OnlyHtml => "only_html",
            Self::NoImages => "no_images",
            Self::OnlyImages => "only_images",
            Self::OnlyAssets => "only_assets",
        };

        f.write_str(value)
    }
}

/// Controls how cacheable responses are stored in memory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressStrategy {
    /// Store cache entries without additional compression.
    None,
    /// Store cache entries with Brotli compression.
    #[default]
    Brotli,
    /// Store cache entries with gzip compression.
    Gzip,
    /// Store cache entries with deflate compression.
    Deflate,
}

impl std::fmt::Display for CompressStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::None => "none",
            Self::Brotli => "brotli",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        };

        f.write_str(value)
    }
}

/// Controls where cached response bodies are stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheStorageMode {
    /// Keep cached bodies in process memory.
    #[default]
    Memory,
    /// Persist cached bodies to the filesystem and load them on cache hits.
    Filesystem,
}

impl std::fmt::Display for CacheStorageMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Memory => "memory",
            Self::Filesystem => "filesystem",
        };

        f.write_str(value)
    }
}

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

    /// Controls which responses should be cached after the backend responds.
    pub cache_strategy: CacheStrategy,

    /// Controls how cached bodies are stored in memory.
    pub compress_strategy: CompressStrategy,

    /// Controls where cached response bodies are stored.
    pub cache_storage_mode: CacheStorageMode,

    /// Optional override for filesystem-backed cache bodies.
    pub cache_directory: Option<PathBuf>,
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
            cache_strategy: CacheStrategy::All,
            compress_strategy: CompressStrategy::Brotli,
            cache_storage_mode: CacheStorageMode::Memory,
            cache_directory: None,
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

    /// Set the cache strategy used to decide which response types are stored.
    pub fn with_cache_strategy(mut self, strategy: CacheStrategy) -> Self {
        self.cache_strategy = strategy;
        self
    }

    /// Alias for callers that prefer a more fluent builder name.
    pub fn caching_strategy(self, strategy: CacheStrategy) -> Self {
        self.with_cache_strategy(strategy)
    }

    /// Set the compression strategy used for stored cache entries.
    pub fn with_compress_strategy(mut self, strategy: CompressStrategy) -> Self {
        self.compress_strategy = strategy;
        self
    }

    /// Alias for callers that prefer a more fluent builder name.
    pub fn compression_strategy(self, strategy: CompressStrategy) -> Self {
        self.with_compress_strategy(strategy)
    }

    /// Set the backing store for cached response bodies.
    pub fn with_cache_storage_mode(mut self, mode: CacheStorageMode) -> Self {
        self.cache_storage_mode = mode;
        self
    }

    /// Set the filesystem directory used for disk-backed cache bodies.
    pub fn with_cache_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.cache_directory = Some(directory.into());
        self
    }
}

/// The main library interface for using phantom-frame as a library
/// Returns a proxy handler function and a refresh trigger
pub fn create_proxy(config: CreateProxyConfig) -> (Router, RefreshTrigger) {
    let refresh_trigger = RefreshTrigger::new();
    let cache = CacheStore::with_storage(
        refresh_trigger.clone(),
        config.cache_404_capacity,
        config.cache_storage_mode.clone(),
        config.cache_directory.clone(),
    );

    // Spawn background task to listen for refresh events
    spawn_refresh_listener(cache.clone());

    let proxy_state = Arc::new(ProxyState::new(cache, config));

    let app = Router::new()
        .fallback(proxy::proxy_handler)
        .layer(Extension(proxy_state));

    (app, refresh_trigger)
}

/// Create a proxy handler with an existing refresh trigger
pub fn create_proxy_with_trigger(
    config: CreateProxyConfig,
    refresh_trigger: RefreshTrigger,
) -> Router {
    let cache = CacheStore::with_storage(
        refresh_trigger,
        config.cache_404_capacity,
        config.cache_storage_mode.clone(),
        config.cache_directory.clone(),
    );

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
                    tracing::debug!(
                        "Cache refresh triggered: clearing entries matching pattern '{}'",
                        pattern
                    );
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

    #[test]
    fn test_cache_strategy_content_types() {
        assert!(CacheStrategy::All.allows_content_type(None));
        assert!(!CacheStrategy::None.allows_content_type(Some("text/html")));
        assert!(CacheStrategy::OnlyHtml.allows_content_type(Some("text/html; charset=utf-8")));
        assert!(!CacheStrategy::OnlyHtml.allows_content_type(Some("image/png")));
        assert!(CacheStrategy::NoImages.allows_content_type(Some("text/css")));
        assert!(!CacheStrategy::NoImages.allows_content_type(Some("image/webp")));
        assert!(CacheStrategy::OnlyImages.allows_content_type(Some("image/svg+xml")));
        assert!(!CacheStrategy::OnlyImages.allows_content_type(Some("application/javascript")));
        assert!(CacheStrategy::OnlyAssets.allows_content_type(Some("application/javascript")));
        assert!(CacheStrategy::OnlyAssets.allows_content_type(Some("image/png")));
        assert!(!CacheStrategy::OnlyAssets.allows_content_type(Some("text/html")));
        assert!(!CacheStrategy::OnlyAssets.allows_content_type(None));
    }

    #[test]
    fn test_compress_strategy_display() {
        assert_eq!(CompressStrategy::default().to_string(), "brotli");
        assert_eq!(CompressStrategy::None.to_string(), "none");
        assert_eq!(CompressStrategy::Gzip.to_string(), "gzip");
        assert_eq!(CompressStrategy::Deflate.to_string(), "deflate");
    }

    #[tokio::test]
    async fn test_create_proxy() {
        let config = CreateProxyConfig::new("http://localhost:8080".to_string());
        assert_eq!(config.compress_strategy, CompressStrategy::Brotli);
        let (_app, trigger) = create_proxy(config);
        trigger.trigger();
        trigger.trigger_by_key_match("GET:/api/*");
        // Just ensure it compiles and runs without panic
    }
}
