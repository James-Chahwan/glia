//! Merge pre-built layouts (LC.10b): combine the `.gmap` layouts of several
//! repos, built separately (a CI artefact per service, each repo's own
//! `<repo>/.glia/graph`), into the graph one build of all of them would give,
//! without their sources checked out.
//!
//! A layout holds each repo's per-language graphs as its own build left them.
//! What depends on WHICH repos were built together is recomputed over the
//! union, never concatenated:
//! - cross edges a resolver or a post-pass emitted (EVIDENCE emitter
//!   `resolver:*` / `pass:*`, LC.3a) are dropped and the resolvers and
//!   passes re-run over the union; every other cross edge (an overlay or
//!   history stage whose inputs only the member had) is kept once;
//! - the confidences a set-dependent pass overwrote are put back first
//!   (`MergedGraph::undo_pass_mutations`, LC.10a);
//! - repo labels are recomputed with [`crate::arch::repo_label_map`] over the
//!   recorded roots in merge order, the call a joint build makes, because a
//!   label deepens (`api` -> `x/api`) only when two basenames collide.
//!
//! Members are merged in the order given; each member's graphs keep their
//! stored shard order, which is `generate_many`'s slot order, so the merged
//! shards are the joint build's shards. What a merge cannot reproduce is
//! reported, never hidden: gRPC client needles (A5.2) are minted from the
//! proto services of the whole build and need the client's sources, so a
//! member missing another member's services gets a `[merge] caveat:` line.
//! There is no cross-repo node dedupe.
//!
//! A shard of another domain (`graph_type` other than `"code"`) is carried
//! through verbatim as a [`ForeignShard`] renamed `<member>-<name>`: the seam
//! a non-code graph joins a code graph by.
//!
//! Local paths only: no network, no git fetch, no private-repo auth.
//!
//! Marker, one line per merge:
//! `[merge] members=<n> (gmap=<g> repo=<r>) repos=<k> code_shards=<s> foreign_shards=<f> cross_edges=<c> kept_member_edges=<e> labels_stored=<l>`,
//! plus `[merge] caveat: <text>` per caveat and, from [`persist_merge`],
//! `[merge] wrote <dir> writer=<w> shards=<s> foreign=<f> members=<n>`.
//! CA.9: `[timing] build repos=<k> resolve=<ms> post=<ms> finalize=<ms> total=<ms> slowest_pass=<name>:<ms>`
//! once per merge (no `external_cells=`: a merge runs no external-cell
//! stage), and `[timing] persist writer=<w> <ms> dir=<dir>` from
//! [`persist_merge`].
//!
//! Module slot declared by L0.2: reached as `glia_engine::merge::<item>`,
//! never flattened into the crate root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::node_kind;
use glia_core::Edge;
use glia_graph::{MergedGraph, RepoGraph};
use glia_store::{
    LayoutExtras, MANIFEST_NAME, MemberMeta, read_layout_extras, read_manifest_lenient,
    write_merged_sharded_extras,
};

pub use glia_store::ForeignShard;

use crate::arch::repo_label_map;
use crate::build::timing::{BuildTimes, persist_marker};
use crate::persist::{
    LoadOutcome, default_layout_dir, layout_meta, load_layout, load_or_rebuild, write_self_ignore,
};
use crate::profile::run_code_passes;
use crate::{BUILD_STAMP, GenerateResult};

/// The default file name of a workspace manifest ([`read_workspace`]).
pub const WORKSPACE_FILE: &str = "glia.workspace.json";

/// One member of a merge, named for the merged manifest and the messages.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MergeMember {
    /// A pre-built layout directory. It is read as it is: without sources
    /// there is nothing to rebuild it from.
    Gmap { name: String, dir: PathBuf },
    /// A checked-out source tree: its default layout (`<root>/.glia/graph`),
    /// rebuilt first when missing or stale (LC.8's `load_or_rebuild`).
    Repo { name: String, root: PathBuf },
}

impl MergeMember {
    pub fn name(&self) -> &str {
        match self {
            MergeMember::Gmap { name, .. } | MergeMember::Repo { name, .. } => name,
        }
    }

    /// `gmap` / `repo`, as [`MemberMeta::source`] records it.
    fn source(&self) -> &'static str {
        match self {
            MergeMember::Gmap { .. } => "gmap",
            MergeMember::Repo { .. } => "repo",
        }
    }
}

/// A merge's output: the union as a [`GenerateResult`] (graph, parse errors,
/// labels, roots), the foreign-domain shards carried through, what the merge
/// could not reproduce exactly, and the members for the merged manifest.
#[non_exhaustive]
pub struct MergeResult {
    pub result: GenerateResult,
    pub foreign: Vec<ForeignShard>,
    pub caveats: Vec<String>,
    pub members: Vec<MemberMeta>,
}

/// The workspace manifest (`glia.workspace.json`, version 1).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Workspace {
    version: u32,
    members: Vec<WorkspaceMember>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceMember {
    name: String,
    #[serde(default)]
    gmap: Option<String>,
    #[serde(default)]
    repo: Option<String>,
}

/// Read a workspace manifest: `{"version": 1, "members": [{"name": "api",
/// "gmap": "../api/.glia/graph"}, {"name": "web", "repo": "../web"}]}`. Each
/// member names exactly one of `gmap` (a pre-built layout dir) or `repo` (a
/// source tree); a relative path resolves against the manifest's directory.
/// Member order is merge order. Unknown keys, another version, a URL (a merge
/// never fetches) and a name [`merge_layouts`] would refuse are errors.
pub fn read_workspace(path: &Path) -> Result<Vec<MergeMember>, String> {
    let fail = |e: &dyn std::fmt::Display| format!("workspace {}: {e}", path.display());
    let bytes = std::fs::read(path).map_err(|e| fail(&e))?;
    let ws: Workspace = serde_json::from_slice(&bytes).map_err(|e| fail(&e))?;
    if ws.version != 1 {
        return Err(fail(&format!("unsupported version {} (this build reads 1)", ws.version)));
    }
    let base = path.parent().unwrap_or(Path::new("."));
    let mut out = Vec::with_capacity(ws.members.len());
    for m in ws.members {
        let (key, value) = match (m.gmap, m.repo) {
            (Some(g), None) => ("gmap", g),
            (None, Some(r)) => ("repo", r),
            _ => {
                return Err(fail(&format!(
                    "member '{}' must name exactly one of \"gmap\" or \"repo\"",
                    m.name
                )));
            }
        };
        if value.contains("://") {
            return Err(fail(&format!(
                "member '{}': {key} {value:?} is a URL; a merge reads local paths only",
                m.name
            )));
        }
        let p = base.join(&value);
        out.push(match key {
            "gmap" => MergeMember::Gmap { name: m.name, dir: p },
            _ => MergeMember::Repo { name: m.name, root: p },
        });
    }
    check_names(&out).map_err(|e| fail(&e))?;
    Ok(out)
}

/// Member names are unique plain file stems: they prefix foreign shard names
/// and name members in the merged manifest.
fn check_names(members: &[MergeMember]) -> Result<(), String> {
    if members.is_empty() {
        return Err("no members to merge".to_string());
    }
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let mut seen = BTreeSet::new();
    for m in members {
        let name = m.name();
        if name.is_empty() || name.starts_with('.') || !name.chars().all(plain) {
            return Err(format!(
                "member name {name:?} must be a plain name ([A-Za-z0-9._-], no leading '.')"
            ));
        }
        if !seen.insert(name) {
            return Err(format!("two members are named '{name}'"));
        }
    }
    Ok(())
}

/// One member, loaded.
struct Loaded {
    result: GenerateResult,
    extras: LayoutExtras,
    build_stamp: String,
}

fn load_member(m: &MergeMember) -> Result<Loaded, String> {
    let fail = |e: &dyn std::fmt::Display| format!("member '{}': {e}", m.name());
    match m {
        MergeMember::Gmap { dir, .. } => {
            let result = load_layout(dir).map_err(|e| fail(&e))?;
            let extras = read_layout_extras(dir).map_err(|e| fail(&e))?;
            let build_stamp = read_manifest_lenient(dir).map(|l| l.build_stamp).unwrap_or_default();
            Ok(Loaded { result, extras, build_stamp })
        }
        MergeMember::Repo { root, .. } => {
            let dir = default_layout_dir(root);
            let (result, outcome) = load_or_rebuild(&dir, Some(root), true).map_err(|e| fail(&e))?;
            // A rebuilt layout's graph is this build's, whatever the disk holds
            // (`GLIA_NO_PERSIST=1` leaves the old layout in place): its foreign
            // shards, if any, still ride along, and a layout it could not read
            // carries none.
            let (extras, build_stamp) = match outcome {
                LoadOutcome::Fresh => (
                    read_layout_extras(&dir).map_err(|e| fail(&e))?,
                    read_manifest_lenient(&dir).map(|l| l.build_stamp).unwrap_or_default(),
                ),
                _ => (read_layout_extras(&dir).unwrap_or_default(), BUILD_STAMP.to_string()),
            };
            Ok(Loaded { result, extras, build_stamp })
        }
    }
}

/// Was `e` emitted by a stage the merge re-runs over the union (a resolver or
/// a post-pass, by its EVIDENCE emitter)? Such an edge is dropped from a
/// member and recomputed; any other cross edge is kept once.
fn is_recomputed(e: &Edge) -> bool {
    Evidence::of(e)
        .is_some_and(|ev| ev.emitter.starts_with("resolver:") || ev.emitter.starts_with("pass:"))
}

/// Merge `members` (in order) into the graph one build of all of them gives:
/// load each (a [`MergeMember::Repo`] is rebuilt first when stale), undo its
/// set-dependent pass mutations, drop its recomputable cross edges, then
/// concatenate the graphs, re-run every resolver and post-pass over the union
/// and append the kept member edges. See the module doc for what is
/// recomputed, what is carried and what is reported as a caveat.
///
/// Errors name the member: an unreadable or missing layout (a `gmap` member
/// that needs a rebuild is returned as is), a repo two members both hold (two
/// layouts built separately from one checkout identity, LB.1, would fuse
/// silently otherwise), a bad or repeated member name.
pub fn merge_layouts(members: &[MergeMember]) -> Result<MergeResult, String> {
    let started = Instant::now();
    check_names(members)?;
    let mut loaded = Vec::with_capacity(members.len());
    for m in members {
        let mut l = load_member(m)?;
        l.result.merged.undo_pass_mutations();
        loaded.push(l);
    }

    // A repo held by two members: never fused, never silently shadowed.
    let mut owner: BTreeMap<u64, usize> = BTreeMap::new();
    for (i, l) in loaded.iter().enumerate() {
        for id in member_repos(&l.result) {
            if let Some(&j) = owner.get(&id).filter(|&&j| j != i) {
                return Err(format!(
                    "members '{}' and '{}' both hold repo {id}: layouts built separately from \
                     one checkout identity (two worktrees of one clone, or two non-git dirs \
                     with the same basename) share a RepoId; build them together, or give \
                     each a distinct identity",
                    members[j].name(),
                    members[i].name()
                ));
            }
            owner.insert(id, i);
        }
    }

    let caveats = caveats_for(members, &loaded);
    for c in &caveats {
        eprintln!("[merge] caveat: {c}");
    }

    let mut graphs: Vec<RepoGraph> = Vec::new();
    let mut kept: Vec<Edge> = Vec::new();
    let mut foreign: Vec<ForeignShard> = Vec::new();
    let mut parse_errors: Vec<String> = Vec::new();
    let mut roots: BTreeMap<u64, String> = BTreeMap::new();
    let mut label_inputs: Vec<(u64, String)> = Vec::new();
    let mut stored_labels: Vec<(u64, String)> = Vec::new();
    let mut member_meta = Vec::with_capacity(members.len());
    for (m, l) in members.iter().zip(loaded) {
        let Loaded { result, extras, build_stamp } = l;
        for id in member_repos(&result) {
            let root = result.repo_roots.get(&id).filter(|r| Path::new(r.as_str()).is_dir());
            match (root, result.repo_labels.get(&id)) {
                (Some(root), _) => label_inputs.push((id, root.clone())),
                (None, Some(label)) => stored_labels.push((id, label.clone())),
                (None, None) => {}
            }
        }
        for (id, root) in &result.repo_roots {
            roots.entry(*id).or_insert_with(|| root.clone());
        }
        parse_errors.extend(result.parse_errors);
        let MergedGraph { graphs: g, cross_edges, .. } = result.merged;
        graphs.extend(g);
        kept.extend(cross_edges.into_iter().filter(|e| !is_recomputed(e)));
        foreign.extend(extras.foreign.into_iter().map(|f| ForeignShard {
            name: format!("{}-{}", m.name(), f.name),
            ..f
        }));
        member_meta.push(MemberMeta {
            name: m.name().to_string(),
            source: m.source().to_string(),
            build_stamp,
        });
    }

    let code_shards = graphs.len();
    let mut merged = MergedGraph::new(graphs);
    let report = run_code_passes(&mut merged);
    let kept_member_edges = kept.len();
    merged.cross_edges.extend(kept);
    merged.sort_cross_edges();

    let labels_stored = stored_labels.len();
    let repo_labels = union_labels(&label_inputs, stored_labels);
    let total_nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
    let total_edges: usize = merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
        + merged.cross_edges.len();
    let gmaps = members.iter().filter(|m| matches!(m, MergeMember::Gmap { .. })).count();
    eprintln!(
        "[merge] members={} (gmap={gmaps} repo={}) repos={} code_shards={code_shards} \
         foreign_shards={} cross_edges={} kept_member_edges={kept_member_edges} \
         labels_stored={labels_stored}",
        members.len(),
        members.len() - gmaps,
        owner.len(),
        foreign.len(),
        merged.cross_edges.len(),
    );
    // CA.9: the merge's build line. A member layout is loaded, not walked or
    // parsed (one rebuilt first prints its own lines), and its external cells
    // are already in it, so the line has passes and a total only.
    eprintln!("{}", BuildTimes::new(owner.len(), &report, None, started.elapsed()).marker());
    Ok(MergeResult {
        result: GenerateResult {
            merged,
            total_nodes,
            total_edges,
            parse_errors,
            repo_labels,
            repo_roots: roots,
        },
        foreign,
        caveats,
        members: member_meta,
    })
}

/// The repos a loaded member holds, in the order a joint build meets them:
/// by first graph (its shard order is `generate_many`'s argument order), then
/// any repo only its metadata names, by id.
fn member_repos(r: &GenerateResult) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    for g in &r.merged.graphs {
        if !out.contains(&g.repo.0) {
            out.push(g.repo.0);
        }
    }
    for id in r.repo_roots.keys().chain(r.repo_labels.keys()) {
        if !out.contains(id) {
            out.push(*id);
        }
    }
    out
}

/// Labels over the union: [`repo_label_map`] over every repo whose root is on
/// disk, in merge order (the call `generate_many` makes), then each repo
/// without one keeps its stored label, suffixed `#<id % 1000>` as
/// `repo_label_map` does when it would repeat a label already given.
fn union_labels(
    label_inputs: &[(u64, String)],
    stored: Vec<(u64, String)>,
) -> BTreeMap<u64, String> {
    let mut labels = repo_label_map(label_inputs);
    for (id, label) in stored {
        let label = if labels.values().any(|l| *l == label) {
            format!("{label}#{}", id % 1000)
        } else {
            label
        };
        labels.insert(id, label);
    }
    labels
}

/// What this merge cannot reproduce exactly. gRPC client needles (A5.2) are
/// minted, at build time, against the proto services of the WHOLE build; a
/// member built without another member's services never had those needles
/// applied, and its sources are not here to apply them now. Also a member
/// whose layout another glia build wrote (its graph is that build's).
fn caveats_for(members: &[MergeMember], loaded: &[Loaded]) -> Vec<String> {
    let services: Vec<BTreeSet<&str>> = loaded
        .iter()
        .map(|l| {
            l.result
                .merged
                .graphs
                .iter()
                .flat_map(|g| {
                    g.nodes.iter().filter_map(move |n| {
                        (g.nav.kind_by_id.get(&n.id) == Some(&node_kind::GRPC_SERVICE))
                            .then(|| g.nav.qname_by_id.get(&n.id).map(String::as_str))
                            .flatten()
                    })
                })
                .collect()
        })
        .collect();
    let mut out = Vec::new();
    for (i, m) in members.iter().enumerate() {
        let others: BTreeSet<&str> = services
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .flat_map(|(_, s)| s.iter().copied())
            .filter(|s| !services[i].contains(s))
            .collect();
        if !others.is_empty() {
            out.push(format!(
                "grpc client needles are per-member: {} proto services from other members were \
                 not applied to member '{}'",
                others.len(),
                m.name()
            ));
        }
    }
    for (m, l) in members.iter().zip(loaded) {
        if !l.build_stamp.is_empty() && l.build_stamp != BUILD_STAMP {
            out.push(format!(
                "member '{}' was built by another glia build ({}); its graphs are that build's",
                m.name(),
                l.build_stamp
            ));
        }
    }
    out
}

/// Write a merge to the layout at `dir`: the self-ignoring `.gitignore`, the
/// code shards and `cross_stack.gmap`, the foreign shards verbatim, and a
/// manifest with LC.7's layout metadata ([`layout_meta`]: labels, roots
/// relative to `dir`, parse errors), the post-pass undo and the members.
/// Then removes the files a previous manifest in `dir` named and this one
/// does not (a re-merge with fewer members), nothing else. `writer` names the
/// caller in the marker and the error text.
pub fn persist_merge(r: &MergeResult, dir: &Path, writer: &str) -> Result<(), String> {
    let started = Instant::now();
    let fail = |e: &dyn std::fmt::Display| format!("{writer}: persist merge to {}: {e}", dir.display());
    std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    write_self_ignore(dir).map_err(|e| fail(&e))?;
    let prior = manifest_files(dir);
    let g = &r.result;
    let meta = layout_meta(&g.repo_labels, &g.repo_roots, &g.parse_errors, dir);
    let manifest = write_merged_sharded_extras(&g.merged, &meta, &r.foreign, &r.members, dir)
        .map_err(|e| fail(&e))?;
    let live: BTreeSet<String> =
        manifest.shards.iter().chain(manifest.cross.as_ref()).map(|e| e.path.clone()).collect();
    for stale in prior.difference(&live) {
        if let Err(e) = std::fs::remove_file(dir.join(stale)) {
            eprintln!("[merge] warning: could not remove {stale} from {}: {e}", dir.display());
        }
    }
    eprintln!(
        "[merge] wrote {} writer={writer} shards={} foreign={} members={}",
        dir.display(),
        manifest.shards.len(),
        r.foreign.len(),
        r.members.len()
    );
    eprintln!("{}", persist_marker(writer, started.elapsed(), dir));
    Ok(())
}

/// The `.gmap` files the manifest in `dir` names (shards and cross), read
/// schema-agnostically; only plain file names, so nothing outside `dir` can be
/// named for removal. Empty without a readable manifest.
fn manifest_files(dir: &Path) -> BTreeSet<String> {
    let Ok(bytes) = std::fs::read(dir.join(MANIFEST_NAME)) else {
        return BTreeSet::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return BTreeSet::new();
    };
    let shards = v.get("shards").and_then(|s| s.as_array()).into_iter().flatten();
    shards
        .chain(v.get("cross"))
        .filter_map(|e| e.get("path")?.as_str())
        .filter(|p| p.ends_with(".gmap") && !p.contains(['/', '\\']) && !p.starts_with('.'))
        .map(str::to_string)
        .collect()
}
