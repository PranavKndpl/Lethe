pub mod crypto;
pub mod storage;
pub mod index;
pub mod config;

pub use config::VaultConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::CryptoEngine;
    use crate::storage::BlockManager;
    use std::fs;

    #[test]
    fn test_full_flow() {
        // 1. Setup
        let test_dir = "./test_vault";
        let _ = fs::remove_dir_all(test_dir); // Clean up old runs
        let manager = BlockManager::new(test_dir).unwrap();
        
        // 2. Create Key
        let password = "my_secret_password";
        let (key, _salt) = CryptoEngine::derive_key(password).unwrap();

        // 3. Write Data
        let my_secret = b"Launch codes: 9999";
        let block_id = manager.write_block(my_secret, &key).unwrap();
        println!("Written block: {}", block_id);

        // 4. Read Data
        let recovered = manager.read_block(&block_id, &key).unwrap();
        assert_eq!(recovered, my_secret);

        // 5. Cleanup
        fs::remove_dir_all(test_dir).unwrap();
    }
}