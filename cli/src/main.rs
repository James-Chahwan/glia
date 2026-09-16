//! glia CLI — `glia <subcommand> [args]`.
//!
//! Subcommands:
//!   - `analyze <repo>` — walk a repo, build the merged graph, print a
//!     summary table + (optionally) Mermaid service-graph + JSON dump.
//!   - `arch <repo> [--with <repo>]` — the services in the stack and the
//!     cross-service links between them; table, `--json` or `--mermaid`.
//!   - `impact <repo> <qname>` — reachability walk; what does this entity
//!     touch, what touches it, cross-service blast-radius.
//!   - `merge <path> [<path>...] [--out <file>]` — build a single
//!     MergedGraph from N repo paths so cross-graph resolvers fire across
//!     the boundary; emit JSON.

use std::collections::BTreeMap;
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
    /// Architecture summary (A9.2): the services in this repo (or across the
    /// merged repos) and the cross-service links between them, each labelled
    /// with its mechanism (http/queue/grpc/ws/event/cli/graphql) and the
    /// channel it travels over.
    Arch {
        /// Path to the repo root.
        repo: String,
        /// Additional repos to merge in (cross-service). Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Emit a Mermaid `graph LR` instead of a table.
        #[arg(long, conflicts_with = "json")]
        mermaid: bool,
        /// Also show non-flow links (SHARES_SCHEMA / SHARES_CONFIG /
        /// DOCUMENTS / …), hidden by default: the co-ownership ones are O(n²)
        /// across merged repos and none of them is a call between services.
        #[arg(long)]
        include_shared: bool,
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
    /// Blast radius (P3): the complete, edge-category-aware, PPR-ranked, located
    /// closure around <qname> — what it affects / what affects it, across service
    /// boundaries, in one call. Excludes structural import/contain edges so the
    /// radius doesn't fan out through shared containers.
    BlastRadius {
        /// Path to the repo root.
        repo: String,
        /// Qname or simple name of the seed entity.
        qname: String,
        /// Additional repos to merge in (cross-service). Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Which way the radius spreads.
        #[arg(long, value_enum, default_value_t = ImpactDirection::Both)]
        direction: ImpactDirection,
        /// Maximum hops along carry edges.
        #[arg(long, default_value_t = 4)]
        depth: usize,
        /// Keep only the top-K by PPR score.
        #[arg(long)]
        top_k: Option<usize>,
        /// Drop nodes not reachable from an entrypoint (likely-dead code).
        #[arg(long)]
        live_only: bool,
        /// Restrict the answer to nodes whose file is under this repo-relative
        /// path (e.g. `services/api`). Narrows WITHIN a repo: each `--with`
        /// repo's paths are relative to its OWN root, so a path passed as a
        /// separate repo will not match here. Nodes with no file (ENDPOINT /
        /// ROUTE / doc spaces) are kept, not dropped.
        #[arg(long)]
        scope: Option<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Docs-for (tier-4 P3): the doc sections that DOCUMENTS <qname> — "what are
    /// the rules for X?" — located.
    DocsFor {
        /// Path to the repo root.
        repo: String,
        /// Qname or simple name of the code entity.
        qname: String,
        /// Additional repos to merge in. Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Restrict the answer to nodes whose file is under this repo-relative
        /// path (e.g. `services/api`). Narrows WITHIN a repo: each `--with`
        /// repo's paths are relative to its OWN root, so a path passed as a
        /// separate repo will not match here. Nodes with no file (ENDPOINT /
        /// ROUTE / doc spaces) are kept, not dropped.
        #[arg(long)]
        scope: Option<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Coverage (P2): for the languages present in the repo, the known
    /// extraction caveats + edges-found per flagged category, so you fall back
    /// to grep deliberately where glia is known-partial.
    Coverage {
        /// Path to the repo root.
        repo: String,
        /// Additional repos to merge in. Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Cross-stack trace (P3): follow <feature> forward across service
    /// boundaries and print the ordered path, each hop labeled with its
    /// mechanism (http/queue/grpc/call) and whether it crossed a service.
    Trace {
        /// Path to the repo root.
        repo: String,
        /// Qname or simple name of the feature/entry entity.
        feature: String,
        /// Additional repos to merge in (cross-service). Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Maximum hops.
        #[arg(long, default_value_t = 6)]
        depth: usize,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Resolve (P3): a failure/change signal (stacktrace, diff, test id) → the
    /// ranked, located nodes it points at, in one call.
    Resolve {
        /// Path to the repo root.
        repo: String,
        /// The signal text (stacktrace, diff hunk, test id, or free text).
        signal: String,
        /// Additional repos to merge in. Repeatable.
        #[arg(long)]
        with: Vec<String>,
        /// Signal kind: `auto` (sniff), `stacktrace`, `test`, or `diff`.
        #[arg(long, default_value = "auto")]
        kind: String,
        /// Keep only the top-K by relevance.
        #[arg(long)]
        top_k: Option<usize>,
        /// Restrict the answer to nodes whose file is under this repo-relative
        /// path (e.g. `services/api`). Narrows WITHIN a repo: each `--with`
        /// repo's paths are relative to its OWN root, so a path passed as a
        /// separate repo will not match here. Nodes with no file (ENDPOINT /
        /// ROUTE / doc spaces) are kept, not dropped.
        #[arg(long)]
        scope: Option<String>,

        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
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
    /// Sync external docs (Confluence) to/from a repo's doc snapshot. This is
    /// the **network** step, deliberately separate from `build` so the
    /// byte-identical build stays deterministic: `sync` fetches into
    /// `<repo>/.glia/docs-snapshot/`, then `build` ingests that snapshot.
    Docs {
        #[command(subcommand)]
        action: DocsCmd,
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

#[derive(Subcommand, Debug)]
enum DocsCmd {
    /// Pull every page in a Confluence space into `<repo>/.glia/docs-snapshot`.
    /// Then `glia build <repo>` ingests it (DOC_SPACE + DOC_SECTION + doc→code
    /// DOCUMENTS links). Credentials resolve flag → env → `./.env`
    /// (CONFLUENCE_SITE / CONFLUENCE_EMAIL / CONFLUENCE_TOKEN).
    Sync {
        /// Repo whose snapshot to write.
        repo: String,
        /// Confluence space key (e.g. `MFS`).
        #[arg(long)]
        space: String,
        /// Keep only pages whose title matches. Repeatable; `*` wildcard;
        /// case-insensitive substring when the pattern has no `*`. Applied
        /// locally after the fetch (Confluence has no title-glob parameter),
        /// so it scopes what is ingested, not what is downloaded.
        #[arg(long, value_name = "PATTERN")]
        include: Vec<String>,
        /// Drop pages whose title matches. Repeatable; same syntax as
        /// `--include`, and wins over it.
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
        #[arg(long)]
        site: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Push a storage-format (XHTML) page body to Confluence — create a new
    /// page, or update an existing one with `--page-id`. With `--markdown` the
    /// file is converted from markdown first.
    Push {
        /// Confluence space key.
        #[arg(long)]
        space: String,
        /// Page title.
        #[arg(long)]
        title: String,
        /// File containing the Confluence storage-format (XHTML) body.
        #[arg(long)]
        file: String,
        /// Treat --file as markdown and convert it to Confluence storage format
        /// first (headings, fenced code, lists, inline code, links).
        #[arg(long)]
        markdown: bool,
        /// Update this page id instead of creating a new page.
        #[arg(long)]
        page_id: Option<String>,
        #[arg(long)]
        site: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        token: Option<String>,
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
        Cmd::Arch { repo, with, json, mermaid, include_shared } => {
            cmd_arch(&repo, &with, json, mermaid, include_shared)
        }
        Cmd::Impact {
            repo,
            qname,
            direction,
            depth,
        } => cmd_impact(&repo, &qname, direction, depth),
        Cmd::BlastRadius {
            repo,
            qname,
            with,
            direction,
            depth,
            top_k,
            live_only,
            scope,
            json,
        } => cmd_blast_radius(
            &repo,
            &qname,
            &with,
            direction,
            depth,
            top_k,
            live_only,
            scope.as_deref(),
            json,
        ),
        Cmd::DocsFor { repo, qname, with, scope, json } => {
            cmd_docs_for(&repo, &qname, &with, scope.as_deref(), json)
        }
        Cmd::Coverage { repo, with, json } => cmd_coverage(&repo, &with, json),
        Cmd::Trace { repo, feature, with, depth, json } => {
            cmd_trace(&repo, &feature, &with, depth, json)
        }
        Cmd::Resolve { repo, signal, with, kind, top_k, scope, json } => {
            cmd_resolve(&repo, &signal, &with, &kind, top_k, scope.as_deref(), json)
        }
        Cmd::Merge { repos, out } => cmd_merge(&repos, out.as_deref()),
        Cmd::Build { repo, out, no_incremental } => {
            cmd_build(&repo, out.as_deref(), !no_incremental)
        }
        Cmd::Docs { action } => cmd_docs(action),
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
        // A9.4: routed at the A9.2 service map. The old `print_mermaid`
        // partitioned by RepoId and labelled each node `repo <u64 hash>`, so on
        // a single repo — which is all `analyze` ever builds — it rendered ONE
        // hash node and ZERO arrows even with 96 cross-edges in the graph.
        // Retired here rather than left beside `print_service_mermaid`: two
        // mermaid paths is how the dead one survived this long.
        AnalyzeFormat::Mermaid => {
            let mut map = repo_graph_engine::service_map(&result.merged, &result.repo_labels);
            // Same view, same default as `glia arch --mermaid`: flows only.
            // `glia arch --include-shared` is where the rest lives.
            drop_non_flow_links(&mut map);
            print_service_mermaid(&map);
        }
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

// ----------------------------------------------------------------------------
// `blast-radius` (P3)
// ----------------------------------------------------------------------------

/// Build a graph from one repo, or merge several (`--with`) so cross-service
/// resolvers fire across the boundary — shared by the P2/P3 commands.
fn generate_for(repo: &str, with: &[String]) -> Result<GenerateResult, String> {
    if with.is_empty() {
        generate_one(repo)
    } else {
        let mut repos = vec![repo.to_string()];
        repos.extend(with.iter().cloned());
        generate_many(&repos)
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_blast_radius(
    repo: &str,
    qname: &str,
    with: &[String],
    direction: ImpactDirection,
    depth: usize,
    top_k: Option<usize>,
    live_only: bool,
    scope: Option<&str>,
    json: bool,
) -> i32 {
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let dir = match direction {
        ImpactDirection::Forward => "forward",
        ImpactDirection::Backward => "backward",
        ImpactDirection::Both => "both",
    };
    let answer = match repo_graph_engine::blast_radius_by_qname(
        &result.merged,
        qname,
        dir,
        depth,
        top_k,
        live_only,
        scope,
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
            return 3;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia blast-radius `{qname}` ({dir}, depth ≤ {depth})");
    println!();
    if answer.is_empty() {
        println!("_(nothing in radius)_");
        return 0;
    }
    println!("| score | depth | live | via | kind | qname | location |");
    println!("|--:|--:|:-:|---|---|---|---|");
    for a in &answer {
        let loc = match (&a.file, a.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        let live = if a.live { "●" } else { "⊘" };
        println!(
            "| {:.4} | {} | {} | {} | {} | `{}` | {} |",
            a.score, a.depth, live, a.reason, a.kind, a.qname, loc
        );
    }
    0
}

// ----------------------------------------------------------------------------
// `docs-for` (tier-4 P3 payoff)
// ----------------------------------------------------------------------------

fn cmd_docs_for(
    repo: &str,
    qname: &str,
    with: &[String],
    scope: Option<&str>,
    json: bool,
) -> i32 {
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let docs = match repo_graph_engine::governing_docs(&result.merged, qname, scope) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
            return 3;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&docs).unwrap_or_default());
        return 0;
    }
    println!("# glia docs-for `{qname}`");
    println!();
    if docs.is_empty() {
        println!("_(no governing docs)_");
        return 0;
    }
    println!("| kind | doc section | location |");
    println!("|---|---|---|");
    for d in &docs {
        let loc = match (&d.file, d.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        println!("| {} | `{}` | {} |", d.kind, d.qname, loc);
    }
    0
}

// ----------------------------------------------------------------------------
// `coverage` (P2)
// ----------------------------------------------------------------------------

fn cmd_coverage(repo: &str, with: &[String], json: bool) -> i32 {
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let report = repo_graph_engine::coverage_report(&result.merged);
    if json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return 0;
    }
    println!("# glia coverage `{repo}`");
    println!();
    println!("_Where glia is known-partial — verify these dimensions with grep._");
    println!();
    println!("| language | edge | found | caveat → verify |");
    println!("|---|---|--:|---|");
    for n in &report {
        println!(
            "| {} | {} | {} | {} — _{}_ |",
            n.language, n.edge_category, n.edges_found, n.note, n.verify
        );
    }
    0
}

// ----------------------------------------------------------------------------
// `arch` (A9.4 — the human surface over A9.2's service map)
// ----------------------------------------------------------------------------

fn cmd_arch(repo: &str, with: &[String], json: bool, mermaid: bool, include_shared: bool) -> i32 {
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    // The `[arch] …` fired_on marker is emitted here, inside the engine.
    let mut map = repo_graph_engine::service_map(&result.merged, &result.repo_labels);
    if !include_shared {
        drop_non_flow_links(&mut map);
    }
    if json {
        println!("{}", serde_json::to_string(&map).unwrap_or_default());
    } else if mermaid {
        print_service_mermaid(&map);
    } else {
        print_service_table(repo, &map);
    }
    0
}

/// Keep only the directional flow links (`FLOW_MECHANISMS`) — a real call from
/// `from` to `to`. Everything else `cross_links` returns is co-ownership
/// (`SHARES_SCHEMA`, `SHARES_DEPENDENCY`, …) or documentation (`DOCUMENTS`):
/// true, but not traffic. The `SHARES_*` ones are also O(n²) across merged
/// repos, so one shared npm dependency would bury every real call.
///
/// Recomputes `inbound` / `outbound`: those count *surviving link rows*, so
/// leaving them at the unfiltered value would print an `in`/`out` column that
/// no visible row accounts for.
fn drop_non_flow_links(map: &mut repo_graph_engine::ServiceMap) {
    let flows: Vec<&'static str> = repo_graph_engine::arch::FLOW_MECHANISMS
        .iter()
        .map(|c| edge_category_name(*c))
        .collect();
    map.links.retain(|l| flows.contains(&l.mechanism));
    let mut io: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for l in &map.links {
        io.entry(l.from.as_str()).or_default().1 += 1;
        io.entry(l.to.as_str()).or_default().0 += 1;
    }
    for s in &mut map.services {
        let (inbound, outbound) = io.get(s.id.as_str()).copied().unwrap_or((0, 0));
        s.inbound = inbound;
        s.outbound = outbound;
    }
}

fn print_service_table(repo: &str, map: &repo_graph_engine::ServiceMap) {
    println!("# glia arch `{repo}` (keying: {})", map.keying);
    println!();
    println!("| service | repo | languages | files | nodes | routes | endpoints | in | out |");
    println!("|---|---|---|--:|--:|--:|--:|--:|--:|");
    for s in &map.services {
        let langs = if s.languages.is_empty() {
            "—".to_string()
        } else {
            s.languages.join(", ")
        };
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.id, s.repo, langs, s.files, s.nodes, s.routes, s.endpoints, s.inbound, s.outbound
        );
    }
    println!();
    if map.links.is_empty() {
        println!("_(no cross-service links)_");
    } else {
        println!("| from | → to | mechanism | channel | × |");
        println!("|---|---|---|---|--:|");
        for l in &map.links {
            let channel = if l.channel.is_empty() { "—" } else { &l.channel };
            println!(
                "| {} | {} | {} | {} | {} |",
                l.from, l.to, l.mechanism, channel, l.count
            );
        }
    }
    if map.self_links > 0 || map.unlocated_nodes > 0 {
        println!();
        println!(
            "_Dropped: {} self-link(s) (both ends in one service), {} unlocated node(s) (no file cell)._",
            map.self_links, map.unlocated_nodes
        );
    }
}

fn print_service_mermaid(map: &repo_graph_engine::ServiceMap) {
    // Mermaid ids must be `[A-Za-z0-9_]`, and a service id is a directory path
    // — so the id is positional (`svc{i}` over the already-sorted `services`)
    // and the real name lives in the label.
    let idx: BTreeMap<&str, usize> = map
        .services
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    println!("```mermaid");
    println!("graph LR");
    for (i, s) in map.services.iter().enumerate() {
        println!("    svc{i}[\"{}\"]", mermaid_label(&s.id));
    }
    // Collapse the per-channel rows to one arrow per (from, to, mechanism).
    let mut agg: BTreeMap<(usize, usize, &'static str), (usize, Vec<&str>)> = BTreeMap::new();
    for l in &map.links {
        let (Some(&f), Some(&t)) = (idx.get(l.from.as_str()), idx.get(l.to.as_str())) else {
            continue;
        };
        let e = agg.entry((f, t, l.mechanism)).or_insert((0, Vec::new()));
        e.0 += l.count;
        if !l.channel.is_empty() {
            e.1.push(l.channel.as_str());
        }
    }
    for ((f, t, mech), (count, channels)) in agg {
        let shown: Vec<String> = channels.iter().take(3).map(|c| mermaid_label(c)).collect();
        let mut label = format!("{mech} ×{count}");
        if !shown.is_empty() {
            label.push_str("<br/>");
            label.push_str(&shown.join(", "));
            if channels.len() > 3 {
                label.push_str(&format!(" +{} more", channels.len() - 3));
            }
        }
        println!("    svc{f} -->|\"{label}\"| svc{t}");
    }
    println!("```");
}

/// Escape the characters that break a `|"…"|` Mermaid edge label. Channels are
/// raw route templates and topic literals, so `"`, `|`, `<` and `>` all turn up
/// in practice (`${…}`, `{id}`, `<T>`), and a single raw `"` silently breaks the
/// whole diagram in the renderer rather than just that one edge.
fn mermaid_label(s: &str) -> String {
    s.replace('"', "#quot;")
        .replace('|', "#124;")
        .replace('<', "#lt;")
        .replace('>', "#gt;")
}

// ----------------------------------------------------------------------------
// `trace` (P3 cross_stack_trace)
// ----------------------------------------------------------------------------

fn cmd_trace(repo: &str, feature: &str, with: &[String], depth: usize, json: bool) -> i32 {
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let hops = match repo_graph_engine::cross_stack_trace(&result.merged, feature, depth) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("hint: use `glia analyze {repo} --format json` to list qnames.");
            return 3;
        }
    };
    if json {
        println!("{}", serde_json::to_string(&hops).unwrap_or_default());
        return 0;
    }
    println!("# glia trace `{feature}` (depth ≤ {depth})");
    println!();
    if hops.is_empty() {
        println!("_(no outward flow)_");
        return 0;
    }
    println!("| depth | mechanism | xsvc | from | → to | location |");
    println!("|--:|---|:-:|---|---|---|");
    for h in &hops {
        let loc = match (&h.to_file, h.to_line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        let xsvc = if h.cross_service { "✔" } else { "" };
        println!(
            "| {} | {} | {} | `{}` | `{}` ({}) | {} |",
            h.depth, h.mechanism, xsvc, h.from_qname, h.to_qname, h.to_kind, loc
        );
    }
    0
}

// ----------------------------------------------------------------------------
// `resolve` (P3)
// ----------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn cmd_resolve(
    repo: &str,
    signal: &str,
    with: &[String],
    kind: &str,
    top_k: Option<usize>,
    scope: Option<&str>,
    json: bool,
) -> i32 {
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let answer =
        repo_graph_engine::resolve_signal_located(&result.merged, signal, kind, top_k, scope);
    if json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }
    println!("# glia resolve ({kind})");
    println!();
    if answer.is_empty() {
        println!("_(nothing resolved)_");
        return 0;
    }
    println!("| score | kind | qname | location |");
    println!("|--:|---|---|---|");
    for a in &answer {
        let loc = match (&a.file, a.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "—".to_string(),
        };
        println!("| {:.4} | {} | `{}` | {} |", a.score, a.kind, a.qname, loc);
    }
    0
}

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

// ----------------------------------------------------------------------------
// `docs` — Confluence sync (network step; snapshot feeds the offline build)
// ----------------------------------------------------------------------------

fn cmd_docs(action: DocsCmd) -> i32 {
    use repo_graph_doc_sources::confluence_rest::{self, Config};
    match action {
        DocsCmd::Sync { repo, space, include, exclude, site, email, token } => {
            let cfg = match Config::resolve(site, email, token) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            let pages = match confluence_rest::pull_space(&cfg, &space) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: pulling space {space}: {e}");
                    return 1;
                }
            };
            let filter = repo_graph_doc_sources::TitleFilter::new(&include, &exclude);
            let fetched = pages.len();
            let pages: Vec<_> = pages.into_iter().filter(|p| filter.keep(&p.title)).collect();
            eprintln!(
                "[docs] sync space={space} fetched={fetched} kept={} include={} exclude={}",
                pages.len(),
                include.len(),
                exclude.len()
            );
            if pages.is_empty() && fetched > 0 {
                eprintln!(
                    "error: every one of {fetched} fetched page(s) was filtered out; refusing to overwrite the snapshot with an empty manifest"
                );
                return 1;
            }
            let records: Vec<_> = pages.iter().map(repo_graph_doc_sources::record_from_page).collect();
            match repo_graph_doc_sources::write_snapshot(Path::new(&repo), &records) {
                Ok(manifest) => {
                    println!("synced {} page(s) from space {space} → {}", records.len(), manifest.display());
                    println!("run `glia build {repo}` to ingest.");
                    0
                }
                Err(e) => {
                    eprintln!("error: writing snapshot: {e}");
                    1
                }
            }
        }
        DocsCmd::Push { space, title, file, markdown, page_id, site, email, token } => {
            let cfg = match Config::resolve(site, email, token) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: {e}");
                    return 2;
                }
            };
            let body = match std::fs::read_to_string(&file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: reading {file}: {e}");
                    return 1;
                }
            };
            // Converted before any network call, so the marker below proves the
            // conversion ran even when the push itself fails on credentials.
            let storage = if markdown {
                let (s, st) =
                    repo_graph_doc_sources::markdown::markdown_to_storage_with_stats(&body);
                eprintln!(
                    "[docs] push markdown→storage: {} md bytes → {} storage bytes (h={} code={} link={} inline={})",
                    body.len(),
                    s.len(),
                    st.headings,
                    st.code_blocks,
                    st.links,
                    st.inline_code
                );
                s
            } else {
                body
            };
            let result = match &page_id {
                Some(id) => confluence_rest::update_page(&cfg, id, &space, &title, &storage),
                None => confluence_rest::create_page(&cfg, &space, &title, &storage),
            };
            match result {
                Ok(p) => {
                    let verb = if page_id.is_some() { "updated" } else { "created" };
                    println!("{verb} page {} (v{}) — {}", p.id, p.version, p.url);
                    0
                }
                Err(e) => {
                    eprintln!("error: pushing page: {e}");
                    1
                }
            }
        }
    }
}

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

// ----------------------------------------------------------------------------
// tests (A9.4 — the two `arch` pieces with a real failure mode)
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_engine::{ServiceLink, ServiceMap, ServiceSummary};

    fn svc(id: &str) -> ServiceSummary {
        ServiceSummary {
            id: id.to_string(),
            repo: "r".to_string(),
            languages: vec![],
            files: 0,
            nodes: 0,
            routes: 0,
            endpoints: 0,
            cli_commands: 0,
            queue_consumers: 0,
            inbound: 9,
            outbound: 9,
        }
    }

    fn link(from: &str, to: &str, mechanism: &'static str) -> ServiceLink {
        ServiceLink {
            from: from.to_string(),
            to: to.to_string(),
            mechanism,
            channel: "GET /x".to_string(),
            count: 1,
            confidence: "strong",
            example_from_qname: String::new(),
            example_to_qname: String::new(),
        }
    }

    #[test]
    fn non_flow_links_are_dropped_and_io_recounted() {
        let mut map = ServiceMap {
            keying: "top_level_dir",
            services: vec![svc("web"), svc("api"), svc("docs")],
            links: vec![
                link("web", "api", "HTTP_CALLS"),
                link("web", "api", "SHARES_SCHEMA"),
                link("docs", "api", "DOCUMENTS"),
            ],
            self_links: 0,
            unlocated_nodes: 0,
        };
        drop_non_flow_links(&mut map);
        assert_eq!(map.links.len(), 1, "only the HTTP_CALLS flow survives");
        assert_eq!(map.links[0].mechanism, "HTTP_CALLS");
        // in/out must describe the SURVIVING rows, not the seeded 9s — a stale
        // count here prints an `in`/`out` no visible table row accounts for.
        let by = |id: &str| {
            let s = map.services.iter().find(|s| s.id == id).unwrap();
            (s.inbound, s.outbound)
        };
        assert_eq!(by("web"), (0, 1));
        assert_eq!(by("api"), (1, 0));
        assert_eq!(by("docs"), (0, 0), "its only link was dropped");
    }

    #[test]
    fn mermaid_label_escapes_what_breaks_the_renderer() {
        // A raw `"` closes the `|"…"|` label early and breaks the WHOLE
        // diagram, not just that edge; `${…}` and `{id}` are ordinary route
        // channels, so this is the common case, not the corner one.
        assert_eq!(
            mermaid_label(r#"GET /u/${id}/"a"|b<c>"#),
            "GET /u/${id}/#quot;a#quot;#124;b#lt;c#gt;"
        );
        assert_eq!(mermaid_label("GET /users"), "GET /users");
    }
}
