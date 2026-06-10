//! glia CLI — `glia <subcommand> [args]`.
//!
//! Subcommands:
//!   - `analyze <repo>` — walk a repo, build the merged graph, print a
//!     summary table + (optionally) Mermaid service-graph + JSON dump.
//!   - `impact <repo> <qname>` — reachability walk; what does this entity
//!     touch, what touches it, cross-service blast-radius.
//!   - `merge <path> [<path>...] [--out <file>]` — build a single
//!     MergedGraph from N repo paths so cross-graph resolvers fire across
//!     the boundary; emit JSON.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use clap::{Parser, Subcommand, ValueEnum};
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{NodeId, NodeKindId};
use repo_graph_engine::{GenerateResult, generate_many, generate_one, generate_one_incremental};
use repo_graph_graph::MergedGraph;

#[derive(Parser, Debug)]
#[command(
    name = "glia",
    version,
    about = "glia — cross-service code-graph engine"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Walk a repo and print a summary of node-kinds + cross-graph edges.
    Analyze {
        /// Path to the repo root.
        repo: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = AnalyzeFormat::Summary)]
        format: AnalyzeFormat,
    },
    /// Reachability walk: which entities does <qname> depend on / get hit by.
    Impact {
        /// Path to the repo root.
        repo: String,
        /// Qname of the entity to analyze (e.g. `app::services::api::Handler`).
        qname: String,
        /// Direction of the walk.
        #[arg(long, value_enum, default_value_t = ImpactDirection::Both)]
        direction: ImpactDirection,
        /// Maximum walk depth.
        #[arg(long, default_value_t = 4)]
        depth: usize,
    },
    /// Merge N repos into one MergedGraph; cross-resolvers fire across repo
    /// boundaries. Emit summary + cross-edge counts + (optionally) JSON.
    Merge {
        /// Repo paths to merge (each becomes its own RepoId).
        repos: Vec<String>,
        /// Write a JSON dump to this path. Pass `-` for stdout.
        #[arg(long)]
        out: Option<String>,
    },
    /// Walk a repo and write one `.gmap` per per-language sub-graph to
    /// `<repo>/.glia/` (or a custom dir). Idempotent + atomic.
    Build {
        /// Path to the repo root.
        repo: String,
        /// Output directory. Defaults to `<repo>/.glia`.
        #[arg(long)]
        out: Option<String>,
        /// Force a full reparse, ignoring the incremental parse cache (WP-D).
        #[arg(long)]
        no_incremental: bool,
    },
    /// Install git hooks (`post-commit`, `post-merge`, `post-checkout`) into
    /// the target repo so its `.gmap` rebuilds automatically on each change.
    /// Opt-in only — rebuild latency on big repos can be noticeable.
    InstallHooks {
        /// Path to the repo (must contain a `.git` dir).
        #[arg(default_value = ".")]
        repo: String,
        /// Uninstall instead of install.
        #[arg(long)]
        uninstall: bool,
        /// Command to run from each hook (defaults to `glia build .`).
        /// Use this to point at a non-default `glia` binary or pass extra
        /// flags like `--out path/to/out`.
        #[arg(long)]
        command: Option<String>,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum AnalyzeFormat {
    /// Markdown-style summary table to stdout (default).
    Summary,
    /// Mermaid `graph LR` of cross-stack edges (HTTP, gRPC, queue, etc.).
    Mermaid,
    /// Full JSON dump (nodes + edges).
    Json,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ImpactDirection {
    /// Forward — what this entity reaches (calls, http_calls, etc.).
    Forward,
    /// Backward — what reaches this entity (predecessors).
    Backward,
    /// Both directions in one pass.
    Both,
}

fn main() {
    let cli = Cli::parse();
    let exit = match cli.cmd {
        Cmd::Analyze { repo, format } => cmd_analyze(&repo, format),
        Cmd::Impact {
            repo,
            qname,
            direction,
            depth,
        } => cmd_impact(&repo, &qname, direction, depth),
        Cmd::Merge { repos, out } => cmd_merge(&repos, out.as_deref()),
        Cmd::Build { repo, out, no_incremental } => {
            cmd_build(&repo, out.as_deref(), !no_incremental)
        }
        Cmd::InstallHooks {
            repo,
            uninstall,
            command,
        } => cmd_install_hooks(&repo, uninstall, command.as_deref()),
    };
    std::process::exit(exit);
}

// ----------------------------------------------------------------------------
// `analyze`
// ----------------------------------------------------------------------------

fn cmd_analyze(repo: &str, format: AnalyzeFormat) -> i32 {
    let result = match generate_one(repo) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    match format {
        AnalyzeFormat::Summary => print_summary_table(&result),
        AnalyzeFormat::Mermaid => print_mermaid(&result.merged),
        AnalyzeFormat::Json => print_json(&result.merged),
    }
    0
}

fn print_summary_table(r: &GenerateResult) {
    let merged = &r.merged;
    let total_intra: usize = merged.graphs.iter().map(|g| g.edges.len()).sum();
    println!("# glia analyze");
    println!();
    println!("- nodes: {}", r.total_nodes);
    println!("- edges (intra-repo): {total_intra}");
    println!("- cross-edges: {}", merged.cross_edges.len());
    if !r.parse_errors.is_empty() {
        println!("- parse errors: {}", r.parse_errors.len());
    }
    println!();

    // Node kind histogram.
    let mut kind_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some(kind) = g.nav.kind_by_id.get(&n.id) {
                *kind_counts.entry(node_kind_name(*kind)).or_insert(0) += 1;
            }
        }
    }
    println!("## Node kinds");
    println!();
    println!("| Kind | Count |");
    println!("|---|---|");
    let mut rows: Vec<_> = kind_counts.into_iter().collect();
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (k, c) in rows {
        println!("| {k} | {c} |");
    }
    println!();

    // Edge category histogram (intra + cross combined).
    let mut cat_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for g in &merged.graphs {
        for e in &g.edges {
            *cat_counts.entry(edge_category_name(e.category)).or_insert(0) += 1;
        }
    }
    for e in &merged.cross_edges {
        *cat_counts.entry(edge_category_name(e.category)).or_insert(0) += 1;
    }
    println!("## Edge categories");
    println!();
    println!("| Category | Count |");
    println!("|---|---|");
    let mut rows: Vec<_> = cat_counts.into_iter().collect();
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (k, c) in rows {
        println!("| {k} | {c} |");
    }
}

fn print_mermaid(merged: &MergedGraph) {
    // Service graph: render cross-edges as a `graph LR`. Each repo gets a
    // subgraph; cross-edges link repo nodes.
    println!("```mermaid");
    println!("graph LR");
    // One node per (repo) with the repo path basename as label.
    let mut repo_labels: BTreeMap<u64, String> = BTreeMap::new();
    for g in &merged.graphs {
        let label = format!("repo_{}", g.repo.0);
        repo_labels.insert(g.repo.0, label);
    }
    for (rid, label) in &repo_labels {
        println!("    {label}[\"repo {rid}\"]");
    }
    // Aggregate cross-edges by (from-repo, to-repo, category). NodeId is
    // Hash+Eq but not Ord, so HashMap rather than BTreeMap.
    let mut node_to_repo: HashMap<NodeId, u64> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            node_to_repo.insert(n.id, g.repo.0);
        }
    }
    let mut agg: BTreeMap<(u64, u64, &'static str), usize> = BTreeMap::new();
    for e in &merged.cross_edges {
        let (Some(&fr), Some(&to)) = (node_to_repo.get(&e.from), node_to_repo.get(&e.to)) else {
            continue;
        };
        if fr == to {
            continue;
        }
        let cat = edge_category_name(e.category);
        *agg.entry((fr, to, cat)).or_insert(0) += 1;
    }
    for ((fr, to, cat), count) in agg {
        let fl = repo_labels.get(&fr).cloned().unwrap_or_else(|| format!("repo_{fr}"));
        let tl = repo_labels.get(&to).cloned().unwrap_or_else(|| format!("repo_{to}"));
        println!("    {fl} -->|\"{cat} ({count})\"| {tl}");
    }
    println!("```");
}

fn print_json(merged: &MergedGraph) {
    let mut nodes = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
            let name = g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            nodes.push(serde_json::json!({
                "id": n.id.0,
                "repo": g.repo.0,
                "kind": kind,
                "kind_name": node_kind_name(NodeKindId(kind)),
                "name": name,
                "qname": qname,
            }));
        }
    }
    let mut edges = Vec::new();
    for g in &merged.graphs {
        for e in &g.edges {
            edges.push(serde_json::json!({
                "from": e.from.0,
                "to": e.to.0,
                "category": edge_category_name(e.category),
                "intra": true,
            }));
        }
    }
    for e in &merged.cross_edges {
        edges.push(serde_json::json!({
            "from": e.from.0,
            "to": e.to.0,
            "category": edge_category_name(e.category),
            "intra": false,
        }));
    }
    let out = serde_json::json!({ "nodes": nodes, "edges": edges });
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
}

// ----------------------------------------------------------------------------
// `impact`
// ----------------------------------------------------------------------------

fn cmd_impact(repo: &str, qname: &str, direction: ImpactDirection, depth: usize) -> i32 {
    let result = match generate_one(repo) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let merged = &result.merged;

    // Resolve qname → NodeId across all repo graphs (one match expected; if
    // multiple, walk all). Sorted by node id so the section order is stable
    // across processes (the resolver no longer leaks HashMap iteration order).
    let targets: Vec<NodeId> = merged.qnames_exact(qname);
    if targets.is_empty() {
        eprintln!("error: no node with qname `{qname}` in {repo}");
        eprintln!();
        eprintln!("hint: use `glia analyze {repo} --format json` to list available qnames.");
        return 3;
    }

    println!("# glia impact `{qname}`");
    println!();
    for target in &targets {
        let info = lookup_node_info(merged, *target);
        println!(
            "## {} `{}` (kind={}, repo={})",
            info.name, info.qname, info.kind_name, info.repo
        );
        println!();
        if matches!(direction, ImpactDirection::Forward | ImpactDirection::Both) {
            println!("### Forward — what this reaches (depth ≤ {depth})");
            println!();
            walk_and_print(merged, *target, depth, /*forward=*/ true);
            println!();
        }
        if matches!(direction, ImpactDirection::Backward | ImpactDirection::Both) {
            println!("### Backward — what reaches this (depth ≤ {depth})");
            println!();
            walk_and_print(merged, *target, depth, /*forward=*/ false);
            println!();
        }
    }
    0
}

fn walk_and_print(merged: &MergedGraph, start: NodeId, max_depth: usize, forward: bool) {
    use std::collections::{HashSet, VecDeque};
    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut frontier: VecDeque<(NodeId, usize)> = VecDeque::new();
    frontier.push_back((start, 0));
    visited.insert(start);
    let mut hits: Vec<(NodeId, usize, &'static str)> = Vec::new();
    while let Some((node, d)) = frontier.pop_front() {
        if d >= max_depth {
            continue;
        }
        for e in all_edges(merged) {
            let (next, cat) = if forward && e.from == node {
                (e.to, edge_category_name(e.category))
            } else if !forward && e.to == node {
                (e.from, edge_category_name(e.category))
            } else {
                continue;
            };
            if visited.insert(next) {
                hits.push((next, d + 1, cat));
                frontier.push_back((next, d + 1));
            }
        }
    }
    if hits.is_empty() {
        println!("_(none)_");
        return;
    }
    println!("| Depth | Kind | Qname | Edge |");
    println!("|---|---|---|---|");
    for (id, d, cat) in hits {
        let info = lookup_node_info(merged, id);
        println!("| {d} | {} | `{}` | {} |", info.kind_name, info.qname, cat);
    }
}

fn all_edges(merged: &MergedGraph) -> impl Iterator<Item = &repo_graph_core::Edge> {
    merged
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(merged.cross_edges.iter())
}

struct NodeInfo {
    name: String,
    qname: String,
    kind_name: &'static str,
    repo: u64,
}

fn lookup_node_info(merged: &MergedGraph, id: NodeId) -> NodeInfo {
    for g in &merged.graphs {
        if g.nav.qname_by_id.contains_key(&id) {
            return NodeInfo {
                name: g.nav.name_by_id.get(&id).cloned().unwrap_or_default(),
                qname: g.nav.qname_by_id.get(&id).cloned().unwrap_or_default(),
                kind_name: g
                    .nav
                    .kind_by_id
                    .get(&id)
                    .map(|k| node_kind_name(*k))
                    .unwrap_or("unknown"),
                repo: g.repo.0,
            };
        }
    }
    NodeInfo {
        name: String::new(),
        qname: format!("(unknown:{})", id.0),
        kind_name: "unknown",
        repo: 0,
    }
}

// ----------------------------------------------------------------------------
// `merge`
// ----------------------------------------------------------------------------

fn cmd_merge(repos: &[String], out: Option<&str>) -> i32 {
    if repos.is_empty() {
        eprintln!("error: at least one repo path required");
        return 1;
    }
    let result = match generate_many(repos) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    print_summary_table(&result);
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

fn write_json_to(merged: &MergedGraph, out: &mut Vec<u8>) {
    use std::io::Write;
    let mut nodes = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
            let name = g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            nodes.push(serde_json::json!({
                "id": n.id.0,
                "repo": g.repo.0,
                "kind": kind,
                "kind_name": node_kind_name(NodeKindId(kind)),
                "name": name,
                "qname": qname,
            }));
        }
    }
    let mut edges = Vec::new();
    for g in &merged.graphs {
        for e in &g.edges {
            edges.push(serde_json::json!({
                "from": e.from.0,
                "to": e.to.0,
                "category": edge_category_name(e.category),
                "intra": true,
            }));
        }
    }
    for e in &merged.cross_edges {
        edges.push(serde_json::json!({
            "from": e.from.0,
            "to": e.to.0,
            "category": edge_category_name(e.category),
            "intra": false,
        }));
    }
    let json = serde_json::json!({ "nodes": nodes, "edges": edges });
    let _ = writeln!(out, "{}", serde_json::to_string(&json).unwrap_or_default());
}

// ----------------------------------------------------------------------------
// Pretty-name lookups for code-domain ID constants.
// ----------------------------------------------------------------------------

fn node_kind_name(k: NodeKindId) -> &'static str {
    // Delegate to the canonical code-domain table (WP-I) — no local copy to go
    // stale when a kind is added.
    node_kind::name(k)
}

// ----------------------------------------------------------------------------
// `build` — walk repo, write per-language `.gmap` files
// ----------------------------------------------------------------------------

fn cmd_build(repo: &str, out: Option<&str>, incremental: bool) -> i32 {
    let built = if incremental {
        generate_one_incremental(repo)
    } else {
        // An explicit clean build also discards the sidecar — otherwise the
        // next default-on build would reuse the cache the user was escaping.
        if let Err(e) = repo_graph_engine::ParseCache::purge(repo) {
            eprintln!("warning: could not remove parse cache: {e}");
        }
        generate_one(repo)
    };
    let result = match built {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let out_dir = match out {
        Some(p) => Path::new(p).to_path_buf(),
        None => Path::new(repo).join(".glia"),
    };
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error creating {}: {e}", out_dir.display());
        return 4;
    }
    let mut total_bytes: u64 = 0;
    let mut written = 0;
    // A merged graph for a single repo has one RepoGraph per detected
    // language — they all share `g.repo.0`. Number them so they don't
    // collide in the output dir.
    for (i, g) in result.merged.graphs.iter().enumerate() {
        let filename = if result.merged.graphs.len() == 1 {
            format!("repo-{}.gmap", g.repo.0)
        } else {
            format!("repo-{}-{:02}.gmap", g.repo.0, i)
        };
        let path = out_dir.join(filename);
        if let Err(e) = repo_graph_store::write_repo_graph(g, &path) {
            eprintln!("error writing {}: {e}", path.display());
            return 5;
        }
        total_bytes += path.metadata().map(|m| m.len()).unwrap_or(0);
        written += 1;
    }
    eprintln!(
        "wrote {} .gmap file{} ({:.1} KiB) to {}",
        written,
        if written == 1 { "" } else { "s" },
        total_bytes as f64 / 1024.0,
        out_dir.display()
    );
    if !result.parse_errors.is_empty() {
        eprintln!("(plus {} parse errors)", result.parse_errors.len());
    }
    0
}

// ----------------------------------------------------------------------------
// `install-hooks` — drop in `.git/hooks/{post-commit,post-merge,post-checkout}`
// ----------------------------------------------------------------------------

const HOOK_NAMES: &[&str] = &["post-commit", "post-merge", "post-checkout"];
const HOOK_MARKER: &str = "# glia-install-hooks: managed";

fn cmd_install_hooks(repo: &str, uninstall: bool, command: Option<&str>) -> i32 {
    let repo_path = Path::new(repo);
    let git_dir = repo_path.join(".git");
    if !git_dir.exists() {
        eprintln!("error: no .git directory at {}", repo_path.display());
        return 1;
    }
    let hooks_dir = if git_dir.is_dir() {
        git_dir.join("hooks")
    } else {
        // Worktree case: `.git` is a file pointing at the real gitdir.
        match resolve_gitdir_file(&git_dir) {
            Some(p) => p.join("hooks"),
            None => {
                eprintln!("error: cannot resolve gitdir from {}", git_dir.display());
                return 1;
            }
        }
    };
    if let Err(e) = std::fs::create_dir_all(&hooks_dir) {
        eprintln!("error creating {}: {e}", hooks_dir.display());
        return 4;
    }

    let cmd = command.unwrap_or("glia build .").to_string();
    let mut written = 0;
    let mut removed = 0;
    let mut skipped = 0;

    for hook in HOOK_NAMES {
        let hook_path = hooks_dir.join(hook);
        if uninstall {
            if remove_glia_hook(&hook_path) {
                removed += 1;
            }
            continue;
        }
        // If a non-glia hook already exists, refuse to clobber.
        if hook_path.exists() && !is_glia_managed(&hook_path) {
            eprintln!(
                "skipping {}: existing hook is not glia-managed (preserve user content)",
                hook_path.display()
            );
            skipped += 1;
            continue;
        }
        let body = render_hook_script(hook, &cmd);
        if let Err(e) = std::fs::write(&hook_path, body) {
            eprintln!("error writing {}: {e}", hook_path.display());
            return 5;
        }
        // chmod +x — ignore failure on platforms without unix perms.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &hook_path,
                std::fs::Permissions::from_mode(0o755),
            );
        }
        written += 1;
    }

    if uninstall {
        eprintln!("removed {removed} glia-managed hook(s) from {}", hooks_dir.display());
    } else {
        eprintln!(
            "installed {written} hook(s) into {} (skipped {skipped} non-managed)",
            hooks_dir.display()
        );
        eprintln!("hook command: {cmd}");
        eprintln!();
        eprintln!("note: rebuild latency scales with repo size. To uninstall:");
        eprintln!("  glia install-hooks {} --uninstall", repo);
    }
    0
}

fn render_hook_script(hook_name: &str, cmd: &str) -> String {
    format!(
        r#"#!/bin/sh
{HOOK_MARKER}
# Managed by `glia install-hooks`. Re-run on changes to keep .gmap fresh.
# Hook: {hook_name}
# Edit `--command` and re-run install-hooks to change. Remove with `--uninstall`.

{cmd}
"#
    )
}

fn is_glia_managed(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.contains(HOOK_MARKER))
        .unwrap_or(false)
}

fn remove_glia_hook(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    if !is_glia_managed(path) {
        eprintln!(
            "skipping {}: not glia-managed",
            path.display()
        );
        return false;
    }
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("error removing {}: {e}", path.display());
            false
        }
    }
}

/// Read a `.git` file (worktree case) and extract the `gitdir:` path.
fn resolve_gitdir_file(git_file: &Path) -> Option<std::path::PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("gitdir:") {
            let p = std::path::PathBuf::from(rest.trim());
            if p.is_absolute() {
                return Some(p);
            }
            return git_file.parent().map(|parent| parent.join(p));
        }
    }
    None
}

// ----------------------------------------------------------------------------
// Pretty-name lookups (continued)
// ----------------------------------------------------------------------------

fn edge_category_name(c: repo_graph_core::EdgeCategoryId) -> &'static str {
    // Delegate to the canonical code-domain table (WP-I).
    edge_category::name(c)
}
