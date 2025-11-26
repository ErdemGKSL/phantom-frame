use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Enum representing different types of cache refresh triggers
#[derive(Clone, Debug)]
pub enum RefreshMessage {
    /// Refresh all cache entries
    All,
    /// Refresh cache entries matching a pattern (supports wildcards)
    Pattern(String),
}

/// A trigger that can be cloned and triggered multiple times
/// Similar to oneshot but reusable
#[derive(Clone)]
pub struct RefreshTrigger {
    sender: broadcast::Sender<RefreshMessage>,
}

impl RefreshTrigger {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self { sender }
    }

    /// Trigger a full cache refresh (clears all entries)
    pub fn trigger(&self) {
        // Ignore errors if there are no receivers
        let _ = self.sender.send(RefreshMessage::All);
    }

    /// Trigger a cache refresh for entries matching a pattern
    /// Supports wildcards: "/api/*", "GET:/api/*", etc.
    pub fn trigger_by_key_match(&self, pattern: &str) {
        // Ignore errors if there are no receivers
        let _ = self.sender.send(RefreshMessage::Pattern(pattern.to_string()));
    }

    /// Subscribe to refresh events
    pub fn subscribe(&self) -> broadcast::Receiver<RefreshMessage> {
        self.sender.subscribe()
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
    store: Arc<RwLock<HashMap<String, CachedResponse>>>,
    // 404-specific store with bounded capacity and FIFO eviction
    store_404: Arc<RwLock<HashMap<String, CachedResponse>>>,
    keys_404: Arc<RwLock<VecDeque<String>>>,
    cache_404_capacity: usize,
    refresh_trigger: RefreshTrigger,
}

#[derive(Clone, Debug)]
pub struct CachedResponse {
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
    pub status: u16,
}

impl CacheStore {
    pub fn new(refresh_trigger: RefreshTrigger, cache_404_capacity: usize) -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            store_404: Arc::new(RwLock::new(HashMap::new())),
            keys_404: Arc::new(RwLock::new(VecDeque::new())),
            cache_404_capacity,
            refresh_trigger,
        }
    }

    pub async fn get(&self, key: &str) -> Option<CachedResponse> {
        let store = self.store.read().await;
        store.get(key).cloned()
    }

    /// Get a 404 cached response (if present)
    pub async fn get_404(&self, key: &str) -> Option<CachedResponse> {
        let store = self.store_404.read().await;
        store.get(key).cloned()
    }

    pub async fn set(&self, key: String, response: CachedResponse) {
        let mut store = self.store.write().await;
        store.insert(key, response);
    }

    /// Set a 404 cached response. Bounded by `cache_404_capacity` and evict the oldest entries when limit reached.
    pub async fn set_404(&self, key: String, response: CachedResponse) {
        if self.cache_404_capacity == 0 {
            // 404 caching disabled
            return;
        }

        let mut store = self.store_404.write().await;
        let mut keys = self.keys_404.write().await;

        // If key already exists, remove it from its position in keys and re-add to the back
        if store.contains_key(&key) {
            // remove the key from keys deque (linear scan; acceptable for bounded deque)
            if let Some(pos) = keys.iter().position(|k| k == &key) {
                keys.remove(pos);
            }
        }

        // Insert into store and push back in keys
        store.insert(key.clone(), response);
        keys.push_back(key.clone());

        // Evict oldest items if capacity exceeded
        while keys.len() > self.cache_404_capacity {
            if let Some(old_key) = keys.pop_front() {
                store.remove(&old_key);
            }
        }
    }

    pub async fn clear(&self) {
        let mut store = self.store.write().await;
        store.clear();
        let mut store404 = self.store_404.write().await;
        store404.clear();
        let mut keys = self.keys_404.write().await;
        keys.clear();
    }

    /// Clear cache entries matching a pattern (supports wildcards)
    pub async fn clear_by_pattern(&self, pattern: &str) {
        let mut store = self.store.write().await;
        store.retain(|key, _| !matches_pattern(key, pattern));

        let mut store404 = self.store_404.write().await;
        let mut keys = self.keys_404.write().await;
        // Remove matching from store_404 and keys
        store404.retain(|key, _| !matches_pattern(key, pattern));
        keys.retain(|k| !matches_pattern(k, pattern));
    }

    pub fn refresh_trigger(&self) -> &RefreshTrigger {
        &self.refresh_trigger
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

impl Default for RefreshTrigger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let trigger = RefreshTrigger::new();
        // capacity 2 for quicker eviction
        let store = CacheStore::new(trigger, 2);

        let resp1 = CachedResponse { body: vec![1], headers: HashMap::new(), status: 404 };
        let resp2 = CachedResponse { body: vec![2], headers: HashMap::new(), status: 404 };
        let resp3 = CachedResponse { body: vec![3], headers: HashMap::new(), status: 404 };

        // Set two 404 entries
        store.set_404("GET:/notfound1".to_string(), resp1.clone()).await;
        store.set_404("GET:/notfound2".to_string(), resp2.clone()).await;

        assert_eq!(store.size_404().await, 2);
        assert_eq!(store.get_404("GET:/notfound1").await.unwrap().body, vec![1]);

        // Add third entry - should evict oldest (notfound1)
        store.set_404("GET:/notfound3".to_string(), resp3.clone()).await;
        assert_eq!(store.size_404().await, 2);
        assert!(store.get_404("GET:/notfound1").await.is_none());
        assert_eq!(store.get_404("GET:/notfound2").await.unwrap().body, vec![2]);
        assert_eq!(store.get_404("GET:/notfound3").await.unwrap().body, vec![3]);
    }

    #[tokio::test]
    async fn test_clear_by_pattern_removes_404_entries() {
        let trigger = RefreshTrigger::new();
        let store = CacheStore::new(trigger, 10);

        let resp = CachedResponse { body: vec![1], headers: HashMap::new(), status: 404 };
        store.set_404("GET:/api/notfound".to_string(), resp.clone()).await;
        store.set_404("GET:/api/another".to_string(), resp.clone()).await;
        assert_eq!(store.size_404().await, 2);

        store.clear_by_pattern("GET:/api/*").await;
        assert_eq!(store.size_404().await, 0);
    }
}
