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

    // ── Spawn execute commands and wait for their ports ──────────────────────
    // Collect servers that have an `execute` command.
    // Spawn all processes first so they boot concurrently, then wait for each.
    let mut _child_processes: Vec<tokio::process::Child> = Vec::new();
    let mut port_waits: Vec<(String, String, u16)> = Vec::new(); // (name, host, port)

    for (name, server_cfg) in &config.server {
        if let Some(ref cmd) = server_cfg.execute {
            let (host, port) = extract_host_port(&server_cfg.proxy_url)?;

            tracing::info!(
                "server '{}': spawning command: {}",
                name, cmd
            );

            let child = spawn_command_chain(cmd, server_cfg.execute_dir.as_deref()).await?;
            _child_processes.push(child);
            port_waits.push((name.clone(), host, port));
        }
    }

    for (name, host, port) in port_waits {
        wait_for_port(&name, &host, port).await?;
    }

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

// ── Execute helpers ───────────────────────────────────────────────────────────

/// Whether the next segment in a chain should run after the previous one
/// succeeded or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainOp {
    /// `&&` — next segment runs only when the previous exited with code 0.
    And,
    /// `||` — next segment runs only when the previous exited non-zero.
    Or,
}

/// Split a command string on `&&` and `||` operators while respecting single-
/// and double-quoted substrings.  Returns `(segment_text, governing_op)`
/// pairs where `governing_op` is the operator *preceding* the segment (the
/// first segment always uses `ChainOp::And` as a sentinel — it is always run).
fn split_command_chain(cmd: &str) -> Vec<(String, ChainOp)> {
    let mut results: Vec<(String, ChainOp)> = Vec::new();
    let mut current = String::new();
    let mut pending_op = ChainOp::And; // sentinel for the first segment
    let chars: Vec<char> = cmd.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Handle quoted sections — consume until the matching closing quote.
        if ch == '\'' || ch == '"' {
            let quote = ch;
            current.push(ch);
            i += 1;
            while i < len && chars[i] != quote {
                current.push(chars[i]);
                i += 1;
            }
            if i < len {
                current.push(chars[i]); // closing quote
                i += 1;
            }
            continue;
        }

        // Check for `&&` or `||`
        if i + 1 < len {
            if ch == '&' && chars[i + 1] == '&' {
                let seg = current.trim().to_string();
                if !seg.is_empty() {
                    results.push((seg, pending_op));
                }
                current.clear();
                pending_op = ChainOp::And;
                i += 2;
                continue;
            }
            if ch == '|' && chars[i + 1] == '|' {
                let seg = current.trim().to_string();
                if !seg.is_empty() {
                    results.push((seg, pending_op));
                }
                current.clear();
                pending_op = ChainOp::Or;
                i += 2;
                continue;
            }
        }

        current.push(ch);
        i += 1;
    }

    let seg = current.trim().to_string();
    if !seg.is_empty() {
        results.push((seg, pending_op));
    }

    results
}

/// Strip leading Linux-style `KEY=VALUE` inline environment variable
/// assignments from the front of a command segment.
///
/// Handles:
/// - Unquoted values:  `PORT=3000 pnpm start`
/// - Single-quoted:    `MSG='hello world' pnpm start`
/// - Double-quoted:    `MSG="hello world" pnpm start`
///
/// Returns the collected `(key, value)` pairs and the remaining command text.
fn parse_env_prefix(segment: &str) -> (Vec<(String, String)>, &str) {
    let bytes = segment.as_bytes();
    let len = bytes.len();
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut pos = 0;

    loop {
        // Skip leading whitespace
        while pos < len && bytes[pos] == b' ' {
            pos += 1;
        }

        // Try to match `IDENTIFIER=`
        let start = pos;
        while pos < len
            && (bytes[pos].is_ascii_alphabetic()
                || bytes[pos] == b'_'
                || (pos > start && bytes[pos].is_ascii_digit()))
        {
            pos += 1;
        }

        // Must have advanced and must be followed by '='
        if pos == start || pos >= len || bytes[pos] != b'=' {
            // Not an env assignment — rewind to start of this token
            pos = start;
            break;
        }

        let key = segment[start..pos].to_string();
        pos += 1; // skip '='

        // Parse value (may be quoted)
        let value = if pos < len && (bytes[pos] == b'\'' || bytes[pos] == b'"') {
            let quote = bytes[pos];
            pos += 1;
            let val_start = pos;
            while pos < len && bytes[pos] != quote {
                pos += 1;
            }
            let val = segment[val_start..pos].to_string();
            if pos < len {
                pos += 1; // skip closing quote
            }
            val
        } else {
            // Unquoted value — read until next space
            let val_start = pos;
            while pos < len && bytes[pos] != b' ' {
                pos += 1;
            }
            segment[val_start..pos].to_string()
        };

        // Make sure something follows (otherwise this is not an env prefix,
        // it's the entire command, e.g., exporting a variable on its own).
        // Peek past whitespace.
        let mut peek = pos;
        while peek < len && bytes[peek] == b' ' {
            peek += 1;
        }
        if peek >= len {
            // Nothing after the assignment — not an inline env prefix
            pos = start;
            break;
        }

        pairs.push((key, value));
    }

    (pairs, &segment[pos..])
}

/// Return the path argument if `segment` is a `cd` command, otherwise `None`.
///
/// Handles:
/// - `cd ./path`
/// - `cd /d C:\path`  (Windows flag, stripped)
/// - `cd 'some path'` / `cd "some path"`
fn parse_cd_path(segment: &str) -> Option<String> {
    let s = segment.trim();
    // Case-insensitive `cd`
    let rest = if s.to_ascii_lowercase().starts_with("cd") {
        &s[2..]
    } else {
        return None;
    };

    // Must be end-of-string or whitespace after "cd"
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }

    let rest = rest.trim();

    // Strip Windows `/d` flag
    let rest = if rest.to_ascii_lowercase().starts_with("/d") {
        rest[2..].trim()
    } else {
        rest
    };

    if rest.is_empty() {
        return None; // bare `cd` — no-op
    }

    // Strip surrounding quotes
    let path = if (rest.starts_with('\'') && rest.ends_with('\''))
        || (rest.starts_with('"') && rest.ends_with('"'))
    {
        rest[1..rest.len() - 1].to_string()
    } else {
        rest.to_string()
    };

    Some(path)
}

/// Resolve `..` and `.` components in `path` without touching the filesystem.
/// `std::fs::canonicalize` requires the path to already exist, but intermediate
/// build steps may create new directories at runtime.
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Spawn a single shell command inside `dir`, injecting `extra_env` pairs.
/// On Windows uses `cmd /C`; on Unix uses `sh -c`.
fn spawn_single_command(
    cmd: &str,
    dir: &std::path::Path,
    extra_env: &[(String, String)],
) -> anyhow::Result<tokio::process::Child> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    };

    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };

    command.current_dir(dir);
    for (k, v) in extra_env {
        command.env(k, v);
    }

    command
        .spawn()
        .map_err(|e| anyhow::anyhow!("execute: failed to spawn '{}': {}", cmd, e))
}

/// Parse and execute a command that may contain `&&` / `||` chains, `cd`
/// virtual directory changes, and Linux-style `KEY=VAL` inline env prefixes.
///
/// All intermediate segments are run to completion before returning; the last
/// non-`cd` segment is returned as a live `Child` (the long-running server).
///
/// # Operator semantics
/// - `&&` — next segment runs only if the previous exited with code 0.
/// - `||` — next segment runs only if the previous exited non-zero.
async fn spawn_command_chain(
    cmd: &str,
    base_dir: Option<&str>,
) -> anyhow::Result<tokio::process::Child> {
    let virtual_dir = match base_dir {
        Some(d) => normalize_path(&std::path::PathBuf::from(d)),
        None => std::env::current_dir()?,
    };
    let mut virtual_dir = if virtual_dir.is_absolute() {
        virtual_dir
    } else {
        normalize_path(&std::env::current_dir()?.join(virtual_dir))
    };

    let segments = split_command_chain(cmd);

    if segments.is_empty() {
        anyhow::bail!("execute: command is empty");
    }

    // We need to track whether each segment should run based on the previous
    // segment's exit status and the governing operator.
    // `last_success` starts as `true` so the first segment always executes.
    let mut last_success = true;

    let total = segments.len();
    for (idx, (raw_seg, op)) in segments.into_iter().enumerate() {
        let is_last = idx == total - 1;

        // Decide whether this segment runs.
        let should_run = match op {
            ChainOp::And => last_success,
            ChainOp::Or => !last_success,
        };

        if !should_run {
            if is_last {
                anyhow::bail!(
                    "execute: last command '{}' was skipped by '{}' condition \
                     (no server process was started)",
                    raw_seg,
                    if op == ChainOp::And { "&&" } else { "||" }
                );
            }
            // Keep last_success unchanged — a skipped segment doesn't flip it.
            continue;
        }

        // Handle `cd` — virtual directory change.
        if let Some(cd_path) = parse_cd_path(&raw_seg) {
            let new_dir = normalize_path(&virtual_dir.join(&cd_path));
            tracing::info!(
                "execute: cd '{}' → virtual dir is now '{}'",
                cd_path,
                new_dir.display()
            );
            virtual_dir = new_dir;
            // `cd` always counts as success for operator evaluation.
            last_success = true;

            if is_last {
                anyhow::bail!(
                    "execute: command chain ends with 'cd' — no server \
                     process to start. Add a command after the cd."
                );
            }
            continue;
        }

        // Strip Linux-style inline env var prefix.
        let (env_pairs, bare_cmd) = parse_env_prefix(&raw_seg);
        let bare_cmd = bare_cmd.trim();

        if bare_cmd.is_empty() {
            anyhow::bail!("execute: empty command segment after env prefix stripping");
        }

        if !is_last {
            // Intermediate segment — spawn, wait, record exit status.
            tracing::info!(
                "execute: running '{}' in '{}'{}",
                bare_cmd,
                virtual_dir.display(),
                if env_pairs.is_empty() {
                    String::new()
                } else {
                    format!(
                        " with env [{}]",
                        env_pairs
                            .iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );

            let mut child =
                spawn_single_command(bare_cmd, &virtual_dir, &env_pairs)?;
            let status = child.wait().await?;
            last_success = status.success();

            if !last_success {
                tracing::warn!(
                    "execute: '{}' exited with {}",
                    bare_cmd,
                    status
                );
            }
        } else {
            // Last segment — this is the long-running server process.
            tracing::info!(
                "execute: starting server '{}' in '{}'{}",
                bare_cmd,
                virtual_dir.display(),
                if env_pairs.is_empty() {
                    String::new()
                } else {
                    format!(
                        " with env [{}]",
                        env_pairs
                            .iter()
                            .map(|(k, v)| format!("{}={}", k, v))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );

            return spawn_single_command(bare_cmd, &virtual_dir, &env_pairs);
        }
    }

    // Unreachable in practice — the loop always returns or bails on the last segment.
    anyhow::bail!("execute: no runnable final command found in chain");
}

/// Parse host and port out of a URL like `http://localhost:5173/path`.
/// Falls back to port 80 for http and 443 for https when no explicit port.
fn extract_host_port(url: &str) -> anyhow::Result<(String, u16)> {
    // Strip scheme
    let rest = if let Some(s) = url.strip_prefix("https://") {
        (s, 443u16)
    } else if let Some(s) = url.strip_prefix("http://") {
        (s, 80u16)
    } else {
        anyhow::bail!("execute: unsupported scheme in proxy_url '{}'", url);
    };

    let (authority, default_port) = rest;
    // Drop any path component
    let authority = authority.split('/').next().unwrap_or(authority);

    if let Some(colon) = authority.rfind(':') {
        let host = authority[..colon].to_string();
        let port: u16 = authority[colon + 1..]
            .parse()
            .map_err(|_| anyhow::anyhow!("execute: invalid port in proxy_url '{}'", url))?;
        Ok((host, port))
    } else {
        Ok((authority.to_string(), default_port))
    }
}

/// Poll `host:port` via TCP every 500 ms until a connection succeeds, with a
/// hard 360-second timeout. Logs progress so the user can see what is pending.
async fn wait_for_port(name: &str, host: &str, port: u16) -> anyhow::Result<()> {
    use tokio::time::{sleep, timeout, Duration};

    tracing::info!(
        "server '{}': waiting for port {} on {} to accept connections …",
        name, port, host
    );

    let addr = format!("{}:{}", host, port);
    let result = timeout(Duration::from_secs(360), async {
        loop {
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(_) => return,
                Err(_) => sleep(Duration::from_millis(500)).await,
            }
        }
    })
    .await;

    match result {
        Ok(()) => {
            tracing::info!("server '{}': port {} is ready", name, port);
            Ok(())
        }
        Err(_) => anyhow::bail!(
            "server '{}': timed out waiting for port {} on {} after 360 s",
            name, port, host
        ),
    }
}
