# WebSocket and Protocol Upgrade Support

## Overview

phantom-frame now supports WebSocket and other HTTP protocol upgrade connections through automatic detection and direct TCP tunneling. This enables long-lived, bidirectional connections to work seamlessly through the proxy.

## How It Works

### 1. Automatic Detection

The proxy automatically detects upgrade requests by checking for:
- `Connection: Upgrade` header (case-insensitive)
- Presence of `Upgrade` header

### 2. Direct TCP Tunneling

When an upgrade request is detected:

1. **Bypass Cache**: The request completely bypasses the caching layer
2. **Backend Connection**: A TCP connection is established to the backend server
3. **Upgrade Handshake**: The upgrade request is forwarded to the backend
4. **Client Upgrade Capture**: The client's upgraded connection is captured using `hyper::upgrade::on()`
5. **Backend Upgrade Capture**: The backend's upgraded connection is captured
6. **Bidirectional Tunnel**: `tokio::io::copy_bidirectional()` creates a tunnel between them
7. **Connection Lifetime**: The tunnel remains open until either side closes

### 3. Architecture

```
Client                    Proxy                      Backend
  |                        |                           |
  |-- Upgrade Request ---->|                           |
  |                        |--- Upgrade Request ------>|
  |                        |                           |
  |                        |<-- 101 Switching ---------|
  |<-- 101 Switching ------|         Protocols         |
  |      Protocols         |                           |
  |                        |                           |
  |<===== Bidirectional TCP Tunnel ==================>|
  |                        |                           |
```

## Implementation Details

### Key Code Components

**Detection Function** (`is_upgrade_request`):
```rust
fn is_upgrade_request(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("upgrade"))
        .unwrap_or(false)
        || headers.contains_key(axum::http::header::UPGRADE)
}
```

**Tunnel Handler** (`handle_upgrade_request`):
- Connects to backend
- Forwards upgrade request
- Captures both client and backend upgrades
- Creates bidirectional tunnel with `copy_bidirectional`

### Dependencies Added

```toml
hyper = { version = "1.5", features = ["full"] }
hyper-util = { version = "0.1", features = ["tokio"] }
http-body-util = "0.1"
```

## Supported Protocols

This implementation supports any HTTP/1.1 protocol upgrade, including:

- **WebSocket** (ws:// and wss://)
- **HTTP/2** (h2c upgrade)
- **Custom protocols** using HTTP upgrade mechanism

## Usage Example

### Client-Side (JavaScript)

```javascript
// Connect to WebSocket through the proxy
const ws = new WebSocket('ws://localhost:3000/api/ws');

ws.onopen = () => {
  console.log('Connected');
  ws.send('Hello Server!');
};

ws.onmessage = (event) => {
  console.log('Received:', event.data);
};

ws.onclose = () => {
  console.log('Disconnected');
};
```

### Backend (Rust with Axum)

```rust
use axum::{
    routing::get,
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    Router,
};

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(msg) = socket.recv().await {
        if let Ok(msg) = msg {
            // Echo messages back
            if socket.send(msg).await.is_err() {
                break;
            }
        }
    }
}

let app = Router::new()
    .route("/api/ws", get(ws_handler));
```

### Proxy Configuration

WebSocket support is enabled by default. You can control it via configuration:

```toml
[server]
proxy_url = "http://localhost:8080"
proxy_port = 3000
enable_websocket = true  # Enable WebSocket support (default: true)
```

Or in library mode:

```rust
let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_websocket_enabled(true);  // Enable WebSocket support
```

To disable WebSocket support:

```toml
[server]
enable_websocket = false
```

Or in library mode:

```rust
let proxy_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    .with_websocket_enabled(false);
```

When disabled, upgrade requests will receive a `501 Not Implemented` response.

## Performance Characteristics

- **Zero-copy tunnel**: Uses `tokio::io::copy_bidirectional` for optimal performance
- **No buffering**: Data flows directly between client and backend
- **Low latency**: Minimal overhead compared to direct connection
- **Concurrent connections**: Supports thousands of concurrent WebSocket connections

## Logging

The proxy logs upgrade-related events:

```
INFO phantom_frame::proxy: Upgrade request detected for GET /api/ws, establishing direct proxy tunnel
INFO phantom_frame::proxy: Backend upgrade successful
INFO phantom_frame::proxy: Both upgrades successful, establishing bidirectional tunnel
INFO phantom_frame::proxy: Tunnel closed gracefully. Transferred 1024 bytes client->backend, 2048 bytes backend->client
```

## Limitations

1. **No Caching**: Upgrade connections are never cached (by design)
2. **No Inspection**: Data flowing through the tunnel is not inspected or logged
3. **HTTP/1.1 Only**: Currently only supports HTTP/1.1 upgrade mechanism
4. **No SSL Termination**: If using wss://, SSL must be terminated before phantom-frame or after it

## Testing

To test WebSocket support:

1. Start your backend with WebSocket support on port 8080
2. Start phantom-frame with proxy_url pointing to your backend
3. Connect a WebSocket client to the proxy port
4. Verify bidirectional communication works

## Error Handling

The implementation handles various error cases:

- **Backend connection failure**: Returns 502 Bad Gateway
- **Backend refuses upgrade**: Returns the backend's response to client
- **Tunnel breaks**: Logs error and closes both connections gracefully
- **Client disconnects**: Backend connection is closed automatically
- **Backend disconnects**: Client connection is closed automatically

## Future Enhancements

Possible improvements for the future:

- [ ] Support for HTTP/2 native streams
- [ ] Optional SSL/TLS termination
- [ ] Metrics and monitoring for WebSocket connections
- [ ] Rate limiting for WebSocket messages
- [ ] Connection pooling for backend WebSocket connections
