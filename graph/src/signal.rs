//! Signal resolution (WP-B / GR-2) — stacktrace / diff / test-id text to seed
//! node ids, plus the hand-rolled frame and POSITION-cell parsers it needs.
//!
//! [`MergedGraph::resolve_signal`] resolves one signal;
//! [`MergedGraph::resolve_signals`] (LF.6b) resolves a batch through ONE
//! [`Resolver`], so the POSITION and name indexes are built once for the
//! whole batch (a test-report ingest resolves hundreds of frames) and the
//! match counts sum into one marker line instead of one per item. Both paths
//! share every resolution rule, so an item resolves in a batch exactly as it
//! does alone.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::cell_type;
use repo_graph_core::{Node, NodeId};

use crate::merged::MergedGraph;

// ============================================================================
// Signal resolution (WP-B / GR-2)
// ============================================================================

impl MergedGraph {
    /// Resolve a failure / change *signal* to seed node ids (GR-2 `locate`).
    /// `kind` is `"stacktrace"`, `"test"`, `"diff"`, or `"auto"` (sniff the
    /// shape). Unresolvable tokens are simply absent from the result. The
    /// sniffer and all frame/symbol/path → node-id logic live here in Rust so
    /// every consumer (repo-graph, Engram, neuropil) shares one resolver.
    pub fn resolve_signal(&self, text: &str, kind: &str) -> Vec<NodeId> {
        let mut r = Resolver::new(self);
        let out = r.resolve(text, kind);
        // fired_on marker (LA.14): only when a token was a full qname, so a
        // plain test-id signal stays silent.
        if r.counts.exact_qname > 0 {
            eprintln!("[resolve] test-id exact-qname tokens={}", r.counts.exact_qname);
        }
        // How each file/frame query landed, accumulated across the whole
        // signal so the marker fires once per call and not once per frame.
        if r.counts.precise + r.counts.loose > 0 {
            eprintln!(
                "[resolve] file signal: path-suffix={} basename-fallback={}",
                r.counts.precise, r.counts.loose
            );
        }
        out
    }

    /// Resolve a batch of `(text, kind)` signals, each exactly as
    /// [`Self::resolve_signal`] would, returning one id list per item in item
    /// order. The POSITION and name indexes are built once for the batch and
    /// the match counts are summed across items, so a non-empty batch prints
    /// ONE marker line, never one per item:
    /// `[resolve] batch items=<n> resolved=<items with a hit> path-suffix=<p> basename-fallback=<l> exact-qname=<e>`.
    pub fn resolve_signals(&self, items: &[(&str, &str)]) -> Vec<Vec<NodeId>> {
        let (out, counts) = self.resolve_signals_counted(items);
        if !items.is_empty() {
            eprintln!(
                "[resolve] batch items={} resolved={} path-suffix={} basename-fallback={} exact-qname={}",
                items.len(),
                out.iter().filter(|ids| !ids.is_empty()).count(),
                counts.precise,
                counts.loose,
                counts.exact_qname
            );
        }
        out
    }

    /// [`Self::resolve_signals`] without the marker, plus the summed counts.
    fn resolve_signals_counted(&self, items: &[(&str, &str)]) -> (Vec<Vec<NodeId>>, MatchCounts) {
        let mut r = Resolver::new(self);
        let out = items.iter().map(|(text, kind)| r.resolve(text, kind)).collect();
        (out, r.counts)
    }
}

/// One POSITION-carrying node in the [`Resolver`]'s file index: its POSITION
/// file and 0-based `[start, end]` rows.
struct Located {
    file: String,
    id: NodeId,
    start: u32,
    end: u32,
}

/// One resolution run (a single signal or a batch): the graph, the lazily
/// built indexes and the running match counts.
///
/// The file index groups every located node by its POSITION file's basename,
/// in graph then node order. Every precise match (a boundary-aligned path
/// suffix) and every loose one (the bare basename) shares the query's
/// basename, so a frame only walks its own bucket, and the bucket keeps the
/// full scan's order (the strict narrowest-span rule breaks ties on the first
/// node seen). The name index groups qnames by their last `::` segment: every
/// tier of [`Resolver::resolve_test_ids`] (exact qname, `::`-suffix, bare
/// name) needs that segment to equal the token's last one.
struct Resolver<'a> {
    graph: &'a MergedGraph,
    files: Option<HashMap<String, Vec<Located>>>,
    names: Option<HashMap<&'a str, Vec<(NodeId, &'a str)>>>,
    counts: MatchCounts,
}

impl<'a> Resolver<'a> {
    fn new(graph: &'a MergedGraph) -> Self {
        Resolver { graph, files: None, names: None, counts: MatchCounts::default() }
    }

    /// One signal (see [`MergedGraph::resolve_signal`]), counted into
    /// `self.counts`.
    fn resolve(&mut self, text: &str, kind: &str) -> Vec<NodeId> {
        let kind = if kind == "auto" { sniff_signal_kind(text) } else { kind };
        let mut out: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        match kind {
            "stacktrace" => {
                for (file, line) in parse_stack_frames(text) {
                    if let Some(id) = self.resolve_frame(&file, line)
                        && seen.insert(id)
                    {
                        out.push(id);
                    }
                }
            }
            "diff" => {
                let frames = parse_diff_frames(text);
                if frames.is_empty() {
                    // Plain changed-file list (one path per line): seed every
                    // node in each named file.
                    for line in text.lines() {
                        let p = line.trim();
                        if p.is_empty() || !p.contains('.') {
                            continue;
                        }
                        for id in self.resolve_file(p) {
                            if seen.insert(id) {
                                out.push(id);
                            }
                        }
                    }
                } else {
                    for (file, line) in frames {
                        if let Some(id) = self.resolve_frame(&file, line)
                            && seen.insert(id)
                        {
                            out.push(id);
                        }
                    }
                }
            }
            "test" => {
                for id in self.resolve_test_ids(text) {
                    if seen.insert(id) {
                        out.push(id);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// The located nodes whose POSITION file has basename `base`, in graph
    /// then node order (the index is built on first use).
    fn bucket(&mut self, base: &str) -> &[Located] {
        let graph = self.graph;
        let files = self.files.get_or_insert_with(|| {
            let mut idx: HashMap<String, Vec<Located>> = HashMap::new();
            for g in &graph.graphs {
                for n in &g.nodes {
                    if let Some((file, start, end)) = position_of(n) {
                        idx.entry(basename(&file).to_string())
                            .or_default()
                            .push(Located { file, id: n.id, start, end });
                    }
                }
            }
            idx
        });
        files.get(base).map_or(&[], Vec::as_slice)
    }

    /// The single most specific node whose POSITION cell spans `line_1based` in
    /// a file matching `file`. Two passes: a boundary-aligned path suffix
    /// first, the bare basename only if that found nothing — so a frame naming
    /// `svc_b/utils.py` no longer seeds `svc_a/utils.py`. Narrowest span wins
    /// within each pass (method over class over module).
    fn resolve_frame(&mut self, file: &str, line_1based: u32) -> Option<NodeId> {
        let want = normalize_query_path(file);
        let line0 = line_1based.saturating_sub(1);
        let mut precise: Option<(NodeId, u32)> = None;
        let mut loose: Option<(NodeId, u32)> = None;
        for n in self.bucket(basename(&want)) {
            if line0 < n.start || line0 > n.end {
                continue;
            }
            let slot = if path_tail_matches(&n.file, &want) { &mut precise } else { &mut loose };
            let width = n.end - n.start;
            if slot.map(|(_, w)| width < w).unwrap_or(true) {
                *slot = Some((n.id, width));
            }
        }
        if let Some((id, _)) = precise {
            self.counts.precise += 1;
            return Some(id);
        }
        if let Some((id, _)) = loose {
            self.counts.loose += 1;
            return Some(id);
        }
        None
    }

    /// Every node in the file `file` names (a changed-file seed when there's no
    /// line). Two passes: nodes whose POSITION matches by boundary-aligned path
    /// suffix, falling back to bare-basename matches only when that pass is
    /// empty. Keeping the basename pass means a bare `utils.py` query behaves
    /// exactly as before, and a query whose prefix disagrees with the stored
    /// POSITION prefix still finds something rather than nothing. Sorted by id
    /// for determinism.
    fn resolve_file(&mut self, file: &str) -> Vec<NodeId> {
        let want = normalize_query_path(file);
        let mut precise = Vec::new();
        let mut loose = Vec::new();
        for n in self.bucket(basename(&want)) {
            if path_tail_matches(&n.file, &want) {
                precise.push(n.id);
            } else {
                loose.push(n.id);
            }
        }
        let mut out = if precise.is_empty() {
            self.counts.loose += loose.len();
            loose
        } else {
            self.counts.precise += precise.len();
            precise
        };
        out.sort_by_key(|id| id.0);
        out
    }

    /// The `(id, qname)` of every node whose qname's last `::` segment is
    /// `last`, in graph then node order (the index is built on first use).
    fn named(&mut self, last: &str) -> &[(NodeId, &'a str)] {
        let graph = self.graph;
        let names = self.names.get_or_insert_with(|| {
            let mut idx: HashMap<&'a str, Vec<(NodeId, &'a str)>> = HashMap::new();
            for g in &graph.graphs {
                for n in &g.nodes {
                    let Some(qn) = g.nav.qname_by_id.get(&n.id) else { continue };
                    let qn = qn.as_str();
                    let tail = qn.rsplit("::").next().unwrap_or(qn);
                    idx.entry(tail).or_default().push((n.id, qn));
                }
            }
            idx
        });
        names.get(last).map_or(&[], Vec::as_slice)
    }

    /// Resolve test ids (pytest-style `path::Class::test_name`, Go
    /// `pkg::TestName`, etc.) and pasted qnames. Tiers, each through
    /// `pick_primary`: a node whose qname IS the `::`-joined non-path segments
    /// (LA.14 — `engine::src::arch::service_map` is that function, not the
    /// busiest node named `service_map`); then one whose qname ends with them;
    /// then the bare test name.
    fn resolve_test_ids(&mut self, text: &str) -> Vec<NodeId> {
        let graph = self.graph;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for tok in text.split_whitespace() {
            if !tok.contains("::") {
                continue;
            }
            let segs: Vec<&str> = tok.split("::").collect();
            // Drop a leading path-like segment (file part): it has a '.' or '/'.
            let name_segs: Vec<&str> = segs
                .iter()
                .copied()
                .filter(|s| !s.is_empty() && !s.contains('.') && !s.contains('/'))
                .collect();
            let Some(last) = name_segs.last() else { continue };
            let joined = name_segs.join("::");
            let suffix = format!("::{joined}");
            // Prefer the exact qname, then a qname ending with the full
            // ::-suffix, else the bare name. Collect ALL matches and
            // pick_primary — first-match over qname_by_id (a HashMap,
            // per-process seed) flapped the resolved seed across processes,
            // the same bug class pick_primary fixed for
            // resolve_name/resolve_span (audit 2026-06-10 #7).
            let mut exact: Vec<NodeId> = Vec::new();
            let mut suffix_matches: Vec<NodeId> = Vec::new();
            let mut name_matches: Vec<NodeId> = Vec::new();
            for &(id, qn) in self.named(last) {
                // One segment is a bare name, not a qname: it keeps its
                // old last-resort tier below, so `file.py::test_b` still
                // prefers a module-qualified `…::test_b`.
                if name_segs.len() > 1 && qn == joined {
                    exact.push(id);
                } else if qn.ends_with(&suffix) {
                    suffix_matches.push(id);
                } else if qn == *last {
                    name_matches.push(id);
                }
            }
            if !exact.is_empty() {
                self.counts.exact_qname += 1;
            }
            let id = graph
                .pick_primary(&exact)
                .or_else(|| graph.pick_primary(&suffix_matches))
                .or_else(|| graph.pick_primary(&name_matches))
                .or_else(|| graph.resolve_name(last));
            if let Some(id) = id
                && seen.insert(id)
            {
                out.push(id);
            }
        }
        out
    }
}

/// Decide which signal kind `text` is when the caller passes `"auto"`.
fn sniff_signal_kind(text: &str) -> &'static str {
    if text.contains("+++ ") || text.contains("--- a/") || text.contains("\n@@ ") {
        return "diff";
    }
    if (text.contains("File \"") && text.contains("line "))
        || text.contains(".go:")
        || text.contains("\n  at ")
    {
        return "stacktrace";
    }
    // A single bare token with `::` and no whitespace is a test id.
    let t = text.trim();
    if t.contains("::") && !t.chars().any(|c| c.is_whitespace()) {
        return "test";
    }
    // Otherwise try frame extraction; if that's empty the caller gets nothing.
    "stacktrace"
}

/// Extract `(file, line_1based)` frames from a stacktrace across languages:
/// Python `File "x", line N`, plus a generic `path.ext:line[:col]` scan that
/// covers Node/JS (`at f (path:line:col)`), Go (`\tpath:line`), and others.
fn parse_stack_frames(text: &str) -> Vec<(String, u32)> {
    let mut frames = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("File \"") {
            if let Some(end) = rest.find('"') {
                let file = &rest[..end];
                if let Some(lpos) = rest[end..].find("line ") {
                    let after = &rest[end + lpos + 5..];
                    let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = num.parse::<u32>() {
                        frames.push((file.to_string(), n));
                        continue;
                    }
                }
            }
        }
        frames.extend(scan_path_line(line));
    }
    frames
}

/// Find `path.ext:line` occurrences in a line (path must carry an extension to
/// avoid matching `http://`, bare `host:port`, etc.).
fn scan_path_line(line: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for tok in line.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',') {
        let mut parts = tok.split(':');
        let path = parts.next().unwrap_or("");
        if path.is_empty() || !path.contains('.') || path.ends_with('.') {
            continue;
        }
        if let Some(num) = parts.next() {
            let digits: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                out.push((path.to_string(), n));
            }
        }
    }
    out
}

/// Extract `(file, new_line)` frames from a unified diff: track the current
/// `+++ b/<file>` and the `@@ +c,d @@` new-file line counter, emitting a frame
/// per added line. Empty if `text` isn't a unified diff.
fn parse_diff_frames(text: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut cur_file: Option<String> = None;
    let mut new_line: u32 = 0;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("+++ ") {
            let p = p.split('\t').next().unwrap_or(p).trim();
            let p = p.strip_prefix("b/").unwrap_or(p);
            cur_file = if p == "/dev/null" { None } else { Some(p.to_string()) };
            continue;
        }
        if line.starts_with("--- ") {
            continue;
        }
        if let Some(h) = line.strip_prefix("@@ ") {
            if let Some(plus) = h.split('+').nth(1) {
                let c: String = plus.chars().take_while(|c| c.is_ascii_digit()).collect();
                new_line = c.parse().unwrap_or(0);
            }
            continue;
        }
        let Some(file) = &cur_file else { continue };
        if line.starts_with('+') {
            out.push((file.clone(), new_line));
            new_line = new_line.saturating_add(1);
        } else if line.starts_with('-') {
            // deletion: does not advance the new-file line counter
        } else {
            new_line = new_line.saturating_add(1);
        }
    }
    out
}

/// Parse a node's POSITION cell into `(file, start_line, end_line)` (0-based
/// rows), or `None`. Hand-rolled so the graph crate stays serde-free.
fn position_of(node: &Node) -> Option<(String, u32, u32)> {
    for c in &node.cells {
        if c.kind == cell_type::POSITION {
            if let repo_graph_core::CellPayload::Json(j) = &c.payload {
                let file = json_str_field(j, "file")?;
                let start = json_num_field(j, "start_line")?;
                let end = json_num_field(j, "end_line")?;
                return Some((file, start, end));
            }
        }
    }
    None
}

fn json_str_field(json: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let start = json.find(&marker)? + marker.len();
    let end = json[start..].find('"')? + start;
    Some(json[start..end].to_string())
}

fn json_num_field(json: &str, key: &str) -> Option<u32> {
    let marker = format!("\"{key}\":");
    let start = json.find(&marker)? + marker.len();
    let digits: String = json[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn basename(path: &str) -> &str {
    path.rsplit(|c: char| c == '/' || c == '\\').next().unwrap_or(path)
}

/// Normalise a caller-supplied path for matching: trim, `\` → `/`, drop a
/// leading `./`. POSITION paths are already repo-relative with forward slashes
/// (CODE_RULES.md §4), so only the query side needs this.
fn normalize_query_path(path: &str) -> String {
    let p = path.trim().replace('\\', "/");
    p.trim_start_matches("./").to_string()
}

/// True when `a` and `b` name the same file by a boundary-aligned path suffix:
/// either is a suffix of the other and the cut lands on a `/`. Lets an absolute
/// stacktrace frame (`/repo/svc_b/utils.py`) match a repo-relative POSITION
/// (`svc_b/utils.py`) while `svc_a/utils.py` does not. Both sides must already
/// use `/` separators — POSITION is canonical, queries go through
/// [`normalize_query_path`].
fn path_tail_matches(a: &str, b: &str) -> bool {
    fn suffix_of(long: &str, short: &str) -> bool {
        !short.is_empty()
            && long.ends_with(short)
            && (long.len() == short.len()
                || long.as_bytes()[long.len() - short.len() - 1] == b'/')
    }
    suffix_of(a, b) || suffix_of(b, a)
}

/// How the queries of one signal (or one batch) landed: file/frame queries on
/// a boundary-aligned path suffix (precise) or on the basename fallback
/// (loose), counted in seed nodes; test-id tokens that were a full qname
/// (`exact_qname`), counted in tokens.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct MatchCounts {
    precise: usize,
    loose: usize,
    exact_qname: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::repo;
    use crate::types::{RepoGraph, SymbolTable};
    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
    use repo_graph_core::{Cell, CellPayload, Confidence};

    fn graph_with_positioned_fn() -> (MergedGraph, NodeId) {
        let r = repo();
        let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::foo::bar");
        let mut nav = CodeNav::default();
        nav.record(id, "bar", "m::foo::bar", node_kind::FUNCTION, None);
        let node = Node {
            id,
            repo: r,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(
                    r#"{"file":"foo/bar.py","start_line":10,"end_line":20}"#.into(),
                ),
            }],
        };
        let g = RepoGraph {
            repo: r,
            nodes: vec![node],
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        (MergedGraph::new(vec![g]), id)
    }

    #[test]
    fn resolve_signal_matches_frames_diffs_and_tests() {
        let (m, id) = graph_with_positioned_fn();

        // Python stacktrace frame inside the node's span (line 15 ∈ [11,21] 1-based).
        let tb = "Traceback:\n  File \"/repo/foo/bar.py\", line 15, in bar\n    x.y()";
        assert_eq!(m.resolve_signal(tb, "stacktrace"), vec![id]);
        // auto-sniff routes it the same way.
        assert_eq!(m.resolve_signal(tb, "auto"), vec![id]);

        // Generic path:line (Node/Go style).
        assert_eq!(m.resolve_signal("at fn (foo/bar.py:16:3)", "stacktrace"), vec![id]);

        // Unified diff touching the file.
        let diff = "--- a/foo/bar.py\n+++ b/foo/bar.py\n@@ -14,1 +14,2 @@\n+    x = 1\n";
        assert_eq!(m.resolve_signal(diff, "diff"), vec![id]);

        // Plain changed-file list.
        assert_eq!(m.resolve_signal("foo/bar.py\n", "diff"), vec![id]);

        // pytest-style test id resolves by qname suffix.
        assert_eq!(m.resolve_signal("tests/test_x.py::bar", "test"), vec![id]);

        // A frame in a different file resolves to nothing.
        assert!(m
            .resolve_signal("File \"other.py\", line 15, in q", "stacktrace")
            .is_empty());
    }

    /// Two same-basename MODULE nodes in different sub-projects — the monorepo
    /// shape `resolve_file` / `resolve_frame` used to conflate.
    fn graph_with_two_utils() -> (MergedGraph, NodeId, NodeId) {
        let r = repo();
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for svc in ["svc_a", "svc_b"] {
            let qname = format!("{svc}::utils");
            let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, &qname);
            nav.record(id, "utils", &qname, node_kind::MODULE, None);
            nodes.push(Node {
                id,
                repo: r,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::POSITION,
                    payload: CellPayload::Json(format!(
                        r#"{{"file":"{svc}/utils.py","start_line":0,"end_line":5}}"#
                    )),
                }],
            });
            ids.push(id);
        }
        let g = RepoGraph {
            repo: r,
            nodes,
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        (MergedGraph::new(vec![g]), ids[0], ids[1])
    }

    #[test]
    fn resolve_file_prefers_path_suffix_over_basename() {
        let (m, a, b) = graph_with_two_utils();
        // A query carrying a directory component must not drag in the
        // same-basename file from the other sub-project.
        assert_eq!(m.resolve_signal("svc_b/utils.py\n", "diff"), vec![b]);
        // A bare basename is genuinely ambiguous: both, sorted by id (today's
        // behaviour, kept byte-identical).
        let mut both = vec![a, b];
        both.sort_by_key(|id| id.0);
        assert_eq!(m.resolve_signal("utils.py\n", "diff"), both);
        // A prefix that disagrees with every stored POSITION prefix falls back
        // to the basename pass rather than resolving to nothing.
        assert_eq!(m.resolve_signal("src/utils.py\n", "diff"), both);
    }

    /// A `::` token that IS a node's full qname (what an agent pastes from a
    /// `find` row) must resolve to that node. The `::`-suffix tier can never
    /// match it (no leading `::`), so before the exact tier it fell through to
    /// the bare last segment, where a busier same-named declaration and a
    /// same-named test MODULE both outrank the node that was actually named.
    fn graph_with_three_fs() -> (MergedGraph, NodeId) {
        let r = repo();
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        let mut id_of = |kind, qname: &str, name: &str| {
            let id = NodeId::from_parts(GRAPH_TYPE, r, kind, qname);
            nav.record(id, name, qname, kind, None);
            nodes.push(Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] });
            id
        };
        let exact = id_of(node_kind::FUNCTION, "m::a::f", "f");
        let busier = id_of(node_kind::FUNCTION, "m::b::f", "f");
        let module = id_of(node_kind::MODULE, "m::tests::f", "f");
        let callers: Vec<NodeId> = (0..3)
            .map(|i| id_of(node_kind::FUNCTION, &format!("m::b::c{i}"), &format!("c{i}")))
            .collect();
        let edge = |from, to, category| repo_graph_core::Edge {
            from,
            to,
            category,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        };
        let mut edges: Vec<repo_graph_core::Edge> = callers
            .iter()
            .map(|&c| edge(c, busier, repo_graph_code_domain::edge_category::CALLS))
            .collect();
        edges.extend(
            callers.iter().map(|&c| edge(module, c, repo_graph_code_domain::edge_category::IMPORTS)),
        );
        let g = RepoGraph {
            repo: r,
            nodes,
            edges,
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        (MergedGraph::new(vec![g]), exact)
    }

    #[test]
    fn exact_qname_token_resolves_to_itself() {
        let (m, exact) = graph_with_three_fs();
        // Precondition: by bare name the busier declaration wins.
        assert_ne!(m.resolve_name("f"), Some(exact));
        // `auto` sniffs a lone `::` token as a test id; both routes land on the
        // node whose qname the token is.
        assert_eq!(m.resolve_signal("m::a::f", "auto"), vec![exact]);
        assert_eq!(m.resolve_signal("m::a::f", "test"), vec![exact]);
        // A pytest id is unaffected: its path segment is dropped, so the
        // remaining `a::f` equals no qname and the `::`-suffix tier answers.
        assert_eq!(m.resolve_signal("tests/test_x.py::a::f", "test"), vec![exact]);
    }

    #[test]
    fn resolve_frame_prefers_path_suffix() {
        let (m, _a, b) = graph_with_two_utils();
        // Absolute frame vs repo-relative POSITION: the `/`-boundary suffix
        // match still lands, and it lands on svc_b only.
        let tb = "Traceback:\n  File \"/repo/svc_b/utils.py\", line 3, in helper\n";
        assert_eq!(m.resolve_signal(tb, "stacktrace"), vec![b]);
    }

    /// Each item's counts, alone, through a fresh [`Resolver`].
    fn counts_alone(m: &MergedGraph, text: &str, kind: &str) -> (Vec<NodeId>, MatchCounts) {
        let mut r = Resolver::new(m);
        let ids = r.resolve(text, kind);
        (ids, r.counts)
    }

    #[test]
    fn resolve_signals_sums_counts() {
        let (m, a, b) = graph_with_two_utils();
        let items: [(&str, &str); 3] = [
            // precise: the path suffix lands on svc_b.
            ("  File \"/repo/svc_b/utils.py\", line 3, in helper\n", "stacktrace"),
            // loose: a prefix no POSITION shares falls back to the basename,
            // which seeds both modules.
            ("src/utils.py\n", "diff"),
            // an exact qname token.
            ("svc_a::utils", "test"),
        ];
        let (batch, summed) = m.resolve_signals_counted(&items);
        let mut want = MatchCounts::default();
        for (i, (text, kind)) in items.iter().enumerate() {
            let (ids, c) = counts_alone(&m, text, kind);
            // Every item resolves in the batch exactly as it does alone ...
            assert_eq!(batch[i], ids, "item {i}");
            assert_eq!(batch[i], m.resolve_signal(text, kind), "item {i}");
            want.precise += c.precise;
            want.loose += c.loose;
            want.exact_qname += c.exact_qname;
        }
        // ... and the one marker line's counts are the per-item sums.
        assert_eq!(summed, want);
        assert_eq!(summed, MatchCounts { precise: 1, loose: 2, exact_qname: 1 });
        let mut both = vec![a, b];
        both.sort_by_key(|id| id.0);
        assert_eq!(batch, vec![vec![b], both, vec![a]]);
        assert_eq!(m.resolve_signals(&items), batch);
        assert!(m.resolve_signals(&[]).is_empty());
    }
}
