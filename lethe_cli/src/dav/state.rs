use std::sync::Arc;
use tokio::sync::Mutex;
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use lethe_core::crypto::MasterKey;

#[derive(Clone, Debug)] 
pub struct LetheState {
    pub index: Arc<Mutex<IndexManager>>,
    pub storage: Arc<BlockManager>,
    pub key: Arc<MasterKey>,
}

impl LetheState {
    pub fn new(index: IndexManager, storage: BlockManager, key: MasterKey) -> Self {
        Self {
            index: Arc::new(Mutex::new(index)),
            storage: Arc::new(storage),
            key: Arc::new(key),
        }
    }
}