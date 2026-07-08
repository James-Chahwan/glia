//! Doc snapshot — build `DocRecord`s from fetched pages and write the manifest
//! the engine's `SnapshotDocSource` ingests (`.glia/docs-snapshot/manifest.jsonl`).
//!
//! This is the format boundary shared by every producer: the `docsync` bin
//! (local storage-format files) and the live `confluence_rest` pull both turn a
//! [`Page`] into a `DocRecord` here, so the on-disk snapshot shape is defined in
//! exactly one place.

use crate::confluence::storage_to_markdown;
use repo_graph_code_domain::{DocProvenance, DocRecord, DocSourceKind};
use std::path::{Path, PathBuf};

/// A page fetched from an external source, pre-conversion. `storage` is the
/// Confluence **storage-format** XHTML body.
pub struct Page {
    pub space: String,
    pub title: String,
    pub url: String,
    pub version: String,
    pub storage: String,
}

/// GitHub-anchor style slug for the manifest path stem.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Convert one fetched page to a `DocRecord`. The title is prepended as an H1
/// (Confluence keeps it out of the body) so the chunker emits a titled
/// top-level DOC_SECTION; the body is `storage_to_markdown`-converted so code
/// spans survive for the doc→code linker.
pub fn record_from_page(p: &Page) -> DocRecord {
    let body_md = storage_to_markdown(&p.storage);
    let text = format!("# {}\n\n{}", p.title.trim(), body_md);
    DocRecord {
        rel_path: format!("confluence/{}/{}.md", p.space, slug(&p.title)),
        text,
        provenance: DocProvenance {
            kind: DocSourceKind::Confluence,
            url: Some(p.url.clone()),
            container: Some(p.space.clone()),
            version: Some(p.version.clone()),
        },
    }
}

/// Write records to `<repo_root>/.glia/docs-snapshot/manifest.jsonl` (one
/// `DocRecord` JSON per line). Returns the manifest path.
pub fn write_snapshot(repo_root: &Path, records: &[DocRecord]) -> Result<PathBuf, String> {
    let dir = repo_root.join(".glia").join("docs-snapshot");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let manifest = dir.join("manifest.jsonl");
    let mut buf = String::new();
    for rec in records {
        buf.push_str(&serde_json::to_string(rec).map_err(|e| format!("serialize: {e}"))?);
        buf.push('\n');
    }
    std::fs::write(&manifest, buf).map_err(|e| format!("write {}: {e}", manifest.display()))?;
    Ok(manifest)
}
