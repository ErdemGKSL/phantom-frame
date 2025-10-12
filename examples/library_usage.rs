use axum::Router;
use phantom_frame::{cache::RefreshTrigger, create_proxy, CreateProxyConfig};

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
            "POST *".to_string(),  // Don't cache any POST requests
            "PUT *".to_string(),   // Don't cache any PUT requests
            "DELETE *".to_string(), // Don't cache any DELETE requests
        ]);

    // Create proxy - proxy_url is the backend server to proxy requests to
    let (proxy_app, refresh_trigger): (Router, RefreshTrigger) = create_proxy(proxy_config);

    // You can clone and use the refresh_trigger in your code
    let trigger_clone = refresh_trigger.clone();

    // Example: Trigger cache refresh from another part of your application
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        trigger_clone.trigger();
        println!("Cache refreshed!");
    });

    // Start the proxy server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    println!("Proxy server listening on http://0.0.0.0:3000");
    println!("Caching paths: /api/*, /public/*, GET /admin/stats");
    println!("Excluding: /api/admin/*, POST *, PUT *, DELETE *");
    println!("Note: Only GET requests will be cached (POST/PUT/DELETE are excluded)");

    axum::serve(listener, proxy_app).await.unwrap();
}
