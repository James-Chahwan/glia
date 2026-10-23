//! Id definitions read at a revision: where each vocabulary id is defined in
//! the tree as it stood, the file side typed id routing reads. Owners: EB.5a
//! (the reader), EB.5b.
//!
//! [`read_defs`] reads the defining docs through a [`Tree`] (review: the base
//! sha; find: the working tree; rank_eval: each test commit's parent), so the
//! rows are those of the markdown files a build of that tree ingests (docs.rs
//! `include_doc`, the same fence rule) plus every `[ids] markdown` tracker
//! glob's `.md` files (the EB-5 ruling: read even when the include rule does
//! not admit them) and every `[[ids.tracker]]` JSON file. The files are read
//! whole, so docs.rs `bound_section`'s 64 KiB cut never hides a row of a long
//! tasks file. Only the Tier-4 docs snapshot (Confluence / Notion / wiki, not
//! in the tree) is read from the built graph's DOC_SECTIONs.
//!
//! A ROW is a list item (`- `, `* `, `+ `, `<digits>. `, an optional `[ ]` /
//! `[x]` checkbox), a table row (not a `|---|` separator) or a heading, outside
//! a fenced block. Its head is the text after the markers, `**` / `__` /
//! backticks removed. It DEFINES id X when the head's first id mention starts
//! the head and is followed by its end or one of `` :.)-|—,``; a table row
//! only when its first cell is X alone (else it is a LABEL ROW, counted: the
//! harness's `label row is not a definition`). A row's block is the row line
//! and the deeper-indented non-row lines below it; its raw path tokens are
//! rank_eval `kpath_tokens` over the block, its lineage rank_eval
//! `klin_phrases`. Normalising and resolving the tokens is EB.5c's
//! `intent::paths`.
//!
//! Fired_on, stderr, once per [`IdDefs::report`]: `[id-defs] repo=<label>
//! rev=<sha12|worktree> pages=<p> rows=<r> pending=<a> done=<d>
//! label_rows=<l> lineage=<n> tracker_rows=<t> trackers_unreadable=<u>
//! snapshot_sections=<s>`.

use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::glia_config::TrackerDecl;
use glia_code_domain::{cell_type, node_kind};
use glia_core::{Cell, CellPayload, RepoId};
use glia_graph::MergedGraph;
use serde_json::Value;

use super::ids::{IdMention, IdVocab};
use crate::docs::{admits_doc, fence_marker, sdd_scope};
use crate::git_rev::read_paths_at;
pub use crate::git_rev::Tree;

/// The most raw path tokens one row keeps.
const MAX_ROW_NAMES: usize = 256;

/// A row's checkbox state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RowState {
    /// `- [ ]`, or a tracker task whose state is not a done value.
    Pending,
    /// `- [x]` / `- [X]`, or a tracker task whose state is a done value.
    Done,
    /// No checkbox.
    None,
}

impl RowState {
    /// `pending` / `done` / `none`.
    pub fn as_str(self) -> &'static str {
        match self {
            RowState::Pending => "pending",
            RowState::Done => "done",
            RowState::None => "none",
        }
    }
}

/// One row that defines an id.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DefRow {
    pub id: String,
    /// The mention's family ([`IdMention::family`]): `T`, `FR-`, `OPS-`.
    pub family: String,
    /// The repo-relative file (a snapshot section: its POSITION file).
    pub page: String,
    /// 1-based.
    pub line: i64,
    /// The SDD directory the page sits in (`specs/003-billing`), docs.rs `sdd_scope`.
    pub scope: Option<String>,
    pub state: RowState,
    /// The row line with every occurrence of the id replaced by a space (a
    /// tracker task: its `title`), for EB.5b's restated-series compare.
    pub head: String,
    /// The raw path tokens of the row's block, rank_eval `kpath_tokens`, at
    /// most 256 (a tracker task: its `files` strings).
    pub names: Vec<String>,
    /// `(new, old, phrase)` from the block's lineage phrases.
    pub lineage: Vec<(String, String, &'static str)>,
    /// Lowercased: the file name, then every heading the row sits under,
    /// nearest first, then `(top)` (rank_eval's `"{file} {chain}"`).
    pub heading_words: String,
    /// Lowercased: the row's own section heading (empty above the first one).
    pub nearest_heading: String,
}

/// Every id definition of one tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdDefs {
    /// Sorted by (page, line, id).
    pub rows: Vec<DefRow>,
    /// Table rows whose head starts with an id their first cell does not hold alone.
    pub label_rows: usize,
    /// Rows read from `[[ids.tracker]]` JSON files.
    pub tracker_rows: usize,
    /// Tracker files that are not JSON, or hold no array where `list` says.
    pub trackers_unreadable: usize,
    /// The tree read: `<sha12>` or `worktree`.
    pub rev: String,
    /// Markdown pages read (tree files and snapshot pages).
    pub pages: usize,
    /// Tier-4 snapshot DOC_SECTIONs read.
    pub snapshot_sections: usize,
    /// Kept files skipped for size (over 4 MiB).
    pub too_large: usize,
    /// Why the tree could not be listed, when it could not (then no tree row).
    pub tree_error: Option<String>,
}

impl IdDefs {
    /// Rows in the pending state.
    pub fn pending(&self) -> usize {
        self.rows.iter().filter(|r| r.state == RowState::Pending).count()
    }

    /// Rows in the done state.
    pub fn done(&self) -> usize {
        self.rows.iter().filter(|r| r.state == RowState::Done).count()
    }

    /// Lineage pairs over every row.
    pub fn lineage(&self) -> usize {
        self.rows.iter().map(|r| r.lineage.len()).sum()
    }

    /// Print the `[id-defs]` marker for repo `label`.
    pub fn report(&self, label: &str) {
        eprintln!(
            "[id-defs] repo={label} rev={} pages={} rows={} pending={} done={} label_rows={} lineage={} tracker_rows={} trackers_unreadable={} snapshot_sections={}",
            self.rev,
            self.pages,
            self.rows.len(),
            self.pending(),
            self.done(),
            self.label_rows,
            self.lineage(),
            self.tracker_rows,
            self.trackers_unreadable,
            self.snapshot_sections
        );
    }
}

/// Every id definition of `tree`: the markdown files the build's include rule
/// admits against `project_dirs` (the walk's non-empty PROJECT root dirs) and
/// those `vocab.markdown_globs()` match, the `vocab.trackers` JSON files, and,
/// when given, repo `RepoId`'s Tier-4 snapshot DOC_SECTIONs of a built graph.
/// A tree that cannot be listed leaves `tree_error` set and reads no file.
pub fn read_defs(
    tree: &Tree,
    project_dirs: &[&str],
    vocab: &IdVocab,
    snapshot_docs: Option<(&MergedGraph, RepoId)>,
) -> IdDefs {
    let mut defs = IdDefs { rev: tree.label(), ..IdDefs::default() };
    let markdown = vocab.markdown_globs();
    let trackers: Vec<_> = (0..vocab.trackers.len()).map(|i| vocab.tracker_globs(i)).collect();
    let is_doc = |p: &str| {
        p.to_ascii_lowercase().ends_with(".md")
            && (admits_doc(p, project_dirs) || markdown.as_ref().is_some_and(|g| g.matches(p)))
    };
    let tracker_of = |p: &str| trackers.iter().position(|g| g.as_ref().is_some_and(|g| g.matches(p)));
    let keep = |p: &str| is_doc(p) || tracker_of(p).is_some();
    match read_paths_at(tree, &keep) {
        Ok(read) => {
            defs.too_large = read.too_large;
            for (path, bytes) in &read.files {
                if is_doc(path) {
                    let Ok(text) = std::str::from_utf8(bytes) else { continue };
                    defs.pages += 1;
                    let scope = sdd_scope(path, project_dirs);
                    read_page(path, scope, &[(0, text)], vocab, &mut defs);
                } else if let Some(decl) = tracker_of(path).and_then(|i| vocab.trackers.get(i)) {
                    read_tracker(path, bytes, decl, vocab, &mut defs);
                }
            }
        }
        Err(e) => defs.tree_error = Some(e),
    }
    if let Some((merged, repo)) = snapshot_docs {
        for (page, sections) in snapshot_pages(merged, repo) {
            defs.pages += 1;
            defs.snapshot_sections += sections.len();
            let segments: Vec<(i64, &str)> = sections.iter().map(|(l, t)| (*l, t.as_str())).collect();
            let scope = sdd_scope(&page, project_dirs);
            read_page(&page, scope, &segments, vocab, &mut defs);
        }
    }
    defs.rows
        .sort_by(|a, b| (&a.page, a.line, &a.id).cmp(&(&b.page, b.line, &b.id)));
    defs
}

/// Repo `repo`'s external (Tier-4 snapshot) DOC_SECTIONs by POSITION file:
/// `(0-based start row, CODE text)` in row order. An external section is one
/// whose ORIGIN names a `source` other than `file` (docs.rs writes
/// `confluence` / `notion` / `wiki`); its POSITION file is the record's
/// `rel_path`.
fn snapshot_pages(merged: &MergedGraph, repo: RepoId) -> BTreeMap<String, Vec<(i64, String)>> {
    let mut pages: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if n.repo != repo
                || g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::DOC_SECTION)
                || !is_external_doc(&n.cells)
                || !seen.insert(n.id.0)
            {
                continue;
            }
            let (Some((file, row)), Some(text)) = (position(&n.cells), code_text(&n.cells)) else { continue };
            pages.entry(file).or_default().push((row, text.to_string()));
        }
    }
    for sections in pages.values_mut() {
        sections.sort();
    }
    pages
}

fn is_external_doc(cells: &[Cell]) -> bool {
    cells.iter().any(|c| match &c.payload {
        CellPayload::Json(s) if c.kind == cell_type::ORIGIN => serde_json::from_str::<Value>(s).is_ok_and(|v| {
            v.get("provenance").and_then(Value::as_str) == Some("documentation")
                && v.get("source").and_then(Value::as_str).is_some_and(|t| t != "file")
        }),
        _ => false,
    })
}

fn position(cells: &[Cell]) -> Option<(String, i64)> {
    cells.iter().find_map(|c| match &c.payload {
        CellPayload::Json(s) if c.kind == cell_type::POSITION => {
            let v: Value = serde_json::from_str(s).ok()?;
            Some((v.get("file")?.as_str()?.replace('\\', "/"), v.get("start_line")?.as_i64()?))
        }
        _ => None,
    })
}

fn code_text(cells: &[Cell]) -> Option<&str> {
    cells.iter().find_map(|c| match &c.payload {
        CellPayload::Text(s) if c.kind == cell_type::CODE => Some(s.as_str()),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Markdown rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    List,
    Table,
    Heading,
}

/// A row line's shape: its kind, indent (bytes), checkbox, cleaned head, and
/// for a table row its first cell trimmed of `*`, backticks and spaces.
struct RowShape {
    kind: RowKind,
    indent: usize,
    state: RowState,
    head: String,
    first_cell: String,
}

/// A `|---|:--|` table separator line.
fn is_table_separator(t: &str) -> bool {
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// The row a line is, if any (the caller skips fenced lines).
fn row_shape(line: &str) -> Option<RowShape> {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    let (kind, body) = if t.starts_with('#') {
        let h = t.bytes().take_while(|&c| c == b'#').count();
        if h > 6 || !t[h..].starts_with(' ') {
            return None;
        }
        (RowKind::Heading, &t[h..])
    } else if let Some(cells) = t.strip_prefix('|') {
        if is_table_separator(t) {
            return None;
        }
        (RowKind::Table, cells)
    } else if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
        (RowKind::List, r)
    } else {
        let d = t.bytes().take_while(u8::is_ascii_digit).count();
        if d == 0 || !t[d..].starts_with(". ") {
            return None;
        }
        (RowKind::List, &t[d + 2..])
    };
    let mut body = body.trim_start();
    let mut state = RowState::None;
    if kind == RowKind::List {
        if let Some(r) = body.strip_prefix("[ ]") {
            state = RowState::Pending;
            body = r;
        } else if let Some(r) = body.strip_prefix("[x]").or_else(|| body.strip_prefix("[X]")) {
            state = RowState::Done;
            body = r;
        }
    }
    let head = body.replace("**", "").replace("__", "").replace('`', "");
    let head = head.trim_start().to_string();
    let first_cell = if kind == RowKind::Table {
        body.split('|').next().unwrap_or("").trim_matches(|c| c == '*' || c == '`' || c == ' ').to_string()
    } else {
        String::new()
    };
    Some(RowShape { kind, indent, state, head, first_cell })
}

/// The id a row's head defines: its first mention, starting the head and
/// followed by the head's end or one of ` : . ) - | — ,`.
fn defined_id(head: &str, vocab: &IdVocab) -> Option<IdMention> {
    let first = vocab.mentions(head).into_iter().next()?;
    if first.start != 0 {
        return None;
    }
    let after = head.get(first.end..)?;
    let ok = after
        .chars()
        .next()
        .is_none_or(|c| matches!(c, ' ' | ':' | '.' | ')' | '-' | '|' | '\u{2014}' | ','));
    ok.then_some(first)
}

/// Read one markdown page's rows into `defs`. `segments` are `(0-based first
/// row, text)` in order (a tree file: one, at 0; a snapshot page: its
/// sections); the heading stack runs across them.
fn read_page(page: &str, scope: Option<String>, segments: &[(i64, &str)], vocab: &IdVocab, defs: &mut IdDefs) {
    let file = page.rsplit('/').next().unwrap_or(page).to_ascii_lowercase();
    // (level, lowercased heading text), the innermost last.
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (row0, text) in segments {
        let lines: Vec<&str> = text.lines().collect();
        let fenced = fenced_lines(&lines);
        for (i, line) in lines.iter().enumerate() {
            if fenced[i] {
                continue;
            }
            let Some(shape) = row_shape(line) else { continue };
            if shape.kind == RowKind::Heading {
                let t = line.trim_start();
                let level = t.bytes().take_while(|&c| c == b'#').count();
                while stack.last().is_some_and(|s| s.0 >= level) {
                    stack.pop();
                }
                stack.push((level, t.trim_start_matches('#').trim().to_ascii_lowercase()));
            }
            let Some(m) = defined_id(&shape.head, vocab) else { continue };
            if shape.kind == RowKind::Table && shape.first_cell != m.id {
                defs.label_rows += 1;
                continue;
            }
            let block = block_lines(&lines, &fenced, i, shape.indent);
            let mut names = Vec::new();
            let mut lineage = Vec::new();
            for &j in &block {
                let l = lines[j];
                for tok in kpath_tokens(l) {
                    if names.len() < MAX_ROW_NAMES {
                        names.push(tok);
                    }
                }
                lineage.extend(lineage_phrases(l, &vocab.mentions(l), &m.id));
            }
            let mut chain = String::new();
            for (_, h) in stack.iter().rev() {
                chain.push_str(h);
                chain.push(' ');
            }
            chain.push_str("(top) ");
            defs.rows.push(DefRow {
                head: line.replace(m.id.as_str(), " "),
                id: m.id,
                family: m.family,
                page: page.to_string(),
                line: row0 + i as i64 + 1,
                scope: scope.clone(),
                state: shape.state,
                names,
                lineage,
                heading_words: format!("{file} {chain}"),
                nearest_heading: stack.last().map(|s| s.1.clone()).unwrap_or_default(),
            });
        }
    }
}

/// Per line, whether it is a fence line or inside a fenced block: docs.rs's
/// rule (`chunk_markdown`): a fence opens on a line starting ```` ``` ```` or
/// `~~~` and closes on the next line starting with the same marker; an
/// unclosed fence runs to the end.
fn fenced_lines(lines: &[&str]) -> Vec<bool> {
    let mut fence: Option<&'static str> = None;
    lines
        .iter()
        .map(|line| {
            let in_fence = fence.is_some();
            match fence {
                Some(open) if line.trim_start().starts_with(open) => fence = None,
                Some(_) => {}
                None => fence = fence_marker(line),
            }
            in_fence || fence.is_some()
        })
        .collect()
}

/// The row at `i` and the lines below it indented deeper than `indent` that
/// are not rows themselves, across blank lines while an indented line follows.
fn block_lines(lines: &[&str], fenced: &[bool], i: usize, indent: usize) -> Vec<usize> {
    let mut block = vec![i];
    for j in i + 1..lines.len() {
        let l = lines[j];
        if fenced[j] {
            break;
        }
        if l.trim().is_empty() {
            continue;
        }
        if l.len() - l.trim_start().len() <= indent || row_shape(l).is_some() {
            break;
        }
        block.push(j);
    }
    block
}

/// rank_eval `kpath_tokens`, exactly: path-like tokens on a line, split at
/// whitespace and at `` ` ' " ( ) [ ] < > ; | = `` and at `,` outside `{..}`,
/// so a brace glob keeps its commas.
pub(crate) fn kpath_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for c in line.chars() {
        let sep = c.is_whitespace()
            || matches!(c, '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '<' | '>' | ';' | '|' | '=')
            || (c == ',' && depth == 0);
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth = depth.saturating_sub(1);
        }
        if sep && depth == 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        if depth > 0 {
            out.extend(cur.split_whitespace().map(str::to_string));
        } else {
            out.push(cur);
        }
    }
    out
}

/// rank_eval `KLIN_PHRASES`: identity lineage phrases (an absorption such as
/// `folded into` is not one): the phrase, its name, and whether the id it
/// names is the NEW one (`superseded by X`) rather than the old one.
const KLIN_PHRASES: &[(&str, &str, bool)] = &[
    ("(was ", "was", false),
    ("formerly ", "formerly", false),
    ("previously ", "previously", false),
    ("renamed from ", "renamed", false),
    ("renumbered from ", "renumbered", false),
    ("supersedes ", "supersedes", false),
    ("replaces ", "replaces", false),
    ("split from ", "split-from", false),
    ("superseded by ", "superseded-by", true),
    ("replaced by ", "replaced-by", true),
    ("(now ", "now", true),
    (" is now ", "now", true),
    ("split into ", "split-into", true),
];

/// rank_eval `klin_phrases` and its lineage fold: per phrase on the line (ASCII
/// case-insensitive), the object is the first mention starting within 3 bytes
/// after the phrase, the subject the last mention ending at most 40 bytes
/// before it, else the row's own id; `(new, old, phrase)`, the subject the new
/// id unless the phrase names the new one. A pair of one id is dropped.
fn lineage_phrases(line: &str, ids: &[IdMention], own: &str) -> Vec<(String, String, &'static str)> {
    let lower = line.to_ascii_lowercase();
    let mut out = Vec::new();
    for (ph, name, names_new) in KLIN_PHRASES {
        let mut from = 0;
        while let Some(p) = lower[from..].find(ph) {
            let s = from + p;
            let e = s + ph.len();
            from = e;
            let Some(obj) = ids.iter().find(|x| x.start >= e && x.start <= e + 3) else { continue };
            let subj = ids
                .iter()
                .rev()
                .find(|x| x.end <= s && s - x.end <= 40)
                .map_or(own, |x| x.id.as_str());
            if subj == obj.id {
                continue;
            }
            let (new, old) = if *names_new { (obj.id.as_str(), subj) } else { (subj, obj.id.as_str()) };
            out.push((new.to_string(), old.to_string(), *name));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// JSON trackers
// ---------------------------------------------------------------------------

/// Read one `[[ids.tracker]]` file: each object of the array at `list` (or the
/// top level) whose `id` field is one whole vocabulary id is a row. A file that
/// is not JSON, or holds no array there, counts `trackers_unreadable`.
fn read_tracker(path: &str, bytes: &[u8], decl: &TrackerDecl, vocab: &IdVocab, defs: &mut IdDefs) {
    let parsed: Option<Value> = serde_json::from_slice(bytes).ok();
    let items = parsed.as_ref().and_then(|v| match &decl.list {
        Some(key) => v.get(key.as_str()).and_then(Value::as_array),
        None => v.as_array(),
    });
    let Some(items) = items else {
        defs.trackers_unreadable += 1;
        return;
    };
    let text = String::from_utf8_lossy(bytes);
    let mut cursor = 0usize;
    for item in items {
        let Some(id) = item.get(decl.id.as_str()).and_then(Value::as_str) else { continue };
        let whole = vocab.mentions(id).into_iter().next().filter(|m| m.start == 0 && m.end == id.len());
        let Some(m) = whole else { continue };
        let line = match id_field_at(&text, cursor, &decl.id, id) {
            Some((at, end)) => {
                cursor = end;
                at
            }
            None => cursor,
        };
        let line = text[..line.min(text.len())].bytes().filter(|&b| b == b'\n').count() as i64 + 1;
        let state = match decl.state.as_deref().and_then(|f| item.get(f)).and_then(Value::as_str) {
            Some(v) if decl.done.iter().any(|d| d == v) => RowState::Done,
            _ => RowState::Pending,
        };
        let names: Vec<String> = match item.get(decl.files.as_str()) {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        defs.rows.push(DefRow {
            id: id.to_string(),
            family: m.family,
            page: path.to_string(),
            line,
            scope: None,
            state,
            head: item.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            names: names.into_iter().take(MAX_ROW_NAMES).collect(),
            lineage: Vec::new(),
            heading_words: String::new(),
            nearest_heading: String::new(),
        });
        defs.tracker_rows += 1;
    }
}

/// The byte span of the first `"<field>" : "<id>"` at or after `from`.
fn id_field_at(text: &str, from: usize, field: &str, id: &str) -> Option<(usize, usize)> {
    let key = format!("\"{field}\"");
    let value = format!("\"{id}\"");
    let mut at = from;
    while let Some(p) = text.get(at..)?.find(&key) {
        let start = at + p;
        let rest = text[start + key.len()..].trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if rest.starts_with(&value) {
                let end = text.len() - rest.len() + value.len();
                return Some((start, end));
            }
        }
        at = start + key.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kpath_tokens_keep_brace_globs_whole() {
        assert_eq!(
            kpath_tokens("touch `src/{a,b}.rs`, docs/x.md (new); see [y](z/w.md)"),
            ["touch", "src/{a,b}.rs", "docs/x.md", "new", "see", "y", "z/w.md"]
        );
    }

    #[test]
    fn rows_define_only_at_the_head() {
        let v = IdVocab::default();
        let id = |l: &str| row_shape(l).and_then(|s| defined_id(&s.head, &v)).map(|m| m.id);
        assert_eq!(id("- **FR-001**: users can pay").as_deref(), Some("FR-001"));
        assert_eq!(id("- [x] T013 done").as_deref(), Some("T013"));
        assert_eq!(id("1. T020: numbered").as_deref(), Some("T020"));
        assert_eq!(id("## T014: retry").as_deref(), Some("T014"));
        assert_eq!(id("- see T012 later"), None);
        assert_eq!(id("- T012x glued"), None);
        assert_eq!(id("plain T012 text"), None);
        assert!(row_shape("|---|:--|").is_none());
        assert!(row_shape("#hashtag").is_none());
    }

    #[test]
    fn lineage_follows_the_harness_subject_rule() {
        let v = IdVocab::default();
        let l = "- [ ] T015 (was T009) superseded by T020";
        let got = lineage_phrases(l, &v.mentions(l), "T015");
        assert_eq!(
            got,
            [
                ("T015".to_string(), "T009".to_string(), "was"),
                ("T020".to_string(), "T009".to_string(), "superseded-by"),
            ]
        );
    }

    #[test]
    fn id_field_is_found_after_the_cursor() {
        let t = "[{\"id\": \"A-1\"},\n {\"id\":\"A-1\"}]";
        let (a, e) = id_field_at(t, 0, "id", "A-1").expect("first");
        assert_eq!(a, 2);
        let (b, _) = id_field_at(t, e, "id", "A-1").expect("second");
        assert!(b > e && t[..b].contains('\n'));
    }
}
