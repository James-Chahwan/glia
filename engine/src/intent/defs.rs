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

// ---------------------------------------------------------------------------
// Bare-id namespaces and the IDR resolution cascade (EB.5b)
// ---------------------------------------------------------------------------

/// The size limit of one lineage class (rank_eval `KLin::build(.., 6, ..)`).
const LINEAGE_CAP: usize = 6;

/// One series an id is defined in, under its namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Series {
    /// The namespace: `<scope>/<file name>`, or the page path when no member has a scope.
    pub ns: String,
    /// A findings series: visible only on its page and from its scope.
    pub local: bool,
    /// The namespace's scope (the named member's), if any.
    pub scope: Option<String>,
    /// The first member page (in page order) defining the id.
    pub page: String,
}

/// Bare ids separated by the page and family that define them (rank_eval
/// `KNs`), plus the lineage classes over namespaced keys (`KLin`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Namespaces {
    /// id -> its series, one per namespace, in namespace-component order.
    pub by_id: BTreeMap<String, Vec<Series>>,
    /// (page, id) -> the namespace of that page's definition.
    pub def: BTreeMap<(String, String), String>,
    /// namespace -> the scopes its member pages sit in.
    pub scopes: BTreeMap<String, BTreeSet<String>>,
    /// (namespace, id) -> the defining [`DefRow`] indices (into [`IdDefs::rows`]).
    pub rows: BTreeMap<(String, String), Vec<usize>>,
    /// Series (page, family) read.
    pub series: usize,
    /// Of them, findings (local) series.
    pub local: usize,
    /// Restated-series unions.
    pub merges: usize,
    /// Ids defined in more than one namespace.
    pub multi_series_ids: usize,
    /// Namespaces (merged components).
    pub namespaces: usize,
    /// Lineage unions done.
    pub lineage_merges: usize,
    /// Lineage pairs that redirect an old key to a new one.
    pub lineage_redirects: usize,
    /// Lineage unions refused for passing the class cap.
    pub lineage_capped: usize,
    /// (namespace, id) holding a pending row.
    pending: BTreeSet<(String, String)>,
    /// Every scope a namespace spans.
    all_scopes: BTreeSet<String>,
    /// key -> its lineage class representative (the least key), class members only.
    lineage_rep: BTreeMap<String, String>,
    /// representative -> the class's keys, sorted.
    lineage_members: BTreeMap<String, Vec<String>>,
    /// old key -> the new keys it redirects to, in pair order.
    lineage_redirect: BTreeMap<String, Vec<String>>,
}

/// The context of one bare mention, strongest evidence first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ctx<'a> {
    /// The page the mention sits on, when it is a doc page.
    pub own_page: Option<&'a str>,
    /// That page's scope.
    pub own_scope: Option<&'a str>,
    /// The scopes the mention's line names ([`line_scopes`]), in line order.
    pub line_scopes: Vec<String>,
    /// Scopes earlier commits resolved to, newest first.
    pub recent: &'a [String],
    /// The mention's text (its line or subject).
    pub text: &'a str,
}

/// One resolved bare mention.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Resolved {
    pub id: String,
    pub ns: String,
    /// `fact` / `derived` / `heuristic`.
    pub tier: &'static str,
    /// own-page, unique, line-scope, own-scope, global, open-row, one-left,
    /// recent-scope, newest-scope, sibling.
    pub route: &'static str,
}

/// Resolution routes counted for a caller's marker (`routes=<route>:<n>,..`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteCounts {
    /// route (or Err reason) -> count.
    pub counts: BTreeMap<&'static str, usize>,
}

impl RouteCounts {
    /// Count one resolution's route, or its Err reason.
    pub fn add(&mut self, r: &Result<Resolved, &'static str>) {
        let k = match r {
            Ok(r) => r.route,
            Err(e) => e,
        };
        *self.counts.entry(k).or_default() += 1;
    }

    /// `<route>:<n>,..` in route-name order.
    pub fn render(&self) -> String {
        self.counts.iter().map(|(k, n)| format!("{k}:{n}")).collect::<Vec<_>>().join(",")
    }
}

/// rank_eval `law_words`: alphanumeric runs, lowercased.
fn dice_words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// rank_eval `law_dice`: Dice over word multisets; empty against empty is 1.0.
fn word_dice(a: &str, b: &str) -> f64 {
    let wa = dice_words(a);
    let wb = dice_words(b);
    let mut ca: BTreeMap<&str, i64> = BTreeMap::new();
    for w in &wa {
        *ca.entry(w.as_str()).or_default() += 1;
    }
    let mut inter = 0i64;
    for w in &wb {
        if let Some(c) = ca.get_mut(w.as_str())
            && *c > 0
        {
            *c -= 1;
            inter += 1;
        }
    }
    let tot = (wa.len() + wb.len()) as f64;
    if tot == 0.0 { 1.0 } else { 2.0 * inter as f64 / tot }
}

fn uf_find(p: &mut [usize], mut x: usize) -> usize {
    while p[x] != x {
        p[x] = p[p[x]];
        x = p[x];
    }
    x
}

/// A spec-kit scope: its last two segments are `specs/<NNN-slug>`.
fn is_spec_scope(scope: &str) -> bool {
    let mut seg = scope.rsplit('/');
    seg.next().is_some_and(|d| !d.is_empty()) && seg.next() == Some("specs")
}

/// One series being read: (page, family) -> its ids' rows.
struct SerAcc {
    tracker: bool,
    local: bool,
    scope: Option<String>,
    ids: BTreeMap<String, Vec<usize>>,
}

impl IdDefs {
    /// The bare-id namespaces of these rows (rank_eval `KNs::build`) and their
    /// lineage classes (`KLin::build`). `local_words` are `[ids] local_words`
    /// (compared lowercased). The result does not depend on row order.
    pub fn namespaces(&self, local_words: &[String]) -> Namespaces {
        let words: Vec<String> =
            local_words.iter().map(|w| w.to_lowercase()).filter(|w| !w.is_empty()).collect();
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (ra, rb) = (&self.rows[a], &self.rows[b]);
            (&ra.page, ra.line, &ra.id, &ra.family, &ra.head).cmp(&(&rb.page, rb.line, &rb.id, &rb.family, &rb.head))
        });
        let mut ser: BTreeMap<(String, String), SerAcc> = BTreeMap::new();
        for &i in &order {
            let r = &self.rows[i];
            // a tracker row (no heading words): one series per file, never local
            let tracker = r.heading_words.is_empty();
            let is_local = !tracker
                && words.iter().any(|w| r.heading_words.contains(w.as_str()))
                && !r.nearest_heading.contains("decision");
            let e = ser.entry((r.page.clone(), r.family.clone())).or_insert_with(|| SerAcc {
                tracker,
                local: is_local,
                scope: r.scope.clone(),
                ids: BTreeMap::new(),
            });
            e.local = e.local && is_local;
            e.ids.entry(r.id.clone()).or_default().push(i);
        }
        let keys: Vec<&(String, String)> = ser.keys().collect();
        let accs: Vec<&SerAcc> = ser.values().collect();
        let mut out = Namespaces { series: keys.len(), ..Namespaces::default() };
        out.local = accs.iter().filter(|a| a.local).count();
        let mut parent: Vec<usize> = (0..keys.len()).collect();
        let head = |a: &SerAcc, id: &str| -> String {
            a.ids.get(id).and_then(|v| v.first()).map(|&i| self.rows[i].head.clone()).unwrap_or_default()
        };
        for i in 0..keys.len() {
            for j in i + 1..keys.len() {
                let (a, b) = (accs[i], accs[j]);
                if keys[i].1 != keys[j].1 || keys[i].0 == keys[j].0 || a.local || b.local || a.tracker || b.tracker {
                    continue;
                }
                let shared: Vec<&String> = a.ids.keys().filter(|k| b.ids.contains_key(*k)).collect();
                if shared.len() < 3 {
                    continue;
                }
                let dice: f64 =
                    shared.iter().map(|k| word_dice(&head(a, k), &head(b, k))).sum::<f64>() / shared.len() as f64;
                if dice >= 0.3 {
                    let (ra, rb) = (uf_find(&mut parent, i), uf_find(&mut parent, j));
                    if ra != rb {
                        parent[rb] = ra;
                        out.merges += 1;
                    }
                }
            }
        }
        let mut comp: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..keys.len() {
            let r = uf_find(&mut parent, i);
            comp.entry(r).or_default().push(i);
        }
        out.namespaces = comp.len();
        let name0 = |i: usize| -> String {
            let page = &keys[i].0;
            match &accs[i].scope {
                Some(s) => format!("{s}/{}", page.rsplit('/').next().unwrap_or(page)),
                None => page.clone(),
            }
        };
        for members in comp.values() {
            let scoped = members.iter().filter(|&&i| accs[i].scope.is_some()).min_by_key(|&&i| name0(i));
            let (name, scope) = match scoped {
                Some(&i) => (name0(i), accs[i].scope.clone()),
                None => (members.iter().map(|&i| name0(i)).min().unwrap_or_default(), None),
            };
            for &i in members {
                let acc = accs[i];
                let page = &keys[i].0;
                if let Some(s) = &acc.scope {
                    out.scopes.entry(name.clone()).or_default().insert(s.clone());
                    out.all_scopes.insert(s.clone());
                }
                for (id, idxs) in &acc.ids {
                    out.rows.entry((name.clone(), id.clone())).or_default().extend(idxs.iter().copied());
                    if idxs.iter().any(|&x| self.rows[x].state == RowState::Pending) {
                        out.pending.insert((name.clone(), id.clone()));
                    }
                    out.def.insert((page.clone(), id.clone()), name.clone());
                    let v = out.by_id.entry(id.clone()).or_default();
                    if !v.iter().any(|x| x.ns == name) {
                        v.push(Series { ns: name.clone(), local: acc.local, scope: scope.clone(), page: page.clone() });
                    }
                }
            }
        }
        out.multi_series_ids = out.by_id.values().filter(|v| v.len() > 1).count();
        out.build_lineage(self, &order);
        out
    }
}

impl Namespaces {
    /// A namespaced key, `<ns>|<id>`.
    pub fn key(ns: &str, id: &str) -> String {
        format!("{ns}|{id}")
    }

    /// rank_eval `KLin::build` over EB.5a's lineage pairs, each id resolved in
    /// its row's own page context: a pair whose ends have no other partner
    /// merges into one class (union-find in key order, capped at
    /// [`LINEAGE_CAP`]); any other pair redirects the old key to the new one.
    fn build_lineage(&mut self, defs: &IdDefs, order: &[usize]) {
        let mut pairs: BTreeSet<(String, String)> = BTreeSet::new();
        for &i in order {
            let r = &defs.rows[i];
            if r.lineage.is_empty() {
                continue;
            }
            let cx = Ctx { own_page: Some(r.page.as_str()), own_scope: r.scope.as_deref(), ..Ctx::default() };
            for (new, old, _) in &r.lineage {
                let (Ok(n), Ok(o)) = (self.resolve(new, &cx), self.resolve(old, &cx)) else { continue };
                let (kn, ko) = (Namespaces::key(&n.ns, new), Namespaces::key(&o.ns, old));
                if kn != ko {
                    pairs.insert((kn, ko));
                }
            }
        }
        let mut part: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (n, o) in &pairs {
            part.entry(n.as_str()).or_default().insert(o.as_str());
            part.entry(o.as_str()).or_default().insert(n.as_str());
        }
        let one = |k: &str| part.get(k).is_some_and(|s| s.len() == 1);
        let mut merge: Vec<(&String, &String)> = Vec::new();
        let mut redirect: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (n, o) in &pairs {
            if one(n) && one(o) {
                merge.push((n, o));
            } else {
                redirect.entry(o.clone()).or_default().push(n.clone());
                self.lineage_redirects += 1;
            }
        }
        let mut ix: BTreeMap<String, usize> = BTreeMap::new();
        let mut names: Vec<String> = Vec::new();
        let mut parent: Vec<usize> = Vec::new();
        let mut size: Vec<usize> = Vec::new();
        for (a, b) in merge {
            let mut id = |k: &String| -> usize {
                if let Some(&i) = ix.get(k) {
                    return i;
                }
                let i = names.len();
                ix.insert(k.clone(), i);
                names.push(k.clone());
                parent.push(i);
                size.push(1);
                i
            };
            let (ia, ib) = (id(a), id(b));
            let (ra, rb) = (uf_find(&mut parent, ia), uf_find(&mut parent, ib));
            if ra == rb {
                continue;
            }
            if size[ra] + size[rb] > LINEAGE_CAP {
                self.lineage_capped += 1;
                continue;
            }
            let (big, small) = if size[ra] >= size[rb] { (ra, rb) } else { (rb, ra) };
            parent[small] = big;
            size[big] += size[small];
            self.lineage_merges += 1;
        }
        let mut classes: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (i, name) in names.iter().enumerate() {
            let r = uf_find(&mut parent, i);
            classes.entry(r).or_default().push(name.clone());
        }
        for mut members in classes.into_values() {
            if members.len() < 2 {
                continue;
            }
            members.sort();
            let rep = members[0].clone();
            for m in &members {
                self.lineage_rep.insert(m.clone(), rep.clone());
            }
            self.lineage_members.insert(rep, members);
        }
        self.lineage_redirect = redirect;
    }

    fn class_of<'a>(&'a self, k: &'a str) -> &'a str {
        self.lineage_rep.get(k).map_or(k, String::as_str)
    }

    /// rank_eval `KLin::exp`, as keys: every member of `key`'s lineage class,
    /// then of the classes it redirects to, in insertion order (members sorted
    /// within a class). A key with no lineage expands to itself.
    pub fn expand(&self, key: &str) -> Vec<String> {
        let mut reps: Vec<&str> = vec![self.class_of(key)];
        for n in self.lineage_redirect.get(key).into_iter().flatten() {
            let c = self.class_of(n);
            if !reps.contains(&c) {
                reps.push(c);
            }
        }
        let mut out = Vec::new();
        for r in reps {
            match self.lineage_members.get(r) {
                Some(ms) => out.extend(ms.iter().cloned()),
                None => out.push(r.to_string()),
            }
        }
        out
    }

    fn in_scope(&self, x: &Series, c: &str) -> bool {
        x.scope.as_deref() == Some(c) || self.scopes.get(&x.ns).is_some_and(|s| s.contains(c))
    }

    /// rank_eval `KNs::resolve`'s pick: one visible series in the first scope
    /// of `list` that has any; an exported one when several.
    fn pick(&self, vis: &[&Series], list: &[&str]) -> Option<String> {
        for c in list {
            let hit: Vec<&&Series> = vis.iter().filter(|x| self.in_scope(x, c)).collect();
            if hit.len() == 1 {
                return Some(hit[0].ns.clone());
            }
            let exported: Vec<&&&Series> = hit.iter().filter(|x| !x.local).collect();
            if exported.len() == 1 {
                return Some(exported[0].ns.clone());
            }
        }
        None
    }

    /// Resolve a bare mention of `id`, first rule that applies: own page,
    /// undefined, local-only, unique, line-scope, own-scope, global, then
    /// rank_eval `r2_pick` over the pending pool (open-row / one-left,
    /// recent-scope, newest-scope).
    pub fn resolve(&self, id: &str, cx: &Ctx) -> Result<Resolved, &'static str> {
        let ok = |ns: String, tier: &'static str, route: &'static str| {
            Ok(Resolved { id: id.to_string(), ns, tier, route })
        };
        if let Some(p) = cx.own_page
            && let Some(n) = self.def.get(&(p.to_string(), id.to_string()))
        {
            return ok(n.clone(), "fact", "own-page");
        }
        let Some(all) = self.by_id.get(id) else { return Err("undefined") };
        let vis: Vec<&Series> = all
            .iter()
            .filter(|x| {
                !x.local || Some(x.page.as_str()) == cx.own_page || (x.scope.is_some() && x.scope.as_deref() == cx.own_scope)
            })
            .collect();
        match vis.len() {
            0 => return Err("local-only"),
            1 => return ok(vis[0].ns.clone(), "derived", "unique"),
            _ => {}
        }
        let line: Vec<&str> = cx.line_scopes.iter().map(String::as_str).collect();
        if let Some(n) = self.pick(&vis, &line) {
            return ok(n, "derived", "line-scope");
        }
        if let Some(n) = cx.own_scope.and_then(|s| self.pick(&vis, &[s])) {
            return ok(n, "derived", "own-scope");
        }
        let global: Vec<&&Series> = vis.iter().filter(|x| x.scope.is_none()).collect();
        if global.len() == 1 {
            return ok(global[0].ns.clone(), "derived", "global");
        }
        let pend: Vec<&Series> =
            vis.iter().copied().filter(|x| self.pending.contains(&(x.ns.clone(), id.to_string()))).collect();
        let pool: Vec<&Series> = if pend.is_empty() { vis.clone() } else { pend.clone() };
        if pool.len() == 1 {
            let route = if vis.len() > 1 && pend.len() == 1 { "open-row" } else { "one-left" };
            return ok(pool[0].ns.clone(), "derived", route);
        }
        for r in cx.recent {
            let mut hit: Vec<&Series> = pool.iter().copied().filter(|p| self.in_scope(p, r)).collect();
            hit.sort_by(|a, b| (a.local, &a.ns).cmp(&(b.local, &b.ns)));
            if let Some(h) = hit.first() {
                return ok(h.ns.clone(), "heuristic", "recent-scope");
            }
        }
        let newest = pool
            .iter()
            .filter_map(|p| p.scope.as_deref().filter(|s| is_spec_scope(s)).map(|s| (s, p)))
            .max_by(|(sa, a), (sb, b)| sa.cmp(sb).then_with(|| b.ns.cmp(&a.ns)));
        let n = match newest {
            Some((_, p)) => p.ns.clone(),
            None => pool.iter().map(|p| p.ns.clone()).min().unwrap_or_default(),
        };
        ok(n, "heuristic", "newest-scope")
    }

    /// Resolve every mention of one line, then rank_eval `KNs::siblings`: a
    /// mention resolved by `recent-scope` / `newest-scope` takes the namespace
    /// a same-family mention of the line got by another route, when that
    /// namespace defines it (route `sibling`, derived). When `cx.line_scopes`
    /// is empty the line's own [`line_scopes`] are read.
    pub fn resolve_line(&self, line: &str, mentions: &[IdMention], cx: &Ctx) -> Vec<Result<Resolved, &'static str>> {
        let own;
        let cx = if cx.line_scopes.is_empty() {
            let mut c = cx.clone();
            c.line_scopes = line_scopes(line, self);
            own = c;
            &own
        } else {
            cx
        };
        let mut res: Vec<Result<Resolved, &'static str>> = mentions.iter().map(|m| self.resolve(&m.id, cx)).collect();
        let weak = |r: &str| matches!(r, "recent-scope" | "newest-scope");
        for i in 0..mentions.len() {
            let Ok(ri) = &res[i] else { continue };
            if !weak(ri.route) {
                continue;
            }
            let id = &mentions[i].id;
            let mut got: Option<String> = None;
            for j in 0..mentions.len() {
                if j == i || mentions[j].family != mentions[i].family {
                    continue;
                }
                if let Ok(rj) = &res[j]
                    && !weak(rj.route)
                    && self.by_id.get(id).is_some_and(|v| v.iter().any(|x| x.ns == rj.ns))
                {
                    got = Some(rj.ns.clone());
                    break;
                }
            }
            if let Some(n) = got {
                res[i] = Ok(Resolved { id: id.clone(), ns: n, tier: "derived", route: "sibling" });
            }
        }
        res
    }

    /// Print the `[id-ns]` marker for repo `label`.
    pub fn report(&self, label: &str) {
        eprintln!(
            "[id-ns] repo={label} series={} local={} merged={} multi_series_ids={} namespaces={} lineage_merges={} lineage_redirects={} lineage_capped={}",
            self.series,
            self.local,
            self.merges,
            self.multi_series_ids,
            self.namespaces,
            self.lineage_merges,
            self.lineage_redirects,
            self.lineage_capped
        );
    }
}

/// The scopes `line` names, in line order, each once: a path token under a
/// scope dir (or the dir itself), a token equal to a scope dir's last segment,
/// or a spec-kit number (>= 3 digits) within one token of the word `spec` /
/// `specs` that names exactly one `specs/<NNN-..>` scope of `ns`.
pub fn line_scopes(line: &str, ns: &Namespaces) -> Vec<String> {
    let toks: Vec<&str> = line
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !(c.is_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))))
        .map(|t| t.trim_end_matches('.'))
        .collect();
    let mut out: Vec<String> = Vec::new();
    let push = |s: &String, out: &mut Vec<String>| {
        if !out.contains(s) {
            out.push(s.clone());
        }
    };
    for (t, tok) in toks.iter().enumerate() {
        if tok.is_empty() {
            continue;
        }
        for s in &ns.all_scopes {
            let under = *tok == s.as_str() || tok.strip_prefix(s.as_str()).is_some_and(|r| r.starts_with('/'));
            let last = s.rsplit('/').next() == Some(*tok);
            if under || last {
                push(s, &mut out);
            }
        }
        let lower = tok.to_ascii_lowercase();
        if lower != "spec" && lower != "specs" {
            continue;
        }
        let near = toks.iter().enumerate().skip(t.saturating_sub(2)).take(t + 3 - t.saturating_sub(2)).filter(|(j, _)| *j != t);
        for (_, near_tok) in near {
            let digits: String = near_tok.chars().take_while(char::is_ascii_digit).collect();
            if digits.len() < 3 {
                continue;
            }
            let prefix = format!("{digits}-");
            let named: Vec<&String> = ns
                .all_scopes
                .iter()
                .filter(|s| is_spec_scope(s) && s.rsplit('/').next().is_some_and(|d| d.starts_with(&prefix)))
                .collect();
            if let [one] = named.as_slice() {
                push(one, &mut out);
            }
        }
    }
    out
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
