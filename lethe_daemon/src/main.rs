use anyhow::Result;
use rdev::{listen, Event, EventType, Key};

fn main() -> Result<()> {
    println!("Lethe Daemon started. Waiting for trigger (Ctrl+Alt+])...");
    
    // In a real app, you run this in a thread so it doesn't block everything
    if let Err(error) = listen(callback) {
        println!("Error: {:?}", error)
    }

    Ok(())
}

fn callback(event: Event) {
    // This is a VERY basic example. 
    // You will need state management to detect the combo (Ctrl+Alt+])
    match event.event_type {
        EventType::KeyPress(Key::RightBracket) => {
             println!("Trigger detected! (Logic to check modifiers needed)");
             // spawn_mount_process();
        },
        _ => (),
    }
}