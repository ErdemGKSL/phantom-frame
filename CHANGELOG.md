# Changelog

## v0.2.7

Release date: 2026-03-26

### Added

- **`cache_key` webhook type**. A new `type = "cache_key"` webhook makes phantom-frame POST the request metadata to a URL and use the plain-text response body as the cache key for that request (both for the cache read and the subsequent cache write). On failure, a non-`2xx` response, a timeout, or an empty body, the default cache key (`method:path?query`) is used instead — the request is never denied by a `cache_key` webhook. Works in both Dynamic and PreGenerate modes.
- **Redirect passthrough for blocking webhooks**. When a blocking webhook returns a `3xx` status, phantom-frame now forwards the redirect to the client with the `Location` header intact (e.g. `302` + `Location: /login`). Redirects are not followed internally. Previously, `3xx` responses from the webhook server were silently followed by `reqwest`, masking the redirect entirely.

### Changed

- `call_webhook` (internal) now disables `reqwest`'s automatic redirect following (`redirect::Policy::none()`) and returns a richer result that includes the HTTP status, the `Location` header, and the response body.

## v0.2.6

Release date: 2026-03-26

### Added

- **Webhook support** (`webhooks` per `[server.NAME]`). Each server can now declare one or more webhooks that are called on every request **before** cache reads, ensuring access control is enforced even for cached responses.
  - **`type = "blocking"`** — phantom-frame POSTs the request metadata to the webhook URL and awaits the response. A `2xx` reply allows the request to proceed; any non-`2xx` reply causes the same status code to be returned to the client immediately (the request is never forwarded to the backend or served from cache). A timeout or network error returns `503 Service Unavailable`.
  - **`type = "notify"`** — the POST is dispatched as a fire-and-forget background task; the request always proceeds immediately regardless of the webhook outcome.
  - `url` — the endpoint to POST to.
  - `timeout_ms` — optional per-webhook timeout in milliseconds (default: `5000`). Only meaningful for `blocking` webhooks.
  - Multiple webhooks per server are supported via the `[[server.NAME.webhooks]]` TOML array syntax. Blocking webhooks run sequentially; the first denial short-circuits the chain.
  - Webhook POST body: `{ "method", "path", "query", "headers" }`. The request body is never consumed so latency overhead is minimal.
- `serde_json` added as a dependency (used for webhook payload serialisation).

## v0.2.5

Release date: 2026-03-26

### Added

- **`.env` file loading via `dotenv` config key**. A new top-level `dotenv` field controls whether a `.env` file is loaded before environment variable resolution:
  - Absent or `false` — disabled (default).
  - `true` — load `.env` from the current working directory (silently ignored if absent).
  - `"./path/to/.env"` — load from the given path (error if the file does not exist).
- **`$env:VAR` interpolation in config values**. Any string value in the TOML config that matches `"$env:VAR_NAME"` is replaced at startup with the value of the corresponding environment variable. If the variable is not set, the key is silently dropped (optional fields become `None`, fields with defaults fall back to their defaults). Works for all string fields — `control_auth`, `proxy_url`, `cert_path`, `key_path`, etc. Pairs naturally with `dotenv` to keep secrets out of the config file.
- **`&&` / `||` command chaining in `execute`**. Intermediate segments are run to completion in order; the final segment becomes the long-running server process. `&&` runs the next segment only on success (exit code 0); `||` runs it only on failure. Example: `execute = "pnpm install && pnpm run build && pnpm run start"`.
- **`cd` support in `execute` chains**. A segment that is a `cd <path>` command (including `cd /d` on Windows) changes the virtual working directory for all subsequent segments in the chain without spawning a subprocess.
- **Inline `KEY=VALUE` env-prefix support in `execute`**. Linux-style `KEY=VALUE cmd` prefixes are parsed and injected as environment variables for that command segment on all platforms. Example: `execute = "PORT=5173 NODE_ENV=production pnpm run start"`.
- `dotenvy` added as a dependency for `.env` file loading.

## v0.2.4

Release date: 2026-03-26

### Added

- **`execute` and `execute_dir` fields on `[server.NAME]`**. When set, phantom-frame spawns the specified command before the proxy begins serving traffic and polls `proxy_url`'s TCP port every 500 ms until it accepts connections (360 s hard timeout). All processes are spawned concurrently so multiple servers start booting in parallel.
  - `execute = "pnpm run dev"` — shell command to run.
  - `execute_dir = "./apps/client"` — optional working directory (relative to where phantom-frame is invoked).
  - Cross-platform: on Windows commands are dispatched via `cmd /C`, which resolves `.cmd` shims (`pnpm.cmd`, `npm.cmd`, `yarn.cmd`, etc.) transparently. On Unix, `sh -c` is used.

### Changed

- **Control server endpoints renamed to match the `CacheHandle` API**. The previous `/refresh-cache` endpoint has been replaced. All new routes use underscore-separated names that mirror the corresponding `CacheHandle` methods:
  - `POST /invalidate_all` — invalidate every cached entry across all servers (replaces `/refresh-cache`).
  - `POST /invalidate` — invalidate entries matching a wildcard pattern. Body: `{ "pattern": "..." }`.
  - `POST /add_snapshot` — fetch a path from upstream, cache it, and add it to the snapshot list (PreGenerate mode only). Body: `{ "path": "..." }`.
  - `POST /refresh_snapshot` — re-fetch a single cached snapshot from upstream (PreGenerate mode only). Body: `{ "path": "..." }`.
  - `POST /remove_snapshot` — remove a path from the cache and snapshot list (PreGenerate mode only). Body: `{ "path": "..." }`.
  - `POST /refresh_all_snapshots` — re-fetch every tracked snapshot from upstream (PreGenerate mode only).
- Snapshot endpoints called against a `Dynamic`-mode proxy return `400 Bad Request` with a descriptive error message.

### Breaking Changes

- `POST /refresh-cache` has been removed. Replace with `POST /invalidate_all`.

## v0.2.3

Release date: 2026-03-26

### Changed

- **Default feature changed from `native-tls` to `rustls`**. The default build now uses `axum-server/tls-rustls` + `reqwest/rustls-tls` — pure Rust, no system dependencies.
- `native-tls` feature now uses `axum-server/tls-openssl` for server-side TLS. Requires OpenSSL as a system library (`libssl-dev` on Ubuntu, `openssl-devel` on Fedora, vcpkg/`OPENSSL_DIR` on Windows).

## v0.2.2

Release date: 2026-03-26

### Breaking Changes

- **Multi-server TOML config**: the single `[server]` block is replaced by named `[server.NAME]` blocks. At least one named block is required. Old configs must be migrated.
- `proxy_port` removed. Replaced by top-level `http_port` (default: `3000`).
- `control_port` and `control_auth` moved from `[server]` to the TOML root (no section header).

### Added

- **Multi-server support**: multiple `[server.NAME]` blocks can be declared in a single config file. Each block is mounted as an independent Axum router entry.
- `bind_to` field on each server block:
  - `"*"` (default) — catch-all fallback, registered last.
  - Any path prefix (e.g. `"/api"`) — nested via `Router::nest`, registered longest-first so more-specific paths shadow shorter ones.
- `http_port` (top-level, default `3000`) — HTTP listen port.
- `https_port` (top-level, optional) — HTTPS listen port. When set, `cert_path` and `key_path` are required.
- `cert_path` / `key_path` — PEM certificate and private key paths for HTTPS.
  - `rustls` feature (default): TLS via `axum-server/tls-rustls` — pure Rust, no system dependencies.
  - `native-tls` feature: TLS via `axum-server/tls-openssl` — requires OpenSSL installed as a system library.
- **Default feature changed from `native-tls` to `rustls`**. Users who relied on the previous default must now explicitly opt in with `--features native-tls --no-default-features`.
- Startup validation: missing cert/key when `https_port` is set, or an empty `server` map, produce a clear error before the server starts.
- `control::create_control_router` now accepts `Vec<CacheHandle>`. A single `/refresh-cache` call invalidates all registered server caches.

### Changed

- **WebSocket / upgrade gating**: `enable_websocket = true` is now ignored in pure SSG mode (`proxy_mode = "pre_generate"` with `pre_generate_fallthrough = false`). Upgrade requests on such servers always return `501 Not Implemented` because there is no live backend to tunnel to. Upgrade support remains fully functional in Dynamic mode and PreGenerate mode with `fallthrough = true`.

## v0.2.1

Release date: 2026-03-26

### Breaking Changes

- `RefreshTrigger` renamed to `CacheHandle`. Update all usages of `phantom_frame::cache::RefreshTrigger` to `phantom_frame::cache::CacheHandle`.
- Methods on `CacheHandle` renamed:
  - `trigger()` → `invalidate_all()`
  - `trigger_by_key_match(pattern)` → `invalidate(pattern)`
- `create_proxy_with_trigger()` renamed to `create_proxy_with_handle()`.

### Added

- **PreGenerate (SSG) mode** via `ProxyMode::PreGenerate { paths, fallthrough }`:
  - Specified paths are pre-fetched from the upstream server at startup and served exclusively from the cache.
  - `fallthrough: false` (default) — cache misses return 404 immediately without contacting the backend.
  - `fallthrough: true` — cache misses fall through to the upstream backend.
- `CacheHandle` gains four async snapshot-management methods (only available in PreGenerate mode):
  - `add_snapshot(path)` — fetch and cache a new path, append it to the snapshot list.
  - `refresh_snapshot(path)` — re-fetch a single path from the backend and overwrite its cache entry.
  - `remove_snapshot(path)` — evict a path from the cache and remove it from the snapshot list.
  - `refresh_all_snapshots()` — re-fetch every tracked snapshot path.
- `ProxyMode` enum exported from the crate root.
- TOML config fields for PreGenerate mode:
  - `proxy_mode = "pre_generate"` (or `"dynamic"`, the default)
  - `pre_generate_paths = ["/book/1", "/about"]`
  - `pre_generate_fallthrough = false`
- `CreateProxyConfig::with_proxy_mode(mode)` builder method.

## v0.1.19

Release date: 2026-03-23

### Fixed

- Fixed 502 Bad Gateway errors when phantom-frame is served behind HTTPS / HTTP/2. HTTP/2 requests carry an absolute-form URI (e.g. `https://example.com/path`) rather than origin-form (`/path`). The upstream `target_url` was being constructed by appending the full absolute URI to `proxy_url`, producing a malformed URL like `http://localhost:5173https://example.com/path`. The proxy now correctly extracts only the path and query from the incoming request URI before forming the upstream URL. This fix applies to both regular proxied requests and WebSocket upgrade requests.

## v0.1.18

Release date: 2026-03-23

### Added

- `native-tls` and `rustls` Cargo features for selecting the TLS backend used by the upstream HTTP client.
- `native-tls` is the default, using the platform's native TLS stack (SChannel on Windows, Secure Transport on macOS, OpenSSL on Linux).
- `rustls` feature (`--no-default-features --features rustls`) compiles in rustls with bundled webpki root certificates instead.
- Compile-time mutual-exclusivity guard: enabling both features simultaneously produces a clear `compile_error!`.

## v0.1.17

Release date: 2026-03-10

### Fixed

- Re-exported `CacheStorageMode` from `phantom_frame::cache`, so code importing `phantom_frame::cache::CacheStorageMode` now compiles.
- Updated the library usage example to show filesystem cache storage configuration directly.

## v0.1.16

Release date: 2026-03-10

This release includes the new disk-backed cache body implementation and supersedes `v0.1.15`, which missed `src/compression.rs` in the tagged commit.

### Added

- Filesystem-backed cache body storage through `CacheStorageMode::Filesystem`.
- New `src/compression.rs` module for cache compression, decompression, upstream body normalization, and Accept-Encoding negotiation.
- Startup cleanup for orphaned cache files left in phantom-frame managed cache directories.
- Tests covering compression behavior, disk-backed cache round-trips, wildcard cleanup, 404 eviction cleanup, and startup cleanup.

### Changed

- Cache metadata remains in memory while cached body bytes can now be written to the filesystem.
- Cache invalidation now removes backing files during full clear, wildcard clear, and 404 FIFO eviction.
- Proxy cache hits now serve compressed or identity-decoded bodies based on client `Accept-Encoding` support.
- Upstream response handling now disables automatic reqwest decompression so cache normalization is deterministic.
- Configuration now supports:
  - `compress_strategy`
  - `cache_storage_mode`
  - `cache_directory`
- The example template server dependency was bumped to `phantom-frame = "0.1.16"`.

### Documentation

- Updated `README.md` with cache compression and cache body storage documentation.
- Updated `examples/configs/basic.toml` with the new cache storage options.
- Updated `examples/library_usage.rs` to show compression strategy usage.

### Release Notes

- `v0.1.16` is the valid corrective release tag for this work.
- `v0.1.15` was created before `src/compression.rs` was tracked and should not be treated as the correct release for these changes.