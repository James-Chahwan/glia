use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "shopctl",
    version,
    about = "Shop admin"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync records
    SyncUsers { #[arg(long)] force: bool },
    #[command(name = "gc")]
    GarbageCollect,
}

fn main() { let _ = Cli::parse(); }
