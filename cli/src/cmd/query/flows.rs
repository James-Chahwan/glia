//! `glia flows` (LD.4b) — every entry point's forward flow: the human surface
//! over the engine's `trace::entry_flows`. The table has one row per flow,
//! `| key | kind | entry | reach | xsvc | mechanisms | location |`; `--json`
//! is the engine's `Vec<EntryFlow>`, each row carrying its located hops and
//! services. The key is the feature word `glia trace` resolves to that entry
//! when the word names a dead end. A report: exits 0 whatever it finds; exit
//! 2 is a build failure.
//!
//! **Feature files (LG.3c).** `--features` or `--out <dir>` switches to the
//! engine's `feature_flows`: the entries grouped into per-feature records
//! (callers, located steps, data sources with their evidence tier), the
//! surface `generate-repo-map.py` produced for the dogfood repos. Without
//! either flag the output above is unchanged, byte for byte (LD.4b's
//! contract); `--group-by`, `--feature` and `--scope` need one of them.
//!
//! - `--features --json`: the `Vec<FeatureFlow>` on stdout; nothing written.
//! - `--out <dir>`, or `--features` alone (the dir is then the repo's
//!   `default_flows_dir`, `<repo>/.glia/graph/flows`): the engine writes
//!   `<dir>/<feature>.yaml` plus `<dir>/index.json`, touching only files whose
//!   bytes change and removing only files its previous index listed. Stdout is
//!   the index path; stderr carries the fired_on line `[feature-flows] wrote
//!   <w> feature file(s), <u> unchanged, <r> removed -> <dir>` (grep
//!   `^\[feature-flows\] wrote [0-9]`), after the engine's own
//!   `[feature-flows] wrote <dir> written=..` line.
//!
//! Exit 0 with zero features too (an empty `index.json` is an answer). Exit
//! 2 for a build failure, a `--group-by` other than `feature` / `entry`, an
//! `--out` inside a built repo but not under its `.glia/` (the next build
//! would walk the files as sources; the engine's refusal names the dir) or
//! any other write failure; `--out` with `--json` is a usage error.

use std::path::{Path, PathBuf};

use repo_graph_engine::GenerateResult;
use repo_graph_engine::feature_flows::{
    FlowGrouping, FlowOptions, INDEX_FILE, default_flows_dir, feature_flows, write_feature_flows,
};
use repo_graph_engine::trace::{DEFAULT_DEPTH, EntryFlow, entry_flows};

use crate::common::generate_for;

#[derive(clap::Args, Debug)]
#[command(group(clap::ArgGroup::new("feature_mode").args(["features", "out"]).multiple(true)))]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Maximum hops each flow follows from its entry point.
    #[arg(long, default_value_t = DEFAULT_DEPTH)]
    depth: usize,
    /// Emit JSON (the list of flows; with --features, the feature records)
    /// instead of a table. Writes nothing.
    #[arg(long)]
    json: bool,
    /// Group the entry flows into per-feature records (LG.3c): written to the
    /// repo's default flows dir (`<repo>/.glia/graph/flows`), or printed with
    /// --json.
    #[arg(long)]
    features: bool,
    /// Write the feature files (`<feature>.yaml` + `index.json`) to this dir.
    /// Implies --features. Refused inside a built repo unless under its
    /// `.glia/`.
    #[arg(long, conflicts_with = "json")]
    out: Option<String>,
    /// One record per feature key (`feature`) or per entry point (`entry`).
    /// Needs --features or --out.
    #[arg(long, default_value = "feature", value_parser = ["feature", "entry"], requires = "feature_mode")]
    group_by: String,
    /// Keep only the record with this key. Needs --features or --out.
    #[arg(long, requires = "feature_mode")]
    feature: Option<String>,
    /// Keep only entries under this path or project label. Needs --features
    /// or --out.
    #[arg(long, requires = "feature_mode")]
    scope: Option<String>,
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.features || args.out.is_some() {
        return run_features(&args, &result);
    }
    // The `[flows] entries=` fired_on marker is printed inside the engine.
    let flows = entry_flows(&result.merged, &result.repo_labels, args.depth);
    if args.json {
        println!("{}", serde_json::to_string(&flows).unwrap_or_default());
        return 0;
    }
    render(&args, &flows);
    0
}

/// `--features` / `--out`: the feature records, printed or written.
fn run_features(args: &Args, result: &GenerateResult) -> i32 {
    let Some(grouping) = FlowGrouping::from_name(&args.group_by) else {
        eprintln!(
            "error: --group-by must be `feature` or `entry`, not `{}`",
            args.group_by
        );
        return 2;
    };
    let mut opts = FlowOptions::default()
        .with_grouping(grouping)
        .with_depth(args.depth);
    if let Some(f) = &args.feature {
        opts = opts.with_feature(f.as_str());
    }
    if let Some(s) = &args.scope {
        opts = opts.with_scope(s.as_str());
    }
    // The `[feature-flows] features=` marker is printed inside the engine.
    let flows = feature_flows(&result.merged, &result.repo_labels, &opts);
    if args.json {
        return match serde_json::to_string(&flows) {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(e) => {
                eprintln!("error: feature flows: {e}");
                2
            }
        };
    }
    let dir: PathBuf = match &args.out {
        Some(out) => PathBuf::from(out),
        None => default_flows_dir(Path::new(&args.repo)),
    };
    let roots: Vec<&Path> = result.repo_roots.values().map(Path::new).collect();
    match write_feature_flows(&roots, &dir, &flows, grouping) {
        Ok(w) => {
            eprintln!(
                "[feature-flows] wrote {} feature file(s), {} unchanged, {} removed -> {}",
                w.written,
                w.unchanged,
                w.removed,
                dir.display()
            );
            println!("{}", dir.join(INDEX_FILE).display());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

/// `file:line`, `file`, or `—`.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

fn render(args: &Args, flows: &[EntryFlow]) {
    println!("# glia flows `{}` (depth ≤ {})", args.repo, args.depth);
    println!();
    if flows.is_empty() {
        println!(
            "_(no entry point reaches anything within {} hops)_",
            args.depth
        );
        return;
    }
    println!("| key | kind | entry | reach | xsvc | mechanisms | location |");
    println!("|---|---|---|--:|:-:|---|---|");
    for f in flows {
        println!(
            "| `{}` | {} | `{}` | {} | {} | {} | {} |",
            f.key,
            f.entry.kind,
            f.entry.qname,
            f.reach,
            if f.cross_service { "✔" } else { "" },
            f.mechanisms.join(", "),
            at(f.entry.file.as_deref(), f.entry.line)
        );
    }
}
