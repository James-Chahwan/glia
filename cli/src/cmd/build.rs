//! `glia build` — walk a repo and write its graph layout (manifest.json +
//! shards + cross_stack.gmap) to `<repo>/.glia/graph/` (or `--out`), through
//! the engine's single writer `persist::persist_result` (LC.9): the directory
//! pyo3's `default_gmap_dir`, `load_from_gmap` and the MCP read, refreshed by
//! the `install-hooks` hooks (`glia build .`).

use std::path::PathBuf;

use repo_graph_engine::persist::{default_layout_dir, persist_result};
use repo_graph_engine::{generate_one, generate_one_incremental};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Layout directory to write (manifest.json + shards + cross_stack.gmap).
    /// Defaults to `<repo>/.glia/graph`, the one the MCP server reads.
    #[arg(long)]
    out: Option<String>,
    /// Force a full reparse, ignoring the incremental parse cache (WP-D).
    #[arg(long)]
    no_incremental: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let built = if args.no_incremental {
        // An explicit clean build also discards the sidecar — otherwise the
        // next default-on build would reuse the cache the user was escaping.
        if let Err(e) = repo_graph_engine::ParseCache::purge(repo) {
            eprintln!("warning: could not remove parse cache: {e}");
        }
        generate_one(repo)
    } else {
        generate_one_incremental(repo)
    };
    let result = match built {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let out_dir = match args.out {
        Some(p) => PathBuf::from(p),
        None => default_layout_dir(std::path::Path::new(repo)),
    };
    if let Err(e) = persist_result(&result, &out_dir, "cli") {
        eprintln!("error: {e}");
        return 5;
    }
    if !result.parse_errors.is_empty() {
        eprintln!("(plus {} parse errors)", result.parse_errors.len());
    }
    0
}
