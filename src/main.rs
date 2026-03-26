use axum::Router;
use phantom_frame::{
    cache::CacheHandle,
    config::{Config, ProxyModeConfig},
    control, CreateProxyConfig, ProxyMode,
};
use std::{env, path::PathBuf};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <config-file.toml>", args[0]);
        eprintln!("Example: {} ./config.toml", args[0]);
        std::process::exit(1);
    }

    let config = Config::from_file(&args[1])?;

    tracing::info!("Loaded configuration from: {}", args[1]);
    tracing::info!("HTTP port: {}", config.http_port);
    if let Some(p) = config.https_port {
        tracing::info!("HTTPS port: {}", p);
    }
    tracing::info!("Control port: {}", config.control_port);
    tracing::info!("Server entries: {}", config.server.len());

    // ── Build per-server routers ────────────────────────────────────────────
    // Collect (name, bind_to, router, handle) tuples.
    let mut entries: Vec<(String, String, Router, CacheHandle)> = Vec::new();

    for (name, server_cfg) in &config.server {
        let mut proxy_config = CreateProxyConfig::new(server_cfg.proxy_url.clone())
            .with_include_paths(server_cfg.include_paths.clone())
            .with_exclude_paths(server_cfg.exclude_paths.clone())
            .with_websocket_enabled(server_cfg.enable_websocket)
            .with_forward_get_only(server_cfg.forward_get_only)
            .with_cache_404_capacity(server_cfg.cache_404_capacity)
            .with_use_404_meta(server_cfg.use_404_meta)
            .with_cache_strategy(server_cfg.cache_strategy.clone())
            .with_compress_strategy(server_cfg.compress_strategy.clone())
            .with_cache_storage_mode(server_cfg.cache_storage_mode.clone());

        if let Some(ref dir) = server_cfg.cache_directory {
            proxy_config = proxy_config.with_cache_directory(dir.clone());
        }

        let proxy_mode = match server_cfg.proxy_mode {
            ProxyModeConfig::Dynamic => ProxyMode::Dynamic,
            ProxyModeConfig::PreGenerate => ProxyMode::PreGenerate {
                paths: server_cfg.pre_generate_paths.clone(),
                fallthrough: server_cfg.pre_generate_fallthrough,
            },
        };
        proxy_config = proxy_config.with_proxy_mode(proxy_mode);

        let (router, handle) = phantom_frame::create_proxy(proxy_config);

        tracing::info!(
            "  server '{}': bind_to='{}', proxy_url='{}', mode={:?}",
            name,
            server_cfg.bind_to,
            server_cfg.proxy_url,
            server_cfg.proxy_mode,
        );

        entries.push((name.clone(), server_cfg.bind_to.clone(), router, handle));
    }

    // ── Sort routes ─────────────────────────────────────────────────────────
    // Axum nested routers are matched in registration order (first match wins).
    // Register longest/most-specific paths first so they shadow shorter ones.
    // bind_to = "*" is always last (becomes the fallback).
    entries.sort_by(|a, b| match (a.1.as_str(), b.1.as_str()) {
        ("*", "*") => std::cmp::Ordering::Equal,
        ("*", _) => std::cmp::Ordering::Greater,
        (_, "*") => std::cmp::Ordering::Less,
        _ => b.1.len().cmp(&a.1.len()),
    });

    // ── Compose top-level router ─────────────────────────────────────────────
    let mut app = Router::new();
    let mut star_router: Option<Router> = None;
    let mut handles: Vec<CacheHandle> = Vec::new();

    for (_, bind_to, server_router, handle) in entries {
        handles.push(handle);
        if bind_to == "*" {
            star_router = Some(server_router);
        } else {
            app = app.nest(&bind_to, server_router);
        }
    }

    // Catch-all fallback (bind_to = "*") goes on last.
    if let Some(star) = star_router {
        app = app.fallback_service(star);
    }

    // ── Control server ───────────────────────────────────────────────────────
    let control_app =
        control::create_control_router(handles, config.control_auth.clone());

    // ── HTTP listener ────────────────────────────────────────────────────────
    let http_addr = format!("0.0.0.0:{}", config.http_port);
    let http_listener = tokio::net::TcpListener::bind(&http_addr).await?;
    tracing::info!("HTTP proxy listening on {}", http_addr);

    let http_app = app.clone();
    let http_server = tokio::spawn(async move {
        axum::serve(http_listener, http_app)
            .await
            .expect("HTTP proxy server failed");
    });

    // ── Optional HTTPS listener ──────────────────────────────────────────────
    let https_port = config.https_port;
    let cert_path = config.cert_path.clone();
    let key_path = config.key_path.clone();
    let https_app = app.clone();

    let https_task = tokio::spawn(async move {
        if let Some(port) = https_port {
            let cert = cert_path.unwrap();
            let key = key_path.unwrap();
            if let Err(e) = run_https_server(port, cert, key, https_app).await {
                tracing::error!("HTTPS server error: {}", e);
            }
        } else {
            // No HTTPS configured — park this task indefinitely so the
            // select! below never fires on it spuriously.
            std::future::pending::<()>().await;
        }
    });

    // ── Control listener ─────────────────────────────────────────────────────
    let control_addr = format!("0.0.0.0:{}", config.control_port);
    let control_listener = tokio::net::TcpListener::bind(&control_addr).await?;
    tracing::info!("Control server listening on {}", control_addr);

    let control_server = tokio::spawn(async move {
        axum::serve(control_listener, control_app)
            .await
            .expect("Control server failed");
    });

    tokio::select! {
        _ = http_server => {
            tracing::error!("HTTP proxy server stopped unexpectedly");
        }
        _ = https_task => {
            tracing::error!("HTTPS proxy server stopped unexpectedly");
        }
        _ = control_server => {
            tracing::error!("Control server stopped unexpectedly");
        }
    }

    Ok(())
}

// ── TLS helpers ──────────────────────────────────────────────────────────────

async fn run_https_server(
    port: u16,
    cert_path: PathBuf,
    key_path: PathBuf,
    app: Router,
) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse()?;
    start_tls(addr, cert_path, key_path, app).await
}

#[cfg(feature = "rustls")]
async fn start_tls(
    addr: std::net::SocketAddr,
    cert_path: PathBuf,
    key_path: PathBuf,
    app: Router,
) -> anyhow::Result<()> {
    let tls_config =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path).await?;
    axum_server::bind_rustls(addr, tls_config)
        .serve(app.into_make_service())
        .await
        .map_err(Into::into)
}

#[cfg(feature = "native-tls")]
async fn start_tls(
    addr: std::net::SocketAddr,
    cert_path: PathBuf,
    key_path: PathBuf,
    app: Router,
) -> anyhow::Result<()> {
    let tls_config =
        axum_server::tls_openssl::OpenSSLConfig::from_pem_file(cert_path, key_path)?;
    axum_server::bind_openssl(addr, tls_config)
        .serve(app.into_make_service())
        .await
        .map_err(Into::into)
}
