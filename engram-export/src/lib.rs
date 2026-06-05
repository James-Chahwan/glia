//! repo-graph-engram-export — write a resolved glia graph as an
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
//! ## The span story
//!
//! engram's `SpanRef { file: u32, start: u32, end: u32 }` is a **byte range**
//! into an interned file. glia stores POSITION cells as
//! `{"file": "<repo-relative path>", "start_line": r, "end_line": r}` — row
//! numbers, no columns, no byte offsets, path as a string. We close the gap at
//! export time (the repo source is present, since we export straight off a
//! freshly generated graph):
//!
//!   1. Distinct file paths are interned to **stable, 1-based** `u32` ids
//!      (sorted order; `0` is reserved for "no/unknown position").
//!   2. Each source file is read once and its line-start byte offsets indexed,
//!      converting `(start_line, end_line)` → a real `(start_byte, end_byte)`
//!      range spanning those whole lines.
//!   3. The `id → path` table is written as a sidecar `*.files.json` next to
//!      the bincode, so the span round-trips back to a path. The sidecar is
//!      glia's own concern — engram reads only the bincode and never computes
//!      on the span; it just hands it back.
//!
//! If a POSITION file can't be read (e.g. exporting against a moved repo) the
//! node keeps its interned file id but gets a `{file, 0, 0}` span; nodes with
//! no POSITION cell get `{0, 0, 0}`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::Path;

use engram_core::{Content, EdgeKind, Gmap, GmapEdge, GmapNode, SpanRef};
use repo_graph_code_domain::{cell_type, edge_category as ec, node_kind};
use repo_graph_core::{Cell, CellPayload, EdgeCategoryId, NodeId};
use repo_graph_graph::MergedGraph;

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
    /// Nodes skipped because their qname duplicated an already-emitted key.
    pub duplicate_keys: usize,
    /// Edges skipped: endpoint qname unknown, or a degenerate self-edge.
    pub skipped_edges: usize,
    /// Interned POSITION files that could not be read under `repo_root`, so
    /// their nodes fell back to a `{file, 0, 0}` span.
    pub unreadable_files: usize,
    /// Nodes dropped by the default noise filter (ORIGIN provenance in the
    /// drop set) — suppressed when `include_noise` is set. (glia-v2 G6/G9/G11)
    pub dropped_noise: usize,
    /// Nodes dropped by a caller `--exclude <glob>` pattern. (glia-v2 G15)
    pub dropped_excluded: usize,
}

/// Knobs for [`build_gmap`] / [`export_engram_gmap`].
#[derive(Debug, Default, Clone)]
pub struct ExportOptions {
    /// Keep substrate-only synthetic nodes (`provenance` in the drop set) that
    /// are filtered by default. Off = clean gmap; on = everything.
    pub include_noise: bool,
    /// Glob patterns (matched against node keys); any match drops the node.
    pub exclude: Vec<String>,
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

/// Build `NodeId → identity_hint` (`<file>:<kind>:<ordinal>`) for every node
/// that has a POSITION cell. Ordinal = rank among nodes of the same kind in the
/// same file, ordered by start line. Name-free, so it survives surface renames
/// AND body edits (engram preserves the FactId + learned salience); it only
/// shifts if same-kind siblings are reordered or the file moves. (glia-v3 G4)
fn build_identity_hints(merged: &MergedGraph) -> HashMap<NodeId, String> {
    let mut groups: HashMap<(String, u32), Vec<(u32, NodeId)>> = HashMap::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some((file, start_line, _)) = position_of(&n.cells) {
                let kind = g.nav.kind_by_id.get(&n.id).map(|k| k.0).unwrap_or(0);
                groups.entry((file, kind)).or_default().push((start_line, n.id));
            }
        }
    }
    let mut out = HashMap::new();
    for ((file, kind), mut v) in groups {
        v.sort_by_key(|(line, id)| (*line, id.0));
        for (ordinal, (_, id)) in v.into_iter().enumerate() {
            out.insert(id, format!("{file}:{kind}:{ordinal}"));
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
/// from the shared `repo-graph-doc` helper, but Python docstrings come through
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
/// Rows are 0-indexed (tree-sitter `Point::row`), matching what the parsers
/// write. Returns `None` when the node has no parseable POSITION cell.
fn position_of(cells: &[Cell]) -> Option<(String, u32, u32)> {
    for c in cells {
        if c.kind != cell_type::POSITION {
            continue;
        }
        let CellPayload::Json(j) = &c.payload else {
            continue;
        };
        let v: serde_json::Value = serde_json::from_str(j).ok()?;
        let file = v.get("file")?.as_str()?.to_string();
        let start = v.get("start_line")?.as_u64()? as u32;
        let end = v.get("end_line")?.as_u64()? as u32;
        return Some((file, start, end));
    }
    None
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

/// `(start_byte, end_byte)` covering whole lines `[start_line, end_line]`.
/// `end_byte` is the start of the line *after* `end_line` (or EOF), so the
/// range includes `end_line`'s trailing newline. Clamped to be non-empty.
fn byte_range(starts: &[u32], file_len: u32, start_line: u32, end_line: u32) -> (u32, u32) {
    let s = starts.get(start_line as usize).copied().unwrap_or(0);
    let e = starts
        .get(end_line as usize + 1)
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
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some((file, _, _)) = position_of(&n.cells) {
                paths.insert(file);
            }
        }
    }
    let mut file_id: HashMap<String, u32> = HashMap::new();
    let mut id_to_path: BTreeMap<u32, String> = BTreeMap::new();
    for (i, p) in paths.iter().enumerate() {
        let id = (i + 1) as u32; // 0 reserved for "no position"
        file_id.insert(p.clone(), id);
        id_to_path.insert(id, p.clone());
    }
    stats.files = id_to_path.len();

    // Read each source file once for line → byte conversion (span ranges).
    let mut line_cache: HashMap<String, (Vec<u32>, u32)> = HashMap::new();
    for p in &paths {
        match std::fs::read(repo_root.join(p)) {
            Ok(bytes) => {
                let len = bytes.len() as u32;
                line_cache.insert(p.clone(), (line_starts(&bytes), len));
            }
            Err(_) => stats.unreadable_files += 1,
        }
    }

    // Name-free stable identity per node (G4), for engram salience preservation.
    let identity = build_identity_hints(merged);

    // A node's qname can be the endpoint of a *cross-repo* edge, so flatten
    // every graph's nav into one lookup before walking edges.
    let mut qname_of: HashMap<NodeId, &str> = HashMap::new();
    for g in &merged.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname_of.entry(*id).or_insert(q.as_str());
        }
    }

    // Pass 2 — nodes. Key = full qname; name = short symbol (qname tail as a
    // fallback). Dedup on key so each engram concept-cell fact is unambiguous.
    let mut nodes = Vec::new();
    let mut seen_keys: BTreeSet<&str> = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                stats.skipped_nodes += 1;
                continue;
            };
            // Caller exclude globs win first — explicit intent.
            if opts.exclude.iter().any(|p| glob_match(p, qname)) {
                stats.dropped_excluded += 1;
                continue;
            }
            // Default-drop substrate-only synthetic pseudo-nodes (npm deps, event
            // names, generated stubs) unless the caller asked to keep them. Region
            // anchors are NOT in the drop set, so the spatial map survives.
            let provenance = origin_provenance(&n.cells);
            if !opts.include_noise
                && provenance
                    .as_deref()
                    .is_some_and(|p| DROP_PROVENANCE.contains(&p))
            {
                stats.dropped_noise += 1;
                continue;
            }
            if !seen_keys.insert(qname.as_str()) {
                stats.duplicate_keys += 1;
                continue;
            }
            let name = g
                .nav
                .name_by_id
                .get(&n.id)
                .cloned()
                .unwrap_or_else(|| qname.rsplit("::").next().unwrap_or(qname).to_string());
            let pos = position_of(&n.cells);
            let span = match &pos {
                Some((file, sl, el)) => {
                    let fid = file_id.get(file).copied().unwrap_or(0);
                    match line_cache.get(file) {
                        Some((starts, len)) => {
                            let (start, end) = byte_range(starts, *len, *sl, *el);
                            SpanRef { file: fid, start, end }
                        }
                        None => SpanRef { file: fid, start: 0, end: 0 },
                    }
                }
                None => SpanRef { file: 0, start: 0, end: 0 },
            };
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
                (Content::Proposition(prose), ch)
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
            nodes.push(GmapNode {
                key: qname.clone(),
                content,
                provenance,
                concept_hint,
                identity_hint: identity.get(&n.id).cloned(),
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
        if from == to || !seen_keys.contains(*from) || !seen_keys.contains(*to) {
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
        files: id_to_path.iter().map(|(k, v)| (*k, v.clone())).collect(),
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
    let (gmap, id_to_path, stats) = build_gmap(merged, repo_root, opts);

    let bytes = bincode::serialize(&gmap)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
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

/// Write to `<path>.tmp` then rename over `path` so readers never see a
/// half-written file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::CodeNav;
    use repo_graph_core::{Confidence, Node, NodeKindId, RepoId};
    use repo_graph_graph::RepoGraph;

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
                    span: SpanRef { file: 1, start: 12, end: 20 },
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
            files: HashMap::from([(1u32, "src/user.rs".to_string())]),
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
        // exporter; AST comment docs arrive pre-cleaned from repo-graph-doc.
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
            edges: vec![repo_graph_core::Edge {
                from: login,
                to: query,
                category: ec::CALLS,
                confidence: Confidence::Strong,
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
                assert_eq!(span.file, 1);
                assert_eq!(span.start, 12);
                assert_eq!(span.end, 20);
            }
            other => panic!("expected Symbol, got {other:?}"),
        }
        // node with no POSITION cell → placeholder span, file id 0.
        let query_node = gmap.nodes.iter().find(|n| n.key == "app::db::query").unwrap();
        match &query_node.content {
            Content::Symbol { span, .. } => {
                assert_eq!(*span, SpanRef { file: 0, start: 0, end: 0 })
            }
            _ => unreachable!(),
        }

        assert_eq!(gmap.edges.len(), 1);
        assert_eq!(gmap.edges[0].from, "app::User::login");
        assert_eq!(gmap.edges[0].to, "app::db::query");
        assert_eq!(gmap.edges[0].kind, EdgeKind::Calls); // CALLS → Calls (G12)
        assert_eq!(gmap.edges[0].weight, Some(0.8)); // CALLS weight (G13)

        assert_eq!(sidecar.get(&1).map(String::as_str), Some("a.py"));

        let _ = std::fs::remove_dir_all(&root);
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
        let opts = ExportOptions { include_noise: true, exclude: vec![] };
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
        };
        let (gmap, _, stats) = build_gmap(&merged, &std::env::temp_dir(), &opts);
        let keys: Vec<&str> = gmap.nodes.iter().map(|n| n.key.as_str()).collect();
        assert!(!keys.contains(&"region:www"));
        assert!(keys.contains(&"package:npm:react")); // include_noise kept the dep
        assert_eq!(stats.dropped_excluded, 1);
    }
}
