//! G18 — `.md` doc ingest (README/ARCHITECTURE/docs/) → prose nodes the
//! exporter maps to `Content::Proposition` with provenance `documentation`,
//! plus the Tier-4 `DocSource` seam external adapters feed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use repo_graph_code_domain::{
    CodeNav, DocProvenance, DocRecord, GRAPH_TYPE, edge_category, node_kind,
};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};

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
/// a top-level `docs/`, or under `.ai/` (≤2 levels). License boilerplate skipped.
fn include_doc(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    if lower.ends_with("license.md") || lower.contains("license") {
        return false;
    }
    if is_wellknown_doc(rel) {
        return true;
    }
    rel.starts_with("docs/") || rel.starts_with(".ai/")
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

struct DocChunk {
    slug: String,
    text: String,
    start_line: u32,
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
        let t = line.trim_start();
        if t.starts_with("# ") || t.starts_with("## ") {
            flush(&mut chunks, &cur_slug, &buf, cur_start, i as u32, &mut seq);
            buf.clear();
            // An emoji/punctuation-only heading slugs to "" — fall through to the
            // ordinal fallback (overview/section-N) so sections don't collide.
            let s = heading_slug(t.trim_start_matches('#').trim());
            cur_slug = if s.is_empty() { None } else { Some(s) };
            cur_start = i as u32;
            buf.push(line);
        } else {
            buf.push(line);
        }
    }
    flush(
        &mut chunks,
        &cur_slug,
        &buf,
        cur_start,
        lines.len() as u32,
        &mut seq,
    );

    // Fallback: no headings → one chunk of the whole file.
    if chunks.is_empty() {
        let body = cap_prose(text);
        if !body.is_empty() {
            chunks.push(DocChunk { slug: "overview".into(), text: body, start_line: 0, end_line: lines.len() as u32 });
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
pub(crate) fn build_docs_graph(records: &[DocRecord], repo: RepoId) -> Option<repo_graph_graph::RepoGraph> {
    use repo_graph_code_domain::{DocSourceKind, cell_type};
    use repo_graph_core::{Cell, CellPayload};

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

        let stem = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("doc")
            .to_string();

        for chunk in chunk_markdown(text) {
            // File qname unchanged; external qnames are namespaced by container.
            let qname = match rec.provenance.container.as_deref() {
                Some(container) => format!("docs::{container}::{stem}::{}", chunk.slug),
                None => format!("docs::{stem}::{}", chunk.slug),
            };
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
                edges.push(Edge {
                    from: sid,
                    to: id,
                    category: edge_category::CONTAINS,
                    confidence: Confidence::Strong,
                });
            }
        }
    }
    if nodes.is_empty() {
        return None;
    }
    Some(repo_graph_graph::RepoGraph {
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
        // Include rules.
        assert!(include_doc("README.md"));
        assert!(include_doc("docs/architecture.md"));
        assert!(!include_doc("LICENSE.md"));
        assert!(!include_doc("src/notes.md")); // not root-wellknown / docs/ / .ai/
    }
}
