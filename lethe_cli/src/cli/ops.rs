// lethe_cli/src/cli/ops.rs

use anyhow::{anyhow, Context, Result};
use log::error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use lethe_core::crypto::{CryptoEngine, MasterKey};
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;

// Add these imports at the top
use std::collections::HashSet;
use std::ffi::OsStr;

// --- SHARED HELPERS ---

pub fn resolve_vault_path(path: Option<&str>) -> Result<PathBuf> {
    match path {
        Some(p) => Ok(PathBuf::from(p)),
        None => dirs::home_dir()
            .map(|p| p.join(".lethe_vault"))
            .context("Could not determine home directory"),
    }
}

pub fn unlock_vault(vault_path_str: &str) -> Result<(PathBuf, MasterKey)> {
    let vault_path = resolve_vault_path(Some(vault_path_str))?;
    let salt_path = vault_path.join("salt.loader");

    if !salt_path.exists() {
        anyhow::bail!(
            "Invalid vault path: {:?}. (Did you run 'lethe init'?)",
            vault_path
        );
    }

    let password = rpassword::prompt_password("Enter Vault Password: ")?;
    let salt = fs::read_to_string(salt_path).context("Failed to read salt file")?;

    let (key, _) = CryptoEngine::derive_key_with_salt(&password, salt.trim())?;
    Ok((vault_path, key))
}

fn upload_worker(
    path: &Path,
    dest: &str,
    block_mgr: &BlockManager,
    index_mgr: &mut IndexManager,
    key: &MasterKey,
) -> Result<()> {
    print!("Processing {} ... ", path.display());
    io::stdout().flush()?;

    let data = fs::read(path).context("Failed to read source file")?;
    let size = data.len() as u64;

    // Note: This is still the "simple" upload.
    // Ideally this should use the chunking logic too, but it's acceptable for CLI tool v1.
    let block_id = block_mgr.write_block(&data, key)?;

    let clean_dest = dest.replace("//", "/");
    index_mgr.add_file(clean_dest, vec![block_id], size);

    println!("OK");
    Ok(())
}

// --- COMMAND HANDLERS ---

pub fn do_init(path: Option<String>) -> Result<()> {
    let vault_path = resolve_vault_path(path.as_deref())?;
    if vault_path.exists() {
        anyhow::bail!("Vault already exists at {:?}", vault_path);
    }

    println!("🛡️  Initializing vault at: {:?}", vault_path);

    let password = rpassword::prompt_password("Set Master Password: ")?;
    let confirm = rpassword::prompt_password("Confirm Password: ")?;

    if password != confirm {
        anyhow::bail!("Passwords do not match.");
    }
    if password.is_empty() {
        anyhow::bail!("Password cannot be empty.");
    }

    fs::create_dir_all(&vault_path).context("Failed to create vault directory")?;

    println!("🔑 Generating keys (Argon2id)...");

    let (key, salt) = tokio::task::block_in_place(|| CryptoEngine::derive_key(&password))?;
    fs::write(vault_path.join("salt.loader"), &salt).context("Failed to write salt")?;

    let mut index_mgr = IndexManager::new_empty(vault_path.clone(), salt);
    index_mgr.save(&key)?;

    let _ = BlockManager::new(&vault_path)?;

    println!("✅ Vault initialized successfully.");
    Ok(())
}

pub fn do_put(file: PathBuf, dest: String, vault: String) -> Result<()> {
    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(&vault))?;
    let mut index_mgr = IndexManager::load(vault_path.clone(), &key)?;
    let block_mgr = BlockManager::new(&vault_path)?;

    if !file.exists() {
        anyhow::bail!("Source file not found: {:?}", file);
    }

    if file.is_dir() {
        println!("📂 Uploading directory: {:?}", file);

        for entry in WalkDir::new(&file).min_depth(1) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let path = entry.path();
                let relative = path.strip_prefix(&file)?;
                let clean_relative = relative.to_string_lossy().replace("\\", "/");

                let clean_dest = dest.trim_end_matches('/');
                let vault_dest = format!("{}/{}", clean_dest, clean_relative);

                upload_worker(path, &vault_dest, &block_mgr, &mut index_mgr, &key)?;
            }
        }
    } else {
        upload_worker(&file, &dest, &block_mgr, &mut index_mgr, &key)?;
    }

    index_mgr.save(&key)?;
    println!("✅ Upload complete.");
    Ok(())
}

pub fn do_ls(vault: String) -> Result<()> {
    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(&vault))?;
    let index_mgr = IndexManager::load(vault_path, &key)?;

    println!("\n📂 Vault Contents:");
    println!("{:<12} | {:<40}", "SIZE", "PATH");
    println!("{:-<60}", "-");

    let mut paths: Vec<_> = index_mgr.data.files.keys().collect();
    paths.sort();

    for path in paths {
        let entry = &index_mgr.data.files[path];
        let size_str = humansize::format_size(entry.size, humansize::BINARY);
        println!("{:<12} | {}", size_str, path);
    }

    println!();
    Ok(())
}

pub fn do_get(src: String, out: PathBuf, vault: String) -> Result<()> {
    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(&vault))?;
    let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
    let block_mgr = BlockManager::new(&vault_path)?;

    if let Some(entry) = index_mgr.get_file(&src) {
        println!(
            "📥 Downloading {} ({})",
            src,
            humansize::format_size(entry.size, humansize::BINARY)
        );

        let mut full_data = Vec::with_capacity(entry.size as usize);
        for block_id in &entry.blocks {
            let mut chunk = block_mgr.read_block(block_id, &key)?;
            full_data.append(&mut chunk);
        }

        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&out, full_data)?;
        println!("✅ Saved to {:?}", out);
    } else {
        anyhow::bail!("File not found in vault: {}", src);
    }

    Ok(())
}

pub fn do_repair(vault: String) -> Result<()> {
    println!("🛠️  Starting repair process...");

    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(&vault))?;

    match IndexManager::load(vault_path, &key) {
        Ok(mut index_mgr) => {
            println!(
                "✅ Valid index replica found (Rev: {}).",
                index_mgr.data.revision
            );
            println!("🔄 Resyncing all replicas...");
            index_mgr.save(&key)?;
            println!("✅ Repair complete.");
            Ok(())
        }
        Err(e) => {
            error!("Repair failed: {}", e);
            anyhow::bail!("CRITICAL: Could not recover index. Vault may be corrupted.");
        }
    }
}

// ... (existing functions) ...

pub fn do_clean(vault: String, dry_run: bool) -> Result<()> {
    println!("🧹 Starting Garbage Collection...");
    if dry_run {
        println!("ℹ️  DRY RUN: No files will be deleted.");
    }

    // 1. Unlock and Load Index
    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(&vault))?;
    let index_mgr = IndexManager::load(vault_path.clone(), &key)?;

    // 2. Build Set of Valid Blocks
    println!("📊 Analyzing Index...");
    let mut valid_blocks = HashSet::new();
    for entry in index_mgr.data.files.values() {
        for block in &entry.blocks {
            valid_blocks.insert(block.clone());
        }
    }
    println!(
        "   Found {} active blocks referenced in Index.",
        valid_blocks.len()
    );

    // 3. Scan Disk for Orphans
    let mut reclaimed_bytes: u64 = 0;
    let mut deleted_count: u64 = 0;
    let mut kept_count: u64 = 0;

    // Assuming blocks are stored directly in vault_path or vault_path/store
    // We scan the vault_path for blk_*.bin files
    let read_dir = fs::read_dir(&vault_path).context("Failed to read vault directory")?;

    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();

        // Filter for files starting with "blk_" and ending with ".bin"
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(OsStr::to_str) {
                if name.starts_with("blk_") && name.ends_with(".bin") {
                    // Extract ID: blk_XYZ.bin -> XYZ
                    let id_part = &name[4..name.len() - 4];

                    if !valid_blocks.contains(id_part) {
                        // ORPHAN DETECTED
                        let len = entry.metadata()?.len();
                        if !dry_run {
                            fs::remove_file(&path)
                                .context("Failed to delete orphan block")?;
                        }
                        reclaimed_bytes += len;
                        deleted_count += 1;

                        if dry_run {
                            println!("   [DRY] Would delete orphan: {}", name);
                        }
                    } else {
                        kept_count += 1;
                    }
                }
            }
        }
    }

    println!("---------------------------------------------------");
    println!("✅ GC Complete.");
    println!("   Active Blocks: {}", kept_count);
    println!("   Orphans Removed: {}", deleted_count);
    println!(
        "   Space Reclaimed: {}",
        humansize::format_size(reclaimed_bytes, humansize::BINARY)
    );

    Ok(())
}
