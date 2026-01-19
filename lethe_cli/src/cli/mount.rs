// lethe_cli/src/cli/mount.rs
use anyhow::Result;
use std::sync::Arc;
use std::process::Command;
use log::{error, info};
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use crate::dav::{LetheWebDav, LetheState};
use crate::cli::ops::{resolve_vault_path, unlock_vault};

// Guard to unmount on crash/exit
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

pub async fn do_mount(vault: Option<String>, mountpoint: Option<String>) -> Result<()> {
    let vault_path = resolve_vault_path(vault.as_deref())?;
    println!("🔓 Unlocking vault at {:?}...", vault_path);

    // 1. Interactive Unlock
    // Note: In a future daemon version, we wouldn't unlock immediately here.
    let (vault_path, key) = tokio::task::block_in_place(|| unlock_vault(vault_path.to_str().unwrap()))?;
    
    // 2. Load Core Resources
    let index_mgr = IndexManager::load(vault_path.clone(), &key)?;
    let block_mgr = BlockManager::new(&vault_path)?;

    // 3. Initialize Global State
    let state = Arc::new(LetheState::new());
    state.unlock(index_mgr, block_mgr, key).await;

    // 4. Setup WebDAV Server
    let lethe_fs = LetheWebDav { state: state.clone() };
    let dav_server = dav_server::DavHandler::builder()
        .filesystem(Box::new(lethe_fs))
        .build_handler();

    let port = 4918;
    let addr = ([127, 0, 0, 1], port);
    println!("🚀 Lethe Server starting at http://127.0.0.1:{}", port);

    // 5. Spawn Server
    let server_handle = tokio::spawn(async move {
        warp::serve(dav_server::warp::dav_handler(dav_server))
            .run(addr)
            .await;
    });

    // 6. Platform Specific Mount
    #[cfg(target_os = "windows")]
    {
        let drive_letter = mountpoint.clone().unwrap_or_else(|| "Z:".to_string());
        let _guard = MountGuard { drive: drive_letter.clone() }; // RAII cleanup

        println!("🔄 Mounting to Drive {}...", drive_letter);
        // Clean previous
        let _ = Command::new("net")
            .args(&["use", &drive_letter, "/delete", "/y"])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();

        // Mount
        let status = Command::new("net")
            .args(&["use", &drive_letter, &format!("http://127.0.0.1:{}", port)])
            .status()?;

        if status.success() {
            println!("✅ Mounted successfully!");
            // Cosmetic Rename
            let rename_script = format!(
                "$sh = New-Object -ComObject Shell.Application; $sh.NameSpace('{}').Self.Name = 'Lethe Vault'", 
                drive_letter
            );
            let _ = Command::new("powershell")
                .args(&["-NoProfile", "-Command", &rename_script])
                .stdout(std::process::Stdio::null())
                .status();
                
            let _ = Command::new("explorer").arg(&drive_letter).spawn();
        } else {
            error!("Mount failed.");
            eprintln!("⚠️  Could not map drive letter. Access manually via: http://127.0.0.1:{}", port);
        }

        println!("Press Ctrl+C to unmount and exit.");
        tokio::signal::ctrl_c().await?;
        println!("🛑 Stopping server...");
        // Guard drops here
    }

    #[cfg(target_os = "linux")]
    {
        println!("🐧 Linux detected.");
        let _ = Command::new("xdg-open").arg(format!("dav://127.0.0.1:{}", port)).spawn();
        println!("(Press Ctrl+C to stop)");
        tokio::signal::ctrl_c().await?;
        println!("🛑 Stopping server...");
    }

    server_handle.abort();
    Ok(())
}

pub fn do_panic() -> Result<()> {
    println!("🚨 PANIC SEQUENCE INITIATED 🚨");
    println!("   Forcing unmount of all Lethe drives...");
    
    #[cfg(target_os = "windows")]
    for drive in ["Z:", "Y:", "X:"] {
        let _ = Command::new("net")
            .args(&["use", drive, "/delete", "/y"])
            .stdout(std::process::Stdio::null())
            .status();
    }
    
    println!("✅ Cleanup commands sent.");
    Ok(())
}