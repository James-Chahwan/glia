//! Signal resolution (WP-B / GR-2) — stacktrace / diff / test-id text to seed
//! node ids, plus the hand-rolled frame and POSITION-cell parsers it needs.

use std::collections::HashSet;

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
        let kind = if kind == "auto" { sniff_signal_kind(text) } else { kind };
        let mut out: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        // How each file/frame query landed, accumulated across the whole signal
        // so the marker fires once per call and not once per stack frame.
        let mut counts = MatchCounts::default();
        match kind {
            "stacktrace" => {
                for (file, line) in parse_stack_frames(text) {
                    if let Some(id) = self.resolve_frame(&file, line, &mut counts) {
                        if seen.insert(id) {
                            out.push(id);
                        }
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
                        for id in self.resolve_file(p, &mut counts) {
                            if seen.insert(id) {
                                out.push(id);
                            }
                        }
                    }
                } else {
                    for (file, line) in frames {
                        if let Some(id) = self.resolve_frame(&file, line, &mut counts) {
                            if seen.insert(id) {
                                out.push(id);
                            }
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
        if counts.precise + counts.loose > 0 {
            eprintln!(
                "[resolve] file signal: path-suffix={} basename-fallback={}",
                counts.precise, counts.loose
            );
        }
        out
    }

    /// The single most specific node whose POSITION cell spans `line_1based` in
    /// a file matching `file`. Two passes: a boundary-aligned path suffix
    /// first, the bare basename only if that found nothing — so a frame naming
    /// `svc_b/utils.py` no longer seeds `svc_a/utils.py`. Narrowest span wins
    /// within each pass (method over class over module).
    fn resolve_frame(
        &self,
        file: &str,
        line_1based: u32,
        counts: &mut MatchCounts,
    ) -> Option<NodeId> {
        let want = normalize_query_path(file);
        let base = basename(&want);
        let line0 = line_1based.saturating_sub(1);
        let mut precise: Option<(NodeId, u32)> = None;
        let mut loose: Option<(NodeId, u32)> = None;
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some((f, s, e)) = position_of(n) {
                    if line0 < s || line0 > e {
                        continue;
                    }
                    let slot = if path_tail_matches(&f, &want) {
                        &mut precise
                    } else if basename(&f) == base {
                        &mut loose
                    } else {
                        continue;
                    };
                    let width = e - s;
                    if slot.map(|(_, w)| width < w).unwrap_or(true) {
                        *slot = Some((n.id, width));
                    }
                }
            }
        }
        if let Some((id, _)) = precise {
            counts.precise += 1;
            return Some(id);
        }
        if let Some((id, _)) = loose {
            counts.loose += 1;
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
    fn resolve_file(&self, file: &str, counts: &mut MatchCounts) -> Vec<NodeId> {
        let want = normalize_query_path(file);
        let base = basename(&want);
        let mut precise = Vec::new();
        let mut loose = Vec::new();
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some((f, _, _)) = position_of(n) {
                    if path_tail_matches(&f, &want) {
                        precise.push(n.id);
                    } else if basename(&f) == base {
                        loose.push(n.id);
                    }
                }
            }
        }
        let mut out = if precise.is_empty() {
            counts.loose += loose.len();
            loose
        } else {
            counts.precise += precise.len();
            precise
        };
        out.sort_by_key(|id| id.0);
        out
    }

    /// Resolve test ids (pytest-style `path::Class::test_name`, Go
    /// `pkg::TestName`, etc.) and pasted qnames. Tiers, each through
    /// `pick_primary`: a node whose qname IS the `::`-joined non-path segments
    /// (LA.14 — `engine::src::arch::service_map` is that function, not the
    /// busiest node named `service_map`); then one whose qname ends with them;
    /// then the bare test name.
    fn resolve_test_ids(&self, text: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut exact_hits = 0usize;
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
            for g in &self.graphs {
                for n in &g.nodes {
                    let Some(qn) = g.nav.qname_by_id.get(&n.id) else { continue };
                    // One segment is a bare name, not a qname: it keeps its
                    // old last-resort tier below, so `file.py::test_b` still
                    // prefers a module-qualified `…::test_b`.
                    if name_segs.len() > 1 && *qn == joined {
                        exact.push(n.id);
                    } else if qn.ends_with(&suffix) {
                        suffix_matches.push(n.id);
                    } else if qn.as_str() == *last {
                        name_matches.push(n.id);
                    }
                }
            }
            if !exact.is_empty() {
                exact_hits += 1;
            }
            let id = self
                .pick_primary(&exact)
                .or_else(|| self.pick_primary(&suffix_matches))
                .or_else(|| self.pick_primary(&name_matches))
                .or_else(|| self.resolve_name(last));
            if let Some(id) = id {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
        // fired_on marker (LA.14): only when a token was a full qname, so a
        // plain test-id signal stays silent.
        if exact_hits > 0 {
            eprintln!("[resolve] test-id exact-qname tokens={exact_hits}");
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

/// How the file/frame queries in one signal landed: on a boundary-aligned path
/// suffix (precise) or on the basename fallback (loose), counted in seed nodes.
#[derive(Default)]
struct MatchCounts {
    precise: usize,
    loose: usize,
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
}
