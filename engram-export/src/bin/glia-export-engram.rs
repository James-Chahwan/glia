//! `glia-export-engram` — write a resolved glia graph as an `engram_core::Gmap`
//! bincode file (engram's Path-A seed) plus a `<out>.files.json` span sidecar,
//! and record the glia graph it exported in a `<out>.glia/` directory beside
//! them (the LC.9 layout, the one `glia build` and the MCP read).
//!
//! Lives here, not in the main `glia` CLI, on purpose: this is the ONLY code
//! path that touches `engram-core` in the sibling `Engram` repo (a `../../`
//! path dep that doesn't exist in CI or a fresh clone). Keeping it in its own
//! excluded crate lets the engine workspace — and the published `glia-py`
//! wheel build — stay free of that cross-repo dependency. For the same reason
//! `cargo run -p glia-engram-export` from the glia root cannot find it; build
//! it with the check script (Engram checked out next to glia) and run the
//! binary it leaves behind:
//!
//! ```text
//! bash scripts/check-engram-export.sh
//! engram-export/target/debug/glia-export-engram <repo> --out <file> \
//!     [--since <prior>] [--include-noise] [--exclude <glob>]...
//! ```
//!
//! `--since <prior>` names the gmap the Engram store last applied (a full v6
//! export; it may be `--out` itself — everything prior is read before anything
//! is written). The run pairs files moved since then against the graph recorded
//! in `<prior>.glia/` (LB.6 `detect_moves`) and carries each file's
//! identity-hint token forward, so a moved file keeps its facts' identity. A
//! prior this build cannot read is refused (exit 6); a missing or outdated
//! `<prior>.glia/` only means moves go undetected this run. Keep `--out`
//! outside the exported repo, and move `<out>.glia/` with the gmap.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Parser;
use engram_core::GMAP_FORMAT_VERSION;
use glia_engine::generate_one;
use glia_engine::persist::{load_layout, persist_result};
use glia_engram_export::diff::read_gmap;
use glia_engram_export::{
    ExportOptions, export_engram_gmap, history_dir, position_paths, prior_tokens, sidecar_path,
};
use glia_graph::identity::{MoveMap, carry_file_tokens, detect_moves};

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
    /// The gmap the Engram store last applied: carry file identities across
    /// the moves since then (reads `<since>.glia/`; may equal --out).
    #[arg(long)]
    since: Option<String>,
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

/// What `--since` read before anything was written.
struct Prior {
    /// `path -> file token` read out of the prior gmap's hints.
    tokens: BTreeMap<String, String>,
    /// The graph recorded beside the prior gmap, when it loads.
    graph: Option<glia_graph::MergedGraph>,
}

/// Read the `--since` prior: its gmap (refused -> `Err(exit code)`) and its
/// recorded graph (a failure is a warning: no moves this run).
fn read_prior(since: &Path) -> Result<Prior, i32> {
    let (prior, _base_digest) = match read_gmap(since) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[engram-export] v6 since: refused - {}: {e}", since.display());
            return Err(6);
        }
    };
    let dir = history_dir(since);
    let graph = match load_layout(&dir) {
        Ok(r) => Some(r.merged),
        Err(e) => {
            // `reason` leads with the dir and carries no rebuild advice: the
            // export itself rebuilds nothing here.
            let at = format!("{}: ", dir.display());
            eprintln!(
                "[engram-export] v6 identity: prior graph unavailable at {} ({}) - moves not detected this run",
                dir.display(),
                e.reason.strip_prefix(&at).unwrap_or(&e.reason),
            );
            None
        }
    };
    Ok(Prior { tokens: prior_tokens(&prior), graph })
}

fn run(args: &Args) -> i32 {
    let prior = match args.since.as_deref().map(|s| read_prior(Path::new(s))) {
        Some(Ok(p)) => Some(p),
        Some(Err(code)) => return code,
        None => None,
    };
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
            PathBuf::from(format!("{name}.engram-gmap"))
        }
    };
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("error creating {}: {e}", parent.display());
        return 4;
    }

    // File identity: carry the prior's tokens across the moves LB.6 finds
    // between the recorded prior graph and this build. Without a recorded
    // graph nothing moves, but files that stayed still keep their tokens.
    let (file_identity, moves, carried) = match &prior {
        Some(p) => {
            let moves = p
                .graph
                .as_ref()
                .map_or_else(MoveMap::default, |g| detect_moves(g, &result.merged));
            let current: Vec<String> = position_paths(&result.merged).into_iter().collect();
            let tokens = carry_file_tokens(&p.tokens, &moves, &current);
            // Under a prior identity: a token that is not the path and came
            // from the prior (a fresh `<path>#N` never did).
            let from_prior: BTreeSet<&String> = p.tokens.values().collect();
            let carried =
                tokens.iter().filter(|(path, t)| path != t && from_prior.contains(t)).count();
            (tokens, moves.files.len(), carried)
        }
        None => Default::default(),
    };
    let opts = ExportOptions {
        include_noise: args.include_noise,
        exclude: args.exclude.clone(),
        file_identity,
    };
    let stats = match export_engram_gmap(&result.merged, repo_root, &out_path, &opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error writing {}: {e}", out_path.display());
            return 5;
        }
    };
    // The graph this gmap was exported from, for the next `--since` run. The
    // gmap is complete without it, so a failure is a warning.
    let history = history_dir(&out_path);
    if let Err(e) = persist_result(&result, &history, "glia-export-engram") {
        eprintln!(
            "[engram-export] warning: history not written to {}: {e} - the next --since run cannot detect moves",
            history.display()
        );
    }
    // The grep-able success marker: the contract version the bytes follow and
    // their content address (what a `GmapDiff` names as base / target).
    eprintln!(
        "[engram-export] wrote {} nodes + {} edges ({} files) to {} format_version={} digest={:016x}\n  span sidecar: {}\n  history: {}",
        stats.nodes,
        stats.edges,
        stats.files,
        out_path.display(),
        GMAP_FORMAT_VERSION,
        stats.digest,
        sidecar_path(&out_path).display(),
        history.display(),
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
    // The v6 identity marker (LG.9): hint coverage, files whose hints carry a
    // token from before a move, and keys several nodes shared.
    eprintln!(
        "[engram-export] v6 identity: {}/{} hints, {carried} file(s) under a prior identity ({moves} move(s) detected), {} duplicate key(s) resolved to the located node",
        stats.identity_hints, stats.nodes, stats.duplicate_keys,
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
