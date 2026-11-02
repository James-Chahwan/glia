//! `glia targets sync|owners|deps|rdeps|affected` (HT.7) — the CLI surface of
//! the build-target domain: the snapshot step (`glia_snapshots::targets_sync`,
//! HT.3) and the answers over the targets sidecar
//! (`glia_engine::targets`, HT.6).
//!
//! `sync` asks bazel once (`bazel query '<expr>' --output=streamed_jsonproto`
//! in one workspace of the repo, or a saved query output with `--from`) and
//! writes `<repo>/.glia/targets-snapshot/`, replacing only that workspace's
//! rows; a build never runs it. The answers read that snapshot through
//! `targets::load_or_build` (the `.glia/graph/targets.gmap` sidecar, rebuilt
//! when the snapshot or glia moved) and never run bazel:
//! - `owners <repo> <file>..`: the targets naming each file in `srcs` /
//!   `hdrs` / `textual_hdrs`, one `<file> -> <label>` line each;
//! - `deps` / `rdeps <repo> <label>`: what a target reaches / what reaches
//!   it over the carry edges, at most `--depth` hops; `--workspace <w>`
//!   picks one workspace's label (the engine's `<w>|<label>`);
//! - `affected <repo> [<file>..]`: the targets and test targets the changed
//!   files reach backwards; no files reads the work tree against `--base`
//!   (default HEAD).
//!
//! A row reads `<label>  <rule_class>  <role>  <file>:<line>  depth=<d>
//! via=<attrs>` (`-` for a field the row lacks), prefixed `[<workspace>] `
//! when the snapshot holds more than one workspace. `--json` prints the
//! engine's rows: `owners` `[{file, owners}]`, `deps` / `rdeps` the
//! `{results, absence}` envelope, `affected` the `Affected` record, `sync`
//! the summary. Every row is tier `fact` (read off the build system's own
//! query output).
//!
//! Exits 0 with rows, 1 with none (the absence: an unknown label, no edge,
//! no owner, nothing affected), 2 on an error (a failed sync, no targets
//! snapshot, a sidecar that cannot be written, an unknown `--base`).
//!
//! fired_on, on stderr: the snapshot crate's `[targets] sync workspace=..`
//! and the engine's `[targets] sidecar ..` / `[targets] answer kind=..`,
//! then this surface's own `[targets] surface=cli cmd=<sync|owners|deps|
//! rdeps|affected> ..` line once per call.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use glia_engine::absence::Answer;
use glia_engine::targets::{Affected, TargetHit, TargetsView, load_or_build, targets_dir};
use glia_snapshots::{DEFAULT_EXPR, TargetsOptions, TargetsSummary, targets_sync};

use crate::cmd::resolve::print_absence;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: TargetsCmd,
}

#[derive(Subcommand, Debug)]
enum TargetsCmd {
    /// Ask bazel for one workspace's targets (or read a saved query output)
    /// and write `<repo>/.glia/targets-snapshot/`, replacing only that
    /// workspace's rows. Exits 2 when the sync fails (nothing written).
    Sync {
        /// Repo to write the snapshot into.
        repo: String,
        /// The bazel workspace, repo-relative (default: the repo root).
        #[arg(long, value_name = "DIR", default_value = "")]
        workspace: String,
        /// The bazel (or bazelisk) binary to run.
        #[arg(long, value_name = "BIN", default_value = "bazel")]
        bazel: String,
        /// The query expression.
        #[arg(long, value_name = "QUERY", default_value = DEFAULT_EXPR)]
        expr: String,
        /// Read this saved `bazel query --output=streamed_jsonproto` output
        /// instead of running bazel.
        #[arg(long, value_name = "FILE")]
        from: Option<PathBuf>,
        /// Print the summary as JSON.
        #[arg(long)]
        json: bool,
    },
    /// The targets naming each file in `srcs` / `hdrs` / `textual_hdrs`.
    Owners {
        /// Repo holding the targets snapshot.
        repo: String,
        /// Repo-relative files.
        #[arg(required = true)]
        files: Vec<String>,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// What a target reaches over the carry edges (deps, runtime deps,
    /// sources).
    Deps(WalkArgs),
    /// What reaches a target over the carry edges.
    Rdeps(WalkArgs),
    /// The targets and test targets changed files reach. No files: the work
    /// tree's changes against `--base`.
    Affected {
        /// Repo holding the targets snapshot.
        repo: String,
        /// Repo-relative changed files (none: diff the work tree).
        files: Vec<String>,
        /// The revision to diff the work tree against when no files are given.
        #[arg(long, value_name = "REV", default_value = "HEAD")]
        base: String,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args, Debug)]
struct WalkArgs {
    /// Repo holding the targets snapshot.
    repo: String,
    /// The target's label (`//main:hello-world`, `//main`).
    label: String,
    /// Only this workspace's target of that label (repo-relative dir).
    #[arg(long, value_name = "DIR")]
    workspace: Option<String>,
    /// At most this many hops.
    #[arg(long, value_name = "N", default_value_t = 3)]
    depth: usize,
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        TargetsCmd::Sync {
            repo,
            workspace,
            bazel,
            expr,
            from,
            json,
        } => sync(&repo, workspace, bazel, expr, from, json),
        TargetsCmd::Owners { repo, files, json } => owners(&repo, &files, json),
        TargetsCmd::Deps(a) => walk(a, false),
        TargetsCmd::Rdeps(a) => walk(a, true),
        TargetsCmd::Affected {
            repo,
            files,
            base,
            json,
        } => affected(&repo, &files, &base, json),
    }
}

/// Print `--json` text; 2 when it did not serialise.
fn print_json(text: Result<String, serde_json::Error>) -> i32 {
    match text {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

fn sync(
    repo: &str,
    workspace: String,
    bazel: String,
    expr: String,
    from: Option<PathBuf>,
    json: bool,
) -> i32 {
    let mut opts = TargetsOptions::in_workspace(workspace);
    opts.bazel = bazel;
    opts.expr = expr;
    opts.from = from;
    let summary: TargetsSummary = match targets_sync(Path::new(repo), &opts) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let ws = if summary.workspace.is_empty() {
        "."
    } else {
        summary.workspace.as_str()
    };
    eprintln!(
        "[targets] surface=cli cmd=sync workspace={ws} rows={}",
        summary.targets
    );
    if json {
        return print_json(serde_json::to_string_pretty(&summary));
    }
    println!(
        "synced {} rows ({} rules, {} source files, {} external labels, {} skipped) \
         of workspace {ws} (bazel {}) -> {}/",
        summary.targets,
        summary.rules,
        summary.source_files,
        summary.external_labels,
        summary.skipped,
        summary.bazel_version,
        targets_dir(Path::new(repo)).display()
    );
    0
}

/// The targets view of `repo`, or the exit code and message of why none.
fn view(repo: &str) -> Result<TargetsView, i32> {
    let root = Path::new(repo);
    match load_or_build(root) {
        Ok(Some(v)) => Ok(v),
        Ok(None) => {
            eprintln!("{}", no_snapshot(root));
            Err(2)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Err(2)
        }
    }
}

fn no_snapshot(root: &Path) -> String {
    format!(
        "error: no targets snapshot at {}/; run `glia targets sync {}` first",
        targets_dir(root).display(),
        root.display()
    )
}

/// Whether the snapshot holds more than one workspace (rows then carry a
/// `[<workspace>] ` prefix).
fn many_workspaces(v: &TargetsView) -> bool {
    v.workspaces().len() > 1
}

fn or_dash(s: Option<&str>) -> &str {
    match s {
        Some(s) if !s.is_empty() => s,
        _ => "-",
    }
}

/// `[<workspace>] <label>  <rule_class>  <role>  <file>:<line>  depth=<d>  via=<attrs>`.
fn row_line(h: &TargetHit, prefix: bool) -> String {
    let at = match (h.file.as_deref(), h.line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "-".to_string(),
    };
    let ws = if prefix {
        format!(
            "[{}] ",
            if h.workspace.is_empty() {
                "."
            } else {
                h.workspace.as_str()
            }
        )
    } else {
        String::new()
    };
    format!(
        "{ws}{}  {}  {}  {at}  depth={}  via={}",
        h.label,
        or_dash(h.rule_class.as_deref()),
        or_dash(h.role.as_deref()),
        h.depth,
        or_dash(Some(h.via.as_str()))
    )
}

/// One file's owners.
struct FileOwners {
    file: String,
    owners: Vec<TargetHit>,
}

fn owners(repo: &str, files: &[String], json: bool) -> i32 {
    let v = match view(repo) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let mut seen = BTreeSet::new();
    let per_file: Vec<FileOwners> = files
        .iter()
        .filter(|f| seen.insert(f.as_str()))
        .map(|f| FileOwners {
            file: f.clone(),
            owners: v.owners(std::slice::from_ref(f)),
        })
        .collect();
    let rows: usize = per_file.iter().map(|f| f.owners.len()).sum();
    eprintln!(
        "[targets] surface=cli cmd=owners files={} rows={rows}",
        per_file.len()
    );
    let code = if rows == 0 { 1 } else { 0 };
    if json {
        return match print_json(serde_json::to_string_pretty(
            &per_file
                .iter()
                .map(|f| serde_json::json!({"file": f.file, "owners": f.owners}))
                .collect::<Vec<_>>(),
        )) {
            0 => code,
            e => e,
        };
    }
    let prefix = many_workspaces(&v);
    for f in &per_file {
        if f.owners.is_empty() {
            println!("{} -> (no target names it)", f.file);
        }
        for h in &f.owners {
            let label = if prefix {
                format!("[{}] {}", or_dash(Some(h.workspace.as_str())), h.label)
            } else {
                h.label.clone()
            };
            println!("{} -> {label}", f.file);
        }
    }
    code
}

fn walk(a: WalkArgs, reverse: bool) -> i32 {
    let v = match view(&a.repo) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let query = match &a.workspace {
        Some(w) => format!("{w}|{}", a.label.trim()),
        None => a.label.trim().to_string(),
    };
    let answer: Answer<TargetHit> = if reverse {
        v.rdeps(&query, a.depth)
    } else {
        v.deps(&query, a.depth)
    };
    let cmd = if reverse { "rdeps" } else { "deps" };
    eprintln!(
        "[targets] surface=cli cmd={cmd} rows={} absence={}",
        answer.results.len(),
        answer.absence.as_ref().map_or("-", |a| a.reason)
    );
    let code = if answer.absence.is_some() { 1 } else { 0 };
    if a.json {
        return match print_json(serde_json::to_string_pretty(&answer)) {
            0 => code,
            e => e,
        };
    }
    if let Some(abs) = &answer.absence {
        println!("_(no rows)_");
        print_absence(abs);
        return code;
    }
    let prefix = many_workspaces(&v);
    for h in &answer.results {
        println!("{}", row_line(h, prefix));
    }
    code
}

fn affected(repo: &str, files: &[String], base: &str, json: bool) -> i32 {
    let v = match view(repo) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let a: Affected = if files.is_empty() {
        match v.affected_since(Path::new(repo), base) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    } else {
        v.affected(files)
    };
    eprintln!(
        "[targets] surface=cli cmd=affected changed={} rows={} tests={} unowned={} caveats={}",
        a.changed.len(),
        a.targets.len(),
        a.tests.len(),
        a.unowned.len(),
        a.caveats.len()
    );
    let code = if a.targets.is_empty() { 1 } else { 0 };
    if json {
        return match print_json(serde_json::to_string_pretty(&a)) {
            0 => code,
            e => e,
        };
    }
    let prefix = many_workspaces(&v);
    println!("## targets");
    if a.targets.is_empty() {
        println!("_(none)_");
    }
    for h in &a.targets {
        println!("{}", row_line(h, prefix));
    }
    println!();
    println!("## tests");
    if a.tests.is_empty() {
        println!("_(none)_");
    }
    for h in &a.tests {
        println!("{}", row_line(h, prefix));
    }
    if !a.unowned.is_empty() {
        println!();
        println!("unowned: {}", a.unowned.join(", "));
    }
    for c in &a.caveats {
        println!("> caveat: {c}");
    }
    code
}
