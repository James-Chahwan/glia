//! Typed id routing (IDR): a text's ids routed by their type to the files that
//! define or carry them, the ranking the intent blend fuses. Owner: EB.6.
//!
//! This is rank_eval.rs's round-2 IDR (`r2_kina_reach`, `r2_glia_commit`,
//! `r2_scope_commit`, `r2_finish`, `r2_max`, `r2_scopes`, `kcomment`, the
//! `R2_W_*` weights and `R2_RECENT`), read through EB.5a's definitions, EB.5b's
//! namespaces and EB.5c's path resolver:
//!
//! 1. The text's id mentions ([`IdVocab::mentions`], first occurrence of each
//!    id kept) are resolved line by line with [`Namespaces::resolve_line`]
//!    (the line's scopes, the caller's recent scopes). An undefined id that
//!    is a FAMILY (defined ids equal to it plus one or two trailing lowercase
//!    letters, rank_eval `family`: `DT.5` -> `DT.5a`..) resolves to every
//!    member, route `family`, tier derived.
//! 2. Each resolved key expands to its lineage class ([`Namespaces::expand`],
//!    rank_eval `cl.exp(key)`). NAMED: every defining row of every member key
//!    names files ([`PathResolver::block`] over the row's path tokens), 1.0
//!    for a pending row or one with no checkbox, 0.5 for a done row, the MAX
//!    per file over the key's rows. CITED: a code line whose COMMENT part
//!    (rank_eval `kcomment`) names a member id, 0.5 per file, counted when
//!    the id has one series or the citing line resolves to the member's
//!    namespace (own scope: the first scope whose rows name the citing file).
//! 3. `idr[f]` is the SUM over resolved keys, in mention order, of
//!    `named[f] + cited[f]` (rank_eval `r2_finish`).
//! 4. SCOPE ROUTE (rank_eval `r2_scope_commit`), only when no mention
//!    resolved: the subject's conventional-commit scopes (`feat(billing): ..`)
//!    -> the tree's `*.md` files (outside `node_modules/`) under a directory
//!    named `billing` or `NNN-billing` -> every line's path tokens, 0.5 on a
//!    `- [x]` line, else 1.0, the MAX per file per scope, summed over scopes.
//!
//! Everything is read at one revision, the [`Tree`] the answer stands on
//! (review: the base; find: the working tree). Unlike the harness (each test
//! commit's parent), the recent scopes ([`recent_scopes`]) resolve earlier
//! subjects against that same revision. Query-time: nothing is stored.
//!
//! Fired_on, stderr, once per [`route`]: `[idr] repo=<label> surface=<s>
//! rev=<sha12|worktree> ids=<m> resolved=<r> unresolved=<u>
//! routes=<route:n,..> lineage_keys=<k> scopes=<sc> named_files=<n>
//! cited_files=<c> files=<f> recent=<k>`, after the `[id-paths]` line of
//! the routed text; the first route over one [`IdrSide`] also prints the
//! `[ids] vocab`, `[id-defs]` and `[id-ns]` markers of the side it reads.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::snapshots::{HistoryMessage, HistorySnapshot};
use serde::Serialize;

use super::defs::{kpath_tokens, line_scopes, read_defs, Ctx, IdDefs, Namespaces, Resolved, RouteCounts, RowState, Tree};
use super::ids::{IdMention, IdVocab};
use super::paths::PathResolver;
use super::text::{subject, Candidates};
use crate::git_rev::{grep_words_at, read_paths_at};

/// rank_eval `R2_W_NAMED`: a file a pending (or checkbox-less) defining row names.
pub const W_NAMED: f64 = 1.0;
/// rank_eval `R2_W_DONE`: a file a done defining row names.
pub const W_DONE: f64 = 0.5;
/// rank_eval `R2_W_CITED`: a code file whose comment cites the id.
pub const W_CITED: f64 = 0.5;
/// rank_eval `R2_RECENT`: the resolved scopes [`recent_scopes`] keeps.
pub const RECENT: usize = 5;
/// rank_eval `MAX_FILES`: a test commit names at most this many candidate files.
const MAX_TEST_FILES: usize = 30;

/// One id (or scope) the text routed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct RoutedId {
    /// The id, or `scope:<s>` for a conventional-commit scope.
    pub id: String,
    /// The namespace it resolved into; None for a scope or an unresolved id.
    pub ns: Option<String>,
    /// `fact` / `derived` / `heuristic`; None when unresolved.
    pub tier: Option<&'static str>,
    /// The resolution route (`unique`, `line-scope`, .., `family`,
    /// `scope-dir`), or the reason it did not resolve (`undefined`, `local-only`).
    pub route: &'static str,
    /// The first defining row: (page, 1-based line).
    pub defined_at: Option<(String, i64)>,
    /// That row's state: `pending` / `done` / `none`.
    pub state: Option<&'static str>,
    /// Files the key's defining rows name (lineage class included).
    pub named_files: usize,
    /// Files whose comments cite the key's ids.
    pub cited_files: usize,
}

/// The routing of one text: per candidate scores in [`Candidates`] order.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Idr {
    /// Every mention (one row per resolved key; a family one per member)
    /// and every scope with docs, in text order.
    pub ids: Vec<RoutedId>,
    /// named + cited, summed over the resolved keys.
    pub idr: Vec<f64>,
    pub named: Vec<f64>,
    pub cited: Vec<f64>,
    /// An id resolved, or a scope had docs.
    pub has_id: bool,
}

/// What a route reads: one revision's definitions, namespaces and paths.
#[non_exhaustive]
pub struct IdrSide<'a> {
    pub label: &'a str,
    pub surface: &'a str,
    pub cands: &'a Candidates,
    pub paths: &'a PathResolver,
    pub defs: &'a IdDefs,
    pub ns: &'a Namespaces,
    pub vocab: &'a IdVocab,
    pub tree: &'a Tree<'a>,
    reported: Cell<bool>,
    scope_of: RefCell<Option<BTreeMap<usize, String>>>,
}

impl<'a> IdrSide<'a> {
    /// A side over these parts, labelled `-` on surface `-` until [`IdrSide::labelled`].
    pub fn new(
        cands: &'a Candidates,
        paths: &'a PathResolver,
        defs: &'a IdDefs,
        ns: &'a Namespaces,
        vocab: &'a IdVocab,
        tree: &'a Tree<'a>,
    ) -> IdrSide<'a> {
        IdrSide {
            label: "-",
            surface: "-",
            cands,
            paths,
            defs,
            ns,
            vocab,
            tree,
            reported: Cell::new(false),
            scope_of: RefCell::new(None),
        }
    }

    /// The repo label and surface the markers print.
    pub fn labelled(mut self, label: &'a str, surface: &'a str) -> IdrSide<'a> {
        self.label = label;
        self.surface = surface;
        self
    }

    /// The side's `[ids] vocab`, `[id-defs]` and `[id-ns]` markers, once.
    fn report_once(&self) {
        if self.reported.replace(true) {
            return;
        }
        self.vocab.report(self.label, self.surface);
        self.defs.report(self.label);
        self.ns.report(self.label);
    }

    /// The first scope (row order) whose defining rows name candidate `f`.
    fn scope_of(&self, f: usize) -> Option<String> {
        let mut cache = self.scope_of.borrow_mut();
        let map = cache.get_or_insert_with(|| {
            let mut m: BTreeMap<usize, String> = BTreeMap::new();
            for row in &self.defs.rows {
                let Some(s) = &row.scope else { continue };
                for (g, _, _) in self.paths.block(&row.names) {
                    m.entry(g).or_insert_with(|| s.clone());
                }
            }
            m
        });
        map.get(&f).cloned()
    }
}

/// The parts of an [`IdrSide`] read from one tree: its definitions, their
/// namespaces and the path resolver over the candidates.
#[non_exhaustive]
pub struct IdrInputs {
    pub defs: IdDefs,
    pub ns: Namespaces,
    pub paths: PathResolver,
}

impl IdrInputs {
    /// Read `tree`'s definitions ([`read_defs`], no Tier-4 snapshot), build
    /// their namespaces with `vocab`'s local words, and the path resolver over
    /// `cands` and `renames` ([`super::paths::rename_pairs`], empty without a
    /// history snapshot).
    pub fn read(
        tree: &Tree,
        project_dirs: &[&str],
        vocab: &IdVocab,
        cands: &Candidates,
        renames: &[(String, String)],
    ) -> IdrInputs {
        let defs = read_defs(tree, project_dirs, vocab, None);
        let ns = defs.namespaces(&vocab.local_words);
        IdrInputs { defs, ns, paths: PathResolver::new(cands, renames) }
    }

    /// The side these inputs make.
    pub fn side<'a>(&'a self, cands: &'a Candidates, vocab: &'a IdVocab, tree: &'a Tree<'a>) -> IdrSide<'a> {
        IdrSide::new(cands, &self.paths, &self.defs, &self.ns, vocab, tree)
    }
}

/// rank_eval `r2_max`.
fn put_max(m: &mut BTreeMap<usize, f64>, f: usize, w: f64) {
    let e = m.entry(f).or_default();
    *e = e.max(w);
}

/// rank_eval `kcomment`: the comment part of a code line, where a citation
/// can sit (a line starting `* `, `#`, `--` or `*/` whole, else the text from
/// the first `//`, `/*`, `<!--`, ` -- ` or ` # `).
pub fn comment_part(l: &str) -> Option<&str> {
    let t = l.trim_start();
    if t.starts_with("* ") || t.starts_with('#') || t.starts_with("--") || t.starts_with("*/") {
        return Some(t);
    }
    ["//", "/*", "<!--", " -- ", " # "].iter().filter_map(|m| l.find(m)).min().map(|i| &l[i..])
}

/// rank_eval `r2_scopes`: the conventional-commit scopes of a subject,
/// `feat(a,b): ..` / `feat(a)!: ..` -> `[a, b]`, lowercased, each at least 3
/// chars of `[a-z0-9_-]`.
pub fn conventional_scopes(subj: &str) -> Vec<String> {
    let Some(open) = subj.find('(') else { return Vec::new() };
    let head = &subj[..open];
    if head.is_empty() || !head.chars().all(|c| c.is_ascii_alphabetic()) {
        return Vec::new();
    }
    let Some(close) = subj[open..].find(')') else { return Vec::new() };
    let after = &subj[open + close + 1..];
    if !(after.starts_with(':') || after.starts_with("!:")) {
        return Vec::new();
    }
    subj[open + 1..open + close]
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .collect()
}

/// A directory segment naming scope `s`: `s` itself or `NNN-s`.
fn names_scope(seg: &str, s: &str) -> bool {
    seg == s
        || seg.len() > 4
            && seg.as_bytes()[..3].iter().all(u8::is_ascii_digit)
            && seg.as_bytes()[3] == b'-'
            && &seg[4..] == s
}

/// The defined ids of `id`'s family: `id` plus one or two lowercase letters.
fn family_members<'n>(ns: &'n Namespaces, id: &str) -> Vec<&'n str> {
    ns.by_id
        .range::<str, _>((std::ops::Bound::Excluded(id), std::ops::Bound::Unbounded))
        .map(|(k, _)| k.as_str())
        .take_while(|k| k.starts_with(id))
        .filter(|k| {
            let rest = &k[id.len()..];
            (1..=2).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_lowercase())
        })
        .collect()
}

/// Each id's first mention, in text order.
fn first_mentions(vocab: &IdVocab, text: &str) -> Vec<IdMention> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    vocab.mentions(text).into_iter().filter(|m| seen.insert(m.id.clone())).collect()
}

/// One resolved key: (namespace, id, tier, route).
type Key = (String, String, &'static str, &'static str);

/// Route `text` over `side` ([module doc](self)); `recent` are the recent
/// scopes, newest first ([`recent_scopes`]).
pub fn route(text: &str, side: &IdrSide, recent: &[String]) -> Idr {
    side.report_once();
    let ns = side.ns;
    let n = side.cands.len();
    let mut out = Idr { idr: vec![0.0; n], named: vec![0.0; n], cited: vec![0.0; n], ..Idr::default() };
    let mut counts = RouteCounts::default();
    let mentions = first_mentions(side.vocab, text);
    let (mut resolved, mut unresolved) = (0usize, 0usize);
    // keys in mention order; per mention, its keys' indices or its failure row
    let mut keys: Vec<Key> = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut slots: Vec<Result<Vec<usize>, RoutedId>> = Vec::new();
    let mut line_start = 0usize;
    for line in text.split('\n') {
        let end = line_start + line.len();
        let ms: Vec<IdMention> =
            mentions.iter().filter(|m| m.start >= line_start && m.start < end).cloned().collect();
        line_start = end + 1;
        if ms.is_empty() {
            continue;
        }
        let cx = Ctx { recent, text: line, line_scopes: line_scopes(line, ns), ..Ctx::default() };
        let res = ns.resolve_line(line, &ms, &cx);
        for (m, r) in ms.iter().zip(res) {
            let mut got: Vec<usize> = Vec::new();
            let mut push = |k: Key, got: &mut Vec<usize>| {
                if seen.insert((k.0.clone(), k.1.clone())) {
                    keys.push(k);
                    got.push(keys.len() - 1);
                }
            };
            match &r {
                Ok(x) => {
                    counts.add(&r);
                    resolved += 1;
                    push((x.ns.clone(), x.id.clone(), x.tier, x.route), &mut got);
                    slots.push(Ok(got));
                }
                Err(e) => {
                    let fam: Vec<Resolved> = if *e == "undefined" {
                        family_members(ns, &m.id).into_iter().filter_map(|id| ns.resolve(id, &cx).ok()).collect()
                    } else {
                        Vec::new()
                    };
                    if fam.is_empty() {
                        counts.add(&r);
                        unresolved += 1;
                        slots.push(Err(RoutedId {
                            id: m.id.clone(),
                            ns: None,
                            tier: None,
                            route: e,
                            defined_at: None,
                            state: None,
                            named_files: 0,
                            cited_files: 0,
                        }));
                        continue;
                    }
                    counts.add(&Ok(Resolved { id: m.id.clone(), ns: String::new(), tier: "derived", route: "family" }));
                    resolved += 1;
                    for x in fam {
                        push((x.ns, x.id, "derived", "family"), &mut got);
                    }
                    slots.push(Ok(got));
                }
            }
        }
    }
    // every key's lineage members, then one grep for every member id
    let members: Vec<Vec<(String, String)>> = keys
        .iter()
        .map(|(kns, kid, _, _)| {
            ns.expand(&Namespaces::key(kns, kid))
                .into_iter()
                .filter_map(|k| k.rsplit_once('|').map(|(a, b)| (a.to_string(), b.to_string())))
                .collect()
        })
        .collect();
    let lineage_keys: usize = members.iter().zip(&keys).map(|(ms, k)| ms.iter().filter(|m| (&m.0, &m.1) != (&k.0, &k.1)).count()).sum();
    let words: Vec<String> =
        members.iter().flatten().map(|m| m.1.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let only: &[String] = match side.tree {
        Tree::Work(_) => &side.cands.paths,
        _ => &[],
    };
    let hits = if words.is_empty() {
        Vec::new()
    } else {
        grep_words_at(side.tree, &words, only).unwrap_or_else(|e| {
            eprintln!("[idr] repo={} surface={} cite grep failed: {e}", side.label, side.surface);
            Vec::new()
        })
    };
    // per hit: (candidate, comment, its mentions)
    let cites: Vec<(usize, &str, &str, Vec<IdMention>)> = hits
        .iter()
        .filter_map(|(p, l)| {
            let f = side.paths.map(p)?;
            let c = comment_part(l)?;
            let ms = first_mentions(side.vocab, c);
            (!ms.is_empty()).then_some((f, l.as_str(), c, ms))
        })
        .collect();
    let mut blocks: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut routed: Vec<RoutedId> = Vec::with_capacity(keys.len());
    for ((kns, kid, tier, route_name), ms) in keys.iter().zip(&members) {
        let mut named: BTreeMap<usize, f64> = BTreeMap::new();
        let mut cited: BTreeMap<usize, f64> = BTreeMap::new();
        for (mns, mid) in ms {
            for &ri in ns.rows.get(&(mns.clone(), mid.clone())).into_iter().flatten() {
                let Some(row) = side.defs.rows.get(ri) else { continue };
                let w = if row.state == RowState::Done { W_DONE } else { W_NAMED };
                let files =
                    blocks.entry(ri).or_insert_with(|| side.paths.block(&row.names).into_iter().map(|x| x.0).collect());
                for &f in files.iter() {
                    put_max(&mut named, f, w);
                }
            }
            let one_series = ns.by_id.get(mid).is_some_and(|v| v.len() == 1);
            for (f, line, c, cms) in &cites {
                let Some(i) = cms.iter().position(|m| &m.id == mid) else { continue };
                let ok = one_series || {
                    let own = side.scope_of(*f);
                    let cx = Ctx {
                        own_scope: own.as_deref(),
                        line_scopes: line_scopes(line, ns),
                        text: line,
                        ..Ctx::default()
                    };
                    ns.resolve_line(c, cms, &cx).get(i).is_some_and(|r| r.as_ref().is_ok_and(|r| &r.ns == mns))
                };
                if ok {
                    put_max(&mut cited, *f, W_CITED);
                }
            }
        }
        for (&f, &w) in &named {
            out.idr[f] += w;
            out.named[f] += w;
        }
        for (&f, &w) in &cited {
            out.idr[f] += w;
            out.cited[f] += w;
        }
        let first = ns.rows.get(&(kns.clone(), kid.clone())).and_then(|v| v.first()).and_then(|&i| side.defs.rows.get(i));
        routed.push(RoutedId {
            id: kid.clone(),
            ns: Some(kns.clone()),
            tier: Some(tier),
            route: route_name,
            defined_at: first.map(|r| (r.page.clone(), r.line)),
            state: first.map(|r| r.state.as_str()),
            named_files: named.len(),
            cited_files: cited.len(),
        });
    }
    out.has_id = !keys.is_empty();
    for slot in slots {
        match slot {
            Ok(ix) => out.ids.extend(ix.into_iter().filter_map(|i| routed.get(i).cloned())),
            Err(r) => out.ids.push(r),
        }
    }
    let mut scopes_with_docs = 0usize;
    if keys.is_empty() {
        scopes_with_docs = scope_route(text, side, &mut out, &mut counts);
        out.has_id = scopes_with_docs > 0;
    }
    side.paths.report(side.label, side.surface);
    let nz = |v: &[f64]| v.iter().filter(|w| **w > 0.0).count();
    eprintln!(
        "[idr] repo={} surface={} rev={} ids={} resolved={resolved} unresolved={unresolved} routes={} lineage_keys={lineage_keys} scopes={scopes_with_docs} named_files={} cited_files={} files={} recent={}",
        side.label,
        side.surface,
        side.tree.label(),
        mentions.len(),
        counts.render(),
        nz(&out.named),
        nz(&out.cited),
        nz(&out.idr),
        recent.len()
    );
    out
}

/// The scope route ([module doc](self) step 4); returns the scopes with docs.
fn scope_route(text: &str, side: &IdrSide, out: &mut Idr, counts: &mut RouteCounts) -> usize {
    let scopes = conventional_scopes(subject(text));
    if scopes.is_empty() {
        return 0;
    }
    let dir_segs = |p: &str| -> Vec<String> {
        let d = p.rsplit_once('/').map_or("", |(d, _)| d);
        d.split('/').map(str::to_string).collect()
    };
    let keep = |p: &str| {
        p.ends_with(".md")
            && !p.split('/').any(|s| s == "node_modules")
            && dir_segs(p).iter().any(|seg| scopes.iter().any(|s| names_scope(seg, s)))
    };
    let read = match read_paths_at(side.tree, &keep) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[idr] repo={} surface={} scope docs unreadable: {e}", side.label, side.surface);
            return 0;
        }
    };
    let mut with_docs = 0;
    for s in &scopes {
        let mut named: BTreeMap<usize, f64> = BTreeMap::new();
        let mut any = false;
        for (p, bytes) in &read.files {
            if !dir_segs(p).iter().any(|seg| names_scope(seg, s)) || bytes[..bytes.len().min(8000)].contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(bytes);
            for l in text.split_terminator('\n') {
                any = true;
                let t = l.trim_start();
                let done = t.starts_with("- [x]") || t.starts_with("- [X]") || t.starts_with("* [x]");
                for (f, _, _) in side.paths.block(&kpath_tokens(l)) {
                    put_max(&mut named, f, if done { W_DONE } else { W_NAMED });
                }
            }
        }
        if !any {
            continue;
        }
        with_docs += 1;
        counts.add(&Ok(Resolved { id: format!("scope:{s}"), ns: String::new(), tier: "derived", route: "scope-dir" }));
        for (&f, &w) in &named {
            out.idr[f] += w;
            out.named[f] += w;
        }
        out.ids.push(RoutedId {
            id: format!("scope:{s}"),
            ns: None,
            tier: Some("derived"),
            route: "scope-dir",
            defined_at: None,
            state: None,
            named_files: named.len(),
            cited_files: 0,
        });
    }
    with_docs
}

/// rank_eval's recent list: `subjects_newest_first` processed oldest to
/// newest, each subject's mentions resolved with the list so far, every
/// resolved namespace's scope pushed to the front (deduped, at most
/// [`RECENT`]). Newest first.
pub fn recent_scopes(subjects_newest_first: &[&str], ns: &Namespaces, vocab: &IdVocab) -> Vec<String> {
    let mut recent: Vec<String> = Vec::new();
    for subj in subjects_newest_first.iter().rev() {
        let ms = first_mentions(vocab, subj);
        if ms.is_empty() {
            continue;
        }
        let got: Vec<String> = {
            let cx = Ctx { recent: &recent, text: subj, ..Ctx::default() };
            ns.resolve_line(subj, &ms, &cx)
                .into_iter()
                .filter_map(Result::ok)
                .filter_map(|r| {
                    ns.by_id.get(&r.id).and_then(|v| v.iter().find(|x| x.ns == r.ns)).and_then(|x| x.scope.clone())
                })
                .collect()
        };
        for s in got {
            recent.retain(|x| *x != s);
            recent.insert(0, s);
            recent.truncate(RECENT);
        }
    }
    recent
}

/// The subjects [`recent_scopes`] reads, newest first: the snapshot's commits
/// shaped like rank_eval's test commits (2..=30 distinct candidate files,
/// `HistoryFile.p` with no rename fold), in snapshot order, `exclude`d shas
/// (the answer's own change) removed, each with a message row.
pub fn recent_subjects<'m>(
    snapshot: &HistorySnapshot,
    messages: &'m [HistoryMessage],
    cands: &Candidates,
    exclude: &BTreeSet<String>,
) -> Vec<&'m str> {
    let by_sha: BTreeMap<&str, &HistoryMessage> = messages.iter().map(|m| (m.c.as_str(), m)).collect();
    snapshot
        .commits
        .iter()
        .filter(|c| !exclude.contains(&c.c))
        .filter(|c| {
            let files: BTreeSet<usize> = c.files.iter().filter_map(|f| cands.position(&f.p)).collect();
            (2..=MAX_TEST_FILES).contains(&files.len())
        })
        .filter_map(|c| by_sha.get(c.c.as_str()).map(|m| m.s.as_str()))
        .collect()
}
