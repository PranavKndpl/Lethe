use clap::{Parser, Subcommand};
use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use std::fs;
use std::io::{self, Write};
use walkdir::WalkDir;

use lethe_core::crypto::{CryptoEngine, MasterKey};
use lethe_core::storage::BlockManager;
use lethe_core::index::IndexManager;

use dirs;

#[cfg(unix)]
mod fs_fuse;

#[derive(Parser)]
#[command(name = "lethe")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        /// Optional: Path to create vault (Default: ~/.lethe_vault)
        #[arg(short, long)]
        path: Option<String>,
    },
    Put {
        #[arg(short, long)]
        file: PathBuf,
        #[arg(short, long)]
        dest: String,
        #[arg(long)]
        vault: String,
    },
    Ls {
        #[arg(long)]
        vault: String,
    },
    Get {
        #[arg(short, long)]
        src: String,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        vault: String,
    },
    Repair {
        #[arg(long)]
        vault: String,
    },
    Mount {
        /// Optional: Vault path (Default: ~/.lethe_vault)
        #[arg(long)]
        vault: Option<String>,
        /// Optional: Mount point (Default: ~/LetheDrive)
        #[arg(long)]
        mountpoint: Option<String>,
    },
    /// Unmount the drive safely
    Unmount {
        /// Optional: Mount point (Default: ~/LetheDrive)
        #[arg(long)]
        mountpoint: Option<String>,
    },
    /// EMERGENCY: Instantly kill all mounts
    Panic,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { path } => {
            let vault_path = match path {
                Some(p) => PathBuf::from(p),
                None => dirs::home_dir().unwrap().join(".lethe_vault"),
            };

            if vault_path.exists() { anyhow::bail!("Vault path already exists!"); }

            println!("Initializing vault at: {:?}", vault_path);
            let password = rpassword::prompt_password("Set Master Password: ")?;
            let confirm = rpassword::prompt_password("Confirm Password: ")?;

            if password != confirm { anyhow::bail!("Passwords do not match."); }

            fs::create_dir_all(&vault_path)?;

            println!("Generating keys...");
            let (key, salt) = CryptoEngine::derive_key(&password)?;

            fs::write(vault_path.join("salt.loader"), &salt)?;

            let mut index_mgr = IndexManager::new_empty(vault_path.clone(), salt);
            index_mgr.save(&key)?;

            let _ = BlockManager::new(&vault_path)?;

            println!("✅ Vault initialized.");
        }

        Commands::Put { file, dest, vault } => {
            let (vault_path, key) = unlock_vault(vault)?;
            let mut index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

            if file.is_dir() {
                println!("📂 Uploading directory: {:?}", file);
                for entry in WalkDir::new(file).min_depth(1) {
                    let entry = entry?;
                    if entry.file_type().is_file() {
                        let path = entry.path();
                        let relative_path = path.strip_prefix(file)?;
                        let vault_dest = if dest.ends_with('/') {
                            format!("{}{}", dest, relative_path.to_string_lossy().replace("\\", "/"))
                        } else {
                            format!("{}/{}", dest, relative_path.to_string_lossy().replace("\\", "/"))
                        };
                        upload_file(path, &vault_dest, &block_mgr, &mut index_mgr, &key)?;
                    }
                }
            } else {
                upload_file(file, dest, &block_mgr, &mut index_mgr, &key)?;
            }

            index_mgr.save(&key)?;
            println!("✅ All operations completed.");
        }

        Commands::Ls { vault } => {
            let (vault_path, key) = unlock_vault(vault)?;
            let index_mgr = IndexManager::load(vault_path, &key)?;

            println!("\n📂 Vault Contents:");
            println!("{:<12} | {:<40}", "SIZE (B)", "PATH");
            println!("{:-<55}", "-");

            let mut paths: Vec<_> = index_mgr.data.files.keys().collect();
            paths.sort();

            for path in paths {
                let entry = &index_mgr.data.files[path];
                println!("{:<12} | {}", entry.size, path);
            }
            println!();
        }

        Commands::Get { src, out, vault } => {
            let (vault_path, key) = unlock_vault(vault)?;
            let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

            if let Some(entry) = index_mgr.get_file(src) {
                let mut full_data = Vec::new();
                for block_id in &entry.blocks {
                    let mut chunk = block_mgr.read_block(block_id, &key)?;
                    full_data.append(&mut chunk);
                }

                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(out, full_data)?;
                println!("✅ Decrypted and saved to {:?}", out);
            } else {
                anyhow::bail!("File not found: {}", src);
            }
        }

        Commands::Repair { vault } => {
            println!("🛠️  Starting repair process...");
            let (vault_path, key) = unlock_vault(vault)?;

            match IndexManager::load(vault_path, &key) {
                Ok(mut index_mgr) => {
                    println!("✅ Found a valid index replica.");
                    println!("🔄 Resyncing all replicas...");
                    index_mgr.save(&key)?;
                    println!("✅ Repair complete! All index replicas are now in sync.");
                },
                Err(e) => {
                    eprintln!("❌ CRITICAL FAILURE: Could not recover any index replicas.");
                    eprintln!("   Error: {}", e);
                }
            }
        }

        Commands::Mount { vault, mountpoint } => {
            #[cfg(unix)]
            {
                let vault_path = match vault {
                    Some(v) => PathBuf::from(v),
                    None => dirs::home_dir().unwrap().join(".lethe_vault"),
                };

                let mount_path = match mountpoint {
                    Some(m) => PathBuf::from(m),
                    None => dirs::home_dir().unwrap().join("LetheDrive"),
                };

                if !mount_path.exists() {
                    fs::create_dir_all(&mount_path)?;
                }

                let (vault_path, key) = unlock_vault(vault_path.to_str().unwrap())?;
                println!("🔓 Vault unlocked.");

                let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
                let block_mgr = BlockManager::new(&vault_path)?;

                println!("🚀 Mounting LetheFS at {}", mount_path.display());
                println!("   (Press Ctrl+C to stop, or run 'lethe unmount')");

                let mut inode_map = std::collections::HashMap::new();
                inode_map.insert(1, "/".to_string());

                for (path, _) in &index_mgr.data.files {
                    let file_ino = fxhash::hash64(path);
                    inode_map.insert(file_ino, path.clone());

                    let path_obj = Path::new(path);
                    for ancestor in path_obj.ancestors() {
                        let ancestor_str = ancestor.to_string_lossy().to_string();
                        let clean_path = if ancestor_str == "/" || ancestor_str.is_empty() {
                            "/".to_string()
                        } else if !ancestor_str.starts_with('/') {
                            format!("/{}", ancestor_str)
                        } else {
                            ancestor_str
                        };

                        if clean_path != "/" {
                            let dir_ino = fxhash::hash64(&clean_path);
                            inode_map.insert(dir_ino, clean_path);
                        }
                    }
                }

                let fs = fs_fuse::LetheFS {
                    index: index_mgr,
                    storage: block_mgr,
                    key,
                    inode_map,
                    write_buffer: std::collections::HashMap::new(),
                };

                let options = vec![
                    fuser::MountOption::FSName("Lethe".to_string()),
                    fuser::MountOption::AutoUnmount,
                    fuser::MountOption::AllowOther,
                ];

                fuser::mount2(fs, &mount_path, &options)?;
            }

            #[cfg(not(unix))]
            {
                anyhow::bail!("Mounting is only supported on Linux/WSL.");
            }
        }

        Commands::Unmount { mountpoint } => {
            #[cfg(unix)]
            {
                use std::process::Command;

                let mount_path = match mountpoint {
                    Some(m) => PathBuf::from(m),
                    None => dirs::home_dir().unwrap().join("LetheDrive"),
                };

                println!("🛑 Unmounting {}...", mount_path.display());

                let status = Command::new("fusermount")
                    .arg("-u")
                    .arg(&mount_path)
                    .status();

                match status {
                    Ok(s) if s.success() => println!("✅ Successfully unmounted."),
                    Ok(_) => eprintln!("❌ Unmount failed. Drive might be busy (open in another terminal?)."),
                    Err(e) => eprintln!("❌ Failed to execute fusermount: {}", e),
                }
            }

            #[cfg(not(unix))]
            {
                anyhow::bail!("Unmounting is only supported on Linux/WSL.");
            }
        }

        Commands::Panic => {
            #[cfg(unix)]
            {
                use std::process::Command;
                use std::thread; // <--- Import thread for sleep
                use std::time::Duration;

                println!("🚨 PANIC SEQUENCE INITIATED 🚨");
                
                let mount_path = dirs::home_dir().unwrap().join("LetheDrive");
                
                // 1. Force Unmount
                let _ = Command::new("fusermount")
                    .arg("-u")
                    .arg("-z") 
                    .arg(&mount_path)
                    .output();

                // 2. Kill the process
                let _ = Command::new("pkill")
                    .arg("-9") 
                    .arg("-f") 
                    .arg("lethe mount") 
                    .output();
                
                // 3. WAIT for the OS to release the folder
                thread::sleep(Duration::from_millis(200)); 

                // 4. Remove the mount point directory (Scorched Earth)
                match fs::remove_dir(&mount_path) {
                    Ok(_) => println!("💥 Drive vanished. Mount point deleted."),
                    Err(e) => println!("⚠️  Could not delete folder (too busy?): {}", e),
                }
            }
        }
    }

    Ok(())
}

fn unlock_vault(vault_path_str: &str) -> Result<(PathBuf, MasterKey)> {
    let vault_path = PathBuf::from(vault_path_str);
    let salt_path = vault_path.join("salt.loader");

    if !salt_path.exists() {
        anyhow::bail!("Invalid vault path: {:?}. (Did you run 'lethe init'?)", vault_path);
    }

    let password = rpassword::prompt_password("Enter Vault Password: ")?;
    let salt = fs::read_to_string(salt_path)?;

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

    let data = fs::read(path).context("Failed to read input file")?;
    let size = data.len() as u64;

    let block_id = block_mgr.write_block(&data, key)?;
    index_mgr.add_file(dest.to_string(), vec![block_id], size);

    println!("OK");
    Ok(())
}
