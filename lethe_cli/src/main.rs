use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Parser)]
#[command(name = "lethe")]
#[command(about = "Distributed Hidden Filesystem", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new vault
    Init {
        /// Path to store the blocks
        #[arg(short, long)]
        path: String,
    },
    /// Manually mount the vault (debug mode)
    Mount {
        /// Path to the vault storage
        #[arg(short, long)]
        path: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init { path } => {
            println!("Initializing vault at: {}", path);
            // logic will go here: lethe_core::init(path, password)...
        }
        Commands::Mount { path } => {
            println!("Mounting vault from: {}", path);
        }
    }

    Ok(())
}