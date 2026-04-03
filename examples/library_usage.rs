use axum::Router;
use phantom_frame::{
    cache::CacheHandle, create_proxy, CacheStrategy, CompressStrategy, CreateProxyConfig, ProxyMode,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    // Initialize tracing (optional but recommended)
    // tracing_subscriber::fmt::init();

    // Create proxy configuration
    // You can specify method prefixes to filter by HTTP method
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
        .caching_strategy(CacheStrategy::None)
        .compression_strategy(CompressStrategy::Brotli)
        .with_cache_storage_mode(phantom_frame::CacheStorageMode::Filesystem)
        .with_cache_directory(PathBuf::from("./.phantom-frame-cache"))
        .with_websocket_enabled(true); // Enable WebSocket support (default: true)

    // Create proxy - proxy_url is the backend server to proxy requests to
    let (proxy_app, handle): (Router, CacheHandle) = create_proxy(proxy_config);

    // You can clone and use the handle in your code
    let handle_clone = handle.clone();

    // Example: Trigger cache invalidation from another part of your application
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

        // Invalidate all cache entries
        handle_clone.invalidate_all();
        println!("All cache invalidated!");

        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

        // Invalidate only cache entries matching a pattern (supports wildcards)
        handle_clone.invalidate("GET:/api/*");
        println!("Cache invalidated for GET:/api/* pattern!");
    });

    // Example: PreGenerate (SSG) mode with snapshot management
    // let ssg_config = CreateProxyConfig::new("http://localhost:8080".to_string())
    //     .with_proxy_mode(ProxyMode::PreGenerate {
    //         paths: vec!["/".to_string(), "/about".to_string(), "/book/1".to_string()],
    //         fallthrough: false, // return 404 on cache miss (default)
    //     });
    // let (ssg_app, ssg_handle) = create_proxy(ssg_config);
    // // At runtime, manage snapshots:
    // ssg_handle.add_snapshot("/book/2").await.unwrap();
    // ssg_handle.refresh_snapshot("/book/1").await.unwrap();
    // ssg_handle.remove_snapshot("/about").await.unwrap();
    // ssg_handle.refresh_all_snapshots().await.unwrap();
    let _ = ProxyMode::Dynamic; // suppress unused import warning

    // Start the proxy server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    println!("Proxy server listening on http://0.0.0.0:3000");
    println!("Caching paths: /api/*, /public/*, GET /admin/stats");
    println!("Excluding: /api/admin/*, POST *, PUT *, DELETE *");
    println!("Cache strategy: none (proxy-only mode)");
    println!("Compression strategy: brotli (applies only to cached responses)");
    println!("Cache storage mode: filesystem (custom cache directory)");
    println!("Note: Cache reads and writes are disabled in this example");
    println!("WebSocket support: enabled");

    axum::serve(listener, proxy_app).await.unwrap();
}
