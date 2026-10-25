//! `glia find` (LD.3b) — ranked fuzzy symbol lookup over name / qname, each
//! row naming the tier that matched it and located. The engine's
//! `find::find_nodes` does the work; this parses `--kind` names and prints.
//! `--json` is the LD.8a envelope `{results, absence}`; an empty table prints
//! the absence block under `_(no match)_`.
//!
//! DD.9: a FACT / DERIVED identity class (one package, message or contract op
//! in several places) is one row; its location cell appends `(also in: <repo>
//! <file>:<line>, ...)` and `; diverged` when the members' contents differ,
//! and `--json` carries the row's `identity`. `--no-collapse` is one row per
//! member.
//!
//! EB.8: `--files` is a separate answer over FILES (`find::find_files_with_ids`):
//! the repo's files the query text is about, `LX[IDR,T1s]` (typed id links
//! first, then past commits whose messages match it, ties by degree then
//! path). It prints the routed ids (when any) and the ranked files; `--json`
//! is the `{results, absence}` envelope of the files. One repo only: with
//! `--with` the engine refuses (exit 2).
//!
//! EC.13: the files follow the repo's calibrated `SELf2t` pick (`glia history
//! calibrate`), else `LX[IDR,T1s]`; the table ends with `ranked by <pick>
//! (calibrated on <t> commits | fallback: <reason>)` and `--json` carries the
//! `selection` (stderr: `[select] surface=find_files set=SELf2t ..`).
//!
//! FB.7b: `--files` reads the repo's embeddings when it has them: with rows
//! of the surface model (`GLIA_EMBED_MODEL`, default bge-small) under the
//! fused recipe in `<repo>/.glia/vectors.jsonl` (`glia embed`), the files
//! rank `LX[IDR,FUSE[T1s,SEMp:t2]]` uncalibrated (FB.7a) and the table gains
//! a `sem` column (the file's raw SEMp:t2, 4 dp); `--json` rows carry `sem`.
//! No sidecar, or none of that model: no index is read and the output is
//! EC.13's byte for byte. An embedder that fails to load is a `warning:` and
//! the unfused answer. `--no-semantic` (only with `--files`) never reads the
//! sidecar.
//!
//! FB.13c: the node answer always passes the build's repo roots as
//! `FindOptions::vocab_roots`, so with a vocabulary snapshot (`glia vocab
//! sync`) a no-match absence names where each identifier of the query occurs
//! (`named_in` in `--json`, an `occurs in` clause in the note).

use glia_code_domain::node_kind;
use glia_core::NodeKindId;
use glia_engine::GenerateResult;
use glia_engine::calibrate::Selection;
use glia_engine::find::{
    DEFAULT_TOP_K, FindFilesOptions, FindOptions, FoundNode, find_files_with_ids, find_nodes,
};
use glia_engine::identity::Collapse;
use glia_engine::semantic::{SemanticSource, open_for_surface, surface_model};
use std::sync::Arc;

use crate::cmd::resolve::{live_glyph, print_absence};
use crate::common::{generate_for, load_embedder, rank_cache};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// A symbol, a qname (`UserService::get_user`), a dotted or slashed path,
    /// or a fragment of one.
    query: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Keep the first N rows; 0 keeps every match.
    #[arg(long, default_value_t = DEFAULT_TOP_K)]
    top_k: usize,
    /// Keep only nodes of this kind (a node-kind name such as FUNCTION or
    /// CLASS). Repeatable.
    #[arg(long)]
    kind: Vec<String>,
    /// Restrict the answer to a repo-relative path or a project label (see
    /// `glia projects`). Nodes with no file (ENDPOINT / ROUTE / doc spaces)
    /// are kept, not dropped.
    #[arg(long)]
    scope: Option<String>,
    /// One row per member of an identity class. By default a package,
    /// message or contract op found in several places (a FACT / DERIVED
    /// SAME_AS class) is one row naming the other places (`also in`).
    #[arg(long)]
    no_collapse: bool,
    /// Rank the repo's files for this text instead of matching node names:
    /// ids in the text first (through the repo's task / requirement rows and
    /// trackers), then past commits whose messages match it.
    #[arg(long, conflicts_with_all = ["kind", "no_collapse", "scope"])]
    files: bool,
    /// With --files: rank without the repo's embeddings. By default a repo
    /// whose `.glia/vectors.jsonl` (glia embed) holds vectors of the surface
    /// model (GLIA_EMBED_MODEL, default bge-small) blends their similarity
    /// into the text ranking and prints a `sem` column.
    #[arg(long, requires = "files")]
    no_semantic: bool,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
}

/// Node-kind names → ids, case-insensitively. An unknown name is an error
/// naming every valid one.
fn parse_kinds(names: &[String]) -> Result<Vec<NodeKindId>, String> {
    names
        .iter()
        .map(|n| {
            node_kind::ALL
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(n))
                .map(|(id, _)| *id)
                .ok_or_else(|| {
                    let valid: Vec<&str> = node_kind::ALL.iter().map(|(_, name)| *name).collect();
                    format!("unknown --kind '{n}'; valid kinds: {}", valid.join(", "))
                })
        })
        .collect()
}

pub(crate) fn run(args: Args) -> i32 {
    let kinds = match parse_kinds(&args.kind) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if args.files {
        return run_files(&args, &result);
    }
    let mut opts = FindOptions::default();
    opts.top_k = args.top_k;
    opts.kinds = (!kinds.is_empty()).then_some(kinds);
    opts.scope = args.scope;
    opts.collapse = if args.no_collapse {
        Collapse::off()
    } else {
        Collapse::with_labels(&result.repo_labels)
    };
    opts.vocab_roots = Some(
        result
            .repo_roots
            .iter()
            .map(|(id, p)| (*id, std::path::PathBuf::from(p)))
            .collect(),
    );
    let mut answer = find_nodes(&result.merged, &args.query, &opts);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia find `{}`", args.query);
    println!();
    if let Some(a) = &answer.absence {
        println!("_(no match)_");
        print_absence(a);
        return 0;
    }
    println!("| match | live | kind | qname | location |");
    println!("|---|:-:|---|---|---|");
    for r in &answer.results {
        println!(
            "| {} | {} | {} | `{}` | {} |",
            r.r#match,
            live_glyph(r.live),
            r.kind,
            r.qname,
            location_cell(r)
        );
    }
    0
}

/// `glia find --files`: the routed ids, then the ranked files. Exit 0 with
/// rows and on the absence (as the node answer), 2 on an engine error.
fn run_files(args: &Args, result: &GenerateResult) -> i32 {
    let mut opts = FindFilesOptions::default();
    opts.top_k = args.top_k;
    opts.surface = "cli";
    opts.rank_cache = Some(rank_cache(result));
    if !args.no_semantic {
        opts.semantic = semantic_source(result);
    }
    let sem_ran = opts.semantic.is_some();
    let (mut answer, ids) =
        match find_files_with_ids(&result.merged, &result.repo_roots, &args.query, &opts) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    let cell = |s: Option<&str>| s.unwrap_or("—").to_string();
    println!("# files for `{}`", args.query);
    println!();
    if !ids.is_empty() {
        println!("| id | namespace | tier | route | defined at | state |");
        println!("|---|---|---|---|---|---|");
        for d in &ids {
            let at = d
                .defined_at
                .as_ref()
                .map_or_else(|| "—".to_string(), |(p, l)| format!("{p}:{l}"));
            println!(
                "| `{}` | {} | {} | {} | {at} | {} |",
                d.id,
                cell(d.ns.as_deref()),
                cell(d.tier),
                d.route,
                cell(d.state)
            );
        }
        println!();
    }
    if let Some(a) = &answer.absence {
        println!("_(no match)_");
        print_absence(a);
        print_ranked_by(answer.selection.as_ref());
        return 0;
    }
    // The `sem` column only when the semantic tier scored the query.
    let sem_col = sem_ran && answer.results.iter().any(|r| r.sem.is_some());
    if sem_col {
        println!("| # | file | idr | text | sem | ids |");
        println!("|--:|---|--:|--:|--:|---|");
    } else {
        println!("| # | file | idr | text | ids |");
        println!("|--:|---|--:|--:|---|");
    }
    for r in &answer.results {
        let ids = if r.ids.is_empty() {
            "—".to_string()
        } else {
            r.ids
                .iter()
                .map(|i| format!("`{i}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if sem_col {
            let sem = r.sem.map_or_else(|| "—".to_string(), |c| format!("{c:.4}"));
            println!(
                "| {} | `{}` | {} | {:.3} | {sem} | {ids} |",
                r.rank, r.file, r.idr, r.text
            );
        } else {
            println!(
                "| {} | `{}` | {} | {:.3} | {ids} |",
                r.rank, r.file, r.idr, r.text
            );
        }
    }
    print_ranked_by(answer.selection.as_ref());
    0
}

/// FB.7b: the repo's fused-recipe vectors of [`surface_model`] and its query
/// embedder, or None: more than one repo (find --files refuses those), no
/// `.glia/vectors.jsonl` under the root (nothing read, nothing printed), no
/// fresh vector of that model, or an embedder that fails to load (a
/// `warning:` line; the answer is the unfused one).
fn semantic_source(result: &GenerateResult) -> Option<Arc<SemanticSource>> {
    let surface = surface_model();
    open_for_surface(&result.merged, &result.repo_roots, surface, || {
        load_embedder(surface).map(Arc::from)
    })
}

/// The `ranked by <pick> (..)` line under the files: what the order is and
/// where it came from (EC.13).
fn print_ranked_by(sel: Option<&Selection>) {
    let Some(s) = sel else { return };
    let why = match (s.source, s.reason) {
        ("calibrated", _) => format!("calibrated on {} commits", s.test_commits),
        ("explicit", _) => "explicit".to_string(),
        (_, r) => format!("fallback: {}", r.unwrap_or("-")),
    };
    println!();
    println!("ranked by {} ({why})", s.pick);
}

/// `file:line`, `file` or `—`.
fn place(file: Option<&str>, line: Option<i64>) -> Option<String> {
    match (file, line) {
        (Some(f), Some(l)) => Some(format!("{f}:{l}")),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    }
}

/// The table's location cell: the row's place, then, for a row that stands
/// for an identity class, `(also in: <repo> <file>:<line>, ...; diverged)`.
/// An unlocated member is its repo (and its qname when that differs from the
/// row's: a PACKAGE_DEP class shares one qname, so its repo is what differs).
fn location_cell(r: &FoundNode) -> String {
    let at = place(r.file.as_deref(), r.line).unwrap_or_else(|| "—".to_string());
    let Some(id) = &r.identity else {
        return at;
    };
    let others: Vec<String> = id
        .also_in
        .iter()
        .map(|a| match place(a.file.as_deref(), a.line) {
            Some(p) => format!("{} {p}", a.repo),
            None if a.qname != r.qname => format!("{} `{}`", a.repo, a.qname),
            None => a.repo.clone(),
        })
        .collect();
    let diverged = if id.diverged { "; diverged" } else { "" };
    format!("{at} (also in: {}{diverged})", others.join(", "))
}
