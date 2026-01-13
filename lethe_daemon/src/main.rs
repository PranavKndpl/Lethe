use rdev::{listen, Event, EventType, Key};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod filesystem;

#[cfg(target_os = "windows")]
use winfsp::host::FileSystemHost;

// ---------------- Thread-safe wrapper ----------------
#[cfg(target_os = "windows")]
struct SafeFileSystemHost(FileSystemHost<'static>);

#[cfg(target_os = "windows")]
unsafe impl Send for SafeFileSystemHost {}
#[cfg(target_os = "windows")]
unsafe impl Sync for SafeFileSystemHost {}
// -----------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum VaultState {
    Locked,
    Mounted,
}

struct AppState {
    vault_state: VaultState,
    ctrl_pressed: bool,
    alt_pressed: bool,

    #[cfg(target_os = "windows")]
    mount_host: Option<SafeFileSystemHost>,
}

fn main() {
    println!("👁️  Lethe Sentinel Started.");
    println!("    Waiting for signal: [ Ctrl + Alt + ] ]");

    let state = Arc::new(Mutex::new(AppState {
        vault_state: VaultState::Locked,
        ctrl_pressed: false,
        alt_pressed: false,
        #[cfg(target_os = "windows")]
        mount_host: None,
    }));

    let listener_state = state.clone();

    if let Err(error) = listen(move |event| {
        handle_event(event, &listener_state);
    }) {
        eprintln!("Listener error: {:?}", error);
    }
}

fn handle_event(event: Event, state_mutex: &Arc<Mutex<AppState>>) {
    let mut state = state_mutex.lock().unwrap();

    match event.event_type {
        EventType::KeyPress(key) => match key {
            Key::ControlLeft | Key::ControlRight => state.ctrl_pressed = true,
            Key::Alt | Key::AltGr => state.alt_pressed = true,
            Key::RightBracket => {
                if state.ctrl_pressed && state.alt_pressed {
                    let state_clone = state_mutex.clone();
                    thread::spawn(move || toggle_vault(state_clone));
                }
            }
            _ => {}
        },

        EventType::KeyRelease(key) => match key {
            Key::ControlLeft | Key::ControlRight => state.ctrl_pressed = false,
            Key::Alt | Key::AltGr => state.alt_pressed = false,
            _ => {}
        },

        _ => {}
    }
}

fn toggle_vault(state_mutex: Arc<Mutex<AppState>>) {
    // debounce
    thread::sleep(Duration::from_millis(200));

    let mut state = state_mutex.lock().unwrap();

    match state.vault_state {
        VaultState::Locked => {
            println!("\n🔓 SIGNAL RECEIVED: UNLOCK SEQUENCE INITIATED...");

            #[cfg(target_os = "windows")]
            match filesystem::mount_vault("Z:") {
                Ok(host) => {
                    println!("   [SUCCESS] Vault mounted at Z:\\");
                    state.mount_host = Some(SafeFileSystemHost(host));
                    state.vault_state = VaultState::Mounted;
                }
                Err(e) => {
                    eprintln!("   [ERROR] Mount failed: {:?}", e);
                }
            }
        }

        VaultState::Mounted => {
            println!("\n🔒 SIGNAL RECEIVED: LOCKDOWN SEQUENCE...");

            #[cfg(target_os = "windows")]
            if let Some(mut safe_host) = state.mount_host.take() {
                safe_host.0.unmount();
                println!("   [SUCCESS] Vault unmounted.");
            }

            state.vault_state = VaultState::Locked;
        }
    }
}
