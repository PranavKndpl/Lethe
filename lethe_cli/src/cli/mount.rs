// lethe_cli/src/cli/mount.rs
use anyhow::Result;
use std::process::Command;
use log::error;
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use crate::dav::{LetheWebDav, LetheState};
use crate::cli::ops::{resolve_vault_path, unlock_vault};

pub async fn do_mount(vault: Option<String>, mountpoint: Option<String>) -> Result<()> {
    let vault_path = resolve_vault_path(vault.as_deref())?;

    // 1. Unlock logic happens BEFORE state creation now
    println!("🔐 Lethe Daemon Initialized.");
    
    // We block here to prompt for password
    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault_path.to_str().unwrap()))?;
    let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
    let block_mgr = BlockManager::new(&vault_path)?;

    // 2. Create the populated state immediately
    let state = LetheState::new(index_mgr, block_mgr, key);
    println!("🔓 Vault Unlocked.");

    // 3. Start Server
    let lethe_fs = LetheWebDav { state }; // No Arc wrapper needed anymore
    
    let dav_server = dav_server::DavHandler::builder()
        .filesystem(Box::new(lethe_fs))
        // FIX: Import MemLs from the root, not fs::
        .locksystem(dav_server::memls::MemLs::new()) 
        .build_handler();

    let port = 4918;
    let addr = ([127, 0, 0, 1], port);
    
    let server_handle = tokio::spawn(async move {
        warp::serve(dav_server::warp::dav_handler(dav_server))
            .run(addr)
            .await;
    });
    println!("🚀 Server running at http://127.0.0.1:{}", port);

    // 4. Windows Mount & Guard
    #[cfg(target_os = "windows")]
    {
        let drive_letter = mountpoint.unwrap_or_else(|| "Z:".to_string());
        
        // Clean up previous mounts
        let _ = Command::new("net").args(&["use", &drive_letter, "/delete", "/y"]).status();
        
        // Mount
        let status = Command::new("net")
            .args(&["use", &drive_letter, &format!("http://127.0.0.1:{}", port)])
            .status()?;

        if status.success() {
            println!("✅ Mounted to {}.", drive_letter);
            
            // Rename drive in Explorer
            let _ = Command::new("powershell")
                .args(&["-Command", &format!("$sh=New-Object -ComObject Shell.Application;$sh.NameSpace('{}').Self.Name='Lethe Vault'", drive_letter)])
                .status();
                
            let _ = Command::new("explorer").arg(&drive_letter).spawn();
        } else {
            error!("Mount failed.");
            return Ok(());
        }

        println!("running... (Press Ctrl+C to Lock & Quit)");
        tokio::signal::ctrl_c().await?;
        
        println!("\n🛑 Shutdown signal received.");
        // Unmount
        let _ = Command::new("net").args(&["use", &drive_letter, "/delete", "/y"]).status();
    }

    #[cfg(not(target_os = "windows"))]
    {
         println!("ℹ️  Unix support in CLI mode is manual. Connect to http://127.0.0.1:{}", port);
         tokio::signal::ctrl_c().await?;
    }

    server_handle.abort();
    Ok(())
}

pub fn do_panic() -> Result<()> {
    #[cfg(target_os = "windows")]
    for drive in ["Z:", "Y:", "X:"] {
        let _ = Command::new("net").args(&["use", drive, "/delete", "/y"]).status();
    }
    Ok(())
}