use std::collections::{hash_map::DefaultHasher, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot, RwLock};

use crate::compression::ContentEncoding;
pub use crate::CacheStorageMode;

static BODY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Messages sent via the broadcast channel to invalidate cache entries.
#[derive(Clone, Debug)]
pub enum InvalidationMessage {
    /// Invalidate all cache entries.
    All,
    /// Invalidate cache entries whose key matches a pattern (supports wildcards).
    Pattern(String),
}

/// An operation sent to the snapshot worker for runtime SSG management.
pub(crate) struct SnapshotRequest {
    pub(crate) op: SnapshotOp,
    pub(crate) done: oneshot::Sender<()>,
}

/// The kind of snapshot operation to perform.
pub(crate) enum SnapshotOp {
    /// Fetch `path` from upstream, store in the cache, and track it as a snapshot.
    Add(String),
    /// Re-fetch `path` from upstream and overwrite its cache entry.
    Refresh(String),
    /// Remove `path` from the cache and from the tracked snapshot list.
    Remove(String),
    /// Re-fetch every currently tracked snapshot path.
    RefreshAll,
}

/// A cloneable handle for cache management — invalidating entries and (in
/// PreGenerate mode) managing the list of pre-generated SSG snapshots at runtime.
#[derive(Clone)]
pub struct CacheHandle {
    sender: broadcast::Sender<InvalidationMessage>,
    /// Present only when the proxy is in `ProxyMode::PreGenerate`.
    snapshot_tx: Option<mpsc::Sender<SnapshotRequest>>,
}

impl CacheHandle {
    /// Create a new handle without snapshot support (Dynamic mode or tests).
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self {
            sender,
            snapshot_tx: None,
        }
    }

    /// Create a new handle wired to a snapshot worker (PreGenerate mode).
    pub(crate) fn new_with_snapshots(snapshot_tx: mpsc::Sender<SnapshotRequest>) -> Self {
        let (sender, _) = broadcast::channel(16);
        Self {
            sender,
            snapshot_tx: Some(snapshot_tx),
        }
    }

    /// Invalidate all cache entries.
    pub fn invalidate_all(&self) {
        let _ = self.sender.send(InvalidationMessage::All);
    }

    /// Invalidate cache entries whose key matches `pattern`.
    /// Supports wildcards: `"/api/*"`, `"GET:/api/*"`, etc.
    pub fn invalidate(&self, pattern: &str) {
        let _ = self
            .sender
            .send(InvalidationMessage::Pattern(pattern.to_string()));
    }

    /// Subscribe to invalidation events.
    pub fn subscribe(&self) -> broadcast::Receiver<InvalidationMessage> {
        self.sender.subscribe()
    }

    /// Send an operation to the snapshot worker and await completion.
    async fn send_snapshot_op(&self, op: SnapshotOp) -> anyhow::Result<()> {
        let tx = self.snapshot_tx.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Snapshot operations are only available in PreGenerate proxy mode")
        })?;
        let (done_tx, done_rx) = oneshot::channel();
        tx.send(SnapshotRequest { op, done: done_tx })
            .await
            .map_err(|_| anyhow::anyhow!("Snapshot worker is not running"))?;
        done_rx
            .await
            .map_err(|_| anyhow::anyhow!("Snapshot worker dropped the completion signal"))
    }

    /// Fetch `path` from the upstream server, store it in the cache, and add it
    /// to the tracked snapshot list. Only available in PreGenerate mode.
    pub async fn add_snapshot(&self, path: &str) -> anyhow::Result<()> {
        self.send_snapshot_op(SnapshotOp::Add(path.to_string()))
            .await
    }

    /// Re-fetch `path` from the upstream server and update its cached entry.
    /// Only available in PreGenerate mode.
    pub async fn refresh_snapshot(&self, path: &str) -> anyhow::Result<()> {
        self.send_snapshot_op(SnapshotOp::Refresh(path.to_string()))
            .await
    }

    /// Remove `path` from the cache and from the tracked snapshot list.
    /// Only available in PreGenerate mode.
    pub async fn remove_snapshot(&self, path: &str) -> anyhow::Result<()> {
        self.send_snapshot_op(SnapshotOp::Remove(path.to_string()))
            .await
    }

    /// Re-fetch every currently tracked snapshot path from the upstream server.
    /// Only available in PreGenerate mode.
    pub async fn refresh_all_snapshots(&self) -> anyhow::Result<()> {
        self.send_snapshot_op(SnapshotOp::RefreshAll).await
    }
}

/// Helper function to check if a key matches a pattern with wildcard support
fn matches_pattern(key: &str, pattern: &str) -> bool {
    // Handle exact match
    if key == pattern {
        return true;
    }

    // Split pattern by '*' and check if all parts exist in order
    let parts: Vec<&str> = pattern.split('*').collect();

    if parts.len() == 1 {
        // No wildcard, exact match already checked above
        return false;
    }

    let mut current_pos = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        // First part must match from the beginning
        if i == 0 {
            if !key.starts_with(part) {
                return false;
            }
            current_pos = part.len();
        }
        // Last part must match to the end
        else if i == parts.len() - 1 {
            if !key[current_pos..].ends_with(part) {
                return false;
            }
        }
        // Middle parts must exist in order
        else if let Some(pos) = key[current_pos..].find(part) {
            current_pos += pos + part.len();
        } else {
            return false;
        }
    }

    true
}

/// Cache storage for prerendered content
#[derive(Clone)]
pub struct CacheStore {
    store: Arc<RwLock<HashMap<String, StoredCachedResponse>>>,
    // 404-specific store with bounded capacity and FIFO eviction
    store_404: Arc<RwLock<HashMap<String, StoredCachedResponse>>>,
    keys_404: Arc<RwLock<VecDeque<String>>>,
    cache_404_capacity: usize,
    handle: CacheHandle,
    body_store: CacheBodyStore,
}

#[derive(Clone, Debug)]
pub struct CachedResponse {
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
    pub status: u16,
    pub content_encoding: Option<ContentEncoding>,
}

#[derive(Clone, Debug)]
struct StoredCachedResponse {
    body: StoredBody,
    headers: HashMap<String, String>,
    status: u16,
    content_encoding: Option<ContentEncoding>,
}

#[derive(Clone, Debug)]
enum StoredBody {
    Memory(Vec<u8>),
    File(PathBuf),
}

#[derive(Clone, Copy, Debug)]
enum CacheBucket {
    Standard,
    NotFound,
}

impl CacheBucket {
    fn directory_name(self) -> &'static str {
        match self {
            Self::Standard => "responses",
            Self::NotFound => "responses-404",
        }
    }
}

#[derive(Clone, Debug)]
struct CacheBodyStore {
    mode: CacheStorageMode,
    root_dir: Option<PathBuf>,
}

impl CacheBodyStore {
    fn new(mode: CacheStorageMode, root_dir: Option<PathBuf>) -> Self {
        let root_dir = match mode {
            CacheStorageMode::Memory => None,
            CacheStorageMode::Filesystem => {
                let root_dir = root_dir.unwrap_or_else(default_cache_directory);
                cleanup_orphaned_cache_files(&root_dir);
                Some(root_dir)
            }
        };

        Self { mode, root_dir }
    }

    async fn store(&self, key: &str, body: Vec<u8>, bucket: CacheBucket) -> StoredBody {
        match self.mode {
            CacheStorageMode::Memory => StoredBody::Memory(body),
            CacheStorageMode::Filesystem => match self.write_body(key, &body, bucket).await {
                Ok(path) => StoredBody::File(path),
                Err(error) => {
                    tracing::warn!(
                        "Failed to persist cache body for '{}' to filesystem storage: {}",
                        key,
                        error
                    );
                    StoredBody::Memory(body)
                }
            },
        }
    }

    async fn load(&self, body: &StoredBody) -> Option<Vec<u8>> {
        match body {
            StoredBody::Memory(bytes) => Some(bytes.clone()),
            StoredBody::File(path) => match tokio::fs::read(path).await {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    tracing::warn!(
                        "Failed to read cached response body from '{}': {}",
                        path.display(),
                        error
                    );
                    None
                }
            },
        }
    }

    async fn remove(&self, body: StoredBody) {
        if let StoredBody::File(path) = body {
            if let Err(error) = tokio::fs::remove_file(&path).await {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        "Failed to delete cached response body '{}': {}",
                        path.display(),
                        error
                    );
                }
            }
        }
    }

    async fn write_body(
        &self,
        key: &str,
        body: &[u8],
        bucket: CacheBucket,
    ) -> std::io::Result<PathBuf> {
        let root_dir = self
            .root_dir
            .as_ref()
            .expect("filesystem cache storage requires a root directory");
        let bucket_dir = root_dir.join(bucket.directory_name());
        tokio::fs::create_dir_all(&bucket_dir).await?;

        let stem = cache_file_stem(key);
        let tmp_path = bucket_dir.join(format!("{}.tmp", stem));
        let final_path = bucket_dir.join(format!("{}.bin", stem));

        tokio::fs::write(&tmp_path, body).await?;
        tokio::fs::rename(&tmp_path, &final_path).await?;

        Ok(final_path)
    }
}

impl StoredCachedResponse {
    async fn materialize(self, body_store: &CacheBodyStore) -> Option<CachedResponse> {
        let body = body_store.load(&self.body).await?;

        Some(CachedResponse {
            body,
            headers: self.headers,
            status: self.status,
            content_encoding: self.content_encoding,
        })
    }
}

fn default_cache_directory() -> PathBuf {
    std::env::temp_dir().join("phantom-frame-cache")
}

fn cleanup_orphaned_cache_files(root_dir: &std::path::Path) {
    for bucket in [CacheBucket::Standard, CacheBucket::NotFound] {
        let bucket_dir = root_dir.join(bucket.directory_name());
        cleanup_bucket_directory(&bucket_dir);
    }
}

fn cleanup_bucket_directory(bucket_dir: &std::path::Path) {
    let entries = match std::fs::read_dir(bucket_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(
                "Failed to inspect cache directory '{}' during startup cleanup: {}",
                bucket_dir.display(),
                error
            );
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(
                    "Failed to enumerate cache directory '{}' during startup cleanup: {}",
                    bucket_dir.display(),
                    error
                );
                continue;
            }
        };

        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                tracing::warn!(
                    "Failed to inspect cache entry '{}' during startup cleanup: {}",
                    path.display(),
                    error
                );
                continue;
            }
        };

        let cleanup_result = if file_type.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };

        if let Err(error) = cleanup_result {
            tracing::warn!(
                "Failed to remove orphaned cache entry '{}' during startup cleanup: {}",
                path.display(),
                error
            );
        }
    }
}

fn cache_file_stem(key: &str) -> String {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);

    let hash = hasher.finish();
    let counter = BODY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("{:016x}-{:x}-{:016x}", hash, process::id(), counter)
}

fn into_stored_response(body: StoredBody, response: CachedResponse) -> StoredCachedResponse {
    StoredCachedResponse {
        body,
        headers: response.headers,
        status: response.status,
        content_encoding: response.content_encoding,
    }
}

impl CacheStore {
    pub fn new(handle: CacheHandle, cache_404_capacity: usize) -> Self {
        Self::with_storage(handle, cache_404_capacity, CacheStorageMode::Memory, None)
    }

    pub fn with_storage(
        handle: CacheHandle,
        cache_404_capacity: usize,
        storage_mode: CacheStorageMode,
        cache_directory: Option<PathBuf>,
    ) -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            store_404: Arc::new(RwLock::new(HashMap::new())),
            keys_404: Arc::new(RwLock::new(VecDeque::new())),
            cache_404_capacity,
            handle,
            body_store: CacheBodyStore::new(storage_mode, cache_directory),
        }
    }

    pub async fn get(&self, key: &str) -> Option<CachedResponse> {
        let cached = {
            let store = self.store.read().await;
            store.get(key).cloned()
        }?;

        cached.materialize(&self.body_store).await
    }

    /// Get a 404 cached response (if present)
    pub async fn get_404(&self, key: &str) -> Option<CachedResponse> {
        let cached = {
            let store = self.store_404.read().await;
            store.get(key).cloned()
        }?;

        cached.materialize(&self.body_store).await
    }

    pub async fn set(&self, key: String, response: CachedResponse) {
        let body = self
            .body_store
            .store(&key, response.body.clone(), CacheBucket::Standard)
            .await;
        let stored = into_stored_response(body, response);

        let replaced = {
            let mut store = self.store.write().await;
            store.insert(key, stored)
        };

        if let Some(old) = replaced {
            self.body_store.remove(old.body).await;
        }
    }

    /// Set a 404 cached response. Bounded by `cache_404_capacity` and evict the oldest entries when limit reached.
    pub async fn set_404(&self, key: String, response: CachedResponse) {
        if self.cache_404_capacity == 0 {
            // 404 caching disabled
            return;
        }

        let body = self
            .body_store
            .store(&key, response.body.clone(), CacheBucket::NotFound)
            .await;
        let stored = into_stored_response(body, response);

        let removed_bodies = {
            let mut store = self.store_404.write().await;
            let mut keys = self.keys_404.write().await;
            let mut removed = Vec::new();

            if store.contains_key(&key) {
                if let Some(pos) = keys.iter().position(|existing_key| existing_key == &key) {
                    keys.remove(pos);
                }
            }

            if let Some(old) = store.insert(key.clone(), stored) {
                removed.push(old.body);
            }
            keys.push_back(key);

            while keys.len() > self.cache_404_capacity {
                if let Some(old_key) = keys.pop_front() {
                    if let Some(old) = store.remove(&old_key) {
                        removed.push(old.body);
                    }
                }
            }

            removed
        };

        for body in removed_bodies {
            self.body_store.remove(body).await;
        }
    }

    pub async fn clear(&self) {
        let removed_bodies = {
            let mut removed = Vec::new();

            let mut store = self.store.write().await;
            removed.extend(store.drain().map(|(_, response)| response.body));

            let mut store404 = self.store_404.write().await;
            removed.extend(store404.drain().map(|(_, response)| response.body));

            let mut keys = self.keys_404.write().await;
            keys.clear();

            removed
        };

        for body in removed_bodies {
            self.body_store.remove(body).await;
        }
    }

    /// Clear cache entries matching a pattern (supports wildcards)
    pub async fn clear_by_pattern(&self, pattern: &str) {
        let removed_bodies = {
            let mut removed = Vec::new();

            let mut store = self.store.write().await;
            let keys_to_remove: Vec<String> = store
                .keys()
                .filter(|key| matches_pattern(key, pattern))
                .cloned()
                .collect();
            for key in keys_to_remove {
                if let Some(old) = store.remove(&key) {
                    removed.push(old.body);
                }
            }

            let mut store404 = self.store_404.write().await;
            let keys_to_remove_404: Vec<String> = store404
                .keys()
                .filter(|key| matches_pattern(key, pattern))
                .cloned()
                .collect();
            for key in &keys_to_remove_404 {
                if let Some(old) = store404.remove(key) {
                    removed.push(old.body);
                }
            }

            let mut keys = self.keys_404.write().await;
            keys.retain(|key| !matches_pattern(key, pattern));

            removed
        };

        for body in removed_bodies {
            self.body_store.remove(body).await;
        }
    }

    pub fn handle(&self) -> &CacheHandle {
        &self.handle
    }

    /// Get the number of cached items
    pub async fn size(&self) -> usize {
        let store = self.store.read().await;
        store.len()
    }

    /// Size of 404 cache
    pub async fn size_404(&self) -> usize {
        let store = self.store_404.read().await;
        store.len()
    }
}

impl Default for CacheHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "phantom-frame-test-{}-{:x}-{:016x}",
            name,
            process::id(),
            BODY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn test_matches_pattern_exact() {
        assert!(matches_pattern("GET:/api/users", "GET:/api/users"));
        assert!(!matches_pattern("GET:/api/users", "GET:/api/posts"));
    }

    #[test]
    fn test_matches_pattern_wildcard() {
        // Wildcard at end
        assert!(matches_pattern("GET:/api/users", "GET:/api/*"));
        assert!(matches_pattern("GET:/api/users/123", "GET:/api/*"));
        assert!(!matches_pattern("GET:/v2/users", "GET:/api/*"));

        // Wildcard at start
        assert!(matches_pattern("GET:/api/users", "*/users"));
        assert!(matches_pattern("POST:/v2/users", "*/users"));
        assert!(!matches_pattern("GET:/api/posts", "*/users"));

        // Wildcard in middle
        assert!(matches_pattern("GET:/api/v1/users", "GET:/api/*/users"));
        assert!(matches_pattern("GET:/api/v2/users", "GET:/api/*/users"));
        assert!(!matches_pattern("GET:/api/v1/posts", "GET:/api/*/users"));

        // Multiple wildcards
        assert!(matches_pattern("GET:/api/v1/users/123", "GET:*/users/*"));
        assert!(matches_pattern("POST:/v2/admin/users/456", "*/users/*"));
    }

    #[test]
    fn test_matches_pattern_wildcard_only() {
        assert!(matches_pattern("GET:/api/users", "*"));
        assert!(matches_pattern("POST:/anything", "*"));
    }

    #[tokio::test]
    async fn test_404_cache_set_get_and_eviction() {
        let trigger = CacheHandle::new();
        // capacity 2 for quicker eviction
        let store = CacheStore::new(trigger, 2);

        let resp1 = CachedResponse {
            body: vec![1],
            headers: HashMap::new(),
            status: 404,
            content_encoding: None,
        };
        let resp2 = CachedResponse {
            body: vec![2],
            headers: HashMap::new(),
            status: 404,
            content_encoding: None,
        };
        let resp3 = CachedResponse {
            body: vec![3],
            headers: HashMap::new(),
            status: 404,
            content_encoding: None,
        };

        // Set two 404 entries
        store
            .set_404("GET:/notfound1".to_string(), resp1.clone())
            .await;
        store
            .set_404("GET:/notfound2".to_string(), resp2.clone())
            .await;

        assert_eq!(store.size_404().await, 2);
        assert_eq!(store.get_404("GET:/notfound1").await.unwrap().body, vec![1]);

        // Add third entry - should evict oldest (notfound1)
        store
            .set_404("GET:/notfound3".to_string(), resp3.clone())
            .await;
        assert_eq!(store.size_404().await, 2);
        assert!(store.get_404("GET:/notfound1").await.is_none());
        assert_eq!(store.get_404("GET:/notfound2").await.unwrap().body, vec![2]);
        assert_eq!(store.get_404("GET:/notfound3").await.unwrap().body, vec![3]);
    }

    #[tokio::test]
    async fn test_clear_by_pattern_removes_404_entries() {
        let trigger = CacheHandle::new();
        let store = CacheStore::new(trigger, 10);

        let resp = CachedResponse {
            body: vec![1],
            headers: HashMap::new(),
            status: 404,
            content_encoding: None,
        };
        store
            .set_404("GET:/api/notfound".to_string(), resp.clone())
            .await;
        store
            .set_404("GET:/api/another".to_string(), resp.clone())
            .await;
        assert_eq!(store.size_404().await, 2);

        store.clear_by_pattern("GET:/api/*").await;
        assert_eq!(store.size_404().await, 0);
    }

    #[tokio::test]
    async fn test_filesystem_cache_round_trip() {
        let cache_dir = unique_test_directory("round-trip");
        let trigger = CacheHandle::new();
        let store =
            CacheStore::with_storage(trigger, 10, CacheStorageMode::Filesystem, Some(cache_dir));

        let response = CachedResponse {
            body: vec![1, 2, 3, 4],
            headers: HashMap::from([("content-type".to_string(), "text/plain".to_string())]),
            status: 200,
            content_encoding: None,
        };

        store
            .set("GET:/asset.js".to_string(), response.clone())
            .await;

        let stored_path = {
            let store_guard = store.store.read().await;
            match &store_guard.get("GET:/asset.js").unwrap().body {
                StoredBody::File(path) => path.clone(),
                StoredBody::Memory(_) => panic!("expected filesystem-backed cache body"),
            }
        };

        assert!(tokio::fs::metadata(&stored_path).await.is_ok());

        let cached = store.get("GET:/asset.js").await.unwrap();
        assert_eq!(cached.body, response.body);

        store.clear().await;
        assert!(tokio::fs::metadata(&stored_path).await.is_err());
    }

    #[tokio::test]
    async fn test_filesystem_404_eviction_removes_body_file() {
        let cache_dir = unique_test_directory("eviction");
        let trigger = CacheHandle::new();
        let store =
            CacheStore::with_storage(trigger, 2, CacheStorageMode::Filesystem, Some(cache_dir));

        for index in 1..=2 {
            store
                .set_404(
                    format!("GET:/missing{}", index),
                    CachedResponse {
                        body: vec![index as u8],
                        headers: HashMap::new(),
                        status: 404,
                        content_encoding: None,
                    },
                )
                .await;
        }

        let evicted_path = {
            let store_guard = store.store_404.read().await;
            match &store_guard.get("GET:/missing1").unwrap().body {
                StoredBody::File(path) => path.clone(),
                StoredBody::Memory(_) => panic!("expected filesystem-backed cache body"),
            }
        };

        store
            .set_404(
                "GET:/missing3".to_string(),
                CachedResponse {
                    body: vec![3],
                    headers: HashMap::new(),
                    status: 404,
                    content_encoding: None,
                },
            )
            .await;

        assert!(store.get_404("GET:/missing1").await.is_none());
        assert!(tokio::fs::metadata(&evicted_path).await.is_err());
    }

    #[tokio::test]
    async fn test_filesystem_clear_by_pattern_removes_matching_files() {
        let cache_dir = unique_test_directory("pattern-clear");
        let trigger = CacheHandle::new();
        let store =
            CacheStore::with_storage(trigger, 10, CacheStorageMode::Filesystem, Some(cache_dir));

        store
            .set(
                "GET:/api/one".to_string(),
                CachedResponse {
                    body: vec![1],
                    headers: HashMap::new(),
                    status: 200,
                    content_encoding: None,
                },
            )
            .await;
        store
            .set(
                "GET:/other/two".to_string(),
                CachedResponse {
                    body: vec![2],
                    headers: HashMap::new(),
                    status: 200,
                    content_encoding: None,
                },
            )
            .await;

        let (removed_path, kept_path) = {
            let store_guard = store.store.read().await;
            let removed = match &store_guard.get("GET:/api/one").unwrap().body {
                StoredBody::File(path) => path.clone(),
                StoredBody::Memory(_) => panic!("expected filesystem-backed cache body"),
            };
            let kept = match &store_guard.get("GET:/other/two").unwrap().body {
                StoredBody::File(path) => path.clone(),
                StoredBody::Memory(_) => panic!("expected filesystem-backed cache body"),
            };
            (removed, kept)
        };

        store.clear_by_pattern("GET:/api/*").await;

        assert!(store.get("GET:/api/one").await.is_none());
        assert!(store.get("GET:/other/two").await.is_some());
        assert!(tokio::fs::metadata(&removed_path).await.is_err());
        assert!(tokio::fs::metadata(&kept_path).await.is_ok());

        store.clear().await;
    }

    #[test]
    fn test_filesystem_startup_cleanup_removes_orphaned_cache_files() {
        let cache_dir = unique_test_directory("startup-cleanup");
        let standard_dir = cache_dir.join(CacheBucket::Standard.directory_name());
        let not_found_dir = cache_dir.join(CacheBucket::NotFound.directory_name());
        let unrelated_file = cache_dir.join("keep.txt");

        std::fs::create_dir_all(&standard_dir).unwrap();
        std::fs::create_dir_all(&not_found_dir).unwrap();
        std::fs::write(standard_dir.join("stale.bin"), b"stale").unwrap();
        std::fs::write(standard_dir.join("stale.tmp"), b"stale tmp").unwrap();
        std::fs::write(not_found_dir.join("stale.bin"), b"stale 404").unwrap();
        std::fs::write(&unrelated_file, b"keep me").unwrap();

        let trigger = CacheHandle::new();
        let _store = CacheStore::with_storage(
            trigger,
            10,
            CacheStorageMode::Filesystem,
            Some(cache_dir.clone()),
        );

        let standard_entries = std::fs::read_dir(&standard_dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let not_found_entries = std::fs::read_dir(&not_found_dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(standard_entries.is_empty());
        assert!(not_found_entries.is_empty());
        assert_eq!(std::fs::read(&unrelated_file).unwrap(), b"keep me");

        std::fs::remove_dir_all(&cache_dir).unwrap();
    }
}
