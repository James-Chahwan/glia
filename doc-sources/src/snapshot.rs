//! Doc snapshot — build `DocRecord`s from fetched pages and write the manifest
//! the engine's `SnapshotDocSource` ingests (`.glia/docs-snapshot/manifest.jsonl`).
//!
//! This is the format boundary shared by every producer: the `docsync` bin
//! (local storage-format files) and the live `confluence_rest` pull both turn a
//! [`Page`] into a `DocRecord` here, so the on-disk snapshot shape is defined in
//! exactly one place.
//!
//! CE.4a made it source-neutral: a [`Page`] names its [`DocSourceKind`] and
//! container and carries a [`PageBody`] in its source's format, every record's
//! text goes through the A13.7 redaction ([`redact_untrusted`]) before it is
//! stored, and [`write_snapshot`] merges one source's container into the
//! manifest instead of overwriting every other container's records.

use crate::confluence::storage_to_markdown;
use glia_code_domain::snapshots::redact_untrusted;
use glia_code_domain::{DocProvenance, DocRecord, DocSourceKind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A page's body, in the format its source serves it.
pub enum PageBody {
    /// Confluence **storage-format** XHTML, converted by `storage_to_markdown`.
    ConfluenceStorage(String),
    /// Markdown, taken as-is (a wiki checkout's `.md` page, a converted Notion page).
    Markdown(String),
}

/// A page fetched from an external source, pre-conversion.
pub struct Page {
    /// Which source served it; names the manifest path's first segment.
    pub kind: DocSourceKind,
    /// Confluence space key / Notion database id / wiki name.
    pub container: String,
    pub title: String,
    pub url: String,
    pub version: String,
    pub body: PageBody,
    /// Names the manifest stem instead of the title, for a source whose titles
    /// can repeat inside one container (a wiki directory's `guides/setup.md`
    /// beside `setup.md`). Every other source passes `None` and slugs the title.
    pub slug_hint: Option<String>,
}

/// One external source's container: the unit [`write_snapshot`] replaces.
pub struct SnapshotSource {
    pub kind: DocSourceKind,
    pub container: String,
}

/// What one [`write_snapshot`] did to the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotWrite {
    /// `<repo>/.glia/docs-snapshot/manifest.jsonl`.
    pub path: PathBuf,
    /// Records of this source written (after rel_path dedupe).
    pub written: usize,
    /// Records of every other (kind, container) kept from the old manifest.
    pub kept_other: usize,
    /// Old records dropped for this write: this source's previous records,
    /// and any other record whose rel_path a new record took.
    pub replaced: usize,
    /// Old manifest lines that did not parse as a `DocRecord` (dropped).
    pub dropped_lines: usize,
}

/// The manifest path's first segment, and the engine's `source_tag` for the kind.
pub fn source_tag(kind: DocSourceKind) -> &'static str {
    match kind {
        DocSourceKind::File => "file",
        DocSourceKind::Confluence => "confluence",
        DocSourceKind::Notion => "notion",
        DocSourceKind::Wiki => "wiki",
    }
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

/// True when the markdown's first non-blank line is a `#` / `##` heading (the
/// levels the engine's chunker splits on) whose slug is `title_slug`.
fn opens_with_heading(markdown: &str, title_slug: &str) -> bool {
    if title_slug.is_empty() {
        return false;
    }
    let Some(first) = markdown.lines().map(str::trim_start).find(|l| !l.trim().is_empty()) else {
        return false;
    };
    (first.starts_with("# ") || first.starts_with("## "))
        && slug(first.trim_start_matches('#').trim()) == title_slug
}

/// Convert one fetched page to a `DocRecord`, and count the secret-shaped
/// spans redacted from its text.
///
/// The body becomes markdown (`storage_to_markdown` for Confluence storage, as
/// is for markdown) so code spans survive for the doc→code linker. The title is
/// prepended as an H1 (Confluence keeps it out of the body) so the chunker
/// emits a titled top-level DOC_SECTION, unless the body already opens with a
/// `#` / `##` heading of the same slug: a second one would give two sections
/// one qname. The text then goes through [`redact_untrusted`] — external doc
/// text is untrusted (SECURITY.md) — and the count is returned for the
/// [`write_snapshot`] marker. The manifest path is
/// `<tag>/<container>/<slug of slug_hint or title>.md`.
pub fn record_from_page(p: &Page) -> (DocRecord, usize) {
    let body_md = match &p.body {
        PageBody::ConfluenceStorage(storage) => storage_to_markdown(storage),
        PageBody::Markdown(md) => md.clone(),
    };
    let title = p.title.trim();
    let text = if opens_with_heading(&body_md, &slug(title)) {
        body_md
    } else {
        format!("# {title}\n\n{body_md}")
    };
    let (text, redacted) = redact_untrusted(&text);
    let stem = slug(p.slug_hint.as_deref().unwrap_or(&p.title));
    let record = DocRecord {
        rel_path: format!("{}/{}/{}.md", source_tag(p.kind), p.container, stem),
        text,
        provenance: DocProvenance {
            kind: p.kind,
            url: Some(p.url.clone()),
            container: Some(p.container.clone()),
            version: Some(p.version.clone()),
        },
    };
    (record, redacted)
}

fn is_source(rec: &DocRecord, source: &SnapshotSource) -> bool {
    rec.provenance.kind == source.kind && rec.provenance.container.as_deref() == Some(source.container.as_str())
}

fn sort_key(rec: &DocRecord) -> (&'static str, &str, &str) {
    (
        source_tag(rec.provenance.kind),
        rec.provenance.container.as_deref().unwrap_or(""),
        rec.rel_path.as_str(),
    )
}

/// Merge one source container's `records` into
/// `<repo_root>/.glia/docs-snapshot/manifest.jsonl` (one `DocRecord` JSON per
/// line) and report what changed.
///
/// The old manifest's records of every other (kind, container) are kept; this
/// source's previous records are replaced by `records`; a line that does not
/// parse is dropped and counted. Records are deduped on `rel_path` (a new
/// record wins over an old one, the first of two new ones is kept) and sorted
/// by (source tag, container, rel_path), then written to
/// `manifest.jsonl.<pid>.tmp` and renamed over the manifest, so a reader never
/// sees half a file. Two syncs of different containers running at once can
/// still lose one's records (the last rename wins); doc syncs are manual.
///
/// An empty `records` is refused: it would delete the container's records, and
/// an empty fetch is far likelier a filter or permission problem than a space
/// that emptied. A record of another source is refused too. `redacted` (the
/// sum [`record_from_page`] returned) is reported in the marker.
pub fn write_snapshot(
    repo_root: &Path,
    source: &SnapshotSource,
    records: &[DocRecord],
    redacted: usize,
) -> Result<SnapshotWrite, String> {
    let tag = source_tag(source.kind);
    if records.is_empty() {
        return Err(format!(
            "refusing to write an empty record set for {tag} container {}: it would delete that container's records from the snapshot",
            source.container
        ));
    }
    if let Some(stray) = records.iter().find(|r| !is_source(r, source)) {
        return Err(format!(
            "record {} is not from {tag} container {}",
            stray.rel_path, source.container
        ));
    }
    let dir = repo_root.join(".glia").join("docs-snapshot");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let manifest = dir.join("manifest.jsonl");

    let mut replaced = 0usize;
    let mut dropped_lines = 0usize;
    let mut old_other: Vec<DocRecord> = Vec::new();
    match std::fs::read_to_string(&manifest) {
        Ok(existing) => {
            for line in existing.lines().filter(|l| !l.trim().is_empty()) {
                match serde_json::from_str::<DocRecord>(line) {
                    Ok(rec) if is_source(&rec, source) => replaced += 1,
                    Ok(rec) => old_other.push(rec),
                    Err(_) => dropped_lines += 1,
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read {}: {e}", manifest.display())),
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<&DocRecord> = Vec::with_capacity(records.len() + old_other.len());
    let mut written = 0usize;
    for rec in records {
        if seen.insert(rec.rel_path.as_str()) {
            out.push(rec);
            written += 1;
        }
    }
    let duplicate_new = records.len() - written;
    let mut kept_other = 0usize;
    for rec in &old_other {
        if seen.insert(rec.rel_path.as_str()) {
            out.push(rec);
            kept_other += 1;
        } else {
            replaced += 1;
        }
    }
    out.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));

    let mut buf = String::new();
    for rec in out {
        buf.push_str(&serde_json::to_string(rec).map_err(|e| format!("serialize: {e}"))?);
        buf.push('\n');
    }
    let tmp = dir.join(format!("manifest.jsonl.{}.tmp", std::process::id()));
    std::fs::write(&tmp, buf).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, &manifest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("rename {} -> {}: {e}", tmp.display(), manifest.display()));
    }

    if dropped_lines > 0 {
        eprintln!("[docs] snapshot dropped {dropped_lines} manifest line(s) that did not parse as a DocRecord");
    }
    if duplicate_new > 0 {
        eprintln!(
            "[docs] snapshot {duplicate_new} {tag}/{} record(s) repeat an earlier record's path; kept the first",
            source.container
        );
    }
    eprintln!(
        "[docs] snapshot source={tag} container={} records={written} kept_other={kept_other} replaced={replaced} redacted={redacted} -> .glia/docs-snapshot/manifest.jsonl",
        source.container
    );
    Ok(SnapshotWrite { path: manifest, written, kept_other, replaced, dropped_lines })
}
