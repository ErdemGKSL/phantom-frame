use crate::cache::{CacheStore, CachedResponse};
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
};
use std::sync::Arc;

#[derive(Clone)]
pub struct ProxyState {
    cache: CacheStore,
    proxy_url: String,
}

impl ProxyState {
    pub fn new(cache: CacheStore, proxy_url: String) -> Self {
        Self { cache, proxy_url }
    }
}

/// Main proxy handler that serves prerendered content from cache
/// or fetches from backend if not cached
pub async fn proxy_handler(
    State(state): State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    let path = req.uri().path();
    let query = req.uri().query().unwrap_or("");
    let cache_key = format!("{}?{}", path, query);

    // Try to get from cache first
    if let Some(cached) = state.cache.get(&cache_key).await {
        tracing::info!("Cache hit for: {}", cache_key);
        return Ok(build_response_from_cache(cached));
    }

    tracing::info!("Cache miss for: {}, fetching from backend", cache_key);

    // Fetch from backend (proxy_url)
    let target_url = format!("{}{}", state.proxy_url, req.uri());
    let client = reqwest::Client::new();

    let method = req.method().clone();
    let headers = req.headers().clone();

    let response = match client
        .request(method, &target_url)
        .headers(convert_headers(&headers))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("Failed to fetch from backend: {}", e);
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    // Cache the response
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

    state
        .cache
        .set(cache_key.clone(), cached_response.clone())
        .await;
    tracing::info!("Cached response for: {}", cache_key);

    Ok(build_response_from_cache(cached_response))
}

fn build_response_from_cache(cached: CachedResponse) -> Response<Body> {
    let mut response = Response::builder().status(cached.status);

    // Add headers
    let headers = response.headers_mut().unwrap();
    for (key, value) in cached.headers {
        if let Ok(header_name) = key.parse::<HeaderName>() {
            if let Ok(header_value) = HeaderValue::from_str(&value) {
                headers.insert(header_name, header_value);
            }
        }
    }

    response.body(Body::from(cached.body)).unwrap()
}

fn convert_headers(headers: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut req_headers = reqwest::header::HeaderMap::new();
    for (key, value) in headers {
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
        }
    }
    map
}
