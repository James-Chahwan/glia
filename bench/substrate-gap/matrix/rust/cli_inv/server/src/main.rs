use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mytool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Sync,
}

fn main() {
    let _cli = Cli::parse();
}
