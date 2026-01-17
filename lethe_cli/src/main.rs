use clap::{Parser, Subcommand};
use anyhow::Result;
use std::path::PathBuf;
use std::fs;
use std::process::Command;
use lethe_core::crypto::{CryptoEngine, MasterKey};
use lethe_core::storage::BlockManager;
use lethe_core::index::IndexManager;
use std::sync::Arc;

// ADDED
use walkdir::WalkDir;
use std::io::{self, Write};

mod fs_webdav;

// MAIN MUST BE ASYNC NOW
#[tokio::main]
async fn main() -> Result<()> {
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
                    let entry: walkdir::DirEntry = entry?;
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

        Commands::Mount { vault, mountpoint } => {
            let vault_path = match vault {
                Some(v) => PathBuf::from(v),
                None => dirs::home_dir().unwrap().join(".lethe_vault"),
            };

            println!("🔓 Unlocking vault at {:?}...", vault_path);

            let (vault_path, key) =
                tokio::task::block_in_place(|| unlock_vault(vault_path.to_str().unwrap()))?;

            let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

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
            println!("🚀 Lethe Server running at http://127.0.0.1:{}", port);

            tokio::spawn(async move {
                warp::serve(dav_server::warp::dav_handler(dav_server))
                    .run(addr)
                    .await;
            });

            #[cfg(target_os = "windows")]
            {
                let drive = mountpoint.clone().unwrap_or_else(|| "Z:".to_string());
                println!("🔄 Mounting to Drive {}...", drive);

                let _ = Command::new("net").args(&["use", &drive, "/delete", "/y"]).output();

                let status = Command::new("net")
                    .args(&["use", &drive, &format!("http://127.0.0.1:{}", port)])
                    .status()?;

                if status.success() {
                    println!("✅ Mounted! Opening Explorer...");
                    let _ = Command::new("explorer").arg(&drive).spawn();
                } else {
                    eprintln!("⚠️ Auto-mount failed. You can manually map network drive to http://127.0.0.1:{}", port);
                }

                tokio::signal::ctrl_c().await?;
                println!("🛑 Unmounting...");
                let _ = Command::new("net").args(&["use", &drive, "/delete", "/y"]).output();
            }

            #[cfg(target_os = "linux")]
            {
                println!("🐧 Linux detected.");
                println!("   Opening file manager at dav://127.0.0.1:{}", port);
                let _ = Command::new("xdg-open")
                    .arg(format!("dav://127.0.0.1:{}", port))
                    .spawn();

                println!("   (Press Ctrl+C to stop the server)");
                tokio::signal::ctrl_c().await?;
            }
        }

        _ => {}
    }

    Ok(())
}

// Structs definitions needed for compilation
#[derive(Parser)]
#[command(name = "lethe")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init { #[arg(short, long)] path: Option<String> },
    Put { #[arg(short, long)] file: PathBuf, #[arg(short, long)] dest: String, #[arg(long)] vault: String },
    Ls { #[arg(long)] vault: String },
    Mount { #[arg(long)] vault: Option<String>, #[arg(long)] mountpoint: Option<String> },
    Panic,
}

// Copy your unlock_vault helper here exactly as it was
fn unlock_vault(vault_path_str: &str) -> Result<(PathBuf, MasterKey)> {
    let vault_path = PathBuf::from(vault_path_str);
    let salt_path = vault_path.join("salt.loader");
    if !salt_path.exists() { anyhow::bail!("Invalid vault path"); }
    let password = rpassword::prompt_password("Enter Vault Password: ")?;
    let salt = fs::read_to_string(salt_path)?;
    let (key, _) = CryptoEngine::derive_key_with_salt(&password, salt.trim())?;
    Ok((vault_path, key))
}

// RESTORED HELPER
fn upload_file(
    path: &std::path::Path,
    dest: &str,
    block_mgr: &BlockManager,
    index_mgr: &mut IndexManager,
    key: &MasterKey
) -> Result<()> {
    print!("Processing {} ... ", path.display());
    io::stdout().flush()?;

    let data = fs::read(path)?;
    let size = data.len() as u64;

    let block_id = block_mgr.write_block(&data, key)?;
    index_mgr.add_file(dest.to_string(), vec![block_id], size);

    println!("OK");
    Ok(())
}
