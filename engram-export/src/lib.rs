//! glia-engram-export — write a resolved glia graph as an
//! `engram_core::Gmap` bincode file: engram's "Path A" structural seed.
//!
//! This is an **additional** output artifact. glia's native sharded `.gmap`
//! (rkyv + mmap, one file per language sub-graph + a manifest) is unchanged.
//! engram cannot read that format; it expects a single `bincode` file holding
//! one `engram_core::Gmap`. We bridge by walking the in-memory resolved
//! [`MergedGraph`] and recovering `(qname, short name, span)` per node from the
//! same `CodeNav` index that `cli`/`projection-text` already use for output.
//!
//! Contract source of truth is the [`engram_core`] crate, pulled in as a path
//! dependency so the structs + serde derive are byte-identical on both sides
//! (byte-compat guaranteed, not hoped for). Only this crate touches that
//! cross-repo dependency, which keeps the rest of the workspace clean for the
//! planned glia repo split.
//!
//! ## The span story (v6)
//!
//! engram's `SpanRef { file, start, end, start_line, end_line }` names one
//! region of an interned file twice: a half-open **byte range** for editors
//! and tools that slice source, and a **1-based, inclusive line range** for
//! humans and renderers (`orders.py:7`), so neither has to re-read the file.
//! glia stores POSITION cells as
//! `{"file": "<repo-relative path>", "start_line": r, "end_line": r}` —
//! 0-based, end-inclusive rows, no columns, no byte offsets, path as a string.
//! Both halves of the span come from those same rows at export time (the repo
//! source is present, since we export straight off a freshly generated graph):
//!
//!   1. Distinct file paths are interned to **stable, 1-based** `u32` ids
//!      (sorted order; `0` is reserved for "no/unknown position").
//!   2. Each source file is read once and its line-start byte offsets indexed:
//!      `byte_range` turns the rows into the bytes of those whole lines, and
//!      `line_span` — the one place rows become lines — turns them into
//!      1-based lines, clamped to the file's real line count.
//!   3. The `id → path` table is written as a sidecar `*.files.json` next to
//!      the bincode and inlined as `Gmap.files`, so the span round-trips back
//!      to a path. engram never computes on the span; it hands it back.
//!
//! Doc sections export as `Content::Proposition`, anchored by the same
//! `SpanRef` built the same way: `span: Some(..)` whenever the section has a
//! POSITION (every markdown section and contract operation does), `None` only
//! when it has none.
//!
//! If a POSITION file can't be read (e.g. exporting against a moved repo) the
//! node keeps its interned file id and its lines — they need no source read —
//! but its bytes are `0..0`; a node with no POSITION cell gets
//! `SpanRef::NONE` (file `0`, lines `0` = unknown).
//!
//! ## The identity story (v6, LG.9)
//!
//! `identity_hint` is `<file token>:<kind>:<ordinal>` (`build_identity_hints`).
//! The file token is the file's path when its chain started and is carried
//! across file moves along a `--since` chain: every bin run records the glia
//! graph it exported in [`history_dir`] (`<out>.glia/`, the LC.9 layout), and
//! the next run with `--since <that gmap>` pairs moved files against it (LB.6
//! `detect_moves`), reads the prior tokens back out of the prior gmap's hints
//! ([`prior_tokens`]) and carries them (LB.6 `carry_file_tokens`) into
//! [`ExportOptions::file_identity`]. Without `--since` every token is the path.
//!
//! Keys are unique in an export. When several nodes share a qname (a Python
//! method and the ATTRIBUTE its `self.m` read mints; a Java / PHP / Scala / C#
//! public type and its file MODULE) exactly one is emitted, chosen by
//! `key_rank`: the located node, then a declaration over a MODULE, then the
//! one with a DOC cell, then the lowest `NodeId`. The others are counted in
//! [`ExportStats::duplicate_keys`]; their edges still land on the shared key.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use engram_core::{Content, EdgeKind, Gmap, GmapEdge, GmapNode, SpanRef, content_digest};
use glia_code_domain::{cell_type, edge_category as ec, node_kind};
use glia_core::{Cell, CellPayload, EdgeCategoryId, Node, NodeId, NodeKindId};
use glia_graph::MergedGraph;

pub mod diff;

/// Counts surfaced after an export so callers can flag lossy runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExportStats {
    /// `GmapNode`s emitted.
    pub nodes: usize,
    /// `GmapEdge`s emitted.
    pub edges: usize,
    /// Distinct source files interned (= sidecar entries).
    pub files: usize,
    /// Nodes skipped for want of a qname (can't form a key).
    pub skipped_nodes: usize,
    /// Nodes skipped because another node with the same qname won the key
    /// (the located one first; see the crate docs).
    pub duplicate_keys: usize,
    /// Emitted nodes carrying an `identity_hint` (every located one). (LG.9)
    pub identity_hints: usize,
    /// Edges skipped: endpoint qname unknown, or a degenerate self-edge.
    pub skipped_edges: usize,
    /// Interned POSITION files that could not be read under `repo_root`, so
    /// their nodes' spans carry lines but bytes `0..0`.
    pub unreadable_files: usize,
    /// Emitted nodes (Symbols and Propositions) that have a POSITION cell.
    /// (glia-v6)
    pub positioned: usize,
    /// Emitted nodes whose span carries a line (`start_line > 0`). Equal to
    /// `positioned` by construction; the bin prints both, so a regression in
    /// the row → line conversion shows as a mismatch. (glia-v6)
    pub spans_with_lines: usize,
    /// Doc sections emitted as `Content::Proposition`. (glia-v6)
    pub propositions: usize,
    /// Propositions whose `span` is `Some` — every one with a POSITION.
    /// (glia-v6)
    pub propositions_anchored: usize,
    /// Nodes dropped by the default noise filter (ORIGIN provenance in the
    /// drop set) — suppressed when `include_noise` is set. (glia-v2 G6/G9/G11)
    pub dropped_noise: usize,
    /// Nodes dropped by a caller `--exclude <glob>` pattern. (glia-v2 G15)
    pub dropped_excluded: usize,
    /// [`engram_core::content_digest`] of the gmap bytes [`export_engram_gmap`]
    /// wrote — the content address a `GmapDiff` names as its base or target.
    /// `0` from [`build_gmap`], which serializes nothing. (glia-v6)
    pub digest: u64,
}

/// Knobs for [`build_gmap`] / [`export_engram_gmap`].
#[derive(Debug, Default, Clone)]
pub struct ExportOptions {
    /// Keep substrate-only synthetic nodes (`provenance` in the drop set) that
    /// are filtered by default. Off = clean gmap; on = everything.
    pub include_noise: bool,
    /// Glob patterns (matched against node keys); any match drops the node.
    pub exclude: Vec<String>,
    /// POSITION path -> file token for the `identity_hint`s: the token a
    /// `--since` chain carried for the file (LB.6 `carry_file_tokens` over
    /// [`prior_tokens`]). A path it does not name is its own token, so the
    /// empty map (no `--since`) starts a chain. (LG.9)
    pub file_identity: BTreeMap<String, String>,
}

/// `ORIGIN`-cell provenances dropped from the export by default. Region anchors
/// (`build_output` / `vendored`) are deliberately NOT here — one anchor per
/// collapsed region is the wanted spatial representation, kept so engram sees
/// the map without the per-file flood. We drop the substrate-only pseudo-nodes:
/// dependency (npm hub), synthetic (event names), generated (framework stubs).
const DROP_PROVENANCE: &[&str] = &["dependency", "synthetic", "generated"];

/// Read the `provenance` string from a node's `ORIGIN` cell, if present.
fn origin_provenance(cells: &[Cell]) -> Option<String> {
    for c in cells {
        if c.kind != cell_type::ORIGIN {
            continue;
        }
        let CellPayload::Json(j) = &c.payload else {
            continue;
        };
        let v: serde_json::Value = serde_json::from_str(j).ok()?;
        return v.get("provenance")?.as_str().map(str::to_string);
    }
    None
}

/// Why [`build_gmap`] leaves a node out before it can hold a key.
enum Filtered {
    /// A caller `--exclude` glob matched its key.
    Excluded,
    /// Its ORIGIN provenance is in [`DROP_PROVENANCE`] and noise is not kept.
    Noise,
}

/// The export filters, in order: caller exclude globs first (explicit
/// intent), then the default noise drop.
fn filtered(qname: &str, provenance: Option<&str>, opts: &ExportOptions) -> Option<Filtered> {
    if opts.exclude.iter().any(|p| glob_match(p, qname)) {
        return Some(Filtered::Excluded);
    }
    if !opts.include_noise && provenance.is_some_and(|p| DROP_PROVENANCE.contains(&p)) {
        return Some(Filtered::Noise);
    }
    None
}

/// `(has a POSITION, is not a MODULE, has a DOC cell, lowest NodeId)`.
type KeyRank = (bool, bool, bool, Reverse<u64>);

/// How a node ranks for a key it shares; the maximum is exported (LG.9). The
/// located node first: it alone carries a span and an `identity_hint`, and
/// the hint's kind must not flip between exports (a Python method beats the
/// ATTRIBUTE twin its `self.m` read mints). Then a declaration over its file
/// MODULE (a Java / PHP / Scala / C# public type shares the MODULE's qname
/// since LB.2 / LB.7: the type is the fact worth keeping, the MODULE's edges
/// land on the same key). Then the one with a DOC cell, then the lowest id.
fn key_rank(n: &Node, kind: Option<NodeKindId>) -> KeyRank {
    (
        position_of(&n.cells).is_some(),
        kind != Some(node_kind::MODULE),
        doc_cell(&n.cells).is_some(),
        Reverse(n.id.0),
    )
}

/// Minimal glob match supporting `*` (any run of chars, including none). Used
/// for `--exclude` patterns against node keys; avoids pulling a glob crate.
fn glob_match(pattern: &str, text: &str) -> bool {
    // Split on '*'; each literal segment must appear in order. A leading/
    // trailing empty segment (from a `*` at the edge) anchors loosely.
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text; // no wildcard → exact match
    }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !text[pos..].starts_with(part) {
                return false;
            }
            pos += part.len();
        } else if i == parts.len() - 1 {
            return text[pos..].ends_with(part);
        } else if let Some(idx) = text[pos..].find(part) {
            pos += idx + part.len();
        } else {
            return false;
        }
    }
    true
}

/// Map a glia [`EdgeCategoryId`] onto one of engram's five memory relations.
///
/// engram's kinds are *memory* relations, not code-structure ones, so this is
/// intentionally lossy. `Supersedes`/`Contradicts` are revision relations that
/// only arise from lived experience (engram's Path B) and are never emitted
/// from structure. Owner decisions baked in here:
///   - `DEFINES` / `CONTAINS` → `Cooccurs` (plain association, not narrowing).
///   - `INHERITS_FROM` → `Specializes` (the one true narrowing relation).
///   - Directed "A triggers / depends on B" → `Causes`.
///   - Everything else (imports, uses, docs, tests, shared-*, structural
///     attributes, return types, config/infra refs) → `Cooccurs`.
pub fn edge_kind(cat: EdgeCategoryId) -> EdgeKind {
    match cat {
        // Pure code-structure relations → the v3 code-shaped kinds (G12).
        ec::DEFINES | ec::CONTAINS => EdgeKind::Contains,
        ec::IMPORTS | ec::USES => EdgeKind::Imports,
        ec::CALLS => EdgeKind::Calls,
        // glia has no separate IMPLEMENTS category — INHERITS_FROM covers both
        // `extends` and `implements`, so it maps to Extends. (handoff note)
        ec::INHERITS_FROM => EdgeKind::Extends,
        ec::IMPLEMENTS => EdgeKind::Implements,
        ec::DEPENDS_ON => EdgeKind::DependsOn,
        ec::RETURNS_TYPE => EdgeKind::Returns,
        // Dynamic / cross-process flows: causal but not pure syntactic code
        // edges — kept as Causes per the handoff.
        ec::INJECTS
        | ec::HANDLED_BY
        | ec::HTTP_CALLS
        | ec::GRPC_CALLS
        | ec::QUEUE_FLOWS
        | ec::GRAPHQL_CALLS
        | ec::WS_CONNECTS
        | ec::EVENT_FLOWS
        | ec::CLI_INVOKES
        | ec::ACCESSES_DATA
        | ec::SCHEDULES
        | ec::READS_CONFIG
        | ec::INFRA_REFERENCES => EdgeKind::Causes,
        // SHARES_*, DOCUMENTS, attributes, co-location → plain association.
        _ => EdgeKind::Cooccurs,
    }
}

/// Per-edge conductance hint for engram's spreading activation (G13). Scale
/// from the handoff: definitional edges propagate strongest, weak
/// co-occurrence weakest. `None` = glia doesn't know; engram falls back to 1.0.
pub fn edge_weight(cat: EdgeCategoryId) -> Option<f32> {
    let w = match cat {
        ec::DEFINES | ec::CONTAINS => 1.0, // definitional
        ec::CALLS | ec::INHERITS_FROM | ec::IMPLEMENTS => 0.8, // strong direct reference
        ec::IMPORTS | ec::USES | ec::RETURNS_TYPE | ec::DEPENDS_ON => 0.5, // indirect
        ec::DOCUMENTS
        | ec::SHARES_SCHEMA
        | ec::SHARES_DATA_ENTITY
        | ec::SHARES_CONFIG
        | ec::SHARES_CRON_SCHEDULE
        | ec::SHARES_INFRA_REF
        | ec::SHARES_DEPENDENCY => 0.3, // weak co-occurrence
        _ => return None, // dynamic flows etc — let engram default to 1.0
    };
    Some(w)
}

// ---- concept_hint (G14) — structural-aware feature-key heuristic ----

/// Path segments that introduce a feature/domain: the segment *after* one of
/// these is the natural feature name. Case-insensitive.
const CONTAINER_MARKERS: &[&str] = &[
    "features", "feature", "modules", "pages", "page", "domains", "domain",
    "services", "service", "controllers", "controller", "handlers", "handler",
    "guards", "guard", "components", "component", "directives", "directive",
    "pipes", "pipe", "models", "model", "repositories", "repository", "repos",
    "routes", "views", "view", "resolvers", "resolver",
];

/// `<name>.<type>` filename suffixes (Angular/Nest conventions) stripped to
/// recover the feature stem (`auth.guard` → `auth`, `login.component` → `login`).
const TYPE_SUFFIXES: &[&str] = &[
    ".component", ".service", ".guard", ".module", ".page", ".directive",
    ".pipe", ".controller", ".resolver", ".interceptor", ".model", ".store",
    ".spec", ".test", ".routes", ".config",
];

/// `<name>_<type>` snake suffixes (Go/Rust conventions).
const NAME_SUFFIXES: &[&str] =
    &["_controller", "_service", "_handler", "_repository", "_repo", "_test", "_spec"];

/// The natural feature/module key a node belongs to, for engram's concept
/// routing (G14). `repo::<feature>`, or `None` to let engram fall back to its
/// namespace-depth heuristic. Structural-aware: a known container segment names
/// the feature directly; otherwise the filename stem (minus a type suffix) does.
pub fn concept_hint_for(qname: &str) -> Option<String> {
    let segs: Vec<&str> = qname.split("::").collect();
    if segs.len() < 2 {
        return None;
    }
    let repo = segs[0];

    // 1. Segment immediately after a container marker.
    let mut feature_raw: Option<&str> = None;
    for w in segs.windows(2) {
        if CONTAINER_MARKERS.contains(&w[0].to_ascii_lowercase().as_str()) {
            feature_raw = Some(w[1]);
            break;
        }
    }
    // 2. Fallback: the first filename-like segment (has a `.`).
    let raw = feature_raw.or_else(|| segs.iter().find(|s| s.contains('.')).copied())?;

    let feature = normalize_feature(raw);
    if feature.is_empty() || feature == repo {
        return None;
    }
    Some(format!("{repo}::{feature}"))
}

fn normalize_feature(s: &str) -> String {
    let mut f = s.to_string();
    for suf in TYPE_SUFFIXES {
        if let Some(stripped) = f.strip_suffix(suf) {
            f = stripped.to_string();
        }
    }
    // Any remaining extension-ish tail (`foo.bar` → `foo`).
    if let Some(idx) = f.find('.') {
        f.truncate(idx);
    }
    for suf in NAME_SUFFIXES {
        if let Some(stripped) = f.strip_suffix(suf) {
            f = stripped.to_string();
        }
    }
    f
}

// ---- identity_hint (G4) — name-free structural location ----

/// Build `NodeId → identity_hint` (`<file token>:<kind>:<ordinal>`) for every
/// node that has a POSITION cell. The file token is `file_identity[path]`, or
/// the path itself when the map does not name it (always, without `--since`:
/// the v5 hint). Ordinal = rank among nodes of the same kind in the same file,
/// ordered by (start row, `NodeId`). Name-free, so it survives surface renames
/// AND body edits (engram preserves the FactId + learned salience), and file
/// moves along a `--since` chain (LG.9); it shifts only when same-kind
/// siblings are reordered within a file. (glia-v3 G4)
fn build_identity_hints(
    merged: &MergedGraph,
    file_identity: &BTreeMap<String, String>,
) -> HashMap<NodeId, String> {
    let mut groups: BTreeMap<(&str, u32), Vec<(u32, NodeId)>> = BTreeMap::new();
    let mut positioned: Vec<(String, u32, u32, NodeId)> = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some((file, start_line, _)) = position_of(&n.cells) {
                let kind = g.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
                positioned.push((file, kind, start_line, n.id));
            }
        }
    }
    for (file, kind, start_line, id) in &positioned {
        let token = file_identity.get(file).map_or(file.as_str(), String::as_str);
        groups.entry((token, *kind)).or_default().push((*start_line, *id));
    }
    // Lookup only: the output is read by NodeId, never iterated into bytes.
    let mut out = HashMap::new();
    for ((token, kind), mut v) in groups {
        v.sort_by_key(|(line, id)| (*line, id.0));
        for (ordinal, (_, id)) in v.into_iter().enumerate() {
            out.insert(id, format!("{token}:{kind}:{ordinal}"));
        }
    }
    out
}

/// Every POSITION file path in `merged` — the paths [`build_gmap`] interns,
/// and the current file list LB.6 `carry_file_tokens` assigns tokens to.
pub fn position_paths(merged: &MergedGraph) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some((file, _, _)) = position_of(&n.cells) {
                paths.insert(file);
            }
        }
    }
    paths
}

/// The `path -> file token` map a prior export used, read back out of its
/// hints: for each node with a hint and a span whose file id `prior.files`
/// names, the hint minus its trailing `:<kind>:<ordinal>` (split from the
/// right, so a path or token holding `:` survives). The first node of a path
/// wins; every node of one path carries the same token by construction. A
/// hint not in that shape is skipped.
pub fn prior_tokens(prior: &Gmap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for n in &prior.nodes {
        let Some(hint) = n.identity_hint.as_deref() else {
            continue;
        };
        let file = match &n.content {
            Content::Symbol { span, .. } => span.file,
            Content::Proposition { span: Some(span), .. } => span.file,
            _ => continue,
        };
        let Some(path) = prior.files.get(&file) else {
            continue;
        };
        let mut parts = hint.rsplitn(3, ':');
        let (Some(ordinal), Some(kind), Some(token)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if digits(ordinal) && digits(kind) && !token.is_empty() {
            out.entry(path.clone()).or_insert_with(|| token.to_string());
        }
    }
    out
}

// ---- doc extraction (D1) — leading docs per node ----

const DOC_MAX: usize = 500;

/// An explicit DOC cell, if the parser already extracted one (Python
/// docstrings via `extract_docstring`).
/// The CODE cell's text (used for DOC_SECTION prose → Proposition). (glia-v5 G18)
fn code_cell(cells: &[Cell]) -> Option<String> {
    cells.iter().find_map(|c| match (&c.kind, &c.payload) {
        (k, CellPayload::Text(t)) if *k == cell_type::CODE => Some(t.clone()),
        _ => None,
    })
}

/// External library names from a node's IMPORTS cell (JSON array), if the
/// parser emitted one. (glia-v5 G15)
fn imports_cell(cells: &[Cell]) -> Option<Vec<String>> {
    for c in cells {
        if c.kind != cell_type::IMPORTS {
            continue;
        }
        if let CellPayload::Json(j) = &c.payload {
            return serde_json::from_str::<Vec<String>>(j).ok();
        }
    }
    None
}

fn doc_cell(cells: &[Cell]) -> Option<String> {
    cells.iter().find_map(|c| match (&c.kind, &c.payload) {
        (k, CellPayload::Text(t)) if *k == cell_type::DOC => Some(t.clone()),
        _ => None,
    })
}

/// Skip-list (license headers, TODO-only) + 500-char cap on a char boundary.
/// Applied to DOC-cell content: AST-extracted comment docs arrive pre-cleaned
/// from the shared `glia-doc` helper, but Python docstrings come through
/// the parser's DOC cell uncapped, so this re-bounds them.
fn clean_and_cap_doc(s: String) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let low = s.to_ascii_lowercase();
    if low.contains("copyright")
        || low.contains("spdx-license")
        || low.contains("licensed under")
        || low.contains("all rights reserved")
        || low.contains("permission is hereby granted")
        || low.starts_with("todo")
        || low.starts_with("fixme")
        || low.starts_with("xxx")
        || low.starts_with("hack")
    {
        return None;
    }
    if s.len() <= DOC_MAX {
        return Some(s.to_string());
    }
    let mut end = DOC_MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].trim_end().to_string())
}

/// Recover the POSITION cell as `(repo_relative_file, start_row, end_row)`.
/// Rows are 0-indexed and end-inclusive (tree-sitter `Point::row`), matching
/// what the parsers write. FIRST PARSEABLE POSITION WINS — the A2.8 rule
/// `engine::answers::locate_node` documents: a node can carry several
/// POSITION cells (`merge_parses` appends one per file that minted it), and
/// every glia surface places it by the first. Returns `None` when the node
/// has no parseable POSITION cell, or the winning one lacks a field.
fn position_of(cells: &[Cell]) -> Option<(String, u32, u32)> {
    for c in cells {
        if c.kind != cell_type::POSITION {
            continue;
        }
        let CellPayload::Json(j) = &c.payload else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(j) else {
            continue;
        };
        let file = v.get("file")?.as_str()?.to_string();
        let start = v.get("start_line")?.as_u64()? as u32;
        let end = v.get("end_line")?.as_u64()? as u32;
        return Some((file, start, end));
    }
    None
}

/// One source file read for span conversion: its line-start byte offsets
/// ([`line_starts`]), its byte length and its line count ([`line_count`]).
struct SourceLines {
    starts: Vec<u32>,
    len: u32,
    lines: u32,
}

impl SourceLines {
    fn new(bytes: &[u8]) -> SourceLines {
        let starts = line_starts(bytes);
        let lines = line_count(bytes, &starts);
        SourceLines { starts, len: bytes.len() as u32, lines }
    }
}

/// Byte offset of the first byte of each 0-indexed line. `[0]` is always 0;
/// an entry is pushed after every `\n`. Length = line count + 1 in the common
/// trailing-newline case, which lets `byte_range` index `end_line + 1`.
fn line_starts(bytes: &[u8]) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            starts.push((i + 1) as u32);
        }
    }
    starts
}

/// The number of lines an editor shows for `bytes`, given its
/// [`line_starts`]: a trailing `\n` ends the last line rather than opening an
/// empty one, and an empty file still has line 1.
fn line_count(bytes: &[u8], starts: &[u32]) -> u32 {
    let n = starts.len() as u32;
    if bytes.last() == Some(&b'\n') { n - 1 } else { n }
}

/// 1-based, inclusive `(start_line, end_line)` for a POSITION's
/// `(start_row, end_row)` — the ONLY place the exporter turns rows into lines.
///
/// Source of truth for the rows: `glia_doc::position_json` (every parser's
/// POSITION writer, and the markdown sections since LG.10a) — 0-based
/// tree-sitter rows, end-inclusive. So a line is its row + 1, and an inverted
/// POSITION is read as one line. A node's end row can sit one past the last
/// line (tree-sitter's end point after a file's trailing newline: a module
/// spanning rows `0..=12` of a 12-line file), so when the file was read
/// (`line_count` is `Some`) both ends are clamped to its real line count.
/// Without the file the rows are all there is and pass through unclamped.
/// If POSITION cells ever stored 1-based lines (LD.1 moved only the answer
/// records), this function is what changes, and `tests/spans_v6.rs` fails first.
fn line_span(start_row: u32, end_row: u32, line_count: Option<u32>) -> (u32, u32) {
    let start = start_row.saturating_add(1);
    let end = end_row.max(start_row).saturating_add(1);
    match line_count {
        // max(1): an empty file still has line 1, so a span never reads 0 (unknown).
        Some(n) => {
            let n = n.max(1);
            (start.min(n), end.min(n))
        }
        None => (start, end),
    }
}

/// `(start_byte, end_byte)` covering whole 0-indexed rows `[start_row, end_row]`.
/// `end_byte` is the start of the row *after* `end_row` (or EOF), so the
/// range includes `end_row`'s trailing newline. Clamped so `end >= start`.
fn byte_range(starts: &[u32], file_len: u32, start_row: u32, end_row: u32) -> (u32, u32) {
    let s = starts.get(start_row as usize).copied().unwrap_or(0);
    let e = starts
        .get(end_row as usize + 1)
        .copied()
        .unwrap_or(file_len);
    (s, e.max(s))
}

/// Build an [`engram_core::Gmap`] plus the file-id → path sidecar table from a
/// resolved [`MergedGraph`]. `repo_root` is joined with each POSITION path to
/// read source for byte-range spans; point it at the repo the graph was built
/// from. Pure (no I/O beyond reading source files); see [`export_engram_gmap`]
/// to also write the artifacts.
pub fn build_gmap(
    merged: &MergedGraph,
    repo_root: &Path,
    opts: &ExportOptions,
) -> (Gmap, BTreeMap<u32, String>, ExportStats) {
    let mut stats = ExportStats::default();

    // Pass 1 — intern distinct POSITION file paths to stable, 1-based ids.
    let paths = position_paths(merged);
    let mut file_id: HashMap<String, u32> = HashMap::new();
    let mut id_to_path: BTreeMap<u32, String> = BTreeMap::new();
    for (i, p) in paths.iter().enumerate() {
        let id = (i + 1) as u32; // 0 reserved for "no position"
        file_id.insert(p.clone(), id);
        id_to_path.insert(id, p.clone());
    }
    stats.files = id_to_path.len();

    // Read each source file once for row → byte conversion and the line-count
    // clamp. Lookup only: nothing iterates this map into the output.
    let mut line_cache: HashMap<String, SourceLines> = HashMap::new();
    for p in &paths {
        match std::fs::read(repo_root.join(p)) {
            Ok(bytes) => {
                line_cache.insert(p.clone(), SourceLines::new(&bytes));
            }
            Err(_) => stats.unreadable_files += 1,
        }
    }

    // Name-free stable identity per node (G4), for engram salience preservation.
    let identity = build_identity_hints(merged, &opts.file_identity);

    // A node's qname can be the endpoint of a *cross-repo* edge, so flatten
    // every graph's nav into one lookup before walking edges.
    let mut qname_of: HashMap<NodeId, &str> = HashMap::new();
    for g in &merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname_of.entry(*id).or_insert(q.as_str());
        }
    }

    // Key winners — one node per qname among those the filters keep, by
    // [`key_rank`]; an exact tie (one NodeId listed twice) goes to the first
    // listed. Filters first: a filtered twin must not win and then be dropped,
    // losing the key.
    type Winner = (KeyRank, Reverse<(usize, usize)>);
    let mut winners: BTreeMap<&str, Winner> = BTreeMap::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            if filtered(qname, origin_provenance(&n.cells).as_deref(), opts).is_some() {
                continue;
            }
            let rank = (key_rank(n, g.nav.kind_by_id.get(&n.id).copied()), Reverse((gi, ni)));
            let best = winners.entry(qname.as_str()).or_insert(rank);
            if rank > *best {
                *best = rank;
            }
        }
    }

    // Pass 2 — nodes. Key = full qname; name = short symbol (qname tail as a
    // fallback). One node per key, the winner, so each engram concept-cell
    // fact is unambiguous.
    let mut nodes = Vec::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                stats.skipped_nodes += 1;
                continue;
            };
            // Caller exclude globs win first — explicit intent. Then the
            // default drop of substrate-only synthetic pseudo-nodes (npm deps,
            // event names, generated stubs) unless the caller asked to keep
            // them. Region anchors are NOT in the drop set, so the spatial map
            // survives.
            let provenance = origin_provenance(&n.cells);
            match filtered(qname, provenance.as_deref(), opts) {
                Some(Filtered::Excluded) => {
                    stats.dropped_excluded += 1;
                    continue;
                }
                Some(Filtered::Noise) => {
                    stats.dropped_noise += 1;
                    continue;
                }
                None => {}
            }
            if winners.get(qname.as_str()).map(|w| w.1) != Some(Reverse((gi, ni))) {
                stats.duplicate_keys += 1;
                continue;
            }
            let name = g
                .nav
                .name_by_id
                .get(&n.id)
                .cloned()
                .unwrap_or_else(|| qname.rsplit("::").next().unwrap_or(qname).to_string());
            // One SpanRef per node, for either content kind: bytes from the
            // read file (0..0 when unreadable), lines from the rows (v6).
            let pos = position_of(&n.cells);
            let span = match &pos {
                Some((file, sr, er)) => {
                    let fid = file_id.get(file).copied().unwrap_or(0);
                    let src = line_cache.get(file);
                    let (start, end) = src.map_or((0, 0), |s| byte_range(&s.starts, s.len, *sr, *er));
                    let (start_line, end_line) = line_span(*sr, *er, src.map(|s| s.lines));
                    SpanRef { file: fid, start, end, start_line, end_line }
                }
                None => SpanRef::NONE,
            };
            if pos.is_some() {
                stats.positioned += 1;
            }
            if span.start_line > 0 {
                stats.spans_with_lines += 1;
            }
            // Leading documentation (D1). Every parser emits a DOC cell via the
            // shared AST `leading_doc` walk (Python via its docstring extractor),
            // so this is purely DOC-cell-driven — no source line-scan. Skip
            // test/generated nodes (boilerplate). `clean_and_cap_doc` re-caps
            // Python docstrings, which the python parser doesn't bound.
            // doc (D1) + imports (G15) share the same provenance skip — test /
            // generated nodes carry boilerplate docs + test-only imports that
            // poison the semantic surface.
            let (doc, imports) = if provenance
                .as_deref()
                .is_some_and(|p| matches!(p, "test_fixture" | "generated" | "generated_proto"))
            {
                (None, None)
            } else {
                (
                    doc_cell(&n.cells).and_then(clean_and_cap_doc),
                    imports_cell(&n.cells),
                )
            };
            // G18: doc-section nodes carry prose, not a symbol → Proposition.
            let is_doc = g.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::DOC_SECTION);
            let (content, concept_hint) = if is_doc {
                let prose = code_cell(&n.cells).unwrap_or_else(|| name.clone());
                // concept_hint = `docs::<stem>` (key minus the section slug).
                let ch = qname.rsplit_once("::").map(|(h, _)| h.to_string());
                // v6: anchored whenever the section has a POSITION.
                let anchor = pos.as_ref().map(|_| span);
                stats.propositions += 1;
                if anchor.is_some() {
                    stats.propositions_anchored += 1;
                }
                (Content::Proposition { text: prose, span: anchor }, ch)
            } else {
                (
                    Content::Symbol {
                        name,
                        span,
                        qname: Some(qname.clone()),
                        doc,
                        imports,
                    },
                    concept_hint_for(qname),
                )
            };
            let identity_hint = identity.get(&n.id).cloned();
            stats.identity_hints += usize::from(identity_hint.is_some());
            nodes.push(GmapNode {
                key: qname.clone(),
                content,
                provenance,
                concept_hint,
                identity_hint,
            });
        }
    }
    stats.nodes = nodes.len();

    // Edges — intra-repo + cross-repo. Both endpoints must resolve to a key we
    // actually emitted; self-edges are degenerate as memory relations.
    let mut edges = Vec::new();
    for e in merged.all_edges() {
        let (Some(from), Some(to)) = (qname_of.get(&e.from), qname_of.get(&e.to)) else {
            stats.skipped_edges += 1;
            continue;
        };
        if from == to || !winners.contains_key(*from) || !winners.contains_key(*to) {
            stats.skipped_edges += 1;
            continue;
        }
        edges.push(GmapEdge {
            from: (*from).to_string(),
            kind: edge_kind(e.category),
            to: (*to).to_string(),
            weight: edge_weight(e.category),
        });
    }
    stats.edges = edges.len();

    let gmap = Gmap {
        format_version: engram_core::GMAP_FORMAT_VERSION,
        nodes,
        edges,
        // G16: inline the file-id → path map (same source as the .files.json
        // sidecar) so engram renders `file.go:42` without a second-file load.
        // A BTreeMap since v6, so the map is written in id order and the
        // export bytes are deterministic.
        files: id_to_path.clone(),
    };
    (gmap, id_to_path, stats)
}

/// Build and write the engram seed: `out_path` gets the `bincode`-serialized
/// [`Gmap`], and `<out_path>.files.json` gets the file-id → path sidecar. Both
/// are written tmp-then-rename, matching glia's `.gmap.tmp` convention.
pub fn export_engram_gmap(
    merged: &MergedGraph,
    repo_root: &Path,
    out_path: &Path,
    opts: &ExportOptions,
) -> io::Result<ExportStats> {
    let (gmap, id_to_path, mut stats) = build_gmap(merged, repo_root, opts);

    let bytes = bincode::serialize(&gmap)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    stats.digest = content_digest(&bytes);
    write_atomic(out_path, &bytes)?;

    let sidecar: BTreeMap<String, &String> =
        id_to_path.iter().map(|(k, v)| (k.to_string(), v)).collect();
    let sidecar_json = serde_json::to_vec_pretty(&sidecar)?;
    write_atomic(&sidecar_path(out_path), &sidecar_json)?;

    Ok(stats)
}

/// `<out_path>.files.json` — the span sidecar lives beside the bincode.
pub fn sidecar_path(out_path: &Path) -> std::path::PathBuf {
    let mut s = out_path.as_os_str().to_os_string();
    s.push(".files.json");
    std::path::PathBuf::from(s)
}

/// `<out_path>.glia` — the directory beside a gmap where the bin records the
/// glia graph that gmap was exported from (the LC.9 layout, written by
/// `glia_engine::persist::persist_result`). A later `--since <out_path>` run
/// loads it as the prior graph LB.6 `detect_moves` compares against. Copy it
/// with the gmap; keep `--out` outside the exported repo so the next build
/// never walks it.
pub fn history_dir(out_path: &Path) -> PathBuf {
    let mut s = out_path.as_os_str().to_os_string();
    s.push(".glia");
    PathBuf::from(s)
}

/// Write to `<path>.tmp` then rename over `path` so readers never see a
/// half-written file. `pub(crate)` for [`diff::write_diff`].
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::CodeNav;
    use glia_core::{Confidence, Node, NodeKindId, RepoId};
    use glia_graph::RepoGraph;

    /// Verification #1 from the spec: a hand-built `Gmap` survives a
    /// bincode round-trip byte-for-byte, proving the contract bytes are
    /// well-formed (`Gmap` doesn't derive `PartialEq`, so compare re-serialized
    /// bytes + the `PartialEq` `Content`).
    #[test]
    fn bincode_roundtrip_wellformed() {
        let gmap = Gmap {
            format_version: engram_core::GMAP_FORMAT_VERSION,
            nodes: vec![GmapNode {
                key: "app::User::login".into(),
                content: Content::Symbol {
                    name: "login".into(),
                    span: SpanRef::bytes(1, 12, 20),
                    qname: Some("app::User::login".into()),
                    doc: None,
                    imports: Some(vec!["bcrypt".into()]),
                },
                provenance: None,
                concept_hint: None,
                identity_hint: None,
            }],
            edges: vec![GmapEdge {
                from: "app::User::login".into(),
                kind: EdgeKind::Calls,
                to: "app::db::query".into(),
                weight: Some(0.8),
            }],
            files: BTreeMap::from([(1u32, "src/user.rs".to_string())]),
        };
        let bytes = bincode::serialize(&gmap).unwrap();
        let back: Gmap = bincode::deserialize(&bytes).unwrap();
        assert_eq!(bytes, bincode::serialize(&back).unwrap());
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.files.get(&1).map(String::as_str), Some("src/user.rs"));
        assert_eq!(back.nodes[0].key, "app::User::login");
        assert_eq!(back.nodes[0].content, gmap.nodes[0].content);
        assert_eq!(back.edges[0].kind, EdgeKind::Calls);
        assert_eq!(back.format_version, engram_core::GMAP_FORMAT_VERSION);
    }

    #[test]
    fn edge_kind_mapping_v3_taxonomy() {
        // G12: code-structure relations map to the code-shaped kinds.
        assert_eq!(edge_kind(ec::CALLS), EdgeKind::Calls);
        assert_eq!(edge_kind(ec::DEFINES), EdgeKind::Contains);
        assert_eq!(edge_kind(ec::CONTAINS), EdgeKind::Contains);
        assert_eq!(edge_kind(ec::IMPORTS), EdgeKind::Imports);
        assert_eq!(edge_kind(ec::USES), EdgeKind::Imports);
        assert_eq!(edge_kind(ec::DEPENDS_ON), EdgeKind::DependsOn);
        assert_eq!(edge_kind(ec::RETURNS_TYPE), EdgeKind::Returns);
        // glia has no IMPLEMENTS category — INHERITS_FROM → Extends.
        assert_eq!(edge_kind(ec::INHERITS_FROM), EdgeKind::Extends);
        // dynamic flows stay Causes; co-occurrence stays Cooccurs.
        assert_eq!(edge_kind(ec::HTTP_CALLS), EdgeKind::Causes);
        assert_eq!(edge_kind(ec::TESTS), EdgeKind::Cooccurs);
        // G13 weights track the scale.
        assert_eq!(edge_weight(ec::CONTAINS), Some(1.0));
        assert_eq!(edge_weight(ec::CALLS), Some(0.8));
        assert_eq!(edge_weight(ec::IMPORTS), Some(0.5));
        assert_eq!(edge_weight(ec::HTTP_CALLS), None);
    }

    #[test]
    fn doc_cell_clean_and_cap() {
        // DOC-cell content (e.g. a Python docstring) is re-bounded by the
        // exporter; AST comment docs arrive pre-cleaned from glia-doc.
        let cells = vec![Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text("Sends the verification email.".into()),
        }];
        assert_eq!(doc_cell(&cells).as_deref(), Some("Sends the verification email."));
        assert_eq!(doc_cell(&[]), None);
        // License header / TODO skipped.
        assert_eq!(clean_and_cap_doc("Copyright 2026 Acme. All rights reserved.".into()), None);
        assert_eq!(clean_and_cap_doc("TODO: fix this later".into()), None);
        // Cap at 500 chars (Python docstrings can be long).
        let long = "a".repeat(800);
        assert_eq!(clean_and_cap_doc(long).unwrap().len(), DOC_MAX);
    }

    #[test]
    fn concept_hint_structural_examples() {
        // From the handoff §4a table.
        assert_eq!(
            concept_hint_for("quokka_web::src::app::features::auth::login.component::LoginComponent"),
            Some("quokka_web::auth".into())
        );
        assert_eq!(
            concept_hint_for("quokka_web::src::app::core::guards::auth.guard::authGuard"),
            Some("quokka_web::auth".into())
        );
        assert_eq!(
            concept_hint_for("turps::Services::auth::HashPassword"),
            Some("turps::auth".into())
        );
        assert_eq!(
            concept_hint_for("turps::Server::Controllers::auth_controller::ResetPasswordRequest"),
            Some("turps::auth".into())
        );
    }

    fn pos_cell(file: &str, start: u32, end: u32) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{start},"end_line":{end}}}"#
            )),
        }
    }

    #[test]
    fn build_gmap_recovers_keys_names_and_byte_spans() {
        // Temp repo with one source file: 4 lines, all newline-terminated.
        // line_starts = [0, 6, 12, 16, 20]; lines 2..=3 => bytes [12, 20).
        let root = std::env::temp_dir().join(format!("glia_engram_export_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.py"), b"line0\nline1\nfoo\nbar\n").unwrap();

        let repo = RepoId(1);
        let login = NodeId(10);
        let query = NodeId(20);

        let mut nav = CodeNav::default();
        nav.record(login, "login", "app::User::login", NodeKindId(1), None);
        nav.record(query, "query", "app::db::query", NodeKindId(1), None);

        let g = RepoGraph {
            repo,
            nodes: vec![
                Node {
                    id: login,
                    repo,
                    confidence: Confidence::Strong,
                    cells: vec![pos_cell("a.py", 2, 3)],
                },
                Node {
                    id: query,
                    repo,
                    confidence: Confidence::Strong,
                    cells: vec![],
                },
            ],
            edges: vec![glia_core::Edge {
                from: login,
                to: query,
                category: ec::CALLS,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            nav,
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        };
        let merged = MergedGraph::new(vec![g]);

        let (gmap, sidecar, stats) = build_gmap(&merged, &root, &ExportOptions::default());

        assert_eq!(stats.nodes, 2);
        assert_eq!(stats.edges, 1);
        assert_eq!(stats.files, 1);
        assert_eq!(stats.unreadable_files, 0);

        let login_node = gmap
            .nodes
            .iter()
            .find(|n| n.key == "app::User::login")
            .expect("login node present");
        match &login_node.content {
            Content::Symbol { name, span, qname, .. } => {
                assert_eq!(name, "login"); // short name, not the qname
                assert_eq!(qname.as_deref(), Some("app::User::login")); // full qname carried (G2)
                // rows 2..=3 -> bytes [12, 20) and 1-based lines 3..=4 (v6).
                assert_eq!(
                    *span,
                    SpanRef { file: 1, start: 12, end: 20, start_line: 3, end_line: 4 }
                );
            }
            other => panic!("expected Symbol, got {other:?}"),
        }
        // node with no POSITION cell → placeholder span, file id 0.
        let query_node = gmap.nodes.iter().find(|n| n.key == "app::db::query").unwrap();
        match &query_node.content {
            Content::Symbol { span, .. } => {
                assert_eq!(*span, SpanRef::NONE)
            }
            _ => unreachable!(),
        }

        assert_eq!(gmap.edges.len(), 1);
        assert_eq!(gmap.edges[0].from, "app::User::login");
        assert_eq!(gmap.edges[0].to, "app::db::query");
        assert_eq!(gmap.edges[0].kind, EdgeKind::Calls); // CALLS → Calls (G12)
        assert_eq!(gmap.edges[0].weight, Some(0.8)); // CALLS weight (G13)

        assert_eq!(sidecar.get(&1).map(String::as_str), Some("a.py"));
        assert_eq!((stats.positioned, stats.spans_with_lines), (1, 1));
        assert_eq!((stats.propositions, stats.propositions_anchored), (0, 0));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Rows are 0-based and end-inclusive; lines are 1-based and inclusive,
    /// clamped to the file's line count only when the file was read.
    #[test]
    fn line_span_converts_and_clamps() {
        assert_eq!(line_span(6, 8, Some(12)), (7, 9));
        assert_eq!(line_span(0, 0, Some(12)), (1, 1));
        // A module's end row one past the last line of a 12-line file.
        assert_eq!(line_span(0, 12, Some(12)), (1, 12));
        // An inverted POSITION reads as its start line.
        assert_eq!(line_span(5, 2, Some(12)), (6, 6));
        // A start past EOF (stale POSITION) stays inside the file, not inverted.
        assert_eq!(line_span(20, 22, Some(12)), (12, 12));
        // Unreadable file: rows pass through unclamped.
        assert_eq!(line_span(0, 12, None), (1, 13));
        assert_eq!(line_span(u32::MAX, u32::MAX, None), (u32::MAX, u32::MAX));
        // An empty file still has line 1, so the span is never "unknown".
        assert_eq!(line_span(0, 1, Some(0)), (1, 1));
    }

    #[test]
    fn line_count_follows_editor_lines() {
        for (src, lines) in [
            (&b""[..], 1),
            (b"a", 1),
            (b"a\n", 1),
            (b"a\nb", 2),
            (b"a\nb\n", 2),
            (b"\n\n", 2),
        ] {
            assert_eq!(line_count(src, &line_starts(src)), lines, "{src:?}");
            assert_eq!(SourceLines::new(src).lines, lines, "{src:?}");
        }
    }

    /// FIRST PARSEABLE POSITION WINS (A2.8): a malformed cell is skipped, and
    /// a later POSITION never overrides the first parseable one.
    #[test]
    fn position_of_takes_first_parseable() {
        let bad = Cell { kind: cell_type::POSITION, payload: CellPayload::Json("{".into()) };
        let cells = vec![bad, pos_cell("a.go", 4, 6), pos_cell("b.go", 1, 2)];
        assert_eq!(position_of(&cells), Some(("a.go".to_string(), 4, 6)));
        assert_eq!(position_of(&[]), None);
    }

    #[test]
    fn glob_match_basics() {
        assert!(glob_match("package:npm:*", "package:npm:react"));
        assert!(glob_match("region:*", "region:www"));
        assert!(glob_match("*event*", "x::event_handle:scroll"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
        assert!(!glob_match("package:npm:*", "package:cargo:tokio"));
    }

    fn origin_cell(provenance: &str) -> Cell {
        Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(format!(r#"{{"provenance":"{provenance}"}}"#)),
        }
    }

    /// Default export drops ORIGIN-tagged noise (dependency/synthetic/generated)
    /// but keeps authored nodes AND region anchors; `include_noise` keeps all;
    /// `exclude` globs drop by key. (glia-v2 G6/G9/G11/G15)
    fn three_node_graph() -> MergedGraph {
        let repo = RepoId(1);
        let (authored, dep, region) = (NodeId(1), NodeId(2), NodeId(3));
        let mut nav = CodeNav::default();
        nav.record(authored, "login", "app::login", NodeKindId(3), None);
        nav.record(dep, "react", "package:npm:react", NodeKindId(40), None);
        nav.record(region, "www", "region:www", NodeKindId(41), None);
        let g = RepoGraph {
            repo,
            nodes: vec![
                Node { id: authored, repo, confidence: Confidence::Strong, cells: vec![] },
                Node { id: dep, repo, confidence: Confidence::Strong, cells: vec![origin_cell("dependency")] },
                Node { id: region, repo, confidence: Confidence::Strong, cells: vec![origin_cell("build_output")] },
            ],
            edges: vec![],
            nav,
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        };
        MergedGraph::new(vec![g])
    }

    #[test]
    fn default_drops_noise_keeps_region_and_authored() {
        let merged = three_node_graph();
        let root = std::env::temp_dir();
        let (gmap, _, stats) = build_gmap(&merged, &root, &ExportOptions::default());
        let keys: Vec<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
        assert!(keys.contains(&"app::login"), "authored kept: {keys:?}");
        assert!(keys.contains(&"region:www"), "region anchor kept: {keys:?}");
        assert!(!keys.contains(&"package:npm:react"), "dependency dropped: {keys:?}");
        assert_eq!(stats.dropped_noise, 1);

        // Contract fields populate (G5/G2 + carry-provenance).
        assert_eq!(gmap.format_version, engram_core::GMAP_FORMAT_VERSION);
        let region = gmap.nodes.iter().find(|n| n.key == "region:www").unwrap();
        assert_eq!(region.provenance.as_deref(), Some("build_output"));
        let authored = gmap.nodes.iter().find(|n| n.key == "app::login").unwrap();
        assert_eq!(authored.provenance, None); // no ORIGIN cell → authored
        match &authored.content {
            Content::Symbol { qname, .. } => {
                assert_eq!(qname.as_deref(), Some("app::login"))
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn include_noise_keeps_everything() {
        let merged = three_node_graph();
        let opts = ExportOptions { include_noise: true, exclude: vec![], ..Default::default() };
        let (gmap, _, stats) = build_gmap(&merged, &std::env::temp_dir(), &opts);
        assert_eq!(gmap.nodes.len(), 3);
        assert_eq!(stats.dropped_noise, 0);
    }

    #[test]
    fn exclude_glob_drops_by_key() {
        let merged = three_node_graph();
        let opts = ExportOptions {
            include_noise: true,
            exclude: vec!["region:*".to_string()],
            ..Default::default()
        };
        let (gmap, _, stats) = build_gmap(&merged, &std::env::temp_dir(), &opts);
        let keys: Vec<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
        assert!(!keys.contains(&"region:www"));
        assert!(keys.contains(&"package:npm:react")); // include_noise kept the dep
        assert_eq!(stats.dropped_excluded, 1);
    }

    /// One graph of `(id, qname, kind, POSITION file, start row)` rows.
    fn located_graph(rows: &[(u64, &str, NodeKindId, &str, u32)]) -> MergedGraph {
        let repo = RepoId(1);
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        for (id, qname, kind, file, row) in rows {
            let name = qname.rsplit("::").next().unwrap_or(qname);
            nav.record(NodeId(*id), name, qname, *kind, None);
            nodes.push(Node {
                id: NodeId(*id),
                repo,
                confidence: Confidence::Strong,
                cells: vec![pos_cell(file, *row, *row + 1)],
            });
        }
        MergedGraph::new(vec![RepoGraph {
            repo,
            nodes,
            edges: vec![],
            nav,
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        }])
    }

    fn hint_of(gmap: &Gmap, key: &str) -> String {
        let n = gmap.nodes.iter().find(|n| n.key == key);
        n.and_then(|n| n.identity_hint.clone()).unwrap_or_else(|| panic!("{key}: no hint"))
    }

    /// A path and a carried token holding `:` (and `#`) come back out of the
    /// hints they were written into; a hint in another shape is skipped.
    #[test]
    fn prior_tokens_round_trip_colons() {
        let merged = located_graph(&[
            (1, "c::w::a", node_kind::FUNCTION, "c:/w:x/a.py", 0),
            (2, "c::w::a::g", node_kind::FUNCTION, "c:/w:x/a.py", 3),
            (3, "b", node_kind::MODULE, "b.py", 0),
        ]);
        let opts = ExportOptions {
            file_identity: [("b.py".to_string(), "old:dir/b.py#2".to_string())].into(),
            ..Default::default()
        };
        let (mut gmap, _, stats) = build_gmap(&merged, &std::env::temp_dir(), &opts);
        assert_eq!(stats.identity_hints, 3);
        assert_eq!(hint_of(&gmap, "c::w::a::g"), format!("c:/w:x/a.py:{}:1", node_kind::FUNCTION.0));
        assert_eq!(hint_of(&gmap, "b"), format!("old:dir/b.py#2:{}:0", node_kind::MODULE.0));
        let want: BTreeMap<String, String> = [
            ("b.py".to_string(), "old:dir/b.py#2".to_string()),
            ("c:/w:x/a.py".to_string(), "c:/w:x/a.py".to_string()),
        ]
        .into();
        assert_eq!(prior_tokens(&gmap), want);

        for bad in ["no-colons", "x:y:z", ":1:0"] {
            for n in &mut gmap.nodes {
                n.identity_hint = Some(bad.to_string());
            }
            assert!(prior_tokens(&gmap).is_empty(), "{bad}");
        }
    }

    /// The whole token chain in memory: hints of export 1 -> prior_tokens ->
    /// LB.6 carry_file_tokens over a move -> hints of export 2. A file that
    /// stayed keeps its token, the moved file keeps its first-sight token, and
    /// a new file at the vacated path gets a fresh suffixed one.
    #[test]
    fn token_chain_carries_a_move_and_suffixes_the_vacated_path() {
        use glia_graph::identity::{FileMove, MoveMap, MoveTier, carry_file_tokens};
        let before = located_graph(&[
            (1, "a", node_kind::MODULE, "a.py", 0),
            (2, "a::f", node_kind::FUNCTION, "a.py", 2),
            (3, "b", node_kind::MODULE, "b.py", 0),
        ]);
        let (g1, _, _) = build_gmap(&before, &std::env::temp_dir(), &ExportOptions::default());
        let after = located_graph(&[
            (4, "sub::a", node_kind::MODULE, "sub/a.py", 0),
            (5, "sub::a::f", node_kind::FUNCTION, "sub/a.py", 2),
            (6, "a", node_kind::MODULE, "a.py", 0),
            (3, "b", node_kind::MODULE, "b.py", 0),
        ]);
        let moves = MoveMap {
            files: vec![FileMove {
                old_path: "a.py".into(),
                new_path: "sub/a.py".into(),
                tier: MoveTier::Identical,
            }],
            ..MoveMap::default()
        };
        let current: Vec<String> = position_paths(&after).into_iter().collect();
        let file_identity = carry_file_tokens(&prior_tokens(&g1), &moves, &current);
        let opts = ExportOptions { file_identity, ..Default::default() };
        let (g2, _, _) = build_gmap(&after, &std::env::temp_dir(), &opts);
        assert_eq!(hint_of(&g2, "sub::a"), hint_of(&g1, "a"));
        assert_eq!(hint_of(&g2, "sub::a::f"), hint_of(&g1, "a::f"));
        assert_eq!(hint_of(&g2, "b"), hint_of(&g1, "b"));
        assert_eq!(hint_of(&g2, "a"), format!("a.py#2:{}:0", node_kind::MODULE.0));
        // The next link reads the carried and the suffixed tokens back.
        let t2 = prior_tokens(&g2);
        assert_eq!(t2.get("sub/a.py").map(String::as_str), Some("a.py"));
        assert_eq!(t2.get("a.py").map(String::as_str), Some("a.py#2"));
    }
}
