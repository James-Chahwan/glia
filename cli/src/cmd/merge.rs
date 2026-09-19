//! `glia merge` — build one MergedGraph from N repo paths so cross-graph
//! resolvers fire across the boundary; summary + optional JSON dump.
//!
//! LC.10c: members can also be pre-built `.gmap` layouts (`--gmap DIR`,
//! `--workspace FILE`), merged by `repo_graph_engine::merge::merge_layouts`
//! (LC.10b, which prints the `[merge] members=...` marker) without their
//! sources checked out; `--layout DIR` writes the merged graph as a layout.
//! Positional REPOS alone keep the source merge exactly as before.
//!
//! `glia --no-overlay merge` (LF.2b) builds the source merge without the
//! overlay; a layout merge refuses it (exit 2): its members were built with
//! their overlay, and no filter over a loaded layout can undo one.

use std::path::{Path, PathBuf};

use repo_graph_engine::merge::{MergeMember, merge_layouts, persist_merge, read_workspace};
use repo_graph_engine::persist::persist_result;
use repo_graph_engine::{GenerateResult, generate_many_opts};

use crate::common::{build_options, print_json, print_summary_table, write_json_to};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Repo paths to merge (each becomes its own RepoId). With --gmap or
    /// --workspace they are merge members too, after those, each loaded from
    /// its own `<repo>/.glia/graph` (rebuilt there first when missing or
    /// stale).
    repos: Vec<String>,
    /// A pre-built layout directory to merge (repeatable). The member is
    /// named by the directory's basename, a trailing `.glia/graph` stripped
    /// (`api/.glia/graph` -> `api`), characters outside [A-Za-z0-9._-] made
    /// `_` and leading dots dropped.
    #[arg(long, value_name = "DIR")]
    gmap: Vec<String>,
    /// A workspace manifest (`glia.workspace.json`, version 1) naming the
    /// members to merge; relative paths resolve against its directory. Its
    /// members come first, then --gmap, then REPOS.
    #[arg(long, value_name = "FILE")]
    workspace: Option<String>,
    /// Write the merged graph as a layout (manifest.json + shards +
    /// cross_stack.gmap) to this directory. A layout merge's manifest
    /// records its members.
    #[arg(long, value_name = "DIR")]
    layout: Option<String>,
    /// Write a JSON dump to this path. Pass `-` for stdout.
    #[arg(long)]
    out: Option<String>,
    /// Reuse the per-repo incremental parse caches (WP-D): each repo gets
    /// its own `<repo>/.glia/graph/parse_cache.bin`. Off by default for
    /// merges, so a merge writes nothing into the repos it reads. Source
    /// merges only: a repo member of a layout merge is already loaded or
    /// rebuilt incrementally.
    #[arg(long)]
    incremental: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    if args.gmap.is_empty() && args.workspace.is_none() {
        return run_sources(&args);
    }
    if args.incremental {
        eprintln!(
            "error: --incremental applies to a source merge only; a layout merge (--gmap / \
             --workspace) reads layouts, and its repo members are loaded or rebuilt incrementally"
        );
        return 1;
    }
    if !build_options().overlay {
        eprintln!(
            "error: --no-overlay applies to a source merge only; a layout merge (--gmap / \
             --workspace) reads layouts built with their overlay"
        );
        return 2;
    }
    run_layouts(&args)
}

/// Positional REPOS only: the source merge (`generate_many`, or the
/// incremental variant), unchanged; `--layout` writes the joint build's
/// layout through the one writer.
fn run_sources(args: &Args) -> i32 {
    let repos = args.repos.as_slice();
    if repos.is_empty() {
        eprintln!("error: at least one repo path required");
        return 1;
    }
    let built = generate_many_opts(repos, args.incremental, &build_options());
    let result = match built {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let code = emit(&result, args.out.as_deref());
    if code != 0 {
        return code;
    }
    if let Some(dir) = &args.layout
        && let Err(e) = persist_result(&result, Path::new(dir), "merge")
    {
        eprintln!("error: {e}");
        return 5;
    }
    0
}

/// --gmap / --workspace: merge the layouts (and any REPOS) with
/// `merge_layouts`; `--layout` writes the result through `persist_merge`.
fn run_layouts(args: &Args) -> i32 {
    let mut members = match &args.workspace {
        Some(file) => match read_workspace(Path::new(file)) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        },
        None => Vec::new(),
    };
    members.extend(args.gmap.iter().map(|dir| MergeMember::Gmap {
        name: member_name(Path::new(dir)),
        dir: PathBuf::from(dir),
    }));
    members.extend(args.repos.iter().map(|root| MergeMember::Repo {
        name: member_name(Path::new(root)),
        root: PathBuf::from(root),
    }));
    let merged = match merge_layouts(&members) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let code = emit(&merged.result, args.out.as_deref());
    if code != 0 {
        return code;
    }
    if let Some(dir) = &args.layout
        && let Err(e) = persist_merge(&merged, Path::new(dir), "merge")
    {
        eprintln!("error: {e}");
        return 5;
    }
    0
}

/// The summary table on stdout, then the `--out` JSON dump.
fn emit(result: &GenerateResult, out: Option<&str>) -> i32 {
    print_summary_table(result);
    if let Some(out_path) = out {
        if out_path == "-" {
            print_json(&result.merged);
        } else {
            let path = Path::new(out_path);
            let mut buffer = Vec::new();
            write_json_to(&result.merged, &mut buffer);
            if let Err(e) = std::fs::write(path, buffer) {
                eprintln!("error writing {out_path}: {e}");
                return 4;
            }
            eprintln!("wrote {} bytes to {}", path.metadata().map(|m| m.len()).unwrap_or(0), out_path);
        }
    }
    0
}

/// The member name of a `--gmap` dir or a REPOS path: the basename, a
/// trailing `.glia/graph` stripped (`api/.glia/graph` -> `api`); a path with
/// no basename as given (`.`, `..`) is named by its canonical path. Every
/// character outside `[A-Za-z0-9._-]` becomes `_` and leading dots are
/// dropped, the plain name `merge_layouts` requires. A name that is still
/// empty, or one two members share, is refused there with a message; a
/// workspace file names members explicitly. pyo3's `merge_gmaps`
/// (py/src/merge.rs) applies the same rule.
fn member_name(path: &Path) -> String {
    let base = |p: &Path| {
        let p = if p.ends_with(".glia/graph") {
            p.parent().and_then(Path::parent).unwrap_or(p)
        } else {
            p
        };
        p.file_name().map(|s| s.to_string_lossy().into_owned())
    };
    let raw = base(path)
        .or_else(|| path.canonicalize().ok().and_then(|c| base(&c)))
        .unwrap_or_default();
    let plain: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    plain.trim_start_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::member_name;
    use std::path::Path;

    #[test]
    fn member_names_follow_the_basename_rule() {
        let name = |p: &str| member_name(Path::new(p));
        assert_eq!(name("ci/api"), "api");
        assert_eq!(name("ci/api/"), "api");
        assert_eq!(name("svc/api/.glia/graph"), "api");
        assert_eq!(name("svc/api/.glia/graph/"), "api");
        assert_eq!(name("/abs/web-v2.1"), "web-v2.1");
        assert_eq!(name("layouts/my api"), "my_api");
        assert_eq!(name("layouts/.hidden"), "hidden");
        assert_eq!(name("layouts/caf\u{e9}"), "caf_");
        assert_eq!(name("/"), "");
        let cwd = std::env::current_dir().expect("cwd");
        let expected = member_name(&cwd);
        assert!(!expected.is_empty(), "the test cwd {} has a basename", cwd.display());
        assert_eq!(name("."), expected);
    }
}
