//! `glia-export-engram` — write a resolved glia graph as an `engram_core::Gmap`
//! bincode file (engram's Path-A seed) plus a `<out>.files.json` span sidecar.
//!
//! Lives here, not in the main `glia` CLI, on purpose: this is the ONLY code
//! path that touches `engram-core` in the sibling `Engram` repo (a `../../`
//! path dep that doesn't exist in CI or a fresh clone). Keeping it in its own
//! excluded crate lets the engine workspace — and the published `glia-py`
//! wheel build — stay free of that cross-repo dependency.
//!
//! Run it from this crate when `Engram` is checked out next to `glia`:
//!   cargo run -p glia-engram-export --bin glia-export-engram -- \
//!       <repo> [--out <file>] [--include-noise] [--exclude <glob>]...

use std::path::Path;

use clap::Parser;
use engram_core::GMAP_FORMAT_VERSION;
use glia_engine::generate_one;
use glia_engram_export::{ExportOptions, export_engram_gmap, sidecar_path};

#[derive(Parser, Debug)]
#[command(
    name = "glia-export-engram",
    version,
    about = "Export a resolved glia graph as an engram_core::Gmap bincode seed (+ span sidecar)."
)]
struct Args {
    /// Path to the repo root.
    repo: String,
    /// Output file. Defaults to `<project-name>.engram-gmap` in the cwd.
    #[arg(long)]
    out: Option<String>,
    /// Keep substrate-only synthetic nodes (npm deps, event names, generated
    /// stubs) that are filtered from the export by default.
    #[arg(long)]
    include_noise: bool,
    /// Drop nodes whose key matches this glob (`*` wildcard). Repeatable.
    #[arg(long = "exclude")]
    exclude: Vec<String>,
}

fn main() {
    let args = Args::parse();
    std::process::exit(run(&args));
}

fn run(args: &Args) -> i32 {
    let result = match generate_one(&args.repo) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let repo_root = Path::new(&args.repo);
    let out_path = match &args.out {
        Some(p) => Path::new(p).to_path_buf(),
        None => {
            let name = glia_code_domain::project_roots::project_name(repo_root)
                .unwrap_or_else(|| "repo".to_string());
            std::path::PathBuf::from(format!("{name}.engram-gmap"))
        }
    };
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("error creating {}: {e}", parent.display());
                return 4;
            }
        }
    }
    let opts = ExportOptions {
        include_noise: args.include_noise,
        exclude: args.exclude.clone(),
    };
    let stats = match export_engram_gmap(&result.merged, repo_root, &out_path, &opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error writing {}: {e}", out_path.display());
            return 5;
        }
    };
    // The grep-able success marker: the contract version the bytes follow and
    // their content address (what a `GmapDiff` names as base / target).
    eprintln!(
        "[engram-export] wrote {} nodes + {} edges ({} files) to {} format_version={} digest={:016x}\n  span sidecar: {}",
        stats.nodes,
        stats.edges,
        stats.files,
        out_path.display(),
        GMAP_FORMAT_VERSION,
        stats.digest,
        sidecar_path(&out_path).display(),
    );
    // The v6 span marker (LG.10): how many positioned nodes got 1-based lines
    // and how many doc Propositions got a source anchor. A healthy export has
    // both pairs equal; an unreadable file still keeps its lines (bytes 0..0).
    eprintln!(
        "[engram-export] v6 spans: {}/{} positioned nodes carry lines, {}/{} propositions anchored, {} unreadable file(s)",
        stats.spans_with_lines,
        stats.positioned,
        stats.propositions_anchored,
        stats.propositions,
        stats.unreadable_files,
    );
    if stats.skipped_nodes > 0 || stats.duplicate_keys > 0 || stats.skipped_edges > 0 {
        eprintln!(
            "  skipped: {} unqualified nodes, {} duplicate keys, {} edges",
            stats.skipped_nodes, stats.duplicate_keys, stats.skipped_edges
        );
    }
    if stats.dropped_noise > 0 || stats.dropped_excluded > 0 {
        eprintln!(
            "  filtered: {} synthetic noise node(s){}, {} by --exclude",
            stats.dropped_noise,
            if args.include_noise { " (kept: --include-noise)" } else { "" },
            stats.dropped_excluded,
        );
    }
    if stats.unreadable_files > 0 {
        eprintln!(
            "  warning: {} POSITION file(s) unreadable under {} — those nodes' spans carry lines but no bytes (0..0)",
            stats.unreadable_files,
            repo_root.display()
        );
    }
    if !result.parse_errors.is_empty() {
        eprintln!("(plus {} parse errors)", result.parse_errors.len());
    }
    0
}
