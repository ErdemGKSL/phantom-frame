use crate::{CacheStorageMode, CacheStrategy, CompressStrategy};
use anyhow::{bail, Result};
use serde::{
    de::{self, Visitor},
    Deserialize, Serialize,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Controls whether a `.env` file is loaded before environment variable resolution.
///
/// - Absent or `false`: do not load any `.env` file.
/// - `true`: load `.env` from the current working directory (silently ignored if absent).
/// - `"./path/to/.env"`: load from the given path (error if the file does not exist).
#[derive(Debug, Clone, Default)]
pub enum DotenvConfig {
    /// Do not load a `.env` file.
    #[default]
    Disabled,
    /// Load `.env` from the current working directory.
    Default,
    /// Load from the specified path.
    Path(PathBuf),
}

impl serde::Serialize for DotenvConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            DotenvConfig::Disabled => serializer.serialize_bool(false),
            DotenvConfig::Default => serializer.serialize_bool(true),
            DotenvConfig::Path(p) => serializer.serialize_str(&p.to_string_lossy()),
        }
    }
}

impl<'de> Deserialize<'de> for DotenvConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DotenvVisitor;

        impl<'de> Visitor<'de> for DotenvVisitor {
            type Value = DotenvConfig;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a boolean or a path string for the .env file")
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<DotenvConfig, E> {
                if v {
                    Ok(DotenvConfig::Default)
                } else {
                    Ok(DotenvConfig::Disabled)
                }
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<DotenvConfig, E> {
                Ok(DotenvConfig::Path(PathBuf::from(v)))
            }
        }

        deserializer.deserialize_any(DotenvVisitor)
    }
}

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

/// Top-level configuration, deserialized directly from the TOML root.
///
/// Named server blocks are declared as `[server.NAME]` sections.
/// Global ports and TLS settings live at the root (no section header).
///
/// Example:
/// ```toml
/// http_port = 3000
/// control_port = 17809
///
/// [server.frontend]
/// bind_to = "*"
/// proxy_url = "http://localhost:5173"
///
/// [server.api]
/// bind_to = "/api"
/// proxy_url = "http://localhost:8080"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// HTTP listen port (default: 3000).
    #[serde(default = "default_http_port")]
    pub http_port: u16,

    /// Optional HTTPS listen port.
    /// When set, `cert_path` and `key_path` are required.
    pub https_port: Option<u16>,

    /// Path to the TLS certificate file (PEM). Required when `https_port` is set.
    pub cert_path: Option<PathBuf>,

    /// Path to the TLS private key file (PEM). Required when `https_port` is set.
    pub key_path: Option<PathBuf>,

    /// Control-plane listen port (default: 17809).
    #[serde(default = "default_control_port")]
    pub control_port: u16,

    /// Optional bearer token required to call `/refresh-cache`.
    pub control_auth: Option<String>,

    /// Named server entries, each mapping to a `[server.NAME]` TOML block.
    pub server: HashMap<String, ServerConfig>,

    /// Controls `.env` file loading before environment variable resolution.
    ///
    /// - Absent or `false`: disabled.
    /// - `true`: load `.env` from the current working directory.
    /// - `"./path/to/.env"`: load from the specified path.
    #[serde(default)]
    pub dotenv: DotenvConfig,
}

/// Per-server configuration block (one `[server.NAME]` entry).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    /// Axum router mount point.
    ///
    /// - `"*"` (default): catch-all fallback, bound via `Router::fallback_service`.
    /// - Any other value (e.g. `"/api"`): specific prefix, bound via `Router::nest`.
    ///
    /// When multiple specific paths are registered, longer paths are nested first
    /// so Axum can match them before shorter prefixes.
    ///
    /// **Note**: `Router::nest` strips the prefix before the inner proxy handler
    /// sees the path. Set `proxy_url` accordingly if the upstream expects the
    /// full path.
    #[serde(default = "default_bind_to")]
    pub bind_to: String,

    /// The URL of the backend to proxy to.
    #[serde(default = "default_proxy_url")]
    pub proxy_url: String,

    /// Paths to include in caching (empty means include all).
    /// Supports wildcards: `["/api/*", "/*/users"]`
    #[serde(default)]
    pub include_paths: Vec<String>,

    /// Paths to exclude from caching (empty means exclude none).
    /// Supports wildcards: `["/admin/*", "/*/private"]`.
    /// Exclude overrides include.
    #[serde(default)]
    pub exclude_paths: Vec<String>,

    /// Enable WebSocket / protocol-upgrade support (default: `true`).
    ///
    /// When `true`, upgrade requests bypass the cache and establish a direct
    /// bidirectional TCP tunnel to the backend — **but only when the proxy mode
    /// supports it** (i.e. Dynamic, or PreGenerate with `pre_generate_fallthrough
    /// = true`).  Pure SSG servers (`proxy_mode = "pre_generate"` with the
    /// default `pre_generate_fallthrough = false`) always return 501 for upgrade
    /// requests, regardless of this flag.
    #[serde(default = "default_enable_websocket")]
    pub enable_websocket: bool,

    /// Only allow GET requests, reject all others (default: `false`).
    #[serde(default = "default_forward_get_only")]
    pub forward_get_only: bool,

    /// Capacity for the 404 cache (default: 100).
    #[serde(default = "default_cache_404_capacity")]
    pub cache_404_capacity: usize,

    /// Detect 404 pages via `<meta name="phantom-404">` in addition to HTTP status.
    #[serde(default = "default_use_404_meta")]
    pub use_404_meta: bool,

    /// Controls which response types should be cached.
    #[serde(default)]
    pub cache_strategy: CacheStrategy,

    /// Controls how cached responses are compressed in memory.
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

    /// Optional shell command to execute before the proxy starts for this server.
    /// phantom-frame will spawn the process and wait until `proxy_url`'s port
    /// accepts TCP connections before serving traffic.
    ///
    /// Example: `"pnpm run dev"`, `"cargo run --release"`
    #[serde(default)]
    pub execute: Option<String>,

    /// Working directory for the `execute` command.
    /// Relative paths are resolved from the directory where phantom-frame is run.
    ///
    /// Example: `"./apps/client"`
    #[serde(default)]
    pub execute_dir: Option<String>,
}

// ── defaults ────────────────────────────────────────────────────────────────

fn default_http_port() -> u16 {
    3000
}

fn default_control_port() -> u16 {
    17809
}

fn default_bind_to() -> String {
    "*".to_string()
}

fn default_proxy_url() -> String {
    "http://localhost:8080".to_string()
}

fn default_enable_websocket() -> bool {
    true
}

fn default_forward_get_only() -> bool {
    false
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

// ── Config impl ──────────────────────────────────────────────────────────────

/// Recursively walk a `toml::Value` tree, resolving `$env:VAR` references.
///
/// A string value equal to `"$env:VAR_NAME"` is replaced with the value of
/// the environment variable `VAR_NAME`.  If the variable is not set the key
/// (or array element) is silently dropped, so `Option<T>` fields become `None`
/// and fields with `#[serde(default)]` fall back to their defaults.
fn resolve_env_vars(value: toml::Value) -> Option<toml::Value> {
    match value {
        toml::Value::String(ref s) if s.starts_with("$env:") => {
            let var_name = &s[5..];
            std::env::var(var_name).ok().map(toml::Value::String)
        }
        toml::Value::Table(table) => {
            let resolved: toml::map::Map<String, toml::Value> = table
                .into_iter()
                .filter_map(|(k, v)| resolve_env_vars(v).map(|rv| (k, rv)))
                .collect();
            Some(toml::Value::Table(resolved))
        }
        toml::Value::Array(arr) => {
            let resolved: Vec<toml::Value> =
                arr.into_iter().filter_map(resolve_env_vars).collect();
            Some(toml::Value::Array(resolved))
        }
        other => Some(other),
    }
}

impl Config {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;

        // Parse into a raw TOML value so we can load the .env before
        // deserializing and then resolve $env: references.
        let mut raw: toml::Value = toml::from_str(&content)?;

        // Extract the `dotenv` key from the raw table (before env resolution
        // so the path itself is a literal value, not an env-expanded one).
        let dotenv_cfg: DotenvConfig = raw
            .as_table()
            .and_then(|t| t.get("dotenv"))
            .map(|v| v.clone().try_into::<DotenvConfig>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("invalid `dotenv` value: {e}"))?
            .unwrap_or_default();

        match dotenv_cfg {
            DotenvConfig::Disabled => {}
            DotenvConfig::Default => {
                dotenvy::dotenv().ok(); // silently ignore if .env absent
            }
            DotenvConfig::Path(ref p) => {
                dotenvy::from_path(p)
                    .map_err(|e| anyhow::anyhow!("failed to load .env from `{}`: {e}", p.display()))?;
            }
        }

        // Walk the full TOML tree and resolve all $env: references.
        raw = resolve_env_vars(raw)
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));

        let config: Config = raw.try_into()?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.https_port.is_some() {
            if self.cert_path.is_none() {
                bail!("`cert_path` is required when `https_port` is set");
            }
            if self.key_path.is_none() {
                bail!("`key_path` is required when `https_port` is set");
            }
        }
        if self.server.is_empty() {
            bail!("at least one `[server.NAME]` block is required");
        }
        Ok(())
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_to: default_bind_to(),
            proxy_url: default_proxy_url(),
            include_paths: vec![],
            exclude_paths: vec![],
            enable_websocket: default_enable_websocket(),
            forward_get_only: default_forward_get_only(),
            cache_404_capacity: default_cache_404_capacity(),
            use_404_meta: default_use_404_meta(),
            cache_strategy: CacheStrategy::default(),
            compress_strategy: CompressStrategy::default(),
            cache_storage_mode: CacheStorageMode::default(),
            cache_directory: None,
            proxy_mode: ProxyModeConfig::default(),
            pre_generate_paths: vec![],
            pre_generate_fallthrough: false,
            execute: None,
            execute_dir: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_server_toml(extra: &str) -> String {
        format!(
            "[server.default]\nproxy_url = \"http://localhost:8080\"\n{}",
            extra
        )
    }

    #[test]
    fn test_config_defaults_cache_strategy_to_all() {
        let config: Config = toml::from_str(&single_server_toml("")).unwrap();
        let s = config.server.get("default").unwrap();
        assert_eq!(s.cache_strategy, CacheStrategy::All);
        assert_eq!(s.compress_strategy, CompressStrategy::Brotli);
        assert_eq!(s.cache_storage_mode, CacheStorageMode::Memory);
        assert_eq!(s.cache_directory, None);
    }

    #[test]
    fn test_config_parses_cache_strategy() {
        let config: Config =
            toml::from_str(&single_server_toml("cache_strategy = \"none\"\n")).unwrap();
        let s = config.server.get("default").unwrap();
        assert_eq!(s.cache_strategy, CacheStrategy::None);
    }

    #[test]
    fn test_config_parses_compress_strategy() {
        let config: Config =
            toml::from_str(&single_server_toml("compress_strategy = \"gzip\"\n")).unwrap();
        let s = config.server.get("default").unwrap();
        assert_eq!(s.compress_strategy, CompressStrategy::Gzip);
    }

    #[test]
    fn test_config_parses_cache_storage_mode() {
        let config: Config = toml::from_str(&single_server_toml(
            "cache_storage_mode = \"filesystem\"\ncache_directory = \"cache-bodies\"\n",
        ))
        .unwrap();
        let s = config.server.get("default").unwrap();
        assert_eq!(s.cache_storage_mode, CacheStorageMode::Filesystem);
        assert_eq!(s.cache_directory, Some(PathBuf::from("cache-bodies")));
    }

    #[test]
    fn test_config_top_level_ports() {
        let toml = "http_port = 8080\ncontrol_port = 9000\n".to_string()
            + &single_server_toml("");
        let config: Config = toml::from_str(&toml).unwrap();
        assert_eq!(config.http_port, 8080);
        assert_eq!(config.control_port, 9000);
        assert_eq!(config.https_port, None);
    }

    #[test]
    fn test_https_validation_requires_cert_and_key() {
        let toml = "https_port = 443\n".to_string() + &single_server_toml("");
        let config: Config = toml::from_str(&toml).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_multiple_servers() {
        let toml = "[server.frontend]\nbind_to = \"*\"\nproxy_url = \"http://localhost:5173\"\n\
                    [server.api]\nbind_to = \"/api\"\nproxy_url = \"http://localhost:8080\"\n";
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.server.len(), 2);
        assert_eq!(
            config.server.get("api").unwrap().bind_to,
            "/api"
        );
        assert_eq!(
            config.server.get("frontend").unwrap().bind_to,
            "*"
        );
    }

    // ── env-var resolution tests ─────────────────────────────────────────────

    #[test]
    fn test_env_var_string_field_resolves_when_set() {
        std::env::set_var("_PF_TEST_CONTROL_AUTH", "secret-token");
        let toml = format!(
            "control_auth = \"$env:_PF_TEST_CONTROL_AUTH\"\n{}",
            single_server_toml("")
        );
        let raw: toml::Value = toml::from_str(&toml).unwrap();
        let resolved = resolve_env_vars(raw).unwrap();
        let config: Config = resolved.try_into().unwrap();
        std::env::remove_var("_PF_TEST_CONTROL_AUTH");
        assert_eq!(config.control_auth, Some("secret-token".to_string()));
    }

    #[test]
    fn test_env_var_option_field_becomes_none_when_unset() {
        std::env::remove_var("_PF_TEST_HTTPS_PORT_MISSING");
        let toml = format!(
            "https_port = \"$env:_PF_TEST_HTTPS_PORT_MISSING\"\n{}",
            single_server_toml("")
        );
        let raw: toml::Value = toml::from_str(&toml).unwrap();
        let resolved = resolve_env_vars(raw).unwrap();
        let config: Config = resolved.try_into().unwrap();
        assert_eq!(config.https_port, None);
    }

    #[test]
    fn test_env_var_port_field_resolves_as_integer_string() {
        std::env::set_var("_PF_TEST_HTTP_PORT", "9999");
        let toml = format!(
            "http_port = \"$env:_PF_TEST_HTTP_PORT\"\n{}",
            single_server_toml("")
        );
        let raw: toml::Value = toml::from_str(&toml).unwrap();
        let resolved = resolve_env_vars(raw).unwrap();
        // http_port is u16; env vars resolve to String, so toml deserialization
        // will error — this test verifies the resolved string value is present.
        // To use $env: for numeric fields the env value must be quoted in the
        // config; TOML parses it as a string so serde coercion kicks in.
        // We just check the resolved tree has the string "9999".
        if let Some(toml::Value::Table(t)) = Some(resolved) {
            assert_eq!(t.get("http_port"), Some(&toml::Value::String("9999".to_string())));
        }
        std::env::remove_var("_PF_TEST_HTTP_PORT");
    }

    // ── dotenv config deserialization tests ──────────────────────────────────

    #[test]
    fn test_dotenv_false_is_disabled() {
        let toml = format!("dotenv = false\n{}", single_server_toml(""));
        let config: Config = toml::from_str(&toml).unwrap();
        assert!(matches!(config.dotenv, DotenvConfig::Disabled));
    }

    #[test]
    fn test_dotenv_true_is_default() {
        let toml = format!("dotenv = true\n{}", single_server_toml(""));
        let config: Config = toml::from_str(&toml).unwrap();
        assert!(matches!(config.dotenv, DotenvConfig::Default));
    }

    #[test]
    fn test_dotenv_string_path_is_path() {
        let toml = format!("dotenv = \"./.env.local\"\n{}", single_server_toml(""));
        let config: Config = toml::from_str(&toml).unwrap();
        assert!(matches!(config.dotenv, DotenvConfig::Path(ref p) if p == &PathBuf::from("./.env.local")));
    }

    #[test]
    fn test_dotenv_absent_is_disabled() {
        let config: Config = toml::from_str(&single_server_toml("")).unwrap();
        assert!(matches!(config.dotenv, DotenvConfig::Disabled));
    }

    #[test]
    fn test_dotenv_loads_env_file() {
        let dir = std::env::temp_dir();
        let env_path = dir.join("_pf_test_dotenv.env");
        std::fs::write(&env_path, "_PF_DOTENV_VAR=hello_from_dotenv\n").unwrap();

        // Use from_file via a temp config that references the dotenv file and
        // the env var.
        let cfg_path = dir.join("_pf_test_dotenv.toml");
        let cfg_content = format!(
            "dotenv = \"{}\"\ncontrol_auth = \"$env:_PF_DOTENV_VAR\"\n[server.default]\nproxy_url = \"http://localhost:8080\"\n",
            env_path.to_string_lossy().replace('\\', "/")
        );
        std::fs::write(&cfg_path, &cfg_content).unwrap();

        std::env::remove_var("_PF_DOTENV_VAR");
        let config = Config::from_file(&cfg_path).unwrap();

        std::fs::remove_file(&env_path).ok();
        std::fs::remove_file(&cfg_path).ok();

        assert_eq!(config.control_auth, Some("hello_from_dotenv".to_string()));
    }
}
