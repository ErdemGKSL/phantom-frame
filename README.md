# Phantom Frame

A high-performance prerendering proxy engine written in Rust. Cache and serve prerendered content with ease.

<p align="center">
  <img src="https://raw.githubusercontent.com/ErdemGKSL/phantom-frame/refs/heads/main/banner.jpg" alt="Phantom Frame Banner" width="100%">
</p>

## Features

- 🚀 **Fast caching proxy** - Cache prerendered content and serve it instantly
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

# Optional: Bearer token for control endpoint authentication
# If set, requests to /refresh-cache must include: Authorization: Bearer <token>
control_auth = "your-secret-token-here"
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
phantom-frame = { version = "0.1.8" }
tokio = { version = "1.40", features = ["full"] }
axum = "0.8.6"
```

Use in your code:

```rust
use phantom_frame::{create_proxy, cache::RefreshTrigger, CreateProxyConfig};
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
