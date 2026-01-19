// lethe_cli/src/dav/state.rs
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use lethe_core::crypto::MasterKey;

/// Holds the active "Session" data.
/// This struct only exists when the vault is decrypted.
pub struct ActiveVault {
    pub index: Arc<Mutex<IndexManager>>,
    pub storage: Arc<BlockManager>,
    pub key: Arc<MasterKey>,
}

/// The Global Server State.
/// It exists even when the vault is locked.
pub struct LetheState {
    inner: RwLock<Option<ActiveVault>>,
}

impl LetheState {
    pub fn new() -> Self {
        Self { inner: RwLock::new(None) }
    }

    pub async fn unlock(&self, index: IndexManager, storage: BlockManager, key: MasterKey) {
        let mut write_guard = self.inner.write().await;
        *write_guard = Some(ActiveVault {
            index: Arc::new(Mutex::new(index)),
            storage: Arc::new(storage),
            key: Arc::new(key),
        });
    }

    pub async fn lock(&self) {
        let mut write_guard = self.inner.write().await;
        *write_guard = None; // Drops the keys and index immediately
    }

    /// Helper for FS operations to get access
    pub async fn get_resources(&self) -> Option<ActiveVault> {
        let read_guard = self.inner.read().await;
        // We clone the ARCs, so the operation can continue 
        // even if a lock happens mid-operation (optional consistency choice)
        // or we can strictly enforce lifetime.
        match &*read_guard {
            Some(v) => Some(ActiveVault {
                index: v.index.clone(),
                storage: v.storage.clone(),
                key: v.key.clone()
            }),
            None => None,
        }
    }
}