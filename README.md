# Phantom Frame

A high-performance prerendering proxy engine written in Rust. Cache and serve prerendered content with ease.

<p align="center">
  <img src="https://raw.githubusercontent.com/ErdemGKSL/phantom-frame/refs/heads/main/banner.jpg" alt="Phantom Frame Banner" width="100%">
</p>

## Features

- 🚀 **Fast caching proxy** - Cache prerendered content and serve it instantly
- 🗂️ **Multi-server routing** - Declare multiple named `[server.NAME]` blocks in one config, each bound to a different path prefix
- 🎛️ **Flexible cache strategies** - Disable caching entirely or target HTML, images, and assets only
- 🗜️ **Cache-aware compression** - Store cached bodies as Brotli, gzip, or deflate and fall back to identity when a client does not support the stored encoding
- 🔧 **Dual mode operation** - Run as standalone HTTP server or integrate as a library
- 🔄 **Dynamic cache refresh** - Trigger cache invalidation via control endpoint or programmatically
- 🔐 **Optional authentication** - Secure control endpoints with bearer token auth
- ⚡ **Async/await** - Built on Tokio and Axum for high performance
- 📦 **Easy integration** - Simple API for library usage
- 🌐 **WebSocket support** - Automatic detection and proxying of WebSocket and other protocol upgrade connections with bidirectional streaming
- 🔒 **HTTPS / TLS** - Optional TLS listener via `https_port` with rustls (default) or OpenSSL
- 📸 **SSG / PreGenerate mode** - Pre-fetch a set of paths at startup and serve them exclusively from cache

## Usage

### Mode 1: Standalone HTTP Server

Run as a standalone server with a TOML configuration file:

```bash
./phantom-frame ./config.toml
```

#### Configuration File (`config.toml`)

Global settings (ports, TLS, control auth) live at the TOML root without a section header.
Each proxy entry is declared as a `[server.NAME]` block.

```toml
# ── Global settings ───────────────────────────────────────────────────────────

# HTTP listen port (default: 3000)
http_port = 3000

# Control port for cache management endpoints (default: 17809)
control_port = 17809

# Optional: Bearer token for /refresh-cache authentication
# If set, callers must include: Authorization: Bearer <token>
# control_auth = "your-secret-token-here"

# Optional: HTTPS port — cert_path and key_path are required when set
# https_port = 443
# cert_path = "/etc/ssl/certs/fullchain.pem"
# key_path  = "/etc/ssl/private/privkey.pem"

# ── Server blocks ─────────────────────────────────────────────────────────────
# bind_to = "*"     → catch-all fallback (registered last)
# bind_to = "/api"  → nested under /api (Router::nest strips the prefix)

[server.default]
bind_to = "*"
proxy_url = "http://localhost:8080"

# Optional: Paths to include in caching (empty means include all)
# Supports wildcards: * can appear anywhere in the pattern
# Supports method prefixes: "GET /api/*", "POST /*/users", etc.
include_paths = ["/api/*", "/public/*", "GET /admin/stats"]

# Optional: Paths to exclude from caching (empty means exclude none)
# Exclude patterns override include patterns
exclude_paths = ["/api/admin/*", "/api/*/private", "POST *", "PUT *", "DELETE *"]

# Optional: Enable WebSocket and protocol upgrade support (default: true)
# Only active in Dynamic mode or PreGenerate mode with pre_generate_fallthrough = true.
# Pure SSG servers always return 501 for upgrade requests.
enable_websocket = true

# Optional: Only allow GET requests, reject all others (default: false)
forward_get_only = false

# Optional: Control which response types are cached (default: "all")
# Available values: "all", "none", "only_html", "no_images", "only_images", "only_assets"
cache_strategy = "all"

# Optional: Control how cached responses are stored in memory (default: "brotli")
# Available values: "none", "brotli", "gzip", "deflate"
compress_strategy = "brotli"

# Optional: Control where cached response bodies are stored (default: "memory")
# Available values: "memory", "filesystem"
cache_storage_mode = "memory"

# Optional: Override the directory used for filesystem-backed cache bodies
# cache_directory = "./.phantom-frame-cache"
```

#### Multi-Server Config

Multiple backends can be composed into a single Axum router. Longer `bind_to` prefixes are matched first so more-specific routes shadow shorter ones. `bind_to = "*"` is always the catch-all fallback.

> **Note**: `Router::nest("/api", …)` strips the `/api` prefix before the inner handler sees the path. Requests to `/api/users` are forwarded upstream as `/users`. If the upstream expects the full path, include the prefix in `proxy_url`.

```toml
http_port = 3000
control_port = 17809

# SSG frontend — pre-generated at startup, no backend at request time
[server.frontend]
bind_to = "*"
proxy_url = "http://localhost:5173"
proxy_mode = "pre_generate"
pre_generate_paths = ["/", "/about", "/blog"]
enable_websocket = false

# Dynamic API backend — requests forwarded and cached on demand
[server.api]
bind_to = "/api"
proxy_url = "http://localhost:8080"
proxy_mode = "dynamic"
enable_websocket = true
```

#### HTTPS / TLS

Set `https_port` at the root to enable a TLS listener. Both `cert_path` and `key_path` are required when this is set. Startup fails with a clear error if either is missing.

```toml
http_port  = 80
https_port = 443
cert_path  = "/etc/ssl/certs/fullchain.pem"
key_path   = "/etc/ssl/private/privkey.pem"

[server.default]
bind_to   = "*"
proxy_url = "http://localhost:8080"
```

TLS backend is selected by the active Cargo feature:
- **`rustls`** (default) — pure Rust, no system dependencies (`axum-server/tls-rustls`)
- **`native-tls`** — OpenSSL (`axum-server/tls-openssl`); requires OpenSSL as a system library

#### SSG / PreGenerate Mode

Set `proxy_mode = "pre_generate"` on a server block to pre-fetch a list of paths at startup.

```toml
[server.frontend]
bind_to = "*"
proxy_url = "http://localhost:5173"
proxy_mode = "pre_generate"
pre_generate_paths = ["/", "/about", "/blog/post-1"]

# On a cache miss:
#   false (default) → return 404 immediately, no backend contact
#   true            → fall through to the upstream backend
pre_generate_fallthrough = false
```

#### Cache Strategies

Use `cache_strategy` to control which backend responses are stored:

- `all`: Cache every response that passes your include/exclude rules.
- `none`: Disable cache reads and writes entirely. Useful for dev mode or plain proxying.
- `only_html`: Cache HTML documents only.
- `no_images`: Cache everything except `image/*` responses.
- `only_images`: Cache `image/*` responses only.
- `only_assets`: Cache static/application assets (CSS, JS, JSON, fonts, WebAssembly, XML, images).

#### Cache Compression Strategies

Use `compress_strategy` to control how cached bodies are stored in memory:

- `none`: Store uncompressed.
- `brotli` (default): Store with Brotli.
- `gzip`: Store with gzip.
- `deflate`: Store with deflate.

If the browser does not support the stored encoding, phantom-frame decodes the cached body and serves identity.

#### Cache Body Storage Modes

- `memory` (default): Cached bodies stay in process memory.
- `filesystem`: Bodies are written to a temp directory (or `cache_directory` if set) and loaded on cache hits. Metadata stays in memory.

#### Path Filtering

- **`include_paths`**: Only paths matching these patterns are cached. Empty = all.
- **`exclude_paths`**: Paths matching these patterns are never cached. Overrides include.
- `*` matches any sequence of characters anywhere in a pattern.
- Method prefixes: `GET /api/*`, `POST *`, `PUT /users/*`.

#### Control Endpoints

**POST /refresh-cache** — invalidate all server caches.

```bash
# Without authentication
curl -X POST http://localhost:17809/refresh-cache

# With authentication
curl -X POST http://localhost:17809/refresh-cache \
  -H "Authorization: Bearer your-secret-token-here"
```

### Mode 2: Library Integration

Add to your `Cargo.toml`:

```toml
[dependencies]
phantom-frame = { version = "0.2.3" }
tokio = { version = "1.40", features = ["full"] }
axum = "0.8"
```

Use in your code:

```rust
use phantom_frame::{
    create_proxy,
    cache::CacheHandle,
    CacheStrategy,
    CompressStrategy,
    CreateProxyConfig,
};
use axum::Router;

#[tokio::main]
async fn main() {
    let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
        .with_include_paths(vec![
            "/api/*".to_string(),
            "/public/*".to_string(),
            "GET /admin/stats".to_string(),
        ])
        .with_exclude_paths(vec![
            "/api/admin/*".to_string(),
            "POST *".to_string(),
            "PUT *".to_string(),
            "DELETE *".to_string(),
        ])
        .caching_strategy(CacheStrategy::OnlyHtml)
        .compression_strategy(CompressStrategy::Brotli)
        .with_websocket_enabled(true);

    let (proxy_app, handle): (Router, CacheHandle) = create_proxy(proxy_config);

    // Invalidate all cache entries
    handle.invalidate_all();

    // Invalidate only entries matching a pattern
    handle.invalidate("GET:/api/*");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();

    axum::serve(listener, proxy_app).await.unwrap();
}
```

For dev mode or plain proxying without any cache reads/writes:

```rust
let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .caching_strategy(CacheStrategy::None)
    .compression_strategy(CompressStrategy::None);
```

#### Custom Cache Key Function

```rust
use phantom_frame::{CreateProxyConfig, create_proxy, RequestInfo};

let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_cache_key_fn(|req_info: &RequestInfo| {
        let filtered_query = req_info.query
            .split('&')
            .filter(|p| !p.starts_with("session=") && !p.starts_with("token="))
            .collect::<Vec<_>>()
            .join("&");

        if filtered_query.is_empty() {
            format!("{}:{}", req_info.method, req_info.path)
        } else {
            format!("{}:{}?{}", req_info.method, req_info.path, filtered_query)
        }
    });
```

The `RequestInfo` struct provides:
- `method`: HTTP method (e.g., "GET", "POST")
- `path`: Request path (e.g., "/api/users")
- `query`: Query string (e.g., "id=123&sort=asc")
- `headers`: Request headers (for cache key logic based on Accept-Language, User-Agent, etc.)

#### Pattern-Based Cache Invalidation

```rust
// Clear all cache entries
handle.invalidate_all();

// Clear entries matching a wildcard pattern
handle.invalidate("GET:/api/*");
handle.invalidate("*/users/*");
handle.invalidate("POST:*");
```

## WebSocket and Protocol Upgrade Support

phantom-frame automatically detects and handles WebSocket connections and other HTTP protocol upgrades via `Connection: Upgrade` / `Upgrade` headers.

### Mode gating

WebSocket support is only active when the proxy has a live backend to tunnel to:

| `proxy_mode`                                | `enable_websocket = true` | Result          |
|---------------------------------------------|---------------------------|-----------------|
| `dynamic`                                   | yes                       | tunnel          |
| `pre_generate` + `pre_generate_fallthrough = true` | yes              | tunnel          |
| `pre_generate` + `pre_generate_fallthrough = false` (pure SSG) | any | 501 Not Implemented |

### Disabling WebSocket Support

```toml
# In server block
enable_websocket = false
```

```rust
// In library mode
let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_websocket_enabled(false);
```

## TLS Feature Flags

```toml
# Default — pure Rust, no system dependencies
phantom-frame = { version = "0.2.3" }

# OpenSSL backend (requires libssl-dev / openssl-devel / OPENSSL_DIR on Windows)
phantom-frame = { version = "0.2.3", default-features = false, features = ["native-tls"] }
```

## Building

```bash
# Build the project (default: rustls)
cargo build --release

# Build with OpenSSL backend
cargo build --release --no-default-features --features native-tls

# Run in development
cargo run -- ./config.toml

# Run the library example
cargo run --example library_usage
```

## How It Works

1. **Request Flow**: Incoming request → check 404 cache → check main cache → fetch from backend → store in cache → return response
2. **WebSocket/Upgrade**: Requests with `Connection: Upgrade` bypass caching and establish a direct bidirectional TCP tunnel to the backend (Dynamic / PreGenerate+fallthrough modes only)
3. **Multi-Server**: Multiple `[server.NAME]` blocks are composed into one Axum router. Specific prefixes (`/api`) are nested longest-first; `bind_to = "*"` is the fallback
4. **SSG Mode**: Specified paths are pre-fetched at startup. Cache misses either return 404 immediately or fall through to the backend depending on `pre_generate_fallthrough`
5. **Cache Refresh**: Invalidation is triggered via `/refresh-cache` or programmatically via `CacheHandle`

## API Reference

### Library API

#### `CreateProxyConfig`

- `CreateProxyConfig::new(proxy_url: String)` — create with defaults
- `with_include_paths(paths: Vec<String>)`
- `with_exclude_paths(paths: Vec<String>)`
- `with_websocket_enabled(enabled: bool)`
- `with_forward_get_only(enabled: bool)`
- `with_cache_key_fn(f: impl Fn(&RequestInfo) -> String)`
- `with_cache_404_capacity(capacity: usize)`
- `with_use_404_meta(enabled: bool)`
- `with_cache_strategy(strategy: CacheStrategy)` / `caching_strategy(…)`
- `with_compress_strategy(strategy: CompressStrategy)` / `compression_strategy(…)`
- `with_cache_storage_mode(mode: CacheStorageMode)`
- `with_cache_directory(directory: impl Into<PathBuf>)`
- `with_proxy_mode(mode: ProxyMode)`

#### `create_proxy(config: CreateProxyConfig) -> (Router, CacheHandle)`

Creates a proxy router and cache handle.

#### `CacheHandle`

- `invalidate_all()` — clear all cache entries
- `invalidate(pattern: &str)` — clear entries matching a wildcard pattern
- `add_snapshot(path)` — (PreGenerate) fetch and cache a new path
- `refresh_snapshot(path)` — (PreGenerate) re-fetch a single cached path
- `remove_snapshot(path)` — (PreGenerate) evict a path from cache
- `refresh_all_snapshots()` — (PreGenerate) re-fetch all tracked paths

### Control Endpoints

#### `POST /refresh-cache`

Invalidates all server caches. Requires `Authorization: Bearer <token>` header if `control_auth` is set.

## Limitations and Important Notes

phantom-frame caches a single rendered version per cache key and serves it to all users. This works well for public, user-agnostic content. Avoid caching:

- Cookie- or session-based SSR (personalized content will be served to the wrong user)
- Pages that vary by user-specific headers (Authorization, Cookie, Session)

**Safe patterns:**

- Use `exclude_paths` to skip routes that depend on session state
- Set `Cache-Control: private` / `no-store` on the backend for user-specific responses
- Vary the cache key by safe attributes (Accept-Language) but never by user identifiers
- Cache a shared skeleton and load user-specific data client-side via XHR/fetch

## License

See LICENSE file for details


- 🚀 **Fast caching proxy** - Cache prerendered content and serve it instantly
- 🎛️ **Flexible cache strategies** - Disable caching entirely or target HTML, images, and assets only
- 🗜️ **Cache-aware compression** - Store cached bodies as Brotli, gzip, or deflate and fall back to identity when a client does not support the stored encoding
- 🔧 **Dual mode operation** - Run as standalone HTTP server or integrate as a library
- 🔄 **Dynamic cache refresh** - Trigger cache invalidation via control endpoint or programmatically
- 🔐 **Optional authentication** - Secure control endpoints with bearer token auth
- ⚡ **Async/await** - Built on Tokio and Axum for high performance
- 📦 **Easy integration** - Simple API for library usage
- 🌐 **WebSocket support** - Automatic detection and proxying of WebSocket and other protocol upgrade connections with bidirectional streaming

## Usage

### Mode 1: Standalone HTTP Server

Run as a standalone server with a TOML configuration file:

```bash
./phantom-frame ./config.toml
```

#### Configuration File (`config.toml`)

```toml
[server]
# Control port for cache management endpoints (default: 17809)
control_port = 17809

# Proxy port for serving prerendered content (default: 3000)
proxy_port = 3000

# The backend URL to proxy requests to (default: http://localhost:8080)
proxy_url = "http://localhost:8080"

# Optional: Paths to include in caching (empty means include all)
# Supports wildcards: * can appear anywhere in the pattern
# Supports method prefixes: "GET /api/*", "POST /*/users", etc.
# Examples: "/api/*", "/*/users", "/public/*/assets", "GET *"
include_paths = ["/api/*", "/public/*", "GET /admin/stats"]

# Optional: Paths to exclude from caching (empty means exclude none)
# Supports wildcards: * can appear anywhere in the pattern
# Supports method prefixes: "POST /api/*", "PUT *", etc.
# Exclude patterns override include patterns
exclude_paths = ["/api/admin/*", "/api/*/private", "POST *", "PUT *", "DELETE *"]

# Optional: Enable WebSocket and protocol upgrade support (default: true)
# When enabled, requests with Connection: Upgrade headers will bypass the cache
# and establish a direct bidirectional TCP tunnel to the backend
# Set to false to disable WebSocket/upgrade support and return 501 Not Implemented
enable_websocket = true

# Optional: Only allow GET requests, reject all others (default: false)
# When enabled, only GET requests are processed; POST, PUT, DELETE, etc. return 405 Method Not Allowed
# Useful for static site prerendering or development proxying where mutations shouldn't be allowed
forward_get_only = false

# Optional: Control which response types are cached (default: "all")
# Available values: "all", "none", "only_html", "no_images", "only_images", "only_assets"
# Use "none" when you want phantom-frame to behave like a plain proxy in development.
cache_strategy = "all"

# Optional: Control how cached responses are stored in memory (default: "brotli")
# Available values: "none", "brotli", "gzip", "deflate"
# Only responses that are actually written to cache are compressed.
# If a later client does not support the stored encoding, phantom-frame decodes
# the cached body and serves it without Content-Encoding.
compress_strategy = "brotli"

# Optional: Control where cached response bodies are stored (default: "memory")
# Available values: "memory", "filesystem"
# Filesystem mode stores cache bodies under the OS temp directory unless cache_directory is set.
cache_storage_mode = "memory"

# Optional: Override the directory used for filesystem-backed cache bodies
# cache_directory = "./.phantom-frame-cache"

# Optional: Bearer token for control endpoint authentication
# If set, requests to /refresh-cache must include: Authorization: Bearer <token>
control_auth = "your-secret-token-here"
```

#### Cache Strategies

Use `cache_strategy` to control which backend responses are stored:

- `all`: Preserve the current behavior and cache every response that passes your include/exclude rules.
- `none`: Disable cache reads and writes entirely, including the 404 cache. Useful for dev mode or plain proxying.
- `only_html`: Cache HTML documents only.
- `no_images`: Cache everything except `image/*` responses.
- `only_images`: Cache `image/*` responses only.
- `only_assets`: Cache static/application assets such as CSS, JavaScript, JSON, fonts, WebAssembly, XML, and images.

Examples:

```toml
# Run as an uncached development proxy
cache_strategy = "none"

# Cache HTML pages only but still proxy assets through
cache_strategy = "only_html"

# Cache scripts, styles, fonts, JSON, and images but skip HTML documents
cache_strategy = "only_assets"
```

#### Cache Compression Strategies

Use `compress_strategy` to control how cached bodies are stored in memory:

- `none`: Keep cached bodies uncompressed.
- `brotli`: Store cached bodies with Brotli compression.
- `gzip`: Store cached bodies with gzip compression.
- `deflate`: Store cached bodies with deflate compression.

Behavior notes:

- Compression is applied only when phantom-frame is going to store the response in cache.
- Non-cacheable responses are proxied directly with the backend body and headers unchanged.
- Cached entries are stored once per cache key, not once per encoding.
- If the browser supports the stored encoding, phantom-frame serves the cached compressed bytes directly.
- If the browser does not support the stored encoding, phantom-frame decodes the cached body and serves identity instead of creating another cache entry.

Examples:

```toml
# Keep cached responses uncompressed
compress_strategy = "none"

# Store cached responses as gzip instead of Brotli
compress_strategy = "gzip"
```

#### Cache Body Storage Modes

Use `cache_storage_mode` to control whether cached bodies stay in RAM or move to the filesystem after compression:

- `memory`: Preserve the previous behavior and keep cached bodies in process memory.
- `filesystem`: Write cached bodies to a phantom-frame temp directory and load them back from disk on cache hits.

Behavior notes:

- Metadata such as status code, headers, and cache keys still stays in memory.
- `compress_strategy` still applies before the body is stored, so cache-hit content negotiation behaves the same in either mode.
- `cache_directory` is optional. If omitted, phantom-frame uses a subdirectory under the OS temp directory.
- On startup, phantom-frame removes orphaned cache files left in its filesystem cache subdirectories from previous process exits.
- Clearing the cache, wildcard invalidation, and 404 eviction also delete the backing files.

Examples:

```toml
# Reduce RAM usage by writing cached bodies to the temp directory
cache_storage_mode = "filesystem"

# Use a project-local directory for cache files instead of the OS temp directory
cache_storage_mode = "filesystem"
cache_directory = "./.phantom-frame-cache"
```

#### Path Filtering

You can control which paths are cached using `include_paths` and `exclude_paths`:

- **include_paths**: If specified, only paths matching these patterns will be cached. If empty, all paths are included (subject to exclusions).
- **exclude_paths**: Paths matching these patterns will never be cached. If empty, no paths are excluded.
- **Wildcard support**: Use `*` anywhere in a pattern to match any sequence of characters.
- **Method filtering**: Prefix patterns with HTTP methods like `GET /api/*`, `POST *`, `PUT /users/*`.
- **Priority**: Exclude patterns override include patterns.

**Examples:**

```toml
# Cache only API and public content
include_paths = ["/api/*", "/public/*"]

# Cache everything except admin and private paths
exclude_paths = ["/admin/*", "/*/private/*"]

# Cache API but exclude admin endpoints
include_paths = ["/api/*"]
exclude_paths = ["/api/admin/*"]

# Cache only GET requests (exclude all mutations)
exclude_paths = ["POST *", "PUT *", "DELETE *", "PATCH *"]

# Cache only specific methods for specific paths
include_paths = ["GET *"]  # Only cache GET requests
exclude_paths = ["GET /api/admin/*"]  # But not admin GET requests

# Mixed method and path filtering
include_paths = ["/api/*", "GET /admin/stats"]
exclude_paths = ["POST /api/*", "PUT /api/*", "/api/*/private"]
```

#### Control Endpoints

**POST /refresh-cache** - Trigger cache invalidation

```bash
# Without authentication
curl -X POST http://localhost:17809/refresh-cache

# With authentication (if control_auth is set)
curl -X POST http://localhost:17809/refresh-cache \
  -H "Authorization: Bearer your-secret-token-here"
```

### Mode 2: Library Integration

Add to your `Cargo.toml`:

```toml
[dependencies]
phantom-frame = { version = "0.1.17" }
tokio = { version = "1.40", features = ["full"] }
axum = "0.8.6"
```

Use in your code:

```rust
use phantom_frame::{
    create_proxy,
    cache::RefreshTrigger,
    CacheStrategy,
    CompressStrategy,
    CreateProxyConfig,
};
use axum::Router;

#[tokio::main]
async fn main() {
    // Create proxy configuration with method-based filtering
    let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
        .with_include_paths(vec![
            "/api/*".to_string(),
            "/public/*".to_string(),
            "GET /admin/stats".to_string(), // Only cache GET requests to this endpoint
        ])
        .with_exclude_paths(vec![
            "/api/admin/*".to_string(),
            "POST *".to_string(),   // Don't cache any POST requests
            "PUT *".to_string(),    // Don't cache any PUT requests  
            "DELETE *".to_string(), // Don't cache any DELETE requests
        ])
        .caching_strategy(CacheStrategy::OnlyHtml)
        .compression_strategy(CompressStrategy::Brotli)
        .with_websocket_enabled(true); // Enable WebSocket support (default: true)
    
    // Create proxy - returns router and refresh trigger
    let (proxy_app, refresh_trigger): (Router, RefreshTrigger) = 
        create_proxy(proxy_config);
    
    // Clone and use the refresh_trigger anywhere in your app
    let trigger_clone = refresh_trigger.clone();
    
    // Trigger cache refresh programmatically
    tokio::spawn(async move {
        // Clear all cache entries
        trigger_clone.trigger();
        
        // Or clear only specific cache entries matching a pattern
        trigger_clone.trigger_by_key_match("GET:/api/*");
        trigger_clone.trigger_by_key_match("*/users/*");
    });
    
    // Start the proxy server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();
    
    axum::serve(listener, proxy_app).await.unwrap();
}
```

For dev mode or plain proxying without any cache reads/writes:

```rust
use phantom_frame::{CacheStrategy, CompressStrategy, CreateProxyConfig};

let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .caching_strategy(CacheStrategy::None)
    .compression_strategy(CompressStrategy::None);
```

#### Custom Cache Key Function

You can customize how cache keys are generated. The cache key function receives a `RequestInfo` struct containing the HTTP method, path, and query string:

```rust
use phantom_frame::{CreateProxyConfig, create_proxy, RequestInfo};

let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_cache_key_fn(|req_info: &RequestInfo| {
        // Custom cache key logic
        // For example, ignore certain query parameters
        
        // Filter out session-specific query params
        let filtered_query = if !req_info.query.is_empty() {
            req_info.query
                .split('&')
                .filter(|p| !p.starts_with("session=") && !p.starts_with("token="))
                .collect::<Vec<_>>()
                .join("&")
        } else {
            String::new()
        };
        
        // Include method in cache key
        if filtered_query.is_empty() {
            format!("{}:{}", req_info.method, req_info.path)
        } else {
            format!("{}:{}?{}", req_info.method, req_info.path, filtered_query)
        }
    });

let (proxy_app, refresh_trigger) = create_proxy(proxy_config);
```

The `RequestInfo` struct provides:
- `method`: HTTP method (e.g., "GET", "POST", "PUT")
- `path`: Request path (e.g., "/api/users")
- `query`: Query string (e.g., "id=123&sort=asc")
- `headers`: Request headers (for cache key logic based on headers like Accept-Language, User-Agent, etc.)

**Advanced example with headers:**

```rust
use phantom_frame::{CreateProxyConfig, create_proxy, RequestInfo};

let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_cache_key_fn(|req_info: &RequestInfo| {
        // Include Accept-Language header in cache key for i18n
        let lang = req_info.headers
            .get("accept-language")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next()) // Get primary language
            .unwrap_or("en");
        
        // Create cache key with method, path, and language
        if req_info.query.is_empty() {
            format!("{}:{}:lang={}", req_info.method, req_info.path, lang)
        } else {
            format!("{}:{}?{}:lang={}", req_info.method, req_info.path, req_info.query, lang)
        }
    });

let (proxy_app, refresh_trigger) = create_proxy(proxy_config);
```

#### Pattern-Based Cache Invalidation

The `RefreshTrigger` supports both full cache clears and pattern-based invalidation using wildcards:

```rust
use phantom_frame::{create_proxy, CreateProxyConfig};

let (proxy_app, refresh_trigger) = create_proxy(
    CreateProxyConfig::new("http://localhost:8080".to_string())
);

// Clear all cache entries
refresh_trigger.trigger();

// Clear only entries matching specific patterns (with wildcard support)
refresh_trigger.trigger_by_key_match("GET:/api/*");        // Clear all GET /api/* requests
refresh_trigger.trigger_by_key_match("*/users/*");         // Clear all requests with /users/ in path
refresh_trigger.trigger_by_key_match("POST:*");            // Clear all POST requests
refresh_trigger.trigger_by_key_match("GET:/api/users");    // Clear exact match

// Use in response to specific events
tokio::spawn(async move {
    // Example: Clear user-related cache when user data changes
    refresh_trigger.trigger_by_key_match("*/users/*");
    
    // Example: Clear API cache after data update
    refresh_trigger.trigger_by_key_match("GET:/api/*");
});
```

**Pattern Matching Rules:**
- `*` matches any sequence of characters
- Patterns can include the HTTP method prefix (e.g., `GET:/api/*`)
- Multiple wildcards are supported (e.g., `*/api/*/users/*`)
- Exact matches work without wildcards (e.g., `GET:/api/users`)

## WebSocket and Protocol Upgrade Support

phantom-frame automatically detects and handles WebSocket connections and other HTTP protocol upgrades (e.g., HTTP/2, Server-Sent Events with upgrade):

### How it works

1. **Automatic Detection**: Any request with `Connection: Upgrade` or `Upgrade` headers is automatically detected
2. **Direct Proxying**: Upgrade requests bypass the cache entirely and establish a direct bidirectional TCP tunnel
3. **Full Transparency**: The WebSocket handshake is completed between client and backend, and all data flows directly through the proxy
4. **Long-lived Connections**: The tunnel remains open for the lifetime of the connection, supporting real-time bidirectional communication

### Example

Your backend WebSocket endpoints will work seamlessly through phantom-frame:

```javascript
// Frontend code - connect to WebSocket through the proxy
const ws = new WebSocket('ws://localhost:3000/api/ws');

ws.onopen = () => {
  console.log('Connected');
  ws.send('Hello Server!');
};

ws.onmessage = (event) => {
  console.log('Received:', event.data);
};
```

```rust
// Backend code - your WebSocket handler runs as normal
// phantom-frame will tunnel the connection transparently
use axum::{
    routing::get,
    extract::ws::{WebSocket, WebSocketUpgrade},
    Router,
};

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        // Handle WebSocket messages
    }
}
```

**Note**: WebSocket and upgrade connections are never cached, as they are inherently stateful and bidirectional. The proxy acts as a transparent tunnel for these connections.

### Disabling WebSocket Support

If you don't need WebSocket support or want to explicitly block protocol upgrades, you can disable it:

**In config.toml:**
```toml
[server]
enable_websocket = false  # Disable WebSocket support
```

**In library mode:**
```rust
let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_websocket_enabled(false);  // Disable WebSocket support
```

When disabled, any upgrade request (WebSocket, etc.) will receive a `501 Not Implemented` response.

## Building

```bash
# Build the project
cargo build --release

# Run in development
cargo run -- ./config.toml

# Run the library example
cargo run --example library_usage
```

## How It Works

1. **Request Flow**: When a request comes in, phantom-frame first checks if the content is cached
2. **Cache Miss**: If not cached, it fetches from the backend, caches the response, and returns it
3. **Cache Hit**: If cached, it serves the cached content immediately
4. **Cache Refresh**: The cache can be invalidated via the control endpoint or programmatically
5. **WebSocket/Upgrade Handling**: Requests with `Connection: Upgrade` or `Upgrade` headers (e.g., WebSocket) are automatically detected and bypass the cache entirely. Instead, a direct bidirectional TCP tunnel is established between the client and backend, allowing long-lived connections to work seamlessly.

## API Reference

### Library API

#### `RequestInfo`

Information about an incoming request for cache key generation.

- **Fields**:
  - `method: &str` - HTTP method (e.g., "GET", "POST")
  - `path: &str` - Request path (e.g., "/api/users")
  - `query: &str` - Query string (e.g., "id=123&sort=asc")
  - `headers: &HeaderMap` - Request headers (e.g., for cache keys based on Accept-Language, User-Agent, etc.)

#### `CreateProxyConfig`

Configuration struct for creating a proxy.

- **Constructor**: `CreateProxyConfig::new(proxy_url: String)` - Create with default settings
- **Methods**:
  - `with_include_paths(paths: Vec<String>)` - Set paths to include in caching (supports method prefixes like "GET /api/*")
  - `with_exclude_paths(paths: Vec<String>)` - Set paths to exclude from caching (supports method prefixes like "POST *")
  - `with_websocket_enabled(enabled: bool)` - Enable or disable WebSocket and protocol upgrade support (default: true)
  - `with_cache_key_fn(f: impl Fn(&RequestInfo) -> String)` - Set custom cache key generator

#### `create_proxy(config: CreateProxyConfig) -> (Router, RefreshTrigger)`

Creates a proxy router and refresh trigger.

- **Parameters**: `config` - Proxy configuration
- **Returns**: Tuple of `(Router, RefreshTrigger)`

#### `create_proxy_with_trigger(config: CreateProxyConfig, refresh_trigger: RefreshTrigger) -> Router`

Creates a proxy router with an existing refresh trigger.

- **Parameters**: 
  - `config` - Proxy configuration
  - `refresh_trigger` - Existing refresh trigger to use
- **Returns**: `Router`

#### `RefreshTrigger`

A clonable trigger for cache invalidation.

- `trigger()` - Trigger a full cache refresh (clears all entries)
- `trigger_by_key_match(pattern: &str)` - Trigger a cache refresh for entries matching a pattern (supports wildcards like `/api/*`, `GET:/api/*`, etc.)
- `subscribe()` - Subscribe to refresh events (returns a broadcast receiver)

### Control Endpoints

#### `POST /refresh-cache`

Triggers cache invalidation. Requires `Authorization: Bearer <token>` header if `control_auth` is configured.

## Limitations and important notes

phantom-frame is designed as a high-performance prerendering proxy that caches responses and serves them to subsequent requests. This works well for pages whose rendered HTML is identical for all users. However, there are important limitations you should be aware of:

- Cookie- or session-based SSR will not work correctly when cached: if your backend renders different content depending on cookies, authentication, or per-user session state, phantom-frame will cache a single rendered version and serve it to other users. That means personalized content (for example, "Hello, Alice" vs "Hello, Bob"), shopping carts, or any user-specific sections may be shown to the wrong user.

- Pages that vary by request headers (besides a safe, small set such as Accept-Language) may be incorrectly cached. If your site renders differently based on headers like Authorization, Cookie, or custom headers, the proxy must avoid caching or must vary the cache key accordingly.

**Safe header-based cache variations:**

You can use the `headers` field in `RequestInfo` to vary cache keys based on safe headers like Accept-Language for internationalization:

```rust
let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_cache_key_fn(|req_info: &RequestInfo| {
        // Vary cache by Accept-Language for i18n
        let lang = req_info.headers
            .get("accept-language")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .unwrap_or("en");
        
        format!("{}:{}:lang={}", req_info.method, req_info.path, lang)
    });
```

**Warning:** Never include user-specific headers (Authorization, Cookie, Session tokens) in cache keys, as this would create a separate cache entry per user, defeating the purpose of caching and potentially exposing user data.

Recommendations

- Only enable caching for pages that are truly public and identical across users (for example, marketing pages, blog posts, documentation, and other static content).

- For personalized pages, prefer one of these patterns:
    - Disable caching for routes that depend on cookies or session state. Let those requests pass through directly to the backend.
    - Use server-side cache-control and vary headers: have your backend set Cache-Control: private or no-store for responses that must never be cached.
    - Add a cache-variation strategy: include relevant request attributes in the cache key (for example, language or AB-test id) but avoid including user-specific identifiers like session ids or user ids.
    - Serve a public, cached shell and hydrate per-user data client-side: render a shared skeleton HTML via phantom-frame, then load user-specific data in the browser over XHR/fetch after page load. This keeps the prerendering benefits while avoiding serving user-specific HTML from the cache.

- If you need mixed content (mostly public content with a small personalized part), prefer using edge-side includes (ESI) or client-side fragments for the personalized bits.

If no cookie- or per-user SSR exists (i.e., your pages are identical across users), phantom-frame will operate stably and provide the full benefits of caching and prerendering.

## License

See LICENSE file for details
