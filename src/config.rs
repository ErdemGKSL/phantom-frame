use crate::{CacheStorageMode, CacheStrategy, CompressStrategy};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// TOML-friendly proxy mode selector.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyModeConfig {
    /// Dynamic mode: requests are proxied and cached on demand.
    #[default]
    Dynamic,
    /// PreGenerate (SSG) mode: a fixed set of paths is fetched at startup and
    /// served exclusively from the cache.
    PreGenerate,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_control_port")]
    pub control_port: u16,

    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,

    /// The URL of the backend to proxy to
    #[serde(default = "default_proxy_url")]
    pub proxy_url: String,

    /// Paths to include in caching (empty means include all)
    /// Supports wildcards: ["/api/*", "/*/users"]
    #[serde(default)]
    pub include_paths: Vec<String>,

    /// Paths to exclude from caching (empty means exclude none)
    /// Supports wildcards: ["/admin/*", "/*/private"]
    /// Exclude overrides include
    #[serde(default)]
    pub exclude_paths: Vec<String>,

    /// Enable WebSocket and protocol upgrade support (default: true)
    /// When enabled, requests with Connection: Upgrade headers will bypass
    /// the cache and establish a direct bidirectional TCP tunnel
    #[serde(default = "default_enable_websocket")]
    pub enable_websocket: bool,

    /// Only allow GET requests, reject all others (default: false)
    /// When true, only GET requests are processed; POST, PUT, DELETE, etc. return 405 Method Not Allowed
    /// Useful for static site prerendering where mutations shouldn't be allowed
    #[serde(default = "default_forward_get_only")]
    pub forward_get_only: bool,

    pub control_auth: Option<String>,

    /// Capacity for the 404 cache (default: 100)
    /// Limits the number of different 404 responses cached to prevent memory abuse
    #[serde(default = "default_cache_404_capacity")]
    pub cache_404_capacity: usize,

    /// Detect 404 pages via meta tag in HTML body in addition to HTTP status
    /// This lowers performance and should be enabled only when needed.
    #[serde(default = "default_use_404_meta")]
    pub use_404_meta: bool,

    /// Controls which response types should be cached.
    #[serde(default)]
    pub cache_strategy: CacheStrategy,

    /// Controls how cached responses are stored in memory.
    #[serde(default)]
    pub compress_strategy: CompressStrategy,

    /// Controls where cached response bodies are stored.
    #[serde(default)]
    pub cache_storage_mode: CacheStorageMode,

    /// Optional directory override for filesystem-backed cache bodies.
    #[serde(default)]
    pub cache_directory: Option<PathBuf>,

    /// Proxy operating mode. Set to `"pre_generate"` to enable SSG mode.
    #[serde(default)]
    pub proxy_mode: ProxyModeConfig,

    /// Paths to pre-generate at startup when `proxy_mode = "pre_generate"`.
    #[serde(default)]
    pub pre_generate_paths: Vec<String>,

    /// In PreGenerate mode, fall through to the upstream backend on a cache miss.
    /// Defaults to `false` (return 404 on miss).
    #[serde(default = "default_pre_generate_fallthrough")]
    pub pre_generate_fallthrough: bool,
}

fn default_enable_websocket() -> bool {
    true
}

fn default_forward_get_only() -> bool {
    false
}

fn default_control_port() -> u16 {
    17809
}

fn default_proxy_port() -> u16 {
    3000
}

fn default_proxy_url() -> String {
    "http://localhost:8080".to_string()
}

fn default_cache_404_capacity() -> usize {
    100
}

fn default_use_404_meta() -> bool {
    false
}

fn default_pre_generate_fallthrough() -> bool {
    false
}

impl Config {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            control_port: default_control_port(),
            proxy_port: default_proxy_port(),
            proxy_url: default_proxy_url(),
            include_paths: vec![],
            exclude_paths: vec![],
            enable_websocket: default_enable_websocket(),
            forward_get_only: default_forward_get_only(),
            control_auth: None,
            cache_404_capacity: default_cache_404_capacity(),
            use_404_meta: default_use_404_meta(),
            cache_strategy: CacheStrategy::default(),
            compress_strategy: CompressStrategy::default(),
            cache_storage_mode: CacheStorageMode::default(),
            cache_directory: None,
            proxy_mode: ProxyModeConfig::default(),
            pre_generate_paths: vec![],
            pre_generate_fallthrough: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults_cache_strategy_to_all() {
        let config: Config =
            toml::from_str("[server]\nproxy_url = \"http://localhost:8080\"\n").unwrap();
        assert_eq!(config.server.cache_strategy, CacheStrategy::All);
        assert_eq!(config.server.compress_strategy, CompressStrategy::Brotli);
        assert_eq!(config.server.cache_storage_mode, CacheStorageMode::Memory);
        assert_eq!(config.server.cache_directory, None);
    }

    #[test]
    fn test_config_parses_cache_strategy() {
        let config: Config = toml::from_str(
            "[server]\nproxy_url = \"http://localhost:8080\"\ncache_strategy = \"none\"\n",
        )
        .unwrap();

        assert_eq!(config.server.cache_strategy, CacheStrategy::None);
    }

    #[test]
    fn test_config_parses_compress_strategy() {
        let config: Config = toml::from_str(
            "[server]\nproxy_url = \"http://localhost:8080\"\ncompress_strategy = \"gzip\"\n",
        )
        .unwrap();

        assert_eq!(config.server.compress_strategy, CompressStrategy::Gzip);
    }

    #[test]
    fn test_config_parses_cache_storage_mode() {
        let config: Config = toml::from_str(
            "[server]\nproxy_url = \"http://localhost:8080\"\ncache_storage_mode = \"filesystem\"\ncache_directory = \"cache-bodies\"\n",
        )
        .unwrap();

        assert_eq!(
            config.server.cache_storage_mode,
            CacheStorageMode::Filesystem
        );
        assert_eq!(
            config.server.cache_directory,
            Some(PathBuf::from("cache-bodies"))
        );
    }
}
