use crate::cache::{CacheStore, CachedResponse};
use crate::compression::{
    client_accepts_encoding, compress_body, configured_encoding, decode_upstream_body,
    decompress_body, identity_acceptable,
};
use crate::path_matcher::should_cache_path;
use crate::{CompressStrategy, CreateProxyConfig};
use axum::{
    body::Body,
    extract::Extension,
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::sync::Arc;

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
            tracing::debug!(
                "Upgrade request detected for {} {}, establishing direct proxy tunnel",
                method_str,
                path
            );
            return handle_upgrade_request(state, req).await;
        } else {
            tracing::warn!(
                "Upgrade request detected for {} {} but WebSocket support is disabled",
                method_str,
                path
            );
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
        tracing::warn!(
            "Non-GET request {} {} rejected (forward_get_only is enabled)",
            method_str,
            path
        );
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
    let cache_reads_enabled = !matches!(state.config.cache_strategy, crate::CacheStrategy::None);

    // Try to get 404 cache first (available even if should_cache is false)
    if cache_reads_enabled && state.config.cache_404_capacity > 0 {
        if let Some(cached) = state.cache.get_404(&cache_key).await {
            if cached_response_is_allowed(&state.config.cache_strategy, &cached) {
                tracing::debug!("404 cache hit for: {} {}", method_str, cache_key);
                return build_response_from_cache(cached, &headers);
            }
        }
    }

    // Try to get from cache first (only if caching is enabled for this path)
    if should_cache && cache_reads_enabled {
        if let Some(cached) = state.cache.get(&cache_key).await {
            if cached_response_is_allowed(&state.config.cache_strategy, &cached) {
                tracing::debug!("Cache hit for: {} {}", method_str, cache_key);
                return build_response_from_cache(cached, &headers);
            }
        }
        tracing::debug!(
            "Cache miss for: {} {}, fetching from backend",
            method_str,
            cache_key
        );
    } else if !cache_reads_enabled {
        tracing::debug!(
            "{} {} not cacheable (cache strategy: none), proxying directly",
            method_str,
            path
        );
    } else {
        tracing::debug!(
            "{} {} not cacheable (filtered), proxying directly",
            method_str,
            path
        );
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
    // Use path+query only — not the full `uri` — because HTTP/2 requests carry an
    // absolute-form URI (e.g. `https://example.com/path`) which would corrupt the
    // concatenated URL when appended to proxy_url.
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| uri.path());
    let target_url = format!("{}{}", state.config.proxy_url, path_and_query);
    let client = match reqwest::Client::builder()
        .no_brotli()
        .no_deflate()
        .no_gzip()
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!("Failed to build upstream HTTP client: {}", error);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

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

    let response_content_type = response_headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let response_is_cacheable = state
        .config
        .cache_strategy
        .allows_content_type(response_content_type);
    let upstream_content_encoding = response_headers
        .get(axum::http::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok());
    let should_try_cache = cache_reads_enabled
        && response_is_cacheable
        && (should_cache || state.config.cache_404_capacity > 0);
    let normalized_body = if should_try_cache || state.config.use_404_meta {
        match decode_upstream_body(&body_bytes, upstream_content_encoding) {
            Ok(body) => Some(body),
            Err(error) => {
                tracing::warn!(
                    "Skipping cache compression for {} {} due to unsupported upstream encoding: {}",
                    method_str,
                    path,
                    error
                );
                None
            }
        }
    } else {
        None
    };

    // Determine if this should be cached as a 404 (either by status or by meta tag if enabled)
    let mut is_404 = status == 404;
    if !is_404 && state.config.use_404_meta {
        if let Some(body) = normalized_body.as_deref() {
            is_404 = body_contains_404_meta(body);
        }
    }

    let should_store_404 = is_404
        && state.config.cache_404_capacity > 0
        && response_is_cacheable
        && cache_reads_enabled
        && normalized_body.is_some();
    let should_store_response = !is_404
        && should_cache
        && response_is_cacheable
        && cache_reads_enabled
        && normalized_body.is_some();

    if should_store_404 || should_store_response {
        let cached_response = match build_cached_response(
            status,
            &response_headers,
            normalized_body.as_deref().unwrap(),
            &state.config.compress_strategy,
        ) {
            Ok(cached_response) => cached_response,
            Err(error) => {
                tracing::warn!(
                    "Failed to prepare cached response for {} {}: {}",
                    method_str,
                    path,
                    error
                );
                return Ok(build_response_from_upstream(
                    status,
                    &response_headers,
                    body_bytes,
                ));
            }
        };

        if should_store_404 {
            state
                .cache
                .set_404(cache_key.clone(), cached_response.clone())
                .await;
            tracing::debug!("Cached 404 response for: {} {}", method_str, cache_key);
        } else {
            state
                .cache
                .set(cache_key.clone(), cached_response.clone())
                .await;
            tracing::debug!("Cached response for: {} {}", method_str, cache_key);
        }

        return build_response_from_cache(cached_response, &headers);
    }

    Ok(build_response_from_upstream(
        status,
        &response_headers,
        body_bytes,
    ))
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
    // Use path+query only for the same reason as in proxy_handler (HTTP/2 absolute-form URI).
    let req_path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| req.uri().path());
    let target_url = format!("{}{}", state.config.proxy_url, req_path_and_query);

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
                tracing::debug!("Backend connection upgraded successfully");
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
        tracing::debug!("Starting upgrade tunnel establishment");

        // Wait for both upgrades to complete
        let (client_result, backend_result) = tokio::join!(client_upgrade, backend_upgrade);

        // Drop the connection task as we now have the upgraded streams
        drop(conn_task);

        match (client_result, backend_result) {
            (Ok(client_upgraded), Ok(backend_upgraded)) => {
                tracing::debug!("Both upgrades successful, establishing bidirectional tunnel");

                // Wrap both in TokioIo for AsyncRead + AsyncWrite
                let mut client_stream = TokioIo::new(client_upgraded);
                let mut backend_stream = TokioIo::new(backend_upgraded);

                // Create bidirectional tunnel
                match tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream).await {
                    Ok((client_to_backend, backend_to_client)) => {
                        tracing::debug!(
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
        response
            .headers_mut()
            .insert(axum::http::header::UPGRADE, upgrade_header.clone());
    }
    if let Some(connection_header) = backend_headers.get(axum::http::header::CONNECTION) {
        response
            .headers_mut()
            .insert(axum::http::header::CONNECTION, connection_header.clone());
    }
    if let Some(sec_websocket_accept) = backend_headers.get("sec-websocket-accept") {
        response.headers_mut().insert(
            HeaderName::from_static("sec-websocket-accept"),
            sec_websocket_accept.clone(),
        );
    }

    tracing::debug!("Upgrade response sent to client, tunnel task spawned");

    Ok(response)
}

fn build_response_from_cache(
    cached: CachedResponse,
    request_headers: &HeaderMap,
) -> Result<Response<Body>, StatusCode> {
    let mut response_headers = cached.headers;
    let body = if let Some(content_encoding) = cached.content_encoding {
        if client_accepts_encoding(request_headers, content_encoding) {
            upsert_vary_accept_encoding(&mut response_headers);
            cached.body
        } else {
            if !identity_acceptable(request_headers) {
                tracing::warn!(
                    "Client does not accept cached encoding '{}' or identity fallback",
                    content_encoding.as_header_value()
                );
                return Err(StatusCode::NOT_ACCEPTABLE);
            }

            response_headers.remove("content-encoding");
            upsert_vary_accept_encoding(&mut response_headers);
            match decompress_body(&cached.body, content_encoding) {
                Ok(body) => body,
                Err(error) => {
                    tracing::error!("Failed to decompress cached response: {}", error);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
    } else {
        cached.body
    };

    response_headers.remove("transfer-encoding");
    response_headers.insert("content-length".to_string(), body.len().to_string());

    Ok(build_response(cached.status, response_headers, body))
}

fn build_cached_response(
    status: u16,
    response_headers: &reqwest::header::HeaderMap,
    normalized_body: &[u8],
    compress_strategy: &CompressStrategy,
) -> anyhow::Result<CachedResponse> {
    let mut headers = convert_headers_to_map(response_headers);
    headers.remove("content-encoding");
    headers.remove("content-length");
    headers.remove("transfer-encoding");

    let content_encoding = configured_encoding(compress_strategy);
    let body = if let Some(content_encoding) = content_encoding {
        let compressed = compress_body(normalized_body, content_encoding)?;
        headers.insert(
            "content-encoding".to_string(),
            content_encoding.as_header_value().to_string(),
        );
        upsert_vary_accept_encoding(&mut headers);
        compressed
    } else {
        normalized_body.to_vec()
    };

    headers.insert("content-length".to_string(), body.len().to_string());

    Ok(CachedResponse {
        body,
        headers,
        status,
        content_encoding,
    })
}

fn build_response_from_upstream(
    status: u16,
    response_headers: &reqwest::header::HeaderMap,
    body: Vec<u8>,
) -> Response<Body> {
    let mut headers = convert_headers_to_map(response_headers);
    headers.remove("transfer-encoding");
    headers.insert("content-length".to_string(), body.len().to_string());
    build_response(status, headers, body)
}

fn build_response(
    status: u16,
    response_headers: HashMap<String, String>,
    body: Vec<u8>,
) -> Response<Body> {
    let mut response = Response::builder().status(status);

    // Add headers
    let headers = response.headers_mut().unwrap();
    for (key, value) in response_headers {
        if let Ok(header_name) = key.parse::<HeaderName>() {
            if let Ok(header_value) = HeaderValue::from_str(&value) {
                headers.insert(header_name, header_value);
            } else {
                tracing::warn!(
                    "Failed to parse header value for key '{}': {:?}",
                    key,
                    value
                );
            }
        } else {
            tracing::warn!("Failed to parse header name: {}", key);
        }
    }

    response.body(Body::from(body)).unwrap()
}

fn cached_response_is_allowed(strategy: &crate::CacheStrategy, cached: &CachedResponse) -> bool {
    strategy.allows_content_type(
        cached
            .headers
            .get("content-type")
            .map(|value| value.as_str()),
    )
}

fn body_contains_404_meta(body: &[u8]) -> bool {
    let Ok(body_str) = std::str::from_utf8(body) else {
        return false;
    };

    let name_dbl = "name=\"phantom-404\"";
    let name_sgl = "name='phantom-404'";
    let content_dbl = "content=\"true\"";
    let content_sgl = "content='true'";

    (body_str.contains(name_dbl) || body_str.contains(name_sgl))
        && (body_str.contains(content_dbl) || body_str.contains(content_sgl))
}

fn upsert_vary_accept_encoding(headers: &mut HashMap<String, String>) {
    match headers.get_mut("vary") {
        Some(value) => {
            let has_accept_encoding = value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("accept-encoding"));
            if !has_accept_encoding {
                value.push_str(", Accept-Encoding");
            }
        }
        None => {
            headers.insert("vary".to_string(), "Accept-Encoding".to_string());
        }
    }
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
            map.insert(key.as_str().to_ascii_lowercase(), val.to_string());
        } else {
            // Log when we can't convert a header (might be binary)
            tracing::debug!("Could not convert header '{}' to string", key);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::{compress_body, ContentEncoding};
    use axum::body::to_bytes;

    fn response_headers() -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("text/html; charset=utf-8"),
        );
        headers
    }

    #[test]
    fn test_build_cached_response_uses_selected_encoding() {
        let cached = build_cached_response(
            200,
            &response_headers(),
            b"<html>compressed</html>",
            &CompressStrategy::Gzip,
        )
        .unwrap();

        assert_eq!(cached.content_encoding, Some(ContentEncoding::Gzip));
        assert_eq!(
            cached.headers.get("content-encoding"),
            Some(&"gzip".to_string())
        );
        assert_eq!(
            cached.headers.get("vary"),
            Some(&"Accept-Encoding".to_string())
        );
    }

    #[tokio::test]
    async fn test_build_response_from_cache_falls_back_to_identity() {
        let body = b"<html>identity</html>";
        let compressed = compress_body(body, ContentEncoding::Brotli).unwrap();
        let cached = CachedResponse {
            body: compressed,
            headers: HashMap::from([
                ("content-type".to_string(), "text/html".to_string()),
                ("content-encoding".to_string(), "br".to_string()),
                ("content-length".to_string(), "123".to_string()),
                ("vary".to_string(), "Accept-Encoding".to_string()),
            ]),
            status: 200,
            content_encoding: Some(ContentEncoding::Brotli),
        };

        let mut request_headers = HeaderMap::new();
        request_headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip"),
        );

        let response = build_response_from_cache(cached, &request_headers).unwrap();
        assert!(response
            .headers()
            .get(axum::http::header::CONTENT_ENCODING)
            .is_none());

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), b"<html>identity</html>");
    }

    #[tokio::test]
    async fn test_build_response_from_cache_keeps_supported_encoding() {
        let body = b"<html>compressed</html>";
        let compressed = compress_body(body, ContentEncoding::Brotli).unwrap();
        let cached = CachedResponse {
            body: compressed.clone(),
            headers: HashMap::from([
                ("content-type".to_string(), "text/html".to_string()),
                ("content-encoding".to_string(), "br".to_string()),
                ("content-length".to_string(), compressed.len().to_string()),
                ("vary".to_string(), "Accept-Encoding".to_string()),
            ]),
            status: 200,
            content_encoding: Some(ContentEncoding::Brotli),
        };

        let mut request_headers = HeaderMap::new();
        request_headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip;q=0.5"),
        );

        let response = build_response_from_cache(cached, &request_headers).unwrap();
        assert_eq!(
            response.headers().get(axum::http::header::CONTENT_ENCODING),
            Some(&HeaderValue::from_static("br"))
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), compressed.as_slice());
    }
}
