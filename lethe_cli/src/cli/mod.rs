use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod ops;
pub mod mount;

#[derive(Parser)]
#[command(name = "lethe", about = "A serverless, encrypted, distributed filesystem.", version = "1.0.0")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new vault
    #[command(alias = "i")]
    Init { 
        /// Path to create vault (Defaults to ~/.lethe_vault)
        #[arg(short, long)] 
        path: Option<String> 
    },

    /// Mount the vault as a drive
    #[command(alias = "m")]
    Mount { 
        /// Path to vault (Defaults to ~/.lethe_vault)
        #[arg(short, long)] 
        vault: Option<String>, 
        
        /// Drive letter (Windows) or Mountpoint (Unix). Defaults to Z:
        #[arg(short, long)] 
        mountpoint: Option<String> 
    },

    Put { 
        #[arg(short, long)] file: PathBuf, 
        #[arg(short, long)] dest: String, 
        #[arg(long)] vault: String 
    },
    Ls { #[arg(long)] vault: String },
    Get { 
        #[arg(short, long)] src: String, 
        #[arg(short, long)] out: PathBuf, 
        #[arg(long)] vault: String 
    },
    Repair { #[arg(long)] vault: String },
    Panic,
    Clean {
        #[arg(long)] vault: String,
        #[arg(long, default_value_t = false)] dry_run: bool,
    },
}