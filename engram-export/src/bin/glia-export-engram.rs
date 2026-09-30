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
//!     [--since <prior>] [--no-persist] [--include-noise] \
//!     [--exclude <glob>]... [--exclude-path <glob>]...
//! ```
//!
//! `--exclude` drops nodes by KEY (the qname); `--exclude-path` drops every
//! node whose POSITION file (repo-relative, `/`-separated) matches, and leaves
//! those files out of the export's file table - the way to drop a directory
//! tree whose code is keyed by namespace, not path (C#, Java), without
//! removing it from glia's graph. In both, `*` matches any run of characters,
//! `/` included.
//!
//! The build is the persisted incremental one by default: the repo's parse
//! cache (`<repo>/.glia/graph/parse_cache.bin`, beside the layout; the file
//! pyo3 `generate(incremental=True)`, `glia build` and the MCP read and write)
//! is loaded, the unchanged files skip their parse, and the cache is saved
//! back. `--no-persist`, or `GLIA_NO_PERSIST=1`, builds clean in memory and
//! writes nothing into the repo. The gmap bytes are the same either way
//! (`engine/tests/byte_identical.rs`).
//!
//! `--since <prior>` names the gmap the Engram store last applied (a full v6
//! export; it may be `--out` itself — everything prior is read before anything
//! is written). The run pairs files moved since then against the graph recorded
//! in `<prior>.glia/` (LB.6 `detect_moves`) and carries each file's
//! identity-hint token forward, so a moved file keeps its facts' identity. A
//! prior this build cannot read is refused (exit 6); a missing or outdated
//! `<prior>.glia/` only means moves go undetected this run. Keep `--out`
//! outside the exported repo, and move `<out>.glia/` with the gmap.
//!
//! With `--since` the run also writes the G16 diff (LG.8a): file ids are
//! seeded from the prior's `files` table, so a surviving path keeps its id and
//! an untouched node keeps its bytes, and `<out>.diff` gets the bincode
//! `GmapDiff` from the prior (base) to this export (target), named by both
//! gmaps' content digests; with no change it is still written, with every list
//! empty. Write order: `<out>.diff` beside `--out` is deleted first on every
//! run, then the full gmap (+ sidecar) is written (the next run's base), then
//! the diff, then `<out>.glia/`. A run that stops between the gmap and the
//! diff leaves a full gmap and no diff (Engram re-seeds from it); a diff that
//! cannot be written is exit 7.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Parser;
use engram_core::{GMAP_FORMAT_VERSION, Gmap};
use glia_engine::persist::{load_layout, persist_graph};
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};
use glia_engram_export::diff::{diff_gmaps, diff_path, read_gmap, write_diff};
use glia_engram_export::{
    ExportOptions, build_gmap, history_dir, position_paths, prior_tokens, sidecar_path,
    write_engram_gmap,
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
    /// the moves since then (reads `<since>.glia/`; may equal --out), keep its
    /// file ids, and write the diff from it to `<out>.diff`.
    #[arg(long)]
    since: Option<String>,
    /// Build clean in memory: no parse cache is read or written in the repo
    /// (`GLIA_NO_PERSIST=1` does the same).
    #[arg(long)]
    no_persist: bool,
    /// Keep substrate-only synthetic nodes (npm deps, event names, generated
    /// stubs) that are filtered from the export by default.
    #[arg(long)]
    include_noise: bool,
    /// Drop nodes whose key matches this glob (`*` wildcard). Repeatable.
    #[arg(long = "exclude")]
    exclude: Vec<String>,
    /// Drop nodes whose POSITION file (repo-relative, `/`-separated) matches
    /// this glob, and leave those files out of the file table (`*` matches
    /// any run, `/` included: `bench/*` covers every file under bench/).
    /// Repeatable.
    #[arg(long = "exclude-path")]
    exclude_path: Vec<String>,
}

fn main() {
    let args = Args::parse();
    std::process::exit(run(&args));
}

/// What `--since` read before anything was written.
struct Prior {
    /// The prior gmap: the diff's base, and the file ids this run keeps.
    gmap: Gmap,
    /// `content_digest` of the prior's bytes: the diff's `base_digest`.
    digest: u64,
    /// `path -> file token` read out of the prior gmap's hints.
    tokens: BTreeMap<String, String>,
    /// The graph recorded beside the prior gmap, when it loads.
    graph: Option<glia_graph::MergedGraph>,
}

/// Read the `--since` prior: its gmap (refused -> `Err(exit code)`) and its
/// recorded graph (a failure is a warning: no moves this run).
fn read_prior(since: &Path) -> Result<Prior, i32> {
    let (prior, digest) = match read_gmap(since) {
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
    Ok(Prior { tokens: prior_tokens(&prior), gmap: prior, digest, graph })
}

/// The parse cache's file counts for one build (LA.12 `ParseCache::last_diff`).
struct CacheCounts {
    reused: usize,
    reparsed: usize,
    evicted: usize,
}

/// Build `repo`: the persisted incremental build (the repo's parse cache
/// loaded, used and saved, the sequence of the engine's
/// `generate_one_incremental`, keeping the cache to read its `last_diff`), or
/// with `persist` false the clean in-memory `generate_one`. The counts are
/// `None` when the cache is off, or when the build recorded no diff.
fn build(repo: &str, persist: bool) -> Result<(GenerateResult, Option<CacheCounts>), String> {
    if !persist {
        return generate_one(repo).map(|r| (r, None));
    }
    let mut cache = ParseCache::load(repo);
    let result = generate_one_with_cache(repo, &mut cache)?;
    if let Err(e) = cache.save(repo) {
        eprintln!("[engram-export] warning: parse cache not saved: {e}");
    }
    let counts = cache.last_diff().map(|d| CacheCounts {
        reused: d.reused.len(),
        reparsed: d.reparsed.len(),
        evicted: d.evicted.len(),
    });
    Ok((result, counts))
}

fn run(args: &Args) -> i32 {
    let prior = match args.since.as_deref().map(|s| read_prior(Path::new(s))) {
        Some(Ok(p)) => Some(p),
        Some(Err(code)) => return code,
        None => None,
    };
    let persist = !args.no_persist && std::env::var("GLIA_NO_PERSIST").as_deref() != Ok("1");
    let (result, cache) = match build(&args.repo, persist) {
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
        exclude_paths: args.exclude_path.clone(),
        file_identity,
        prior_files: prior.as_ref().map(|p| p.gmap.files.clone()),
    };
    let (gmap, _, mut stats) = build_gmap(&result.merged, repo_root, &opts);
    // A diff beside --out reaches the gmap that was there, which this run
    // replaces: drop it before the write, so no crash leaves it beside a gmap
    // it does not reach.
    let diff_out = diff_path(&out_path);
    if let Err(e) = std::fs::remove_file(&diff_out)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("error removing the stale diff {}: {e}", diff_out.display());
        return 5;
    }
    stats.digest = match write_engram_gmap(&gmap, &out_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error writing {}: {e}", out_path.display());
            return 5;
        }
    };
    // The G16 diff from the prior, after the full gmap it reaches.
    let mut code = 0;
    let since = match &prior {
        Some(p) => {
            let (diff, ds) = diff_gmaps(&p.gmap, p.digest, &gmap, stats.digest);
            match write_diff(&diff_out, &diff) {
                Ok(_) => Some((p.digest, ds)),
                Err(e) => {
                    eprintln!("error writing the diff {}: {e}", diff_out.display());
                    code = 7;
                    None
                }
            }
        }
        None => None,
    };
    // The graph this gmap was exported from, for the next `--since` run. The
    // gmap is complete without it, so a failure is a warning. Written with NO
    // repo root: a layout that records its root stores CODE as spans into the
    // sources (CD.7c), and this one is read back after those sources moved or
    // changed - exactly when `detect_moves` needs the prior CODE (the body
    // hash). Rootless, every CODE cell stays inline, a snapshot of the export.
    let history = history_dir(&out_path);
    let no_roots = BTreeMap::new();
    let written = persist_graph(
        &result.merged,
        &result.repo_labels,
        &no_roots,
        &result.parse_errors,
        &history,
        "glia-export-engram",
    );
    if let Err(e) = written {
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
    // The v6 since marker (LG.8a): the diff just written, by digest and
    // counts, and what the parse cache reused for this build (since the cache
    // was last saved, by any writer: not the export diff).
    if let Some((base, ds)) = since {
        let cache = match (&cache, persist) {
            (Some(c), _) => {
                format!("reused={} reparsed={} evicted={}", c.reused, c.reparsed, c.evicted)
            }
            (None, true) => "unrecorded".to_string(),
            (None, false) => "off".to_string(),
        };
        eprintln!(
            "[engram-export] v6 since: base={base:016x} target={:016x}{} added={} removed={} modified={} (moved={} location_only={}) edges +{}/-{}; parse cache {cache}\n  diff: {}",
            stats.digest,
            if base == stats.digest { " unchanged" } else { "" },
            ds.added,
            ds.removed,
            ds.modified,
            ds.moved,
            ds.location_only,
            ds.edges_added,
            ds.edges_removed,
            diff_out.display(),
        );
    }
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
    // The v6 NatSpec marker (LG.12): Solidity tags exported as Propositions
    // and the Documents edges joining them (plus @inheritdoc base -> override)
    // to their symbols. Printed on every run, so a zero is a real zero.
    eprintln!(
        "[engram-export] v6 natspec: {} tag facts + {} Documents edges on {} symbols (inheritdoc {}/{} resolved)",
        stats.natspec_facts,
        stats.natspec_edges,
        stats.natspec_symbols,
        stats.natspec_inheritdoc_resolved,
        stats.natspec_inheritdoc_resolved + stats.natspec_inheritdoc_unresolved,
    );
    // The v6 documents marker (LG.11): glia DOCUMENTS edges exported as
    // Documents (the NatSpec ones are on the line above, not counted here).
    // Printed on every run, so a zero is a real zero.
    eprintln!(
        "[engram-export] v6 documents: {} Documents edges (DOCUMENTS was folded into Cooccurs before v6)",
        stats.documents_edges,
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
    // The CG.2b path-exclusion marker, printed whenever the flag was given
    // (a zero is a real zero: no positioned node sat under the patterns).
    if !args.exclude_path.is_empty() {
        eprintln!(
            "[engram-export] exclude-path: dropped {} node(s) positioned under {} pattern(s)",
            stats.dropped_excluded_path,
            args.exclude_path.len(),
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
    code
}
