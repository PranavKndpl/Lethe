pub mod crypto;
pub mod storage;
pub mod index;
pub mod config;

pub use config::VaultConfig;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::crypto::CryptoEngine;
    use crate::storage::BlockManager;
    use crate::index::IndexManager;

    #[test]
    fn test_full_flow() {
        // 1. Setup
        let test_dir = "./test_vault";
        let _ = fs::remove_dir_all(test_dir); // Clean up old runs (ignore error if missing)
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

    #[test]
    fn test_index_replication() {
        let test_dir = "./test_index";
        let _ = fs::remove_dir_all(test_dir);
        fs::create_dir_all(test_dir).unwrap();
        let test_path = PathBuf::from(test_dir);

        // 1. Setup Key
        let (key, salt) = CryptoEngine::derive_key("password123").unwrap();

        // 2. Create Index & Add Data
        let mut manager = IndexManager::new_empty(test_path.clone(), salt);
        manager.add_file("/docs/secret.txt".to_string(), vec!["blk_1".to_string()], 1024);
        
        // 3. Save (should create meta_0, meta_1, meta_2)
        manager.save(&key).unwrap();

        // 4. Verify physical files exist
        assert!(test_path.join("meta_0.bin").exists());
        assert!(test_path.join("meta_1.bin").exists());
        assert!(test_path.join("meta_2.bin").exists());

        // 5. Load Back
        let loaded_manager = IndexManager::load(test_path.clone(), &key).unwrap();
        
        // 6. Verify Data Persisted
        let file_entry = loaded_manager.get_file("/docs/secret.txt").unwrap();
        assert_eq!(file_entry.blocks[0], "blk_1");

        // 7. Cleanup
        fs::remove_dir_all(test_dir).unwrap();
    }
}