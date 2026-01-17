use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::{Result, Context};
use crate::crypto::{CryptoEngine, MasterKey};

/// The logical structure of a file inside the vault
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileEntry {
    pub path: String,       
    pub size: u64,          
    pub modified: u64,      // Unix timestamp
    pub blocks: Vec<String>,// List of UUIDs: ["uuid1", "uuid2"]

    #[serde(default)] 
    pub is_dir: bool,
}

/// The entire "Database" of the filesystem
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VaultIndex {
    pub version: u8,
    pub revision: u64,      // Increments on every save (for conflict resolution)
    pub salt: String,       // The salt used for the MasterKey
    pub files: HashMap<String, FileEntry>, // Path -> File Info
}

impl VaultIndex {
    pub fn new(salt: String) -> Self {
        Self {
            version: 1,
            revision: 0,
            salt,
            files: HashMap::new(),
        }
    }
}

/// Manages the loading, saving, and syncing of the Index
#[derive(Debug)]
pub struct IndexManager {
    root_path: PathBuf,
    pub data: VaultIndex,
}

impl IndexManager {
    /// Initialize a manager. 
    /// If index exists on disk, use load() instead.
    pub fn new_empty(path: PathBuf, salt: String) -> Self {
        Self {
            root_path: path,
            data: VaultIndex::new(salt),
        }
    }

    /// Tries to load the index from 3 replicas. 
    /// Picks the one with the highest revision number that successfully decrypts.
    pub fn load(path: PathBuf, key: &MasterKey) -> Result<Self> {
        let mut candidates = Vec::new();

        // Try to load all 3 replicas
        for i in 0..3 {
            let file_path = path.join(format!("meta_{}.bin", i));
            if file_path.exists() {
                if let Ok(index) = Self::read_and_decrypt(&file_path, key) {
                    candidates.push(index);
                }
            }
        }

        if candidates.is_empty() {
            return Err(anyhow::anyhow!("No valid index found. Vault corrupted or wrong password."));
        }

        // Sort by revision (highest first)
        candidates.sort_by(|a, b| b.revision.cmp(&a.revision));
        
        // Pick the winner
        let best_index = candidates.remove(0);
        
        Ok(Self {
            root_path: path,
            data: best_index,
        })
    }

    /// Saves the current index state to all 3 replicas safely.
    pub fn save(&mut self, key: &MasterKey) -> Result<()> {
        self.data.revision += 1; // Increment revision

        // Serialize to CBOR
        let plain_data = serde_cbor::to_vec(&self.data)
            .context("Failed to serialize index")?;

        // Encrypt
        let (encrypted_data, nonce) = CryptoEngine::encrypt(&plain_data, key)?;

        // Write to all 3 replicas
        for i in 0..3 {
            let file_name = format!("meta_{}.bin", i);
            let tmp_name = format!("meta_{}.tmp", i);
            let target_path = self.root_path.join(&file_name);
            let tmp_path = self.root_path.join(&tmp_name);

            // 1. Write to .tmp first (Atomic write pattern)
            let mut file = File::create(&tmp_path)?;
            file.write_all(&nonce)?;
            file.write_all(&encrypted_data)?;
            
            // 2. Rename .tmp to .bin (Atomic replace)
            fs::rename(&tmp_path, &target_path)?;
        }

        Ok(())
    }

    // --- Helper Functions ---

    fn read_and_decrypt(path: &Path, key: &MasterKey) -> Result<VaultIndex> {
        let mut file = File::open(path)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        if buffer.len() < 24 {
            return Err(anyhow::anyhow!("Index file too short"));
        }

        let (nonce, ciphertext) = buffer.split_at(24);
        
        let plain_data = CryptoEngine::decrypt(ciphertext, nonce, key)?;
        
        let index: VaultIndex = serde_cbor::from_slice(&plain_data)?;
        Ok(index)
    }

    pub fn add_file(&mut self, path: String, blocks: Vec<String>, size: u64) {
        let entry = FileEntry {
            path: path.clone(),
            size,
            modified: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            blocks,
            is_dir: false,
        };
        self.data.files.insert(path, entry);
    }

    pub fn add_dir(&mut self, path: String) {
        let entry = FileEntry {
            path: path.clone(),
            size: 0,
            modified: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            blocks: vec![],
            is_dir: true,
        };
        self.data.files.insert(path, entry);
    }
    
    pub fn get_file(&self, path: &str) -> Option<&FileEntry> {
        self.data.files.get(path)
    }
}