//! Helpers shared by more than one command: the build options of the
//! global flags, graph generation for the `--with` commands, the summary
//! table and JSON dumps (`analyze`, `merge`), node lookups and pretty names,
//! `ImpactDirection` (`impact`, `blast-radius`), and 0.5.3's `## files`
//! table (EA.8: `blast-radius`, `diff-impact`, `review`); EC.8's
//! `CalibratedRankingArgs`, the file-ranking flags of every ranked answer
//! (no `--ranking` default: the repo's calibrated pick; `pack` since EC.9,
//! which retired EA.8's `RankingArgs`) and the `ranking:` line for
//! `blast-radius`, `diff-impact` and `review`.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use clap::ValueEnum;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::BlastOptions;
use glia_engine::calibrate::Selection;
use glia_engine::rank::{RankBase, RankCache, RankedFile, Ranking};
use glia_engine::{BuildOptions, GenerateResult, generate_many_opts, generate_one_opts};
use glia_graph::MergedGraph;
use glia_graph::roles::roles_in;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum ImpactDirection {
    /// Forward — what this entity reaches (calls, http_calls, etc.).
    Forward,
    /// Backward — what reaches this entity (predecessors).
    Backward,
    /// Both directions in one pass.
    Both,
}

/// The CLI names of the structural file rankings (`RankBase::name`).
const RANKINGS: [&str; 3] = ["ppr", "ppr-wide", "bfs"];

/// 0.5.3 (EC.8): the file-ranking flags of the calibrated answers,
/// flattened into `blast-radius`, `diff-impact`, `review` and (EC.9) `pack`.
/// `--ranking` has no default: absent, the answer applies the repo's
/// calibrated pick (`glia history calibrate`), else `ppr`.
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct CalibratedRankingArgs {
    /// The structural file ranking: ppr (PPR damping 0.5, a file ranks by
    /// its best node), ppr-wide (damping 0.85, a file ranks by the sum of its
    /// nodes) or bfs (carry-edge distance, then file degree); default: the
    /// repo's calibrated pick (glia history calibrate), else ppr.
    #[arg(
        long,
        value_name = "RANKING",
        value_parser = clap::builder::PossibleValuesParser::new(RANKINGS),
    )]
    ranking: Option<String>,
    /// Do not rank files that changed together with the seeds first (read
    /// from .glia/history-snapshot, written by `glia history sync`).
    #[arg(long)]
    no_cochange: bool,
}

impl CalibratedRankingArgs {
    /// The engine's `Ranking`: the base `--ranking` names (`ppr` when absent),
    /// co-change first unless `--no-cochange`. clap has already refused any
    /// other name; one that slipped through is the default base.
    pub(crate) fn ranking(&self) -> Ranking {
        let base = self.ranking.as_deref().map_or(Ok(RankBase::Ppr), RankBase::parse).unwrap_or(RankBase::Ppr);
        Ranking::new(base, !self.no_cochange)
    }

    /// Whether the answer applies the calibration: `--ranking` absent.
    pub(crate) fn selects(&self) -> bool {
        self.ranking.is_none()
    }

    /// Set `opts.ranking` and `opts.select` from the flags.
    pub(crate) fn apply(&self, opts: &mut BlastOptions) {
        opts.ranking = self.ranking();
        opts.select = self.selects();
    }
}

/// The ranking an answer applied: its selection's pick, else `asked`.
pub(crate) fn applied_ranking(selection: Option<&Selection>, asked: Ranking) -> Ranking {
    selection.and_then(|s| Ranking::from_harness_name(&s.pick)).unwrap_or(asked)
}

/// EC.8: the line under an impact table naming the ranking that ordered it:
/// `ranking: <pick> (calibrated, shape <k>, <t> commits)`, `ranking: <pick>
/// (fallback: <reason>, shape <k>)` or `ranking: <pick> (explicit, shape
/// <k>)`. Prints nothing without a selection (no seed resolved).
pub(crate) fn render_selection(selection: Option<&Selection>) {
    if let Some(line) = selection.map(selection_line) {
        println!();
        println!("{line}");
    }
}

fn selection_line(s: &Selection) -> String {
    let how = match (s.source, s.reason) {
        ("calibrated", _) => format!("calibrated, shape {}, {} commits", s.shape, s.test_commits),
        (source, Some(reason)) => format!("{source}: {reason}, shape {}", s.shape),
        (source, None) => format!("{source}, shape {}", s.shape),
    };
    format!("ranking: {} ({how})", s.pick)
}

/// The CLI's held ranking cache over a build: its repo roots name the history
/// snapshots co-change reads, its labels the `files` rows' repos. One per
/// command run (`[rank-cache] holder=cli`).
pub(crate) fn rank_cache(result: &GenerateResult) -> Arc<RankCache> {
    Arc::new(RankCache::new("cli", &result.repo_roots, &result.repo_labels))
}

/// The `## files` markdown table under a ranked answer: rank, file, the repo
/// when the build holds more than one, the answer's rows in the file, its
/// co-change share, then its pooled PPR score (`depth` under `bfs`). Prints
/// nothing for an empty list.
pub(crate) fn render_files(files: &[RankedFile], ranking: Ranking, many_repos: bool) {
    if files.is_empty() {
        return;
    }
    let bfs = ranking.base == RankBase::Bfs;
    let last = if bfs { "depth" } else { "ppr" };
    println!();
    println!("## files");
    println!();
    if many_repos {
        println!("| # | file | repo | rows | co-change | {last} |");
        println!("|--:|---|---|--:|--:|--:|");
    } else {
        println!("| # | file | rows | co-change | {last} |");
        println!("|--:|---|--:|--:|--:|");
    }
    for f in files {
        let score = if bfs {
            f.depth.map_or_else(|| "—".to_string(), |d| d.to_string())
        } else {
            f.ppr.map_or_else(|| "—".to_string(), |p| format!("{p:.3}"))
        };
        let repo = if many_repos { format!(" {} |", f.repo) } else { String::new() };
        println!("| {} | {} |{repo} {} | {:.3} | {score} |", f.rank, f.file, f.rows, f.cochange);
    }
}

/// The build options the global flags ask for (`--no-overlay`), set once by
/// `main` before any command runs. A process of the CLI is one invocation, so
/// a set-once value is the flag as parsed; every build of a command reads it
/// through [`build_options`].
static BUILD_OPTIONS: OnceLock<BuildOptions> = OnceLock::new();

/// Record the global build options (`main`, once, before dispatch).
pub(crate) fn set_build_options(opts: BuildOptions) {
    let _ = BUILD_OPTIONS.set(opts);
}

/// The build options of this invocation; the defaults (overlay applied) when
/// `main` set none (a unit test).
pub(crate) fn build_options() -> BuildOptions {
    BUILD_OPTIONS.get().cloned().unwrap_or_default()
}

/// Build a graph from one repo, or merge several (`--with`) so cross-service
/// resolvers fire across the boundary — shared by the P2/P3 commands. A
/// clean build (no parse cache) with this invocation's [`build_options`].
pub(crate) fn generate_for(repo: &str, with: &[String]) -> Result<GenerateResult, String> {
    let opts = build_options();
    if with.is_empty() {
        generate_one_opts(repo, false, &opts)
    } else {
        let mut repos = vec![repo.to_string()];
        repos.extend(with.iter().cloned());
        generate_many_opts(&repos, false, &opts)
    }
}

pub(crate) fn print_summary_table(r: &GenerateResult) {
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

/// The `--format json` document of `analyze` / `merge`: `{nodes, edges}`.
/// Each node is `{id, repo, kind, kind_name, name, qname, roles}`; `roles`
/// names the framework roles it plays (`["COMPONENT"]`, `[]` when none),
/// read through `roles_in` — since LB.3a a component or service is a CLASS /
/// FUNCTION carrying a ROLE cell, so `kind_name` alone no longer says so.
fn graph_json(merged: &MergedGraph) -> serde_json::Value {
    let mut nodes = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            let name = g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            let roles: Vec<&'static str> =
                roles_in(kind, &n.cells).into_iter().map(node_kind_name).collect();
            let kind = kind.map(|k| k.0).unwrap_or(0);
            nodes.push(serde_json::json!({
                "id": n.id.0,
                "repo": g.repo.0,
                "kind": kind,
                "kind_name": node_kind_name(NodeKindId(kind)),
                "name": name,
                "qname": qname,
                "roles": roles,
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
    serde_json::json!({ "nodes": nodes, "edges": edges })
}

pub(crate) fn print_json(merged: &MergedGraph) {
    println!("{}", serde_json::to_string(&graph_json(merged)).unwrap_or_default());
}

pub(crate) fn write_json_to(merged: &MergedGraph, out: &mut Vec<u8>) {
    use std::io::Write;
    let json = graph_json(merged);
    let _ = writeln!(out, "{}", serde_json::to_string(&json).unwrap_or_default());
}

pub(crate) struct NodeInfo {
    pub(crate) name: String,
    pub(crate) qname: String,
    pub(crate) kind_name: &'static str,
    pub(crate) repo: u64,
}

pub(crate) fn lookup_node_info(merged: &MergedGraph, id: NodeId) -> NodeInfo {
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
// Pretty-name lookups for code-domain ID constants.
// ----------------------------------------------------------------------------

pub(crate) fn node_kind_name(k: NodeKindId) -> &'static str {
    // Delegate to the canonical code-domain table (WP-I) — no local copy to go
    // stale when a kind is added.
    node_kind::name(k)
}

pub(crate) fn edge_category_name(c: glia_core::EdgeCategoryId) -> &'static str {
    // Delegate to the canonical code-domain table (WP-I).
    edge_category::name(c)
}
