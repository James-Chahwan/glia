//! G18 — `.md` doc ingest (README/ARCHITECTURE/docs/) → prose nodes the
//! exporter maps to `Content::Proposition` with provenance `documentation`,
//! plus the Tier-4 `DocSource` seam external adapters feed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::project_roots::ProjectRoot;
use glia_code_domain::{
    CodeNav, DocProvenance, DocRecord, GRAPH_TYPE, edge_category, node_kind,
};
use glia_core::{Confidence, Edge, Node, NodeId, RepoId};

/// A well-known documentation FILE NAME, ASCII case-folded (CJ.2): any
/// `readme*.md` (`README.md`, `readme.md`, `README.dev.md`, `README.zh-CN.md`)
/// or one of the seven other names. Before CJ.2 the match was the eight exact
/// names, case-sensitive, at the repo root only.
fn is_wellknown_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("readme") && lower.ends_with(".md"))
        || matches!(
            lower.as_str(),
            "architecture.md"
                | "changelog.md"
                | "contributing.md"
                | "code_of_conduct.md"
                | "claude.md"
                | "agents.md"
                | "code_rules.md"
        )
}

/// Which ingestion rule admitted a repo markdown file ([`include_doc`]). The
/// discriminant indexes the per-rule counts the `[docs] scope:` marker prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocRule {
    /// A well-known name ([`is_wellknown_name`]) at the repo root.
    RootWellKnown,
    /// A well-known name directly in a PROJECT root's directory (CJ.2).
    ProjectWellKnown,
    /// Anything under the repo-root `docs/`.
    DocsTree,
    /// Anything under the repo-root `.ai/`.
    AiTree,
    /// An ADR directory ([`is_adr_path`], LF.4b).
    Adr,
    /// Anything under a PROJECT root's own `docs/` (CJ.2).
    ProjectDocsTree,
    /// An SDD feature doc: `features/<feature>/<file>.md` (CJ.2).
    Feature,
    /// A spec-kit feature dir: `specs/<NNN-slug>/**/*.md` (CJ.2).
    SpecKit,
}

/// The number of [`DocRule`] variants: the length of the per-rule counts.
const DOC_RULES: usize = 8;

/// Should this markdown path be ingested, and by which rule? `rel` is
/// repo-relative; `project_dirs` are the non-empty `rel_path`s of the walk's
/// PROJECT roots (manifest-rooted or `[[project]]`-declared), only
/// membership-tested. First match wins:
///
/// 1. licence boilerplate (`license` anywhere in the path) -> never;
/// 2. a well-known name at the repo root -> [`DocRule::RootWellKnown`];
/// 3. a well-known name directly in a PROJECT root -> [`DocRule::ProjectWellKnown`];
/// 4. the repo-root `docs/` and `.ai/` trees, an ADR directory (unchanged);
/// 5. a PROJECT root's own `docs/` tree, any depth below it ->
///    [`DocRule::ProjectDocsTree`] (`.ai/` and the ADR rule stay repo-root only);
/// 6. an SDD doc ([`sdd_rule`]) at the repo root or at a PROJECT root.
///
/// Nothing else: a README inside a source directory, `features/` or `specs/`
/// nested in source, or a `docs/` under a directory that is not a PROJECT
/// root stays out.
fn include_doc(rel: &str, project_dirs: &[&str]) -> Option<DocRule> {
    let rel = rel.replace('\\', "/");
    let lower = rel.to_ascii_lowercase();
    if lower.ends_with("license.md") || lower.contains("license") {
        return None;
    }
    match rel.rsplit_once('/') {
        None if is_wellknown_name(&rel) => return Some(DocRule::RootWellKnown),
        Some((dir, name)) if project_dirs.contains(&dir) && is_wellknown_name(name) => {
            return Some(DocRule::ProjectWellKnown);
        }
        _ => {}
    }
    if rel.starts_with("docs/") {
        return Some(DocRule::DocsTree);
    }
    if rel.starts_with(".ai/") {
        return Some(DocRule::AiTree);
    }
    if is_adr_path(&rel) {
        return Some(DocRule::Adr);
    }
    // `rel` below each PROJECT root that prefixes it at a `/` boundary; nested
    // roots (`app` and `app/android`) both test, and either match admits.
    let below_roots = || {
        project_dirs
            .iter()
            .filter_map(|d| rel.strip_prefix(d).and_then(|r| r.strip_prefix('/')))
    };
    if below_roots().any(|rest| rest.starts_with("docs/")) {
        return Some(DocRule::ProjectDocsTree);
    }
    std::iter::once(rel.as_str()).chain(below_roots()).find_map(sdd_rule)
}

/// CJ.2: an SDD feature doc, `rest` relative to the repo root or a PROJECT
/// root. [`DocRule::Feature`]: exactly `features/<feature>/<file>.md`.
/// [`DocRule::SpecKit`]: `specs/<NNN-slug>/...` with at least one more
/// segment ending `.md` (`spec.md`, `plan.md`, `contracts/api.md`,
/// `checklists/requirements.md`), where the feature dir is >= 3 ASCII digits,
/// a `-`, and at least one more byte (`001-refunds`, `0042-x`).
fn sdd_rule(rest: &str) -> Option<DocRule> {
    if !rest.to_ascii_lowercase().ends_with(".md") {
        return None;
    }
    let segments: Vec<&str> = rest.split('/').collect();
    match segments.as_slice() {
        ["features", feature, file] if !feature.is_empty() && file.len() > ".md".len() => {
            Some(DocRule::Feature)
        }
        ["specs", dir, more @ ..]
            if is_spec_kit_dir(dir) && !more.is_empty() && more.iter().all(|s| !s.is_empty()) =>
        {
            Some(DocRule::SpecKit)
        }
        _ => None,
    }
}

/// A spec-kit feature directory name: >= 3 ASCII digits, `-`, then at least
/// one more byte (`001-refunds`); `01-x` and `drafts` are not.
fn is_spec_kit_dir(dir: &str) -> bool {
    let digits = dir.bytes().take_while(u8::is_ascii_digit).count();
    digits >= 3 && dir.as_bytes().get(digits) == Some(&b'-') && dir.len() > digits + 1
}

/// LF.4b: a markdown file inside an architecture-decision-record directory: a
/// path segment `adr`, `adrs` or `decisions` (case-insensitive) whose parents
/// are only `doc`, `docs` or `architecture` segments, so the ADR directory
/// sits at depth <= 3 - `adr/`, `doc/adr/` (adr-tools' default),
/// `docs/decisions/` (MADR's), `architecture/decisions/`,
/// `doc/architecture/decisions/`. The file may sit anywhere below it.
///
/// The widening is ADR-only: `doc/notes.md` stays out, and so does an `adr/`
/// nested in source (`src/adr/x.md`).
pub(crate) fn is_adr_path(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    if !lower.ends_with(".md") {
        return false;
    }
    let mut segments: Vec<&str> = lower.split('/').collect();
    // The file name is never the ADR directory.
    segments.pop();
    for (depth, seg) in segments.iter().enumerate() {
        if matches!(*seg, "adr" | "adrs" | "decisions") {
            return depth <= 2;
        }
        if !matches!(*seg, "doc" | "docs" | "architecture") {
            return false;
        }
    }
    false
}

/// A slug for a heading line: lowercased, alnum runs joined by `-` (GitHub anchor).
fn heading_slug(heading: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in heading.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// CG.3: the most text one DOC_SECTION's CODE cell keeps. A section is prose
/// the doc linker reads in full (every backticked mention, not only those in
/// its first 500 bytes, which is where the pre-CG.3 `cap_prose` cut); past this
/// bound it is a data dump (a pasted log, a generated table), cut at a line end.
const SECTION_TEXT_CAP: usize = 64 * 1024;

/// Sections whose stored text is longer than this are counted by the
/// `[docs] section text:` marker: the length the pre-CG.3 cap cut at.
const OLD_PROSE_CAP: usize = 500;

/// A section's stored text and whether it was truncated (CG.3): trimmed, and
/// whole up to [`SECTION_TEXT_CAP`] bytes. A longer one is cut at the last char
/// boundary at or below the cap, then back to the last `\n` before that, so
/// the text ends at a line end (a single line longer than the cap keeps the
/// char-boundary cut). A section of <= 500 bytes is exactly what the pre-CG.3
/// `cap_prose` stored (it returned such a section trimmed and whole).
fn bound_section(s: &str) -> (String, bool) {
    let s = s.trim();
    if s.len() <= SECTION_TEXT_CAP {
        return (s.to_string(), false);
    }
    let mut end = SECTION_TEXT_CAP;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let slice = &s[..end];
    let slice = match slice.rfind('\n') {
        Some(nl) => &slice[..nl],
        None => slice,
    };
    (slice.trim_end().to_string(), true)
}

/// One heading-delimited markdown chunk. Rows are `str::lines()` indices, so a
/// CRLF file counts rows exactly like an LF one.
struct DocChunk {
    slug: String,
    text: String,
    /// 0-indexed row the chunk starts on: its heading row, or 0 for the
    /// pre-heading preamble. Identity hints order sections by it.
    start_line: u32,
    /// 0-indexed row of the chunk's last non-blank line, inclusive - the same
    /// convention as every code POSITION cell (`glia-doc::position_json`).
    /// A heading followed directly by another heading ends on its own row; the
    /// blank rows before the next heading (or EOF) are not part of the chunk.
    end_line: u32,
    /// CG.3: `text` was cut at [`SECTION_TEXT_CAP`] ([`bound_section`]).
    truncated: bool,
}

/// GitHub-style anchor dedupe within one document (CE.4a): the first `billing`
/// keeps its slug, the second becomes `billing-1`, the third `billing-2`; a
/// candidate that is itself taken (a literal `billing-1` heading earlier) moves
/// on to the next number, so every slug a document hands out is distinct.
#[derive(Default)]
struct SlugDedupe {
    /// Per base slug, the last `-N` suffix handed out; a key present = taken.
    taken: HashMap<String, u32>,
}

impl SlugDedupe {
    fn unique(&mut self, base: String) -> String {
        let mut slug = base.clone();
        while self.taken.contains_key(&slug) {
            let n = self.taken.entry(base.clone()).or_insert(0);
            *n += 1;
            slug = format!("{base}-{n}");
        }
        self.taken.insert(slug.clone(), 0);
        slug
    }
}

/// The fence marker a line opens or closes: its trimmed start is ```` ``` ````
/// or `~~~` (CommonMark's rule, simplified to the fence char - neither the
/// fence length nor the info string is checked).
fn fence_marker(line: &str) -> Option<&'static str> {
    let t = line.trim_start();
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Split markdown at `#`/`##` headings into chunks. Falls back to one chunk
/// of the whole file when the file has no headings. Each chunk's text is the
/// whole trimmed section, bounded at [`SECTION_TEXT_CAP`] (CG.3; the pre-CG.3
/// cap kept its first 500 bytes).
///
/// CE.4a: a `#` line inside a fenced code block (a shell or Python comment) is
/// text of the enclosing section, not a heading: a fence opens on a line
/// starting ```` ``` ```` / `~~~` and closes on the next line starting with the
/// same marker, and an unclosed fence runs to the end of the document. Slugs
/// are deduped within the document ([`SlugDedupe`]), so a repeated heading
/// gets `-1`, `-2` instead of a second node with the first one's NodeId.
fn chunk_markdown(text: &str) -> Vec<DocChunk> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks: Vec<DocChunk> = Vec::new();
    let mut cur_slug: Option<String> = None;
    let mut cur_start = 0u32;
    let mut buf: Vec<&str> = Vec::new();
    // Row of the last non-blank line pushed into `buf` (the heading row counts);
    // `None` while `buf` holds only blank lines. Reset together with `buf`.
    let mut last_content: Option<u32> = None;
    let mut seq = 0u32;
    let mut slugs = SlugDedupe::default();
    // The marker of the fence the current line sits in, if any.
    let mut fence: Option<&'static str> = None;

    let flush = |chunks: &mut Vec<DocChunk>,
                 slugs: &mut SlugDedupe,
                 slug: &Option<String>,
                 buf: &[&str],
                 start: u32,
                 end: u32,
                 seq: &mut u32| {
        let (body, truncated) = bound_section(&buf.join("\n"));
        if body.is_empty() {
            return;
        }
        let slug = slug.clone().unwrap_or_else(|| {
            let s = if *seq == 0 {
                "overview".to_string()
            } else {
                format!("section-{seq}")
            };
            *seq += 1;
            s
        });
        let slug = slugs.unique(slug);
        chunks.push(DocChunk { slug, text: body, start_line: start, end_line: end, truncated });
    };

    for (i, line) in lines.iter().enumerate() {
        let row = i as u32;
        let t = line.trim_start();
        let in_fence = fence.is_some();
        match fence {
            Some(open) if t.starts_with(open) => fence = None,
            Some(_) => {}
            None => fence = fence_marker(line),
        }
        if !in_fence && (t.starts_with("# ") || t.starts_with("## ")) {
            let end = last_content.unwrap_or(cur_start);
            flush(&mut chunks, &mut slugs, &cur_slug, &buf, cur_start, end, &mut seq);
            buf.clear();
            last_content = None;
            // An emoji/punctuation-only heading slugs to "" — fall through to the
            // ordinal fallback (overview/section-N) so sections don't collide.
            let s = heading_slug(t.trim_start_matches('#').trim());
            cur_slug = if s.is_empty() { None } else { Some(s) };
            cur_start = row;
        }
        buf.push(line);
        if !line.trim().is_empty() {
            last_content = Some(row);
        }
    }
    let end = last_content.unwrap_or(cur_start);
    flush(&mut chunks, &mut slugs, &cur_slug, &buf, cur_start, end, &mut seq);

    // Fallback: no headings → one chunk of the whole file, ending on its last
    // non-blank row (0 for an all-blank file, which `bound_section` already
    // trims to nothing and drops).
    if chunks.is_empty() {
        let (body, truncated) = bound_section(text);
        if !body.is_empty() {
            let end_line = lines.iter().rposition(|l| !l.trim().is_empty()).unwrap_or(0) as u32;
            chunks.push(DocChunk { slug: "overview".into(), text: body, start_line: 0, end_line, truncated });
        }
    }
    chunks
}

// ============================================================================
// Doc ingestion seam (Tier-4)
// ============================================================================
//
// `build_docs_graph` consumes `DocRecord`s from any `DocSource` instead of the
// raw file-walk pairs, so external adapters (Confluence/Notion/wiki, in the
// future `doc-sources` crate) feed the SAME DOC_SECTION builder + downstream
// `link_doc_sections`. `FileDocSource` reproduces the repo `.md` walk exactly.

/// Produces the documents to ingest. The repo file walk is `FileDocSource`;
/// external adapters implement this against a fetched snapshot.
pub(crate) trait DocSource {
    fn collect(self) -> Vec<DocRecord>;
}

/// Today's source: the `(rel_path, text)` markdown pairs the file walk collected.
/// Byte-identical to the pre-seam path — provenance is `File` and no new cell is
/// emitted.
pub(crate) struct FileDocSource(pub(crate) Vec<(String, String)>);

impl DocSource for FileDocSource {
    fn collect(self) -> Vec<DocRecord> {
        self.0
            .into_iter()
            .map(|(rel_path, text)| DocRecord {
                rel_path,
                text,
                provenance: DocProvenance::file(),
            })
            .collect()
    }
}

/// External docs previously fetched by `glia docs sync` into the repo's
/// gitignored `.glia/docs-snapshot/manifest.jsonl` (one `DocRecord` JSON per
/// line). Reading a LOCAL snapshot is deterministic — the non-deterministic
/// network fetch that produced it is a separate step — so the byte-identical
/// build gate stays valid. No snapshot → no external docs (repos without one
/// are byte-identical to before).
pub(crate) struct SnapshotDocSource {
    manifest: PathBuf,
}

impl SnapshotDocSource {
    pub(crate) fn for_repo(root: &Path) -> Self {
        Self {
            manifest: root
                .join(".glia")
                .join("docs-snapshot")
                .join("manifest.jsonl"),
        }
    }
}

impl DocSource for SnapshotDocSource {
    fn collect(self) -> Vec<DocRecord> {
        let Ok(content) = std::fs::read_to_string(&self.manifest) else {
            return Vec::new();
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<DocRecord>(l).ok())
            .collect()
    }
}

/// Build a graph of `DOC_SECTION` nodes from the repo's markdown docs. Each node
/// carries the prose in a CODE cell, a POSITION cell (md path + line range), and
/// an ORIGIN cell `provenance=documentation`. The exporter maps the kind to
/// `Content::Proposition`. (glia-v5 G18)
///
/// `roots` are the walk's PROJECT roots: a repo file is admitted by
/// [`include_doc`] against their directories (CJ.2). fired_on, on stderr, when
/// a PROJECT-root or SDD rule admitted at least one doc:
/// `[docs] scope: project_root=<p> project_docs=<q> feature=<f> spec_kit=<s> (root=<r> docs=<d> ai=<a> adr=<x>)`
pub(crate) fn build_docs_graph(
    records: &[DocRecord],
    repo: RepoId,
    roots: &[ProjectRoot],
) -> Option<glia_graph::RepoGraph> {
    use glia_code_domain::{DocSourceKind, cell_type};
    use glia_core::{Cell, CellPayload};

    fn esc(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    fn source_tag(k: DocSourceKind) -> &'static str {
        match k {
            DocSourceKind::File => "file",
            DocSourceKind::Confluence => "confluence",
            DocSourceKind::Notion => "notion",
            DocSourceKind::Wiki => "wiki",
        }
    }

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    // Dedup DOC_SPACE nodes by qname (a space maps to many pages/records).
    let mut spaces: HashMap<String, NodeId> = HashMap::new();
    // For the `[docs] sections=` marker: DOC_SECTIONs emitted, and the docs
    // that contributed at least one; LB.12 `dir_scoped=`: those among them
    // whose sections carry a directory scope (a repo file below the root).
    let mut sections = 0usize;
    let mut section_docs = 0usize;
    let mut dir_scoped = 0usize;
    // CG.3 `[docs] section text:` marker: sections stored longer than the
    // pre-CG.3 500-byte cap, the longest stored text, and those cut at 64 KiB.
    let (mut long_sections, mut longest, mut truncated_sections) = (0usize, 0usize, 0usize);
    // CJ.2: the PROJECT root directories (the repo root itself is the
    // root rules), and the repo files each ingestion rule admitted.
    let dirs: Vec<&str> = roots
        .iter()
        .map(|r| r.rel_path.as_str())
        .filter(|d| !d.is_empty())
        .collect();
    let mut admitted = [0usize; DOC_RULES];

    for rec in records {
        let (path, text) = (&rec.rel_path, &rec.text);
        let is_file = rec.provenance.kind == DocSourceKind::File;
        // `include_doc` gates repo files; external docs are pre-curated by sync.
        if is_file {
            match include_doc(path, &dirs) {
                None => continue,
                Some(rule) => admitted[rule as usize] += 1,
            }
        }

        // A DOC_SPACE for an external container (Confluence space / Notion db /
        // wiki), emitted once. File docs have no container → stay flat, so their
        // output is byte-identical to before the seam.
        let space_id: Option<NodeId> = rec.provenance.container.as_deref().map(|container| {
            let tag = source_tag(rec.provenance.kind);
            let sqname = format!("docspace::{tag}::{container}");
            if let Some(&sid) = spaces.get(&sqname) {
                return sid;
            }
            let sid = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SPACE, &sqname);
            let origin = format!(
                r#"{{"provenance":"documentation","source":"{tag}","container":"{}"}}"#,
                esc(container)
            );
            nodes.push(Node {
                id: sid,
                repo,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::ORIGIN,
                    payload: CellPayload::Json(origin),
                }],
            });
            nav.record(sid, container, &sqname, node_kind::DOC_SPACE, None);
            spaces.insert(sqname, sid);
            sid
        });

        // LB.12: a repo file's sections are scoped by its directories + stem
        // (`docs/a/guide.md` -> `docs::docs::a::guide::<slug>`), so two
        // directories' `guide.md` keep their own nodes; a root README keeps
        // `docs::README::<slug>`. An external record is scoped by its
        // container (the DOC_SPACE) + stem, as before: its `rel_path`
        // directories (`confluence/<space>/`) only restate the container.
        // CE.4a: a Notion or wiki record's scope leads with its source tag
        // (`docs::notion::<db>::<stem>::<slug>`), so a Notion database and a
        // wiki sharing a container name and page slug keep their own nodes;
        // Confluence's shape is the one users' graphs already hold.
        let scope = match rec.provenance.container.as_deref() {
            Some(container) => {
                let stem = std::path::Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("doc");
                match rec.provenance.kind {
                    DocSourceKind::Notion | DocSourceKind::Wiki => {
                        format!("{}::{container}::{stem}", source_tag(rec.provenance.kind))
                    }
                    DocSourceKind::File | DocSourceKind::Confluence => format!("{container}::{stem}"),
                }
            }
            None => glia_code_domain::dir_stem_qname(path),
        };

        let chunks = chunk_markdown(text);
        if !chunks.is_empty() {
            section_docs += 1;
            sections += chunks.len();
            if is_file && path.contains(['/', '\\']) {
                dir_scoped += 1;
            }
        }
        for chunk in chunks {
            if chunk.text.len() > OLD_PROSE_CAP {
                long_sections += 1;
                longest = longest.max(chunk.text.len());
            }
            truncated_sections += usize::from(chunk.truncated);
            let qname = format!("docs::{scope}::{}", chunk.slug);
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SECTION, &qname);
            let pos = format!(
                r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
                esc(path),
                chunk.start_line,
                chunk.end_line
            );
            // File docs keep today's exact ORIGIN cell (byte-identical); external
            // docs carry source + call-back url so a route traces to its page.
            let origin = if is_file {
                r#"{"provenance":"documentation"}"#.to_string()
            } else {
                let tag = source_tag(rec.provenance.kind);
                format!(
                    r#"{{"provenance":"documentation","source":"{tag}","url":"{}"}}"#,
                    esc(rec.provenance.url.as_deref().unwrap_or(""))
                )
            };
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![
                    Cell { kind: cell_type::CODE, payload: CellPayload::Text(chunk.text) },
                    Cell { kind: cell_type::POSITION, payload: CellPayload::Json(pos) },
                    Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(origin) },
                ],
            });
            nav.record(id, &chunk.slug, &qname, node_kind::DOC_SECTION, space_id);
            if let Some(sid) = space_id {
                // LC.3a: the edge's emitter is the doc source that ingested it.
                let ev = Evidence::emitter(format!("docs:{}", source_tag(rec.provenance.kind)));
                edges.push(
                    Edge::new(sid, id, edge_category::CONTAINS, Confidence::Strong)
                        .with_cell(ev.to_cell()),
                );
            }
        }
    }
    if sections > 0 {
        eprintln!(
            "[docs] sections={sections} from {section_docs} doc(s) (rows 0-indexed, end inclusive) dir_scoped={dir_scoped}"
        );
    }
    // CJ.2 fired_on marker: `... 2>&1 | grep '^\[docs\] scope:'`
    let n = |rule: DocRule| admitted[rule as usize];
    let widened = n(DocRule::ProjectWellKnown)
        + n(DocRule::ProjectDocsTree)
        + n(DocRule::Feature)
        + n(DocRule::SpecKit);
    if widened > 0 {
        eprintln!(
            "[docs] scope: project_root={} project_docs={} feature={} spec_kit={} (root={} docs={} ai={} adr={})",
            n(DocRule::ProjectWellKnown),
            n(DocRule::ProjectDocsTree),
            n(DocRule::Feature),
            n(DocRule::SpecKit),
            n(DocRule::RootWellKnown),
            n(DocRule::DocsTree),
            n(DocRule::AiTree),
            n(DocRule::Adr),
        );
    }
    // CG.3 fired_on marker: `... 2>&1 | grep '^\[docs\] section text:'`
    if long_sections > 0 {
        eprintln!(
            "[docs] section text: {long_sections} sections longer than {OLD_PROSE_CAP} bytes kept whole \
             (longest={longest} bytes, truncated_at_64k={truncated_sections})"
        );
    }
    if nodes.is_empty() {
        return None;
    }
    Some(glia_graph::RepoGraph {
        repo,
        nodes,
        edges,
        nav,
        symbols: Default::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: Default::default(),
    })
}

#[cfg(test)]
mod docs_tests {
    use super::*;

    #[test]
    fn markdown_chunking_and_include_rules() {
        // Heading-split chunking (G18).
        let md = "# Overview\nIntro text.\n## Setup\nRun the thing.\n";
        let chunks = chunk_markdown(md);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].slug, "overview");
        assert_eq!(chunks[1].slug, "setup");
        // Fallback: no headings → one "overview" chunk.
        let plain = chunk_markdown("just a paragraph with no heading at all.");
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].slug, "overview");
        assert_eq!((plain[0].start_line, plain[0].end_line), (0, 0));

        // LG.10a: rows are 0-indexed and end-INCLUSIVE, like every code POSITION
        // cell - a section ends on its last content row, not the next heading.
        let rows = |c: &[DocChunk]| c.iter().map(|c| (c.start_line, c.end_line)).collect::<Vec<_>>();
        let md = "# Overview\nIntro text.\n\n## Setup\nRun the thing.\n";
        assert_eq!(rows(&chunk_markdown(md)), vec![(0, 1), (3, 4)]);
        // A heading followed directly by another heading ends on its own row.
        let md = "# A\n## B\nbody\n";
        assert_eq!(rows(&chunk_markdown(md)), vec![(0, 0), (1, 2)]);
        // Trailing blank rows (and a CRLF file's `\r`) are not part of the last chunk.
        let md = "# Only\r\ntext\r\n\r\n\r\n";
        assert_eq!(rows(&chunk_markdown(md)), vec![(0, 1)]);
        // A preamble before the first heading keeps start row 0 and ends on its
        // last content row; blank rows between it and the heading are dropped.
        let md = "\nlead-in\n\n# Title\nbody\n";
        let c = chunk_markdown(md);
        assert_eq!((c[0].slug.as_str(), c[1].slug.as_str()), ("overview", "title"));
        assert_eq!(rows(&c), vec![(0, 1), (3, 4)]);
        // The docs-md-section-lines fixture: 0-2 / 4-6 / 9-11 (HEAD: 4 / 9 / 12).
        let md = "# Orders\n\nSome intro text.\n\n## Placing orders\n\n\
                  Call `OrderService.place` to place an order.\n\n\n## Refunds\n\n\
                  Refunds go through support.\n";
        assert_eq!(rows(&chunk_markdown(md)), vec![(0, 2), (4, 6), (9, 11)]);
        // Include rules.
        assert!(include_doc("README.md", &[]).is_some());
        assert!(include_doc("docs/architecture.md", &[]).is_some());
        assert!(include_doc("LICENSE.md", &[]).is_none());
        assert!(include_doc("src/notes.md", &[]).is_none()); // not root-wellknown / docs/ / .ai/
    }

    /// The pre-CG.3 section rule, verbatim: 500 bytes, cut back to the last
    /// sentence end inside them. Kept here only to pin that a short section
    /// stores exactly what it stored before.
    fn old_cap_prose(s: &str) -> String {
        const MAX: usize = 500;
        let s = s.trim();
        if s.len() <= MAX {
            return s.to_string();
        }
        let mut end = MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let slice = &s[..end];
        if let Some(dot) = slice.rfind(". ") {
            return slice[..=dot].trim().to_string();
        }
        slice.trim_end().to_string()
    }

    /// CG.3: a DOC_SECTION's text is the whole section (HEAD: its first 500
    /// bytes, cut at a sentence end), so the linker sees the `refunds`
    /// section's `OrderService.refund` at byte 640. The fixture's short
    /// sections are byte-identical to the old cap.
    #[test]
    fn a_long_section_keeps_its_whole_text() {
        let md = include_str!("../../bench/substrate-gap/fixtures/docs-link-past-cap/docs/orders.md");
        let chunks = chunk_markdown(md);
        let slugs: Vec<&str> = chunks.iter().map(|c| c.slug.as_str()).collect();
        assert_eq!(slugs, ["orders", "placing-orders", "refunds"]);
        let refunds = &chunks[2];
        assert!(refunds.text.len() > 500, "{}", refunds.text.len());
        assert!(refunds.text.starts_with("## Refunds\n"), "{:?}", refunds.text);
        assert!(refunds.text.ends_with("can still be traced."), "{:?}", refunds.text);
        assert!(refunds.text.contains("`OrderService.refund`"));
        assert!(!refunds.truncated);
        assert!(
            !old_cap_prose(&refunds.text).contains("`OrderService.refund`"),
            "the mention sits past the old cap"
        );
        let placing = &chunks[1];
        assert_eq!(placing.text, "## Placing orders\n\nCall `OrderService.place` to place an order.");
        for c in &chunks[..2] {
            assert_eq!(c.text, old_cap_prose(&c.text), "{}: a short section is unchanged", c.slug);
            assert!(!c.truncated);
        }
        // The section's rows are unchanged: its POSITION already spanned it.
        assert_eq!((refunds.start_line, refunds.end_line), (8, 14));

        let rec = DocRecord {
            rel_path: "docs/orders.md".to_string(),
            text: md.to_string(),
            provenance: DocProvenance::file(),
        };
        let Some(g) = build_docs_graph(&[rec], RepoId(11), &[]) else {
            panic!("a doc with sections builds a graph");
        };
        let id = NodeId::from_parts(GRAPH_TYPE, RepoId(11), node_kind::DOC_SECTION, "docs::docs::orders::refunds");
        let code = g
            .nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| {
                n.cells.iter().find_map(|c| match &c.payload {
                    glia_core::CellPayload::Text(t) if c.kind == glia_code_domain::cell_type::CODE => {
                        Some(t.as_str())
                    }
                    _ => None,
                })
            })
            .unwrap_or_default();
        assert_eq!(code, refunds.text);
    }

    /// CG.3: only a section longer than 64 KiB is cut, at a line end at or
    /// below the bound; a single over-long line keeps a char-boundary cut.
    #[test]
    fn bound_section_cuts_only_past_64k() {
        let mut md = String::from("# Dump\n\n");
        let mut row = 0usize;
        while md.len() < 70_000 {
            md.push_str(&format!("row {row:05} value {}\n", "x".repeat(20)));
            row += 1;
        }
        let chunks = chunk_markdown(&md);
        assert_eq!(chunks.len(), 1);
        let c = &chunks[0];
        assert!(c.truncated);
        assert!(c.text.len() <= SECTION_TEXT_CAP, "{}", c.text.len());
        assert!(c.text.len() > SECTION_TEXT_CAP - 64, "cut near the bound: {}", c.text.len());
        // It ends where a line of the document ended.
        assert!(md.contains(&format!("{}\n", c.text)), "cut at a line end");
        assert!(c.text.ends_with(&"x".repeat(20)), "{:?}", &c.text[c.text.len() - 40..]);

        // At the bound: whole, not truncated.
        let at = "y".repeat(SECTION_TEXT_CAP);
        assert_eq!(bound_section(&at), (at.clone(), false));
        // One line of 3-byte chars, no `\n`: cut at the char boundary below it.
        let (cut, truncated) = bound_section(&"\u{20ac}".repeat(30_000));
        assert!(truncated);
        assert_eq!(cut.len(), SECTION_TEXT_CAP - SECTION_TEXT_CAP % 3);
        // Short text is trimmed, never cut.
        assert_eq!(bound_section("  a b.  "), ("a b.".to_string(), false));
        assert_eq!(bound_section(" \n "), (String::new(), false));
    }

    /// LF.4b: ADR directories are admitted - adr-tools' `doc/adr`, a root
    /// `adr/`, `architecture/decisions/` - and nothing else of `doc/`.
    #[test]
    fn adr_directories_are_included() {
        for rel in [
            "doc/adr/0001-use-flask-for-orders.md",
            "adr/0001-record-architecture-decisions.md",
            "adrs/0002-x.md",
            "architecture/decisions/0003-y.md",
            "doc/architecture/decisions/0004-z.md",
            "docs/decisions/0005-madr.md",
            "Doc/ADR/0006-Upper.md",
            "doc/adr/archive/0007-old.md",
            "doc\\adr\\0008-windows.md",
        ] {
            assert!(is_adr_path(rel), "{rel} is an ADR path");
            assert!(include_doc(rel, &[]).is_some(), "{rel} is ingested");
        }
        for rel in [
            "doc/notes.md",
            "doc/adr.md",
            "src/adr/0001-x.md",
            "services/api/doc/adr/0001-x.md",
            "doc/docs/architecture/adr/0001-x.md",
            "doc/adr/diagram.png",
            "adr",
        ] {
            assert!(!is_adr_path(rel), "{rel} is not an ADR path");
        }
        assert!(include_doc("doc/notes.md", &[]).is_none(), "the widening is ADR-only");
        assert!(include_doc("doc/adr/LICENSE.md", &[]).is_none(), "licence boilerplate stays out");
    }

    /// CJ.2: the well-known docs and the `docs/` tree at every PROJECT root,
    /// and SDD feature docs at the repo root or a PROJECT root, are admitted;
    /// markdown inside a source tree is not.
    #[test]
    fn project_root_and_sdd_docs_are_included() {
        use DocRule::*;
        let dirs = ["services/orders", "web", "svc"];
        for (rel, want) in [
            ("services/orders/README.md", ProjectWellKnown),
            ("services/orders/readme.md", ProjectWellKnown),
            ("services/orders/README.dev.md", ProjectWellKnown),
            ("services/orders/CHANGELOG.md", ProjectWellKnown),
            ("web/CLAUDE.md", ProjectWellKnown),
            ("features/checkout/backend.md", Feature),
            ("web/features/checkout/ui.md", Feature),
            ("specs/001-refunds/spec.md", SpecKit),
            ("specs/001-refunds/contracts/api.md", SpecKit),
            ("specs/0042-x/checklists/requirements.md", SpecKit),
            ("svc/specs/002-x/plan.md", SpecKit),
            ("services/orders/docs/runbook.md", ProjectDocsTree),
            ("services/orders/docs/ops/deploy.md", ProjectDocsTree),
            ("web/docs/adr/0001-x.md", ProjectDocsTree),
            ("web/docs/features/a/b.md", ProjectDocsTree),
            ("services\\orders\\README.md", ProjectWellKnown),
        ] {
            assert_eq!(include_doc(rel, &dirs), Some(want), "{rel}");
        }
        for rel in [
            "services/docs/overview.md",
            "web/src/docs/notes.md",
            "services/orders/doc/notes.md",
            "services/orders/docs/LICENSE.md",
            "services/orders/internal/README.md",
            "services/README.md",
            "web/src/app/features/cart/README.md",
            "features/README.md",
            "features/a/b/c.md",
            "specs/drafts/ideas.md",
            "specs/01-x/spec.md",
            "specs/001-/spec.md",
            "specs/001-refunds",
            "specs/001-refunds/spec.txt",
            "services/orders/LICENSE.md",
            "src/notes.md",
            "services/ordersx/README.md",
            "services/orders-api/docs/x.md",
        ] {
            assert_eq!(include_doc(rel, &dirs), None, "{rel}");
        }
        // The pre-CJ.2 rules are unchanged.
        for (rel, want) in [
            ("README.md", RootWellKnown),
            ("docs/architecture.md", DocsTree),
            (".ai/notes.md", AiTree),
            ("doc/adr/0001-x.md", Adr),
        ] {
            assert_eq!(include_doc(rel, &dirs), Some(want), "{rel}");
        }
        assert_eq!(include_doc("doc/notes.md", &dirs), None);
        assert_eq!(include_doc("LICENSE.md", &dirs), None);
        // The root match is case-insensitive and admits README*.md variants.
        for rel in ["readme.md", "README.zh-CN.md", "Changelog.md", "agents.md"] {
            assert_eq!(include_doc(rel, &[]), Some(RootWellKnown), "{rel}");
        }
        assert_eq!(include_doc("NOTES.md", &[]), None);
        // Without the PROJECT root, its README and docs/ tree stay out.
        assert_eq!(include_doc("services/orders/README.md", &[]), None);
        assert_eq!(include_doc("services/orders/docs/runbook.md", &[]), None);
        // Nested roots: either one admits.
        let nested = ["app", "app/android"];
        assert_eq!(include_doc("app/android/README.md", &nested), Some(ProjectWellKnown));
        assert_eq!(include_doc("app/android/docs/x.md", &nested), Some(ProjectDocsTree));
        assert_eq!(include_doc("app/features/f/x.md", &nested), Some(Feature));
    }

    /// CJ.2: `build_docs_graph` reads the PROJECT roots it is handed; the
    /// repo-root rules need none.
    #[test]
    fn build_docs_graph_admits_project_root_docs() {
        let file = |rel: &str| DocRecord {
            rel_path: rel.to_string(),
            text: "# Notes\nsee `PlaceOrder`\n".to_string(),
            provenance: DocProvenance::file(),
        };
        let records = [
            file("README.md"),
            file("services/orders/README.md"),
            file("services/orders/internal/README.md"),
            file("features/checkout/backend.md"),
        ];
        let roots = [
            ProjectRoot::new(String::new(), "go", "go.mod", None),
            ProjectRoot::new("services/orders".to_string(), "go", "go.mod", None),
        ];
        let qnames = |roots: &[ProjectRoot]| {
            build_docs_graph(&records, RepoId(4), roots).map(|g| sections(&g).0).unwrap_or_default()
        };
        assert_eq!(
            qnames(&roots),
            ["docs::README::notes", "docs::features::checkout::backend::notes", "docs::services::orders::README::notes"]
        );
        assert_eq!(qnames(&[]), ["docs::README::notes", "docs::features::checkout::backend::notes"]);
    }

    /// LB.12: a repo doc's sections are scoped by its directories + stem, so
    /// `docs/a/guide.md` and `docs/b/guide.md` (HEAD: both `docs::guide::setup`,
    /// one NodeId) are two nodes; a root README and an external page keep the
    /// qname they had.
    #[test]
    fn doc_sections_are_scoped_by_their_directory() {
        use glia_code_domain::{DocProvenance, DocSourceKind};
        let file = |rel: &str| DocRecord {
            rel_path: rel.to_string(),
            text: "# Guide\nintro\n## Setup\nrun it\n".to_string(),
            provenance: DocProvenance::file(),
        };
        let page = DocRecord {
            rel_path: "confluence/ENG/guide.md".to_string(),
            text: "## Setup\nrun it\n".to_string(),
            provenance: DocProvenance {
                kind: DocSourceKind::Confluence,
                url: Some("https://x/wiki/ENG/guide".to_string()),
                container: Some("ENG".to_string()),
                version: None,
            },
        };
        let records = [file("docs/a/guide.md"), file("docs/b/guide.md"), file("README.md"), page];
        let repo = RepoId(7);
        let Some(g) = build_docs_graph(&records, repo, &[]) else {
            panic!("four docs, no graph");
        };
        let mut setup: Vec<&str> = g
            .nav
            .qname_by_id
            .values()
            .map(String::as_str)
            .filter(|q| q.ends_with("::setup"))
            .collect();
        setup.sort();
        assert_eq!(
            setup,
            [
                "docs::ENG::guide::setup",
                "docs::README::setup",
                "docs::docs::a::guide::setup",
                "docs::docs::b::guide::setup",
            ]
        );
        // One node per section: no id is pushed twice.
        let mut ids: Vec<u64> = g
            .nodes
            .iter()
            .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::DOC_SECTION))
            .map(|n| n.id.0)
            .collect();
        let sections = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!((sections, ids.len()), (7, 7), "3 files x 2 sections + 1 page section");
        let a = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SECTION, "docs::docs::a::guide::setup");
        let b = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DOC_SECTION, "docs::docs::b::guide::setup");
        assert_ne!(a, b);
        assert!(g.nav.qname_by_id.contains_key(&a) && g.nav.qname_by_id.contains_key(&b));
    }

    fn slugs(md: &str) -> Vec<String> {
        chunk_markdown(md).into_iter().map(|c| c.slug).collect()
    }

    /// The sorted qname of every DOC_SECTION, and whether any NodeId repeats.
    fn sections(g: &glia_graph::RepoGraph) -> (Vec<String>, bool) {
        let mut q: Vec<String> = g
            .nodes
            .iter()
            .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::DOC_SECTION))
            .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
            .collect();
        q.sort();
        let mut ids: Vec<u64> = g.nodes.iter().map(|n| n.id.0).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        (q, ids.len() != total)
    }

    /// CE.4a: a repeated heading slug gets GitHub's `-N` suffix (HEAD: every
    /// `## B` slugged `b`, one NodeId pushed three times).
    #[test]
    fn repeated_headings_get_distinct_slugs() {
        assert_eq!(slugs("# A\n## B\n## B\n## B"), ["a", "b", "b-1", "b-2"]);
        // A literal `b-1` heading is taken, so the next repeat moves on.
        assert_eq!(slugs("## B\n## B-1\n## B\n"), ["b", "b-1", "b-2"]);
        // The preamble's `overview` fallback and an `Overview` heading are two slugs.
        assert_eq!(slugs("lead-in\n# Overview\nbody\n"), ["overview", "overview-1"]);
        // An empty-slug heading keeps the ordinal fallback.
        assert_eq!(slugs("# A\n## !!\ntext\n## ??\nmore\n"), ["a", "overview", "section-1"]);
    }

    /// CE.4a: `#` lines inside a fenced block are the enclosing section's text
    /// (HEAD: each became a DOC_SECTION of its own).
    #[test]
    fn fenced_comments_are_not_headings() {
        let c = chunk_markdown("# T\n```bash\n# run it\nmake\n```\n## U");
        assert_eq!(c.iter().map(|c| c.slug.as_str()).collect::<Vec<_>>(), ["t", "u"]);
        assert!(c[0].text.contains("# run it"), "{:?}", c[0].text);
        assert_eq!((c[0].start_line, c[0].end_line), (0, 4));
        // `~~~` fences too, and a ``` line does not close a ~~~ fence.
        assert_eq!(slugs("# T\n~~~\n# a\n```\n## b\n~~~\n## U\n"), ["t", "u"]);
        // An unclosed fence runs to the end of the document.
        assert_eq!(slugs("# T\n```py\n# x\n## still code\n"), ["t"]);
        // The skills/glia/SKILL.md shape: four `# xstack-go-http...` comment
        // lines, two identical, inside one fence under one `##` heading.
        let md = "# Skill\n\n## Worked examples\n\n```bash\n\
                  # xstack-go-http; match tiers run exact_qname > subsequence\n\
                  glia find . users --json\n\n\
                  # xstack-go-http\n\
                  glia blast-radius . client::client::FetchUsers --json\n\n\
                  # xstack-go-http; \"key\" is the word trace takes\n\
                  glia flows . --json\n\n\
                  # xstack-go-http\n\
                  glia serves . \"GET /users\" --json\n\
                  ```\n\n## After\ntext\n";
        assert_eq!(slugs(md), ["skill", "worked-examples", "after"]);
        let rec = DocRecord {
            rel_path: "docs/skill.md".to_string(),
            text: md.to_string(),
            provenance: DocProvenance::file(),
        };
        let Some(g) = build_docs_graph(&[rec], RepoId(3), &[]) else {
            panic!("a doc with sections builds a graph");
        };
        let (q, dup) = sections(&g);
        assert!(!dup, "no two nodes share a NodeId: {q:?}");
        assert!(q.iter().all(|q| !q.contains("xstack-go-http")), "{q:?}");
        assert_eq!(
            q,
            ["docs::docs::skill::after", "docs::docs::skill::skill", "docs::docs::skill::worked-examples"]
        );
    }

    /// CE.4a: Notion and wiki sections lead with their source tag; a
    /// Confluence page keeps `docs::<space>::<stem>::<slug>`.
    #[test]
    fn notion_and_wiki_scopes_carry_the_tag() {
        use glia_code_domain::DocSourceKind;
        let ext = |kind: DocSourceKind, tag: &str, container: &str, stem: &str| DocRecord {
            rel_path: format!("{tag}/{container}/{stem}.md"),
            text: "# Orders\nplace them\n".to_string(),
            provenance: DocProvenance {
                kind,
                url: Some(format!("https://x/{tag}/{stem}")),
                container: Some(container.to_string()),
                version: Some("1".to_string()),
            },
        };
        let records = [
            ext(DocSourceKind::Notion, "notion", "db1", "orders"),
            ext(DocSourceKind::Wiki, "wiki", "db1", "orders"),
            ext(DocSourceKind::Confluence, "confluence", "ENG", "orders"),
        ];
        let Some(g) = build_docs_graph(&records, RepoId(5), &[]) else {
            panic!("three pages, no graph");
        };
        let (q, dup) = sections(&g);
        assert!(!dup, "{q:?}");
        assert_eq!(
            q,
            ["docs::ENG::orders::orders", "docs::notion::db1::orders::orders", "docs::wiki::db1::orders::orders"]
        );
        let mut spaces: Vec<&str> = g
            .nav
            .qname_by_id
            .values()
            .map(String::as_str)
            .filter(|q| q.starts_with("docspace::"))
            .collect();
        spaces.sort();
        assert_eq!(spaces, ["docspace::confluence::ENG", "docspace::notion::db1", "docspace::wiki::db1"]);
    }

    /// CE.4a: a snapshot written before the fix (`# Billing` prepended to a
    /// body opening `## Billing`) no longer pushes one NodeId twice - the
    /// probe's `docs::OPS::billing::billing` x2 with two identical CONTAINS
    /// edges. doc-sources now writes the body alone; either way each section
    /// is one node with one CONTAINS edge.
    #[test]
    fn a_title_repeated_in_the_body_is_two_sections_or_one_never_one_id_twice() {
        use glia_code_domain::DocSourceKind;
        let page = |text: &str| DocRecord {
            rel_path: "confluence/OPS/billing.md".to_string(),
            text: text.to_string(),
            provenance: DocProvenance {
                kind: DocSourceKind::Confluence,
                url: Some("https://x/wiki/OPS/2".to_string()),
                container: Some("OPS".to_string()),
                version: Some("1".to_string()),
            },
        };
        for (text, want) in [
            ("# Billing\n\n## Billing\n\nsee `BillingService`", vec!["docs::OPS::billing::billing", "docs::OPS::billing::billing-1"]),
            ("## Billing\n\nsee `BillingService`", vec!["docs::OPS::billing::billing"]),
        ] {
            let Some(g) = build_docs_graph(&[page(text)], RepoId(9), &[]) else {
                panic!("one page, no graph");
            };
            let (q, dup) = sections(&g);
            assert!(!dup, "{text:?}: {q:?}");
            assert_eq!(q, want, "{text:?}");
            let mut contains: Vec<(u64, u64)> = g.edges.iter().map(|e| (e.from.0, e.to.0)).collect();
            let n = contains.len();
            contains.sort_unstable();
            contains.dedup();
            assert_eq!((n, contains.len()), (want.len(), want.len()), "one CONTAINS per section");
        }
    }
}
