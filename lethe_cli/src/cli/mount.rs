use anyhow::Result;
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use crate::cli::ops::{resolve_vault_path, unlock_vault};
use std::path::PathBuf;

// --- Platform Specific Imports ---
#[cfg(windows)]
use crate::dav::{LetheWebDav, LetheState};
#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
use log::error;

#[cfg(unix)]
use crate::fs_fuse::LetheFS;
#[cfg(unix)]
use std::collections::HashMap;

pub async fn do_mount(vault: Option<String>, mountpoint: Option<String>) -> Result<()> {
    let vault_path = resolve_vault_path(vault.as_deref())?;

    println!("Lethe Daemon Initialized.");
    
    // 1. Shared Unlock Logic (Same for both platforms)
    // We assume this is a blocking operation prompting for password
    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault_path.to_str().unwrap()))?;
    
    // Load Index & Storage
    let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
    let block_mgr = BlockManager::new(&vault_path)?;
    println!("Vault Unlocked.");

    // =========================================================
    //  WINDOWS EXECUTION PATH (WebDAV)
    // =========================================================
    #[cfg(target_os = "windows")]
    {
        // 1. Prepare State
        let state = LetheState::new(index_mgr, block_mgr, key);
        let lethe_fs = LetheWebDav { state };
        
        let dav_server = dav_server::DavHandler::builder()
            .filesystem(Box::new(lethe_fs))
            .locksystem(dav_server::memls::MemLs::new()) 
            .build_handler();

        let port = 4918;
        let addr = ([127, 0, 0, 1], port);
        
        // 2. Start Server
        let server_handle = tokio::spawn(async move {
            warp::serve(dav_server::warp::dav_handler(dav_server))
                .run(addr)
                .await;
        });
        println!("WebDAV Server running at http://127.0.0.1:{}", port);

        // 3. Mount Drive
        let drive_letter = mountpoint.unwrap_or_else(|| "Z:".to_string());
        
        // Cleanup old mounts silently
        let _ = Command::new("net").args(&["use", &drive_letter, "/delete", "/y"])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
        
        let status = Command::new("net")
            .args(&["use", &drive_letter, &format!("http://127.0.0.1:{}", port)])
            .stdout(Stdio::null())
            .status()?;

        if status.success() {
            println!("Mounted to {}.", drive_letter);
            // Rename Drive
            let _ = Command::new("powershell")
                .args(&["-Command", &format!("$sh=New-Object -ComObject Shell.Application;$sh.NameSpace('{}').Self.Name='Lethe Vault'", drive_letter)])
                .stdout(Stdio::null()).stderr(Stdio::null()).status();
            
            // Open Explorer
            let _ = Command::new("explorer").arg(&drive_letter).spawn();
        } else {
            error!("Mount failed.");
            return Ok(());
        }

        println!("   (Press Ctrl+C to Lock & Quit)");
        tokio::signal::ctrl_c().await?;
        
        println!("\nVault Locked.");
        let _ = Command::new("net").args(&["use", &drive_letter, "/delete", "/y"])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
        
        server_handle.abort();
    }

    // =========================================================
    //  LINUX / MACOS EXECUTION PATH (FUSE)
    // =========================================================
    #[cfg(unix)]
    {
        let mount_path = mountpoint.map(PathBuf::from).unwrap_or_else(|| {
             // Default mountpoint logic for Linux
             let home = dirs::home_dir().unwrap();
             home.join("LetheMount")
        });

        // Ensure mount directory exists
        if !mount_path.exists() {
            std::fs::create_dir_all(&mount_path)?;
        }

        println!("Mounting FUSE filesystem at {:?}", mount_path);
        println!("   (Press Ctrl+C to unmount)");

        let mut inode_map = HashMap::new();
        inode_map.insert(1, "/".to_string());

        // Initialize the LetheFS struct
        let fs = LetheFS {
            index: index_mgr,
            storage: block_mgr,
            key: key,
            inode_map,
            write_buffer: HashMap::new(),
        };

        // Standard FUSE mount options
        let options = vec![
            fuser::MountOption::RW,
            fuser::MountOption::FSName("lethe".to_string()),
            fuser::MountOption::AutoUnmount,
            fuser::MountOption::AllowOther,
        ];

        // This call blocks until the filesystem is unmounted (Ctrl+C)
        fuser::mount2(fs, &mount_path, &options)?;
        
        println!("\nUnmounted successfully.");
    }

    Ok(())
}

pub fn do_panic() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        for drive in ["Z:", "Y:", "X:"] {
            let _ = std::process::Command::new("net")
                .args(&["use", drive, "/delete", "/y"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        println!("Panic Cleanup: Attempted to unmount Z:, Y:, X:");
    }

    #[cfg(unix)]
    {
        println!("Panic command is a Windows-specific cleanup tool.");
        println!("On Unix, FUSE handles auto-unmount.");
        println!("If stuck, try: fusermount -u <path>");
    }

    Ok(())
}
