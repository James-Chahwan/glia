//! Move-stable node identity and move detection (LB.6).
//!
//! Qnames are path-derived, so moving a file is a delete + add to anything
//! that keys on qnames: a stored cell hint, an Engram identity, a graph delta.
//! A build cannot know about a move without history, and history in the graph
//! would break `incremental == clean` (engine `byte_identical`). So identity is
//!
//! 1. **derived** on demand from what every `.gmap` already stores
//!    ([`identity_of`]): the nav name chain up to the node's MODULE, its kind,
//!    the basename of its POSITION file and a hash of its CODE text; and
//! 2. **matched against history** only where the caller has history: a stored
//!    hint ([`IdentityIndex::rebind`], for LF cell sidecars and Engram) or a
//!    prior graph ([`detect_moves_with`], for the LE.1 delta and LG.8 export).
//!
//! Evidence: a [`MoveTier::Declared`] move is a FACT (the caller's VCS said
//! so); [`MoveTier::Identical`] and [`MoveTier::SameName`] are HEURISTIC and
//! callers report the tier. Hints are NOT unique (two `index.ts` modules share
//! one), so a rebind can be [`Rebind::Ambiguous`] and never picks.
//!
//! Not handled: a file moved AND renamed AND edited with no declared rename; a
//! symbol renamed inside a moved file (a delete + add); MODULE qnames that
//! collide across languages in one dir (`Widget.java` + `Widget.kt` are one
//! `dir::Widget` key, so they pair as one file); a path owning several removed
//! or added MODULEs is never paired by path.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_code_domain::{cell_type, node_kind};
use glia_core::{CellPayload, CellTypeId, Node, NodeId, NodeKindId};

use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// Code-domain constants, in one block so LD.14 can lift them into the profile.
const MODULE: NodeKindId = node_kind::MODULE;
const CODE: CellTypeId = cell_type::CODE;
const POSITION: CellTypeId = cell_type::POSITION;
/// Bodies shorter than this (whitespace-collapsed chars) carry no hash: a
/// one-line stub matches too much to be evidence.
const MIN_BODY_CHARS: usize = 40;
const HINT_VERSION: &str = "v1";
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A node's path-independent identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity {
    pub kind: NodeKindId,
    /// Basename of the node's POSITION file (`users.py`).
    pub file: String,
    /// Nav names from the node up to, excluding, its MODULE, joined by `::`
    /// (`User::get`); `""` for the MODULE itself.
    pub local: String,
    /// FNV-1a 64 of the CODE text with whitespace runs collapsed; 0 = no CODE
    /// cell or under 40 chars (a real hash of 0 is stored as 1).
    pub body: u64,
}

impl Identity {
    /// `v1|<kind id>|<file>|<body:016x>|<local>`, with `%` and `|` in `file`
    /// and `local` escaped as `%25` / `%7C`.
    pub fn hint(&self) -> String {
        format!(
            "{HINT_VERSION}|{}|{}|{:016x}|{}",
            self.kind.0,
            escape(&self.file),
            self.body,
            escape(&self.local)
        )
    }

    /// Inverse of [`Identity::hint`]; `None` for any other version or shape.
    pub fn parse_hint(s: &str) -> Option<Identity> {
        let parts: Vec<&str> = s.split('|').collect();
        let [version, kind, file, body, local] = parts.as_slice() else {
            return None;
        };
        if *version != HINT_VERSION
            || body.len() != 16
            || !body.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Identity {
            kind: NodeKindId(kind.parse().ok()?),
            file: unescape(file)?,
            local: unescape(local)?,
            body: u64::from_str_radix(body, 16).ok()?,
        })
    }
}

/// The identity of `id`, or `None` when it has no MODULE ancestor (ROUTE,
/// ENDPOINT, queue, config ... — their qnames are not path-derived) or no
/// POSITION file. One linear node lookup: for many nodes build an
/// [`IdentityIndex`] instead.
pub fn identity_of(merged: &MergedGraph, id: NodeId) -> Option<Identity> {
    let g = merged
        .graphs
        .iter()
        .find(|g| g.nav.kind_by_id.contains_key(&id))?;
    derive(g, |n| g.nodes.iter().find(|x| x.id == n), id).map(|(_, ident)| ident)
}

/// Where a moved node is found: which evidence tier bound it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MoveTier {
    /// FACT: the caller declared the rename (git's rename list).
    Declared,
    /// HEURISTIC: equal body hash.
    Identical,
    /// HEURISTIC: equal file basename (and, for files, shared declarations).
    SameName,
}

impl MoveTier {
    pub fn as_str(self) -> &'static str {
        match self {
            MoveTier::Declared => "declared",
            MoveTier::Identical => "identical",
            MoveTier::SameName => "same-name",
        }
    }
}

/// The outcome of re-binding a stored `(qname, kind, hint)` to a graph.
/// `Moved` is HEURISTIC evidence; `Ambiguous` candidates are sorted by id and
/// must be reported, never silently applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rebind {
    Exact(NodeId),
    Moved { id: NodeId, tier: MoveTier },
    Ambiguous(Vec<NodeId>),
    Orphan,
}

/// Identity lookups over one graph. Building reads every CODE cell once
/// (O(repo bytes)): build once per graph, never per query.
#[derive(Debug, Clone, Default)]
pub struct IdentityIndex {
    by_key: BTreeMap<(u32, String, String), Vec<NodeId>>,
    by_body: BTreeMap<(u32, u64), Vec<NodeId>>,
    by_qname: BTreeMap<String, Vec<(u32, NodeId)>>,
}

impl IdentityIndex {
    pub fn build(merged: &MergedGraph) -> Self {
        let mut idx = IdentityIndex::default();
        let mut seen: HashSet<NodeId> = HashSet::new();
        for g in &merged.graphs {
            let by_id = node_map(g);
            for n in g.nodes.iter().filter(|n| seen.insert(n.id)) {
                if let (Some(q), Some(k)) =
                    (g.nav.qname_by_id.get(&n.id), g.nav.kind_by_id.get(&n.id))
                {
                    idx.by_qname.entry(q.clone()).or_default().push((k.0, n.id));
                }
                let Some((_, ident)) = derive(g, |x| by_id.get(&x).copied(), n.id) else {
                    continue;
                };
                if ident.body != 0 {
                    idx.by_body
                        .entry((ident.kind.0, ident.body))
                        .or_default()
                        .push(n.id);
                }
                idx.by_key
                    .entry((ident.kind.0, ident.file, ident.local))
                    .or_default()
                    .push(n.id);
            }
        }
        idx.by_key.values_mut().for_each(sort_ids);
        idx.by_body.values_mut().for_each(sort_ids);
        idx.by_qname
            .values_mut()
            .for_each(|v| v.sort_by_key(|(k, id)| (id.0, *k)));
        idx
    }

    /// Re-bind a stored node: its exact `(kind?, qname)` first; failing that,
    /// the `hint`'s `(kind, file, local)` key (SameName), then its body hash
    /// (Identical). A hint whose kind contradicts `kind` is ignored.
    pub fn rebind(&self, qname: &str, kind: Option<NodeKindId>, hint: Option<&str>) -> Rebind {
        let exact: Vec<NodeId> = self
            .by_qname
            .get(qname)
            .into_iter()
            .flatten()
            .filter(|(k, _)| kind.is_none_or(|want| want.0 == *k))
            .map(|(_, id)| *id)
            .collect();
        if let [only] = exact.as_slice() {
            return Rebind::Exact(*only);
        }
        let hint = hint
            .and_then(Identity::parse_hint)
            .filter(|h| kind.is_none_or(|k| k == h.kind));
        let Some(h) = hint else {
            return if exact.is_empty() {
                Rebind::Orphan
            } else {
                Rebind::Ambiguous(exact)
            };
        };
        let key_hits: &[NodeId] = self
            .by_key
            .get(&(h.kind.0, h.file.clone(), h.local.clone()))
            .map_or(&[], Vec::as_slice);
        if !exact.is_empty() {
            // Several nodes carry the qname: the hint may single one out.
            let narrowed: Vec<NodeId> = exact
                .iter()
                .copied()
                .filter(|id| key_hits.contains(id))
                .collect();
            return match narrowed.as_slice() {
                [only] => Rebind::Exact(*only),
                _ => Rebind::Ambiguous(exact),
            };
        }
        let body_hits: &[NodeId] = match h.body {
            0 => &[],
            b => self.by_body.get(&(h.kind.0, b)).map_or(&[], Vec::as_slice),
        };
        if let [only] = key_hits {
            return Rebind::Moved {
                id: *only,
                tier: MoveTier::SameName,
            };
        }
        if let [only] = body_hits {
            return Rebind::Moved {
                id: *only,
                tier: MoveTier::Identical,
            };
        }
        let both: Vec<NodeId> = key_hits
            .iter()
            .copied()
            .filter(|id| body_hits.contains(id))
            .collect();
        if let [only] = both.as_slice() {
            return Rebind::Moved {
                id: *only,
                tier: MoveTier::Identical,
            };
        }
        let mut all: Vec<NodeId> = key_hits.iter().chain(body_hits).copied().collect();
        sort_ids(&mut all);
        if all.is_empty() {
            Rebind::Orphan
        } else {
            Rebind::Ambiguous(all)
        }
    }

    /// True when exactly one node carries `id`'s `(kind, file, local)` key — the
    /// check a consumer must pass before using a hint as a unique key.
    pub fn is_unique(&self, id: &Identity) -> bool {
        self.by_key
            .get(&(id.kind.0, id.file.clone(), id.local.clone()))
            .is_some_and(|v| v.len() == 1)
    }
}

/// One file moved between two builds (repo-relative POSITION paths).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileMove {
    pub old_path: String,
    pub new_path: String,
    pub tier: MoveTier,
}

/// One node carried by a file move: the MODULE itself or a declaration
/// aligned by `(kind, local)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeMove {
    pub kind: NodeKindId,
    pub old_id: NodeId,
    pub old_qname: String,
    pub new_id: NodeId,
    pub new_qname: String,
}

/// Every move between two builds. `files` sorted by old path, `nodes` by
/// `(old_qname, kind)`; `rejected` counts same-basename candidates refused for
/// sharing too few declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveMap {
    pub files: Vec<FileMove>,
    pub nodes: Vec<NodeMove>,
    pub rejected: usize,
}

/// [`detect_moves_with`] and no declared renames.
pub fn detect_moves(prior: &MergedGraph, current: &MergedGraph) -> MoveMap {
    detect_moves_with(prior, current, &[])
}

/// Pair the files whose MODULE disappeared from `prior` (by repo, kind, qname)
/// with those that appeared in `current`, each pass requiring a UNIQUE partner
/// on both sides within one repo: (0) a `declared` `(old_path, new_path)`
/// (FACT); (1) an equal MODULE body hash; (2) an equal basename whose new
/// MODULE holds at least half (and >= 1) of the old one's `(kind, local)`
/// keys, else `rejected`. Pure and deterministic.
pub fn detect_moves_with(
    prior: &MergedGraph,
    current: &MergedGraph,
    declared: &[(String, String)],
) -> MoveMap {
    let (old_mods, old_present) = collect_modules(prior);
    let (new_mods, new_present) = collect_modules(current);
    let removed: Vec<&ModuleInfo> = old_mods
        .iter()
        .filter(|m| !new_present.contains(&(m.repo, MODULE.0, m.qname.clone())))
        .collect();
    let added: Vec<&ModuleInfo> = new_mods
        .iter()
        .filter(|m| !old_present.contains(&(m.repo, MODULE.0, m.qname.clone())))
        .collect();
    let (mut used_old, mut used_new) = (vec![false; removed.len()], vec![false; added.len()]);
    let mut pairs: Vec<(usize, usize, MoveTier)> = Vec::new();
    let mut rejected = 0usize;

    let mut decl: Vec<(String, String)> = declared
        .iter()
        .map(|(o, n)| (norm_path(o), norm_path(n)))
        .collect();
    decl.sort();
    decl.dedup();
    for (old, new) in &decl {
        let repos: BTreeSet<u64> = removed
            .iter()
            .filter(|m| &m.path == old)
            .map(|m| m.repo)
            .collect();
        for repo in repos {
            let o = unique(&removed, &used_old, |m| m.repo == repo && &m.path == old);
            let n = unique(&added, &used_new, |m| m.repo == repo && &m.path == new);
            if let (Some(o), Some(n)) = (o, n) {
                (used_old[o], used_new[n]) = (true, true);
                pairs.push((o, n, MoveTier::Declared));
            }
        }
    }
    for tier in [MoveTier::Identical, MoveTier::SameName] {
        let key = |m: &ModuleInfo| match tier {
            MoveTier::Identical if m.body != 0 => Some((m.repo, m.body.to_string())),
            MoveTier::SameName => Some((m.repo, basename(&m.path).to_string())),
            _ => None,
        };
        let olds = group(&removed, &used_old, key);
        let news = group(&added, &used_new, key);
        for (k, os) in &olds {
            let (Some(n), [o]) = (news.get(k).and_then(|ns| single(ns)), os.as_slice()) else {
                continue;
            };
            if tier == MoveTier::SameName && !shares_declarations(removed[*o], added[n]) {
                rejected += 1;
                continue;
            }
            (used_old[*o], used_new[n]) = (true, true);
            pairs.push((*o, n, tier));
        }
    }

    let mut map = MoveMap {
        rejected,
        ..MoveMap::default()
    };
    for (o, n, tier) in pairs {
        let (om, nm) = (removed[o], added[n]);
        map.files.push(FileMove {
            old_path: om.path.clone(),
            new_path: nm.path.clone(),
            tier,
        });
        map.nodes.push(NodeMove {
            kind: MODULE,
            old_id: om.id,
            old_qname: om.qname.clone(),
            new_id: nm.id,
            new_qname: nm.qname.clone(),
        });
        for (key, olds) in &om.desc {
            let (Some([(new_id, new_qname)]), [(old_id, old_qname)]) =
                (nm.desc.get(key).map(Vec::as_slice), olds.as_slice())
            else {
                continue;
            };
            if new_present.contains(&(om.repo, key.0, old_qname.clone()))
                || old_present.contains(&(nm.repo, key.0, new_qname.clone()))
            {
                continue;
            }
            map.nodes.push(NodeMove {
                kind: NodeKindId(key.0),
                old_id: *old_id,
                old_qname: old_qname.clone(),
                new_id: *new_id,
                new_qname: new_qname.clone(),
            });
        }
    }
    map.files.sort();
    map.files.dedup();
    map.nodes.sort_by(|a, b| {
        (&a.old_qname, a.kind.0, &a.new_qname, a.old_id.0).cmp(&(
            &b.old_qname,
            b.kind.0,
            &b.new_qname,
            b.old_id.0,
        ))
    });
    let count = |t: MoveTier| map.files.iter().filter(|f| f.tier == t).count();
    let size = |m: &MergedGraph| m.graphs.iter().map(|g| g.nodes.len()).sum::<usize>();
    eprintln!(
        "[moves] files={} (declared={} identical={} same-name={} rejected={}) nodes={} prior={} current={}",
        map.files.len(),
        count(MoveTier::Declared),
        count(MoveTier::Identical),
        count(MoveTier::SameName),
        map.rejected,
        map.nodes.len(),
        size(prior),
        size(current),
    );
    map
}

/// Carry a `path -> token` map across `moves`: a moved file keeps its old
/// path's token, a file that stayed keeps its own, and any other file (new,
/// or created at a path a move vacated) gets its path — `path#2`, `#3`, ...
/// when that token is already in use. The token of a never-moved file is its
/// path at first sight (LG.9). Only as good as the prior map the caller
/// persists: glia keeps no history.
pub fn carry_file_tokens(
    prior: &BTreeMap<String, String>,
    moves: &MoveMap,
    current_files: &[String],
) -> BTreeMap<String, String> {
    let mut moved_to: BTreeMap<&str, &str> = BTreeMap::new();
    for m in &moves.files {
        moved_to
            .entry(m.new_path.as_str())
            .or_insert(m.old_path.as_str());
    }
    let vacated: BTreeSet<&str> = moves.files.iter().map(|m| m.old_path.as_str()).collect();
    let files: BTreeSet<&str> = current_files.iter().map(String::as_str).collect();
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut fresh: Vec<(&str, &str)> = Vec::new();
    for f in files {
        let carried = match moved_to.get(f) {
            Some(old) => prior.get(*old).ok_or(*old),
            None if !vacated.contains(f) => prior.get(f).ok_or(f),
            None => Err(f),
        };
        match carried {
            Ok(token) => {
                out.insert(f.to_string(), token.clone());
            }
            Err(base) => fresh.push((f, base)),
        }
    }
    let mut taken: BTreeSet<String> = out.values().chain(prior.values()).cloned().collect();
    for (f, base) in fresh {
        let mut token = base.to_string();
        let mut n = 2usize;
        while taken.contains(&token) {
            token = format!("{base}#{n}");
            n += 1;
        }
        taken.insert(token.clone());
        out.insert(f.to_string(), token);
    }
    out
}

// ============================================================================
// Internals
// ============================================================================

/// A MODULE that owns a POSITION file, with its declarations keyed by
/// `(kind, local)`.
struct ModuleInfo {
    repo: u64,
    id: NodeId,
    qname: String,
    path: String,
    body: u64,
    desc: BTreeMap<(u32, String), Vec<(NodeId, String)>>,
}

type Present = HashSet<(u64, u32, String)>;

/// Every MODULE with a POSITION file, sorted by (repo, path, qname), plus the
/// `(repo, kind, qname)` of every node.
fn collect_modules(merged: &MergedGraph) -> (Vec<ModuleInfo>, Present) {
    let mut present: Present = HashSet::new();
    let mut mods: BTreeMap<(u64, u64), ModuleInfo> = BTreeMap::new();
    let mut desc: Vec<(u64, NodeId, u32, String, NodeId, String)> = Vec::new();
    for g in &merged.graphs {
        let by_id = node_map(g);
        let mut seen: HashSet<NodeId> = HashSet::new();
        for n in g.nodes.iter().filter(|n| seen.insert(n.id)) {
            let (Some(qname), Some(kind)) =
                (g.nav.qname_by_id.get(&n.id), g.nav.kind_by_id.get(&n.id))
            else {
                continue;
            };
            present.insert((g.repo.0, kind.0, qname.clone()));
            let Some((module, ident)) = derive(g, |x| by_id.get(&x).copied(), n.id) else {
                continue;
            };
            if module != n.id {
                desc.push((
                    g.repo.0,
                    module,
                    ident.kind.0,
                    ident.local,
                    n.id,
                    qname.clone(),
                ));
                continue;
            }
            if let Some(path) = position_file(n) {
                mods.entry((g.repo.0, n.id.0)).or_insert(ModuleInfo {
                    repo: g.repo.0,
                    id: n.id,
                    qname: qname.clone(),
                    path,
                    body: ident.body,
                    desc: BTreeMap::new(),
                });
            }
        }
    }
    for (repo, module, kind, local, id, qname) in desc {
        if let Some(m) = mods.get_mut(&(repo, module.0)) {
            m.desc.entry((kind, local)).or_default().push((id, qname));
        }
    }
    let mut out: Vec<ModuleInfo> = mods.into_values().collect();
    for m in &mut out {
        m.desc
            .values_mut()
            .for_each(|v| v.sort_by(|a, b| (&a.1, a.0.0).cmp(&(&b.1, b.0.0))));
    }
    out.sort_by(|a, b| (a.repo, &a.path, &a.qname).cmp(&(b.repo, &b.path, &b.qname)));
    (out, present)
}

/// SameName evidence: at least half (and >= 1) of `old`'s `(kind, local)` keys
/// exist under `new`.
fn shares_declarations(old: &ModuleInfo, new: &ModuleInfo) -> bool {
    let hits = old
        .desc
        .keys()
        .filter(|k| new.desc.contains_key(*k))
        .count();
    hits >= 1 && hits * 2 >= old.desc.len()
}

/// The one unpaired module matching `pred`, or `None` for zero or several.
fn unique(
    mods: &[&ModuleInfo],
    used: &[bool],
    pred: impl Fn(&ModuleInfo) -> bool,
) -> Option<usize> {
    let hits: Vec<usize> = (0..mods.len())
        .filter(|&i| !used[i] && pred(mods[i]))
        .collect();
    single(&hits)
}

fn single(v: &[usize]) -> Option<usize> {
    match v {
        [only] => Some(*only),
        _ => None,
    }
}

/// Unpaired modules grouped by `key`, in index order.
fn group(
    mods: &[&ModuleInfo],
    used: &[bool],
    key: impl Fn(&ModuleInfo) -> Option<(u64, String)>,
) -> BTreeMap<(u64, String), Vec<usize>> {
    let mut out: BTreeMap<(u64, String), Vec<usize>> = BTreeMap::new();
    for (i, m) in mods.iter().enumerate().filter(|(i, _)| !used[*i]) {
        if let Some(k) = key(m) {
            out.entry(k).or_default().push(i);
        }
    }
    out
}

/// `NodeId -> &Node`, first occurrence wins.
fn node_map(g: &RepoGraph) -> HashMap<NodeId, &Node> {
    let mut out: HashMap<NodeId, &Node> = HashMap::with_capacity(g.nodes.len());
    for n in &g.nodes {
        out.entry(n.id).or_insert(n);
    }
    out
}

/// `(nearest MODULE ancestor, identity)` of `id` in `g`. The walk is bounded by
/// the nav's size, so a malformed parent cycle ends in `None`.
fn derive<'a>(
    g: &'a RepoGraph,
    node_of: impl Fn(NodeId) -> Option<&'a Node>,
    id: NodeId,
) -> Option<(NodeId, Identity)> {
    let kind = *g.nav.kind_by_id.get(&id)?;
    let mut names: Vec<&str> = Vec::new();
    let mut cur = id;
    let mut budget = g.nav.parent_of.len() + 1;
    let module = loop {
        if *g.nav.kind_by_id.get(&cur)? == MODULE {
            break cur;
        }
        names.push(g.nav.name_by_id.get(&cur)?);
        budget = budget.checked_sub(1)?;
        cur = *g.nav.parent_of.get(&cur)?;
    };
    names.reverse();
    let own = node_of(id);
    let path = own
        .and_then(position_file)
        .or_else(|| node_of(module).and_then(position_file))?;
    let body = own.and_then(code_text).map_or(0, body_hash);
    Some((
        module,
        Identity {
            kind,
            file: basename(&path).to_string(),
            local: names.join("::"),
            body,
        },
    ))
}

/// The node's POSITION `file`, JSON-unescaped; `None` when absent or empty.
fn position_file(n: &Node) -> Option<String> {
    n.cells
        .iter()
        .filter(|c| c.kind == POSITION)
        .find_map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => json_string_field(s, "file"),
            CellPayload::Bytes(_) => None,
        })
        .filter(|f| !f.is_empty())
}

fn code_text(n: &Node) -> Option<&str> {
    n.cells
        .iter()
        .find(|c| c.kind == CODE)
        .and_then(|c| match &c.payload {
            CellPayload::Text(s) | CellPayload::Json(s) => Some(s.as_str()),
            CellPayload::Bytes(_) => None,
        })
}

/// FNV-1a 64 over the whitespace-collapsed text (the `stamp/build.rs`
/// precedent: stable across toolchains, unlike std's `DefaultHasher`).
fn body_hash(code: &str) -> u64 {
    let (mut state, mut chars) = (FNV_OFFSET, 0usize);
    for (i, word) in code.split_whitespace().enumerate() {
        if i > 0 {
            fnv1a(&mut state, b" ");
            chars += 1;
        }
        fnv1a(&mut state, word.as_bytes());
        chars += word.chars().count();
    }
    match state {
        _ if chars < MIN_BODY_CHARS => 0,
        0 => 1,
        h => h,
    }
}

fn fnv1a(state: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *state ^= u64::from(*b);
        *state = state.wrapping_mul(FNV_PRIME);
    }
}

/// `json["key"]` as a string, with JSON escapes decoded. Hand-rolled so the
/// graph crate stays off serde_json.
fn json_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = json.find(&needle)? + needle.len();
    let rest = json[at..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => out.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                // Writers escape only control chars this way (serde_json keeps
                // non-ASCII literal), so a surrogate pair is not decoded: it
                // yields no file rather than a wrong one.
                'u' => char::from_u32(hex4(&mut chars)?)?,
                other => other,
            }),
            c => out.push(c),
        }
    }
    None
}

fn hex4(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    let s: String = chars.take(4).collect();
    (s.len() == 4).then_some(())?;
    u32::from_str_radix(&s, 16).ok()
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Declared paths in POSITION form: `/` separators, no leading `./`.
fn norm_path(p: &str) -> String {
    let p = p.trim().replace('\\', "/");
    p.trim_start_matches("./").to_string()
}

fn escape(s: &str) -> String {
    s.replace('%', "%25").replace('|', "%7C")
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let code = rest.get(i + 1..i + 3)?;
        out.push(match code {
            "25" => '%',
            "7C" => '|',
            _ => return None,
        });
        rest = &rest[i + 3..];
    }
    out.push_str(rest);
    Some(out)
}

fn sort_ids(v: &mut Vec<NodeId>) {
    v.sort_by_key(|id| id.0);
    v.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_reference_vector() {
        let mut s = FNV_OFFSET;
        fnv1a(&mut s, b"a");
        assert_eq!(s, 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn body_hash_collapses_whitespace_and_ignores_short_bodies() {
        let a = "fn totals(rows: &[Row]) -> u64 {\n    rows.iter().map(|r| r.n).sum()\n}";
        let b = "  fn totals(rows: &[Row]) -> u64 { rows.iter().map(|r| r.n).sum() }  ";
        assert_ne!(body_hash(a), 0);
        assert_eq!(body_hash(a), body_hash(b));
        assert_eq!(body_hash("def f(): pass"), 0);
    }

    #[test]
    fn position_file_decodes_json_escapes() {
        let json = r#"{"file":"a\\b \"q\" é😀.py","start_line":1}"#;
        assert_eq!(
            json_string_field(json, "file").as_deref(),
            Some("a\\b \"q\" é😀.py")
        );
        assert_eq!(json_string_field(r#"{"start_line":1}"#, "file"), None);
        assert_eq!(json_string_field(r#"{"file":"unterminated"#, "file"), None);
        // `\u` escapes built at runtime so the source holds no escape to mangle.
        let u = "\\u";
        let escaped = format!(r#"{{"file":"x{u}0041{u}0009.py"}}"#);
        assert_eq!(
            json_string_field(&escaped, "file").as_deref(),
            Some("xA\t.py")
        );
        let lone_surrogate = format!(r#"{{"file":"{u}d83d.py"}}"#);
        assert_eq!(json_string_field(&lone_surrogate, "file"), None);
    }
}
