use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

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

    pub control_auth: Option<String>,
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
            control_auth: None,
        }
    }
}
