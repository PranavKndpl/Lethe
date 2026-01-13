use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    // Size of each block in bytes (default: 65536)
    pub block_size: usize,
    // Zstd compression level (1-22)
    pub compression_level: i32,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            block_size: 65536, // 64KB
            compression_level: 3,
        }
    }
}