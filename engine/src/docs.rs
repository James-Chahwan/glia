//! G18 — `.md` doc ingest (README/ARCHITECTURE/docs/) → prose nodes the
//! exporter maps to `Content::Proposition` with provenance `documentation`,
//! plus the Tier-4 `DocSource` seam external adapters feed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{
    CodeNav, DocProvenance, DocRecord, GRAPH_TYPE, edge_category, node_kind,
};
use glia_core::{Confidence, Edge, Node, NodeId, RepoId};

/// Repo-root markdown files worth ingesting as documentation.
fn is_wellknown_doc(rel: &str) -> bool {
    matches!(
        rel,
        "README.md"
            | "ARCHITECTURE.md"
            | "CHANGELOG.md"
            | "CONTRIBUTING.md"
            | "CODE_OF_CONDUCT.md"
            | "CLAUDE.md"
            | "AGENTS.md"
            | "CODE_RULES.md"
    )
}

/// Should this markdown path be ingested? Root well-known files, anything under
/// a top-level `docs/`, or under `.ai/` (≤2 levels), and (LF.4b) an ADR
/// directory ([`is_adr_path`]). License boilerplate skipped.
fn include_doc(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    if lower.ends_with("license.md") || lower.contains("license") {
        return false;
    }
    if is_wellknown_doc(rel) {
        return true;
    }
    rel.starts_with("docs/") || rel.starts_with(".ai/") || is_adr_path(rel)
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

/// Cap to 500 chars, truncating at a sentence boundary when possible.
fn cap_prose(s: &str) -> String {
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
    // prefer the last sentence end within the cap
    if let Some(dot) = slice.rfind(". ") {
        return slice[..=dot].trim().to_string();
    }
    slice.trim_end().to_string()
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
}

/// Split markdown at `#`/`##` headings into chunks. Falls back to one chunk
/// (first 500 chars) when the file has no headings.
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

    let flush = |chunks: &mut Vec<DocChunk>,
                 slug: &Option<String>,
                 buf: &[&str],
                 start: u32,
                 end: u32,
                 seq: &mut u32| {
        let body = cap_prose(&buf.join("\n"));
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
        chunks.push(DocChunk { slug, text: body, start_line: start, end_line: end });
    };

    for (i, line) in lines.iter().enumerate() {
        let row = i as u32;
        let t = line.trim_start();
        if t.starts_with("# ") || t.starts_with("## ") {
            let end = last_content.unwrap_or(cur_start);
            flush(&mut chunks, &cur_slug, &buf, cur_start, end, &mut seq);
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
    flush(&mut chunks, &cur_slug, &buf, cur_start, end, &mut seq);

    // Fallback: no headings → one chunk of the whole file, ending on its last
    // non-blank row (0 for an all-blank file, which `cap_prose` already drops).
    if chunks.is_empty() {
        let body = cap_prose(text);
        if !body.is_empty() {
            let end_line = lines.iter().rposition(|l| !l.trim().is_empty()).unwrap_or(0) as u32;
            chunks.push(DocChunk { slug: "overview".into(), text: body, start_line: 0, end_line });
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
pub(crate) fn build_docs_graph(records: &[DocRecord], repo: RepoId) -> Option<glia_graph::RepoGraph> {
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

    for rec in records {
        let (path, text) = (&rec.rel_path, &rec.text);
        let is_file = rec.provenance.kind == DocSourceKind::File;
        // `include_doc` gates repo files; external docs are pre-curated by sync.
        if is_file && !include_doc(path) {
            continue;
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
        let scope = match rec.provenance.container.as_deref() {
            Some(container) => {
                let stem = std::path::Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("doc");
                format!("{container}::{stem}")
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
        assert!(include_doc("README.md"));
        assert!(include_doc("docs/architecture.md"));
        assert!(!include_doc("LICENSE.md"));
        assert!(!include_doc("src/notes.md")); // not root-wellknown / docs/ / .ai/
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
            assert!(include_doc(rel), "{rel} is ingested");
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
        assert!(!include_doc("doc/notes.md"), "the widening is ADR-only");
        assert!(!include_doc("doc/adr/LICENSE.md"), "licence boilerplate stays out");
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
        let Some(g) = build_docs_graph(&records, repo) else {
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
}
