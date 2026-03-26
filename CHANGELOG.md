# Changelog

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