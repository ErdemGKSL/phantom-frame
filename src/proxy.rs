use crate::cache::{CacheStore, CachedResponse};
use crate::path_matcher::should_cache_path;
use crate::CreateProxyConfig;
use axum::{
    body::Body,
    extract::Extension,
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
};
use std::sync::Arc;
use hyper_util::rt::TokioIo;

#[derive(Clone)]
pub struct ProxyState {
    cache: CacheStore,
    config: CreateProxyConfig,
}

impl ProxyState {
    pub fn new(cache: CacheStore, config: CreateProxyConfig) -> Self {
        Self { cache, config }
    }
}

/// Check if the request is a WebSocket or other upgrade request
/// 
/// WebSocket and other protocol upgrades are detected by checking for:
/// - `Connection: Upgrade` header (case-insensitive)
/// - Presence of `Upgrade` header
/// 
/// These requests will bypass caching and use direct TCP tunneling instead.
fn is_upgrade_request(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("upgrade"))
        .unwrap_or(false)
        || headers.contains_key(axum::http::header::UPGRADE)
}

/// Main proxy handler that serves prerendered content from cache
/// or fetches from backend if not cached
pub async fn proxy_handler(
    Extension(state): Extension<Arc<ProxyState>>,
    req: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    // Check for upgrade requests FIRST (before consuming anything from the request)
    // This is critical for WebSocket to work properly
    let is_upgrade = is_upgrade_request(req.headers());
    
    if is_upgrade {
        let method_str = req.method().as_str();
        let path = req.uri().path();
        
        if state.config.enable_websocket {
            tracing::info!("Upgrade request detected for {} {}, establishing direct proxy tunnel", method_str, path);
            return handle_upgrade_request(state, req).await;
        } else {
            tracing::warn!("Upgrade request detected for {} {} but WebSocket support is disabled", method_str, path);
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    }
    
    // Extract request details (only after we know it's not an upgrade request)
    let method = req.method().clone();
    let method_str = method.as_str();
    let uri = req.uri().clone();
    let path = uri.path();
    let query = uri.query().unwrap_or("");
    let headers = req.headers().clone();
    
    // Check if only GET requests are allowed
    if state.config.forward_get_only && method != axum::http::Method::GET {
        tracing::warn!("Non-GET request {} {} rejected (forward_get_only is enabled)", method_str, path);
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    
    // Check if this path should be cached based on include/exclude patterns
    let should_cache = should_cache_path(
        method_str,
        path,
        &state.config.include_paths,
        &state.config.exclude_paths,
    );
    
    // Generate cache key using the configured function
    let req_info = crate::RequestInfo {
        method: method_str,
        path,
        query,
        headers: &headers,
    };
    let cache_key = (state.config.cache_key_fn)(&req_info);

    // Try to get from cache first (only if caching is enabled for this path)
    if should_cache {
        if let Some(cached) = state.cache.get(&cache_key).await {
            tracing::info!("Cache hit for: {} {}", method_str, cache_key);
            return Ok(build_response_from_cache(cached));
        }
        tracing::info!("Cache miss for: {} {}, fetching from backend", method_str, cache_key);
    } else {
        tracing::info!("{} {} not cacheable (filtered), proxying directly", method_str, path);
    }
    
    // Convert body to bytes to forward it
    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("Failed to read request body: {}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // Fetch from backend (proxy_url)
    let target_url = format!("{}{}", state.config.proxy_url, uri);
    let client = reqwest::Client::new();

    let response = match client
        .request(method.clone(), &target_url)
        .headers(convert_headers(&headers))
        .body(body_bytes.to_vec())
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Failed to fetch from backend: {}", e);
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    // Cache the response (only if caching is enabled for this path)
    let status = response.status().as_u16();
    let response_headers = response.headers().clone();
    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes.to_vec(),
        Err(e) => {
            tracing::error!("Failed to read response body: {}", e);
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    let cached_response = CachedResponse {
        body: body_bytes.clone(),
        headers: convert_headers_to_map(&response_headers),
        status,
    };

    if should_cache {
        state
            .cache
            .set(cache_key.clone(), cached_response.clone())
            .await;
        tracing::info!("Cached response for: {} {}", method_str, cache_key);
    }

    Ok(build_response_from_cache(cached_response))
}

/// Handle WebSocket and other upgrade requests by establishing a direct TCP tunnel
/// 
/// This function handles long-lived connections like WebSocket by:
/// 1. Connecting to the backend server
/// 2. Forwarding the upgrade request
/// 3. Capturing both client and backend upgrade connections
/// 4. Creating a bidirectional TCP tunnel between them
/// 
/// The tunnel remains open for the lifetime of the connection, allowing
/// full-duplex communication. Data flows directly between client and backend
/// without any caching or inspection.
async fn handle_upgrade_request(
    state: Arc<ProxyState>,
    mut req: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    let target_url = format!("{}{}", state.config.proxy_url, req.uri());
    
    // Parse the backend URL to extract host and port
    let backend_uri = target_url.parse::<hyper::Uri>().map_err(|e| {
        tracing::error!("Failed to parse backend URL: {}", e);
        StatusCode::BAD_GATEWAY
    })?;
    
    let host = backend_uri.host().ok_or_else(|| {
        tracing::error!("No host in backend URL");
        StatusCode::BAD_GATEWAY
    })?;
    
    let port = backend_uri.port_u16().unwrap_or_else(|| {
        if backend_uri.scheme_str() == Some("https") {
            443
        } else {
            80
        }
    });
    
    // IMPORTANT: Set up client upgrade BEFORE processing the request
    // This captures the client's connection for later upgrade
    let client_upgrade = hyper::upgrade::on(&mut req);
    
    // Connect to backend
    let backend_stream = tokio::net::TcpStream::connect((host, port))
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect to backend {}:{}: {}", host, port, e);
            StatusCode::BAD_GATEWAY
        })?;
    
    let backend_io = TokioIo::new(backend_stream);
    
    // Build the backend request with upgrade support
    let (mut sender, conn) = hyper::client::conn::http1::handshake(backend_io)
        .await
        .map_err(|e| {
            tracing::error!("Failed to handshake with backend: {}", e);
            StatusCode::BAD_GATEWAY
        })?;
    
    // Spawn a task to poll the connection - this will handle the upgrade
    let conn_task = tokio::spawn(async move {
        match conn.with_upgrades().await {
            Ok(parts) => {
                tracing::info!("Backend connection upgraded successfully");
                Ok(parts)
            }
            Err(e) => {
                tracing::error!("Backend connection failed: {}", e);
                Err(e)
            }
        }
    });
    
    // Forward the request to the backend
    let backend_response = sender.send_request(req).await.map_err(|e| {
        tracing::error!("Failed to send request to backend: {}", e);
        StatusCode::BAD_GATEWAY
    })?;
    
    // Check if backend accepted the upgrade
    let status = backend_response.status();
    if status != StatusCode::SWITCHING_PROTOCOLS {
        tracing::warn!("Backend did not accept upgrade request, status: {}", status);
        // Convert the backend response to our response type
        let (parts, body) = backend_response.into_parts();
        let body = Body::new(body);
        return Ok(Response::from_parts(parts, body));
    }
    
    // Extract headers before moving backend_response
    let backend_headers = backend_response.headers().clone();
    
    // Get the upgraded backend connection
    let backend_upgrade = hyper::upgrade::on(backend_response);
    
    // Spawn a task to handle bidirectional streaming between client and backend
    tokio::spawn(async move {
        tracing::info!("Starting upgrade tunnel establishment");
        
        // Wait for both upgrades to complete
        let (client_result, backend_result) = tokio::join!(
            client_upgrade,
            backend_upgrade
        );
        
        // Drop the connection task as we now have the upgraded streams
        drop(conn_task);
        
        match (client_result, backend_result) {
            (Ok(client_upgraded), Ok(backend_upgraded)) => {
                tracing::info!("Both upgrades successful, establishing bidirectional tunnel");
                
                // Wrap both in TokioIo for AsyncRead + AsyncWrite
                let mut client_stream = TokioIo::new(client_upgraded);
                let mut backend_stream = TokioIo::new(backend_upgraded);
                
                // Create bidirectional tunnel
                match tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream).await {
                    Ok((client_to_backend, backend_to_client)) => {
                        tracing::info!(
                            "Tunnel closed gracefully. Transferred {} bytes client->backend, {} bytes backend->client",
                            client_to_backend,
                            backend_to_client
                        );
                    }
                    Err(e) => {
                        tracing::error!("Tunnel error: {}", e);
                    }
                }
            }
            (Err(e), _) => {
                tracing::error!("Client upgrade failed: {}", e);
            }
            (_, Err(e)) => {
                tracing::error!("Backend upgrade failed: {}", e);
            }
        }
    });
    
    // Build the response to send back to the client with upgrade support
    let mut response = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .body(Body::empty())
        .unwrap();
    
    // Copy necessary headers from backend response
    // These headers are essential for WebSocket handshake
    if let Some(upgrade_header) = backend_headers.get(axum::http::header::UPGRADE) {
        response.headers_mut().insert(
            axum::http::header::UPGRADE,
            upgrade_header.clone(),
        );
    }
    if let Some(connection_header) = backend_headers.get(axum::http::header::CONNECTION) {
        response.headers_mut().insert(
            axum::http::header::CONNECTION,
            connection_header.clone(),
        );
    }
    if let Some(sec_websocket_accept) = backend_headers.get("sec-websocket-accept") {
        response.headers_mut().insert(
            HeaderName::from_static("sec-websocket-accept"),
            sec_websocket_accept.clone(),
        );
    }
    
    tracing::info!("Upgrade response sent to client, tunnel task spawned");
    
    Ok(response)
}

fn build_response_from_cache(cached: CachedResponse) -> Response<Body> {
    let mut response = Response::builder().status(cached.status);

    // Add headers
    let headers = response.headers_mut().unwrap();
    for (key, value) in cached.headers {
        if let Ok(header_name) = key.parse::<HeaderName>() {
            if let Ok(header_value) = HeaderValue::from_str(&value) {
                headers.insert(header_name, header_value);
            } else {
                tracing::warn!("Failed to parse header value for key '{}': {:?}", key, value);
            }
        } else {
            tracing::warn!("Failed to parse header name: {}", key);
        }
    }

    response.body(Body::from(cached.body)).unwrap()
}

fn convert_headers(headers: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut req_headers = reqwest::header::HeaderMap::new();
    for (key, value) in headers {
        // Skip host header as reqwest will set it
        if key == axum::http::header::HOST {
            continue;
        }
        if let Ok(val) = value.to_str() {
            if let Ok(header_value) = reqwest::header::HeaderValue::from_str(val) {
                req_headers.insert(key.clone(), header_value);
            }
        }
    }
    req_headers
}

fn convert_headers_to_map(
    headers: &reqwest::header::HeaderMap,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for (key, value) in headers {
        if let Ok(val) = value.to_str() {
            map.insert(key.to_string(), val.to_string());
        } else {
            // Log when we can't convert a header (might be binary)
            tracing::debug!("Could not convert header '{}' to string", key);
        }
    }
    map
}
