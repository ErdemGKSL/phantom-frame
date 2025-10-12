use std::collections::HashMap;
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
    refresh_trigger: RefreshTrigger,
}

#[derive(Clone, Debug)]
pub struct CachedResponse {
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
    pub status: u16,
}

impl CacheStore {
    pub fn new(refresh_trigger: RefreshTrigger) -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            refresh_trigger,
        }
    }

    pub async fn get(&self, key: &str) -> Option<CachedResponse> {
        let store = self.store.read().await;
        store.get(key).cloned()
    }

    pub async fn set(&self, key: String, response: CachedResponse) {
        let mut store = self.store.write().await;
        store.insert(key, response);
    }

    pub async fn clear(&self) {
        let mut store = self.store.write().await;
        store.clear();
    }

    /// Clear cache entries matching a pattern (supports wildcards)
    pub async fn clear_by_pattern(&self, pattern: &str) {
        let mut store = self.store.write().await;
        store.retain(|key, _| !matches_pattern(key, pattern));
    }

    pub fn refresh_trigger(&self) -> &RefreshTrigger {
        &self.refresh_trigger
    }

    /// Get the number of cached items
    pub async fn size(&self) -> usize {
        let store = self.store.read().await;
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
}
