use axum::Router;
use phantom_frame::{cache::RefreshTrigger, create_proxy};

#[tokio::main]
async fn main() {
    // Initialize tracing (optional but recommended)
    // tracing_subscriber::fmt::init();

    // Create proxy - proxy_url is the backend server to proxy requests to
    let (proxy_app, refresh_trigger): (Router, RefreshTrigger) =
        create_proxy("http://localhost:8080".to_string());

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

    axum::serve(listener, proxy_app).await.unwrap();
}
