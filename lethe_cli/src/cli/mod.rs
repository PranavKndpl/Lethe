use clap::{Parser, Subcommand};
use std::path::PathBuf;

// --- EXPORT SUBMODULES ---
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
    Init { 
        #[arg(short, long)] path: Option<String> 
    },
    Put { 
        #[arg(short, long)] file: PathBuf, 
        #[arg(short, long)] dest: String, 
        #[arg(long)] vault: String 
    },
    Ls { 
        #[arg(long)] vault: String 
    },
    Get { 
        #[arg(short, long)] src: String, 
        #[arg(short, long)] out: PathBuf, 
        #[arg(long)] vault: String 
    },
    Mount { 
        #[arg(long)] vault: Option<String>, 
        #[arg(long)] mountpoint: Option<String> 
    },
    Repair { 
        #[arg(long)] vault: String 
    },
    Panic,
}