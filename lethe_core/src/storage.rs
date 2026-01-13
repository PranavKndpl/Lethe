use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use anyhow::{Result, Context};
use crate::crypto::{CryptoEngine, MasterKey};

/// Manages the physical storage of encrypted blocks on disk.
pub struct BlockManager {
    root_path: PathBuf,
}

impl BlockManager {
    /// Initialize the manager pointing to a specific directory
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let root_path = path.as_ref().to_path_buf();
        
        // Ensure directory exists
        if !root_path.exists() {
            fs::create_dir_all(&root_path)
                .context("Failed to create vault directory")?;
        }
        
        Ok(Self { root_path })
    }

    /// Takes raw data, compresses it, encrypts it, and saves it to disk.
    /// Returns the UUID of the new block.
    pub fn write_block(&self, data: &[u8], key: &MasterKey) -> Result<String> {
        // 1. Compress (Zstd)
        // Level 3 is a good balance of speed vs ratio
        let compressed_data = zstd::stream::encode_all(data, 3)
            .context("Compression failed")?;

        // 2. Encrypt (XChaCha20-Poly1305)
        // Returns (Ciphertext, Nonce)
        let (encrypted_data, nonce) = CryptoEngine::encrypt(&compressed_data, key)?;

        // 3. Generate Random ID
        let block_id = Uuid::new_v4().to_string();
        let file_path = self.root_path.join(format!("blk_{}.bin", block_id));

        // 4. Write to Disk (Nonce + Encrypted Data)
        let mut file = File::create(&file_path)
            .context("Failed to create block file")?;
        
        // We prepend the nonce to the file so we can read it back later
        file.write_all(&nonce)?;
        file.write_all(&encrypted_data)?;

        Ok(block_id)
    }

    /// Reads a block ID, reads disk, decrypts, and decompresses.
    pub fn read_block(&self, block_id: &str, key: &MasterKey) -> Result<Vec<u8>> {
        let file_path = self.root_path.join(format!("blk_{}.bin", block_id));
        
        // 1. Read File
        let mut file = File::open(&file_path)
            .context(format!("Block not found: {}", block_id))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        // 2. Split Nonce (First 24 bytes) and Data
        // XChaCha20 nonce is 24 bytes
        if buffer.len() < 24 {
            return Err(anyhow::anyhow!("Block file corrupted or too short"));
        }
        let (nonce, ciphertext) = buffer.split_at(24);

        // 3. Decrypt
        let compressed_data = CryptoEngine::decrypt(ciphertext, nonce, key)
            .context("Decryption failed (Wrong password or corrupted block)")?;

        // 4. Decompress
        let original_data = zstd::stream::decode_all(compressed_data.as_slice())
            .context("Decompression failed")?;

        Ok(original_data)
    }

    /// Deletes a block permanently
    pub fn delete_block(&self, block_id: &str) -> Result<()> {
        let file_path = self.root_path.join(format!("blk_{}.bin", block_id));
        if file_path.exists() {
            fs::remove_file(file_path).context("Failed to delete block")?;
        }
        Ok(())
    }
}