use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// A trigger that can be cloned and triggered multiple times
/// Similar to oneshot but reusable
#[derive(Clone)]
pub struct RefreshTrigger {
    sender: broadcast::Sender<()>,
}

impl RefreshTrigger {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self { sender }
    }

    /// Trigger a cache refresh
    pub fn trigger(&self) {
        // Ignore errors if there are no receivers
        let _ = self.sender.send(());
    }

    /// Subscribe to refresh events
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.sender.subscribe()
    }
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
