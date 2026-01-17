use clap::{Parser, Subcommand};
use anyhow::{Result, Context, anyhow};
use std::path::{Path, PathBuf};
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::io::{self, Write};
use walkdir::WalkDir;
use log::{info, error, warn};

use lethe_core::crypto::{CryptoEngine, MasterKey};
use lethe_core::storage::BlockManager;
use lethe_core::index::IndexManager;

mod fs_webdav;

#[derive(Parser)]
#[command(name = "lethe", about = "A serverless, encrypted, distributed filesystem.", version = "1.0.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new vault
    Init { 
        #[arg(short, long)] 
        path: Option<String> 
    },
    /// Encrypt and upload a file or directory
    Put { 
        #[arg(short, long)] 
        file: PathBuf, 
        #[arg(short, long)] 
        dest: String, 
        #[arg(long)] 
        vault: String 
    },
    /// List files in the vault
    Ls { 
        #[arg(long)] 
        vault: String 
    },
    /// Decrypt and retrieve a file
    Get { 
        #[arg(short, long)] 
        src: String, 
        #[arg(short, long)] 
        out: PathBuf, 
        #[arg(long)] 
        vault: String 
    },
    /// Mount the vault as a virtual drive
    Mount { 
        /// Optional: Vault path (Default: ~/.lethe_vault)
        #[arg(long)] 
        vault: Option<String>, 
        /// Windows: Drive Letter (e.g., "Z:"). Linux: Ignored (uses auto-mount).
        #[arg(long)] 
        mountpoint: Option<String> 
    },
    /// Attempt to repair index consistency
    Repair { 
        #[arg(long)] 
        vault: String 
    },
    /// Emergency cleanup of mount points
    Panic,
}

// --- SAFETY GUARD FOR WINDOWS MOUNTS ---
// This ensures that even if the app crashes or panics, the drive is unmounted.
#[cfg(target_os = "windows")]
struct MountGuard {
    drive: String,
}

#[cfg(target_os = "windows")]
impl Drop for MountGuard {
    fn drop(&mut self) {
        println!("🧹 Cleaning up drive {}...", self.drive);
        let _ = Command::new("net")
            .args(&["use", &self.drive, "/delete", "/y"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

// MAIN ENTRY POINT
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging (Run with RUST_LOG=info to see logs)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    
    let cli = Cli::parse();

    match &cli.command {
        // --- 1. INIT ---
        Commands::Init { path } => {
            let vault_path = resolve_vault_path(path.as_deref())?;
            if vault_path.exists() { 
                anyhow::bail!("Vault already exists at {:?}", vault_path); 
            }

            println!("🛡️  Initializing vault at: {:?}", vault_path);
            let password = rpassword::prompt_password("Set Master Password: ")?;
            let confirm = rpassword::prompt_password("Confirm Password: ")?;

            if password != confirm { anyhow::bail!("Passwords do not match."); }
            if password.is_empty() { anyhow::bail!("Password cannot be empty."); }

            fs::create_dir_all(&vault_path).context("Failed to create vault directory")?;

            println!("🔑 Generating keys (Argon2id)...");
            // Run CPU-intensive crypto on a blocking thread
            let (key, salt) = tokio::task::block_in_place(|| CryptoEngine::derive_key(&password))?;

            fs::write(vault_path.join("salt.loader"), &salt).context("Failed to write salt")?;

            let mut index_mgr = IndexManager::new_empty(vault_path.clone(), salt);
            index_mgr.save(&key)?;

            let _ = BlockManager::new(&vault_path)?;
            println!("✅ Vault initialized successfully.");
        }

        // --- 2. PUT ---
        Commands::Put { file, dest, vault } => {
            let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault))?;
            let mut index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

            if !file.exists() { anyhow::bail!("Source file not found: {:?}", file); }

            if file.is_dir() {
                println!("📂 Uploading directory: {:?}", file);
                for entry in WalkDir::new(file).min_depth(1) {
                    let entry: walkdir::DirEntry = entry?;
                    if entry.file_type().is_file() {
                        let path = entry.path();
                        let relative_path = path.strip_prefix(file)?;
                        // Normalize paths to forward slashes for internal consistency
                        let clean_relative = relative_path.to_string_lossy().replace("\\", "/");
                        let clean_dest = dest.trim_end_matches('/');
                        let vault_dest = format!("{}/{}", clean_dest, clean_relative);
                        
                        upload_file(path, &vault_dest, &block_mgr, &mut index_mgr, &key)?;
                    }
                }
            } else {
                upload_file(file, dest, &block_mgr, &mut index_mgr, &key)?;
            }

            index_mgr.save(&key)?;
            println!("✅ Upload complete.");
        }

        // --- 3. LS ---
        Commands::Ls { vault } => {
            let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault))?;
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
        }

        // --- 4. GET ---
        Commands::Get { src, out, vault } => {
            let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault))?;
            let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

            if let Some(entry) = index_mgr.get_file(src) {
                println!("📥 Downloading {} ({})", src, humansize::format_size(entry.size, humansize::BINARY));
                
                let mut full_data = Vec::with_capacity(entry.size as usize);
                for block_id in &entry.blocks {
                    let mut chunk = block_mgr.read_block(block_id, &key)?;
                    full_data.append(&mut chunk);
                }

                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(out, full_data)?;
                println!("✅ Saved to {:?}", out);
            } else {
                anyhow::bail!("File not found in vault: {}", src);
            }
        }

        // --- 5. REPAIR ---
        Commands::Repair { vault } => {
            println!("🛠️  Starting repair process...");
            let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault))?;

            match IndexManager::load(vault_path, &key) {
                Ok(mut index_mgr) => {
                    println!("✅ Valid index replica found (Rev: {}).", index_mgr.data.revision);
                    println!("🔄 Resyncing all replicas...");
                    index_mgr.save(&key)?;
                    println!("✅ Repair complete.");
                },
                Err(e) => {
                    error!("Repair failed: {}", e);
                    anyhow::bail!("CRITICAL: Could not recover index. Vault may be corrupted.");
                }
            }
        }

        // --- 6. MOUNT (WebDAV) ---
        Commands::Mount { vault, mountpoint } => {
            let vault_path = resolve_vault_path(vault.as_deref())?;
            println!("🔓 Unlocking vault at {:?}...", vault_path);

            let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault_path.to_str().unwrap()))?;
            let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

            // Setup Filesystem
            let lethe_fs = fs_webdav::LetheWebDav {
                index: Arc::new(tokio::sync::Mutex::new(index_mgr)),
                storage: Arc::new(block_mgr),
                key: Arc::new(key),
            };

            let dav_server = dav_server::DavHandler::builder()
                .filesystem(Box::new(lethe_fs))
                .build_handler();

            let port = 4918;
            let addr = ([127, 0, 0, 1], port);
            
            // Start Server
            println!("🚀 Lethe Server starting at http://127.0.0.1:{}", port);
            let server_handle = tokio::spawn(async move {
                warp::serve(dav_server::warp::dav_handler(dav_server))
                    .run(addr)
                    .await;
            });

            // WINDOWS MOUNT LOGIC
            #[cfg(target_os = "windows")]
            {
                let drive_letter = mountpoint.clone().unwrap_or_else(|| "Z:".to_string());
                
                // Initialize the Guard. If main() exits for ANY reason after this, 
                // the guard will drop and force-unmount the drive.
                let _guard = MountGuard { drive: drive_letter.clone() };

                println!("🔄 Mounting to Drive {}...", drive_letter);
                
                // Pre-clean (just in case)
                let _ = Command::new("net")
                    .args(&["use", &drive_letter, "/delete", "/y"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();

                // Mount
                let status = Command::new("net")
                    .args(&["use", &drive_letter, &format!("http://127.0.0.1:{}", port)])
                    .status()?;

                if status.success() {
                    println!("✅ Mounted successfully!");
                    // --- NEW: COSMETIC RENAME ---
                    // It changes how it looks in "This PC", but doesn't change the underlying technical name.
                    let label = format!("Lethe Vault");
                    let rename_script = format!(
                        "$sh = New-Object -ComObject Shell.Application; $sh.NameSpace('{}').Self.Name = '{}'", 
                        drive_letter, 
                        label
                    );
                    
                    let _ = Command::new("powershell")
                        .args(&["-NoProfile", "-Command", &rename_script])
                        .stdout(std::process::Stdio::null()) // Hides output
                        .status();

                    println!("📂 Opening Explorer...");
                    let _ = Command::new("explorer").arg(&drive_letter).spawn();
                } else {
                    error!("Mount failed.");
                    eprintln!("⚠️  Could not map drive letter. The server is still running.");
                    eprintln!("   You can access it manually via: http://127.0.0.1:{}", port);
                }

                println!("Press Ctrl+C to unmount and exit.");
                
                // Block until signal
                tokio::signal::ctrl_c().await?;
                println!("🛑 Stopping server...");
                // _guard drops here, executing 'net use /delete' automatically.
            }

            // LINUX MOUNT LOGIC
            #[cfg(target_os = "linux")]
            {
                println!("🐧 Linux detected.");
                println!("📂 Opening file manager at dav://127.0.0.1:{}", port);
                
                let _ = Command::new("xdg-open")
                    .arg(format!("dav://127.0.0.1:{}", port))
                    .spawn();

                println!("(Press Ctrl+C to stop)");
                tokio::signal::ctrl_c().await?;
                println!("🛑 Stopping server...");
            }
            
            // Abort server task
            server_handle.abort();
        }

        // --- 7. PANIC ---
        Commands::Panic => {
            println!("🚨 PANIC SEQUENCE INITIATED 🚨");
            println!("   Forcing unmount of all Lethe drives...");
            
            #[cfg(target_os = "windows")]
            {
                // Aggressively kill common drive letters just in case
                for drive in ["Z:", "Y:", "X:"] {
                    let _ = Command::new("net")
                        .args(&["use", drive, "/delete", "/y"])
                        .stdout(std::process::Stdio::null())
                        .status();
                }
            }
            println!("✅ Cleanup commands sent.");
        }
    }

    Ok(())
}

// --- HELPER FUNCTIONS ---

fn resolve_vault_path(path: Option<&str>) -> Result<PathBuf> {
    match path {
        Some(p) => Ok(PathBuf::from(p)),
        None => dirs::home_dir()
            .map(|p| p.join(".lethe_vault"))
            .context("Could not determine home directory"),
    }
}

fn unlock_vault(vault_path_str: &str) -> Result<(PathBuf, MasterKey)> {
    let vault_path = resolve_vault_path(Some(vault_path_str))?;
    let salt_path = vault_path.join("salt.loader");

    if !salt_path.exists() {
        anyhow::bail!("Invalid vault path: {:?}. (Did you run 'lethe init'?)", vault_path);
    }

    // Use rpassword for secure entry
    let password = rpassword::prompt_password("Enter Vault Password: ")?;
    let salt = fs::read_to_string(salt_path).context("Failed to read salt file")?;
    
    let (key, _) = CryptoEngine::derive_key_with_salt(&password, salt.trim())?;
    Ok((vault_path, key))
}

fn upload_file(
    path: &Path,
    dest: &str,
    block_mgr: &BlockManager,
    index_mgr: &mut IndexManager,
    key: &MasterKey
) -> Result<()> {
    print!("Processing {} ... ", path.display());
    io::stdout().flush()?;

    let data = fs::read(path).context("Failed to read source file")?;
    let size = data.len() as u64;

    let block_id = block_mgr.write_block(&data, key)?;
    
    // WebDAV treats paths as /path/to/file. Ensure we don't have double slashes.
    let clean_dest = dest.replace("//", "/");
    
    index_mgr.add_file(clean_dest, vec![block_id], size);

    println!("OK");
    Ok(())
}