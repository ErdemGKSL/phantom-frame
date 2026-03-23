# Changelog

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