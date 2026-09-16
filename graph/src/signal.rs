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
        match kind {
            "stacktrace" => {
                for (file, line) in parse_stack_frames(text) {
                    if let Some(id) = self.resolve_frame(&file, line) {
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
                        for id in self.resolve_file(p) {
                            if seen.insert(id) {
                                out.push(id);
                            }
                        }
                    }
                } else {
                    for (file, line) in frames {
                        if let Some(id) = self.resolve_frame(&file, line) {
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
        out
    }

    /// The single most specific node whose POSITION cell spans `line_1based` in
    /// a file whose basename matches `file`. Narrowest span wins (method over
    /// class over module).
    fn resolve_frame(&self, file: &str, line_1based: u32) -> Option<NodeId> {
        let base = basename(file);
        let line0 = line_1based.saturating_sub(1);
        let mut best: Option<(NodeId, u32)> = None;
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some((f, s, e)) = position_of(n) {
                    if basename(&f) == base && line0 >= s && line0 <= e {
                        let width = e - s;
                        if best.map(|(_, w)| width < w).unwrap_or(true) {
                            best = Some((n.id, width));
                        }
                    }
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Every node whose POSITION file basename matches `file` (a changed-file
    /// seed when there's no line). Sorted by id for determinism.
    fn resolve_file(&self, file: &str) -> Vec<NodeId> {
        let base = basename(file);
        let mut out = Vec::new();
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some((f, _, _)) = position_of(n) {
                    if basename(&f) == base {
                        out.push(n.id);
                    }
                }
            }
        }
        out.sort_by_key(|id| id.0);
        out
    }

    /// Resolve test ids (pytest-style `path::Class::test_name`, Go
    /// `pkg::TestName`, etc.). Matches a node whose qname ends with the
    /// `::`-joined non-path segments; falls back to the bare test name.
    fn resolve_test_ids(&self, text: &str) -> Vec<NodeId> {
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
            let suffix = format!("::{}", name_segs.join("::"));
            // Prefer a qname ending with the full ::-suffix; else the bare
            // name. Collect ALL matches and pick_primary — first-match over
            // qname_by_id (a HashMap, per-process seed) flapped the resolved
            // seed across processes, the same bug class pick_primary fixed
            // for resolve_name/resolve_span (audit 2026-06-10 #7).
            let mut suffix_matches: Vec<NodeId> = Vec::new();
            let mut name_matches: Vec<NodeId> = Vec::new();
            for g in &self.graphs {
                for n in &g.nodes {
                    let Some(qn) = g.nav.qname_by_id.get(&n.id) else { continue };
                    if qn.ends_with(&suffix) {
                        suffix_matches.push(n.id);
                    } else if qn.as_str() == *last {
                        name_matches.push(n.id);
                    }
                }
            }
            let id = self
                .pick_primary(&suffix_matches)
                .or_else(|| self.pick_primary(&name_matches))
                .or_else(|| self.resolve_name(last));
            if let Some(id) = id {
                if seen.insert(id) {
                    out.push(id);
                }
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
}
