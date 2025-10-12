# phantom-frame

A high-performance prerendering proxy engine written in Rust. Cache and serve prerendered content with ease.

## Features

- 🚀 **Fast caching proxy** - Cache prerendered content and serve it instantly
- 🔧 **Dual mode operation** - Run as standalone HTTP server or integrate as a library
- 🔄 **Dynamic cache refresh** - Trigger cache invalidation via control endpoint or programmatically
- 🔐 **Optional authentication** - Secure control endpoints with bearer token auth
- ⚡ **Async/await** - Built on Tokio and Axum for high performance
- 📦 **Easy integration** - Simple API for library usage

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

# Optional: Bearer token for control endpoint authentication
# If set, requests to /refresh-cache must include: Authorization: Bearer <token>
control_auth = "your-secret-token-here"
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
phantom-frame = { path = "../phantom-frame" }
tokio = { version = "1.40", features = ["full"] }
axum = "0.7"
```

Use in your code:

```rust
use phantom_frame::{create_proxy, cache::RefreshTrigger};
use axum::Router;

#[tokio::main]
async fn main() {
    // Create proxy - proxy_url is the backend server to proxy requests to
    let (proxy_app, refresh_trigger): (Router, RefreshTrigger) = 
        create_proxy("http://localhost:8080".to_string());
    
    // Clone and use the refresh_trigger anywhere in your app
    let trigger_clone = refresh_trigger.clone();
    
    // Trigger cache refresh programmatically
    tokio::spawn(async move {
        // Your custom logic here
        trigger_clone.trigger();
    });
    
    // Start the proxy server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .unwrap();
    
    axum::serve(listener, proxy_app).await.unwrap();
}
```

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

## API Reference

### Library API

#### `create_proxy(proxy_url: String) -> (Router, RefreshTrigger)`

Creates a proxy router and refresh trigger.

- **Parameters**: `proxy_url` - The backend server URL to proxy requests to
- **Returns**: Tuple of `(Router, RefreshTrigger)`

#### `RefreshTrigger`

A clonable trigger for cache invalidation.

- `trigger()` - Trigger a cache refresh
- `subscribe()` - Subscribe to refresh events (returns a broadcast receiver)

### Control Endpoints

#### `POST /refresh-cache`

Triggers cache invalidation. Requires `Authorization: Bearer <token>` header if `control_auth` is configured.

## License

See LICENSE file for details
