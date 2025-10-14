use phantom_frame::{config::Config, control, CreateProxyConfig};
use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Get config file path from command line arguments
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <config-file.toml>", args[0]);
        eprintln!("Example: {} ./config.toml", args[0]);
        std::process::exit(1);
    }

    let config_path = &args[1];

    // Load configuration
    let config = Config::from_file(config_path)?;

    tracing::info!("Loaded configuration from: {}", config_path);
    tracing::info!("Control port: {}", config.server.control_port);
    tracing::info!("Proxy port: {}", config.server.proxy_port);
    tracing::info!("Proxy URL: {}", config.server.proxy_url);
    tracing::info!("Include paths: {:?}", config.server.include_paths);
    tracing::info!("Exclude paths: {:?}", config.server.exclude_paths);
    tracing::info!("WebSocket support: {}", if config.server.enable_websocket { "enabled" } else { "disabled" });

    // Create proxy configuration
    let proxy_config = CreateProxyConfig::new(config.server.proxy_url.clone())
        .with_include_paths(config.server.include_paths.clone())
        .with_exclude_paths(config.server.exclude_paths.clone())
        .with_websocket_enabled(config.server.enable_websocket);

    // Create proxy server with the config
    let (proxy_app, refresh_trigger) = phantom_frame::create_proxy(proxy_config);

    // Create control server
    let control_app =
        control::create_control_router(refresh_trigger.clone(), config.server.control_auth.clone());

    // Spawn proxy server
    let proxy_addr = format!("0.0.0.0:{}", config.server.proxy_port);
    let proxy_listener = tokio::net::TcpListener::bind(&proxy_addr).await?;
    tracing::info!("Proxy server listening on {}", proxy_addr);

    let proxy_server = tokio::spawn(async move {
        axum::serve(proxy_listener, proxy_app)
            .await
            .expect("Proxy server failed");
    });

    // Spawn control server
    let control_addr = format!("0.0.0.0:{}", config.server.control_port);
    let control_listener = tokio::net::TcpListener::bind(&control_addr).await?;
    tracing::info!("Control server listening on {}", control_addr);

    let control_server = tokio::spawn(async move {
        axum::serve(control_listener, control_app)
            .await
            .expect("Control server failed");
    });

    // Wait for both servers
    tokio::select! {
        _ = proxy_server => {
            tracing::error!("Proxy server stopped unexpectedly");
        }
        _ = control_server => {
            tracing::error!("Control server stopped unexpectedly");
        }
    }

    Ok(())
}
