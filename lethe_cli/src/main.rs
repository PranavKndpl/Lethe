use clap::{Parser, Subcommand};
use anyhow::{Result, Context};
use std::path::PathBuf;
use std::fs;
use std::io::{self, Write};

use lethe_core::crypto::CryptoEngine;
use lethe_core::storage::BlockManager;
use lethe_core::index::IndexManager;

#[derive(Parser)]
#[command(name = "lethe")]
#[command(about = "Lethe: Secure Distributed Vault", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new vault
    Init {
        #[arg(short, long)]
        path: String,
    },
    /// Upload a file
    Put {
        #[arg(short, long)]
        file: PathBuf,
        #[arg(short, long)]
        dest: String,
        #[arg(long)]
        vault: String,
    },
    /// List files
    Ls {
        #[arg(long)]
        vault: String,
    },
    /// Download a file
    Get {
        #[arg(short, long)]
        src: String,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long)]
        vault: String,
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { path } => {
            let vault_path = PathBuf::from(path);
            if vault_path.exists() {
                anyhow::bail!("Vault path already exists!");
            }
            
            println!("Initializing vault at: {}", path);
            let password = rpassword::prompt_password("Set Master Password: ")?;
            let confirm = rpassword::prompt_password("Confirm Password: ")?;
            
            if password != confirm {
                anyhow::bail!("Passwords do not match.");
            }

            fs::create_dir_all(&vault_path)?;

            // 1. Generate Key & Salt
            println!("Generating keys...");
            let (key, salt) = CryptoEngine::derive_key(&password)?;

            // 2. Save Salt (Plaintext) so we can unlock later
            // The Index is encrypted, so we can't read the salt from inside it without the key!
            fs::write(vault_path.join("salt.loader"), &salt)?;

            // 3. Create & Save Index
            let mut index_mgr = IndexManager::new_empty(vault_path.clone(), salt);
            index_mgr.save(&key)?;

            // 4. Init Storage
            let _ = BlockManager::new(&vault_path)?;

            println!("✅ Vault initialized.");
        }

        Commands::Put { file, dest, vault } => {
            let (vault_path, key) = unlock_vault(vault)?;
            let mut index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

            print!("Encrypting & Uploading... ");
            io::stdout().flush()?;
            
            let data = fs::read(file).context("Failed to read input file")?;
            let size = data.len() as u64;
            
            let block_id = block_mgr.write_block(&data, &key)?;
            
            // For V1, we assume 1 file = 1 block. 
            // In V2, you would chunk this loop.
            index_mgr.add_file(dest.clone(), vec![block_id], size);
            index_mgr.save(&key)?;

            println!("✅ Done! Saved as {}", dest);
        }

        Commands::Ls { vault } => {
            let (vault_path, key) = unlock_vault(vault)?;
            let index_mgr = IndexManager::load(vault_path, &key)?;
            
            println!("\n📂 Vault Contents:");
            println!("{:<12} | {:<40}", "SIZE (B)", "PATH");
            println!("{:-<55}", "-");
            for (path, entry) in &index_mgr.data.files {
                println!("{:<12} | {}", entry.size, path);
            }
            println!();
        }

        Commands::Get { src, out, vault } => {
            let (vault_path, key) = unlock_vault(vault)?;
            let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
            let block_mgr = BlockManager::new(&vault_path)?;

            if let Some(entry) = index_mgr.get_file(src) {
                // Reassemble blocks
                let mut full_data = Vec::new();
                for block_id in &entry.blocks {
                    let mut chunk = block_mgr.read_block(block_id, &key)?;
                    full_data.append(&mut chunk);
                }
                
                fs::write(out, full_data)?;
                println!("✅ Decrypted and saved to {:?}", out);
            } else {
                anyhow::bail!("File not found: {}", src);
            }
        }
    }
    Ok(())
}

fn unlock_vault(vault_path_str: &str) -> Result<(PathBuf, lethe_core::crypto::MasterKey)> {
    let vault_path = PathBuf::from(vault_path_str);
    let salt_path = vault_path.join("salt.loader");
    
    if !salt_path.exists() {
        anyhow::bail!("Invalid vault: missing 'salt.loader'");
    }

    let password = rpassword::prompt_password("Enter Vault Password: ")?;
    let salt = fs::read_to_string(salt_path)?;
    
    // Trim salt to avoid newline issues
    let (key, _) = CryptoEngine::derive_key_with_salt(&password, salt.trim())?;
    
    Ok((vault_path, key))
}