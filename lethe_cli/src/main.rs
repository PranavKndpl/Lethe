mod cli;

// Only compile the WebDAV module on Windows
#[cfg(windows)]
mod dav;

// Only compile the FUSE module on Unix
#[cfg(unix)]
mod fs_fuse;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => cli::ops::do_init(path),
        Commands::Put { file, dest, vault } => cli::ops::do_put(file, dest, vault),
        Commands::Ls { vault } => cli::ops::do_ls(vault),
        Commands::Get { src, out, vault } => cli::ops::do_get(src, out, vault),
        Commands::Repair { vault } => cli::ops::do_repair(vault),
        Commands::Mount { vault, mountpoint } => cli::mount::do_mount(vault, mountpoint).await,
        Commands::Panic => cli::mount::do_panic(),
        Commands::Clean { vault, dry_run } => cli::ops::do_clean(vault, dry_run),
    }
}