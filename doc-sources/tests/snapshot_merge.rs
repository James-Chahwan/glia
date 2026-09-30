//! CE.4a — the source-neutral doc snapshot seam: `record_from_page` over any
//! `DocSourceKind` (redacted, no duplicate title heading) and a `write_snapshot`
//! that merges one (source, container) into the manifest instead of
//! overwriting every other container's records.
//!
//! Pre-fix (HEAD 86cbe89, target/debug/docsync on a scratch repo): syncing
//! space ENG then space OPS left ONE manifest line (confluence/OPS/billing.md),
//! and the OPS page, whose body opens `<h2>Billing</h2>`, became
//! `# Billing\n\n## Billing\n...` - two chunks, one slug.

use std::path::{Path, PathBuf};

use glia_code_domain::snapshots::redact_untrusted;
use glia_code_domain::{DocRecord, DocSourceKind};
use glia_doc_sources::{
    Page, PageBody, SnapshotSource, SnapshotWrite, record_from_page, write_snapshot,
};

/// A scratch repo under the system temp dir, created fresh and removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("glia-ce4a-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(dir)
    }
    fn manifest(&self) -> PathBuf {
        self.0.join(".glia/docs-snapshot/manifest.jsonl")
    }
    fn records(&self) -> Vec<DocRecord> {
        let text = std::fs::read_to_string(self.manifest()).expect("manifest written");
        text.lines()
            .map(|l| serde_json::from_str(l).expect("every manifest line is a DocRecord"))
            .collect()
    }
    fn paths(&self) -> Vec<String> {
        self.records().into_iter().map(|r| r.rel_path).collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn page(kind: DocSourceKind, container: &str, title: &str, body: PageBody) -> Page {
    Page {
        kind,
        container: container.to_string(),
        title: title.to_string(),
        url: format!("https://x/{container}/{title}"),
        version: "1".to_string(),
        body,
        slug_hint: None,
    }
}

fn confluence(space: &str, title: &str, storage: &str) -> Page {
    page(DocSourceKind::Confluence, space, title, PageBody::ConfluenceStorage(storage.to_string()))
}

/// record_from_page over every page, then one write_snapshot for the source.
fn sync(repo: &Path, kind: DocSourceKind, container: &str, pages: &[Page]) -> SnapshotWrite {
    let mut redacted = 0;
    let records: Vec<DocRecord> = pages
        .iter()
        .map(|p| {
            let (r, n) = record_from_page(p);
            redacted += n;
            r
        })
        .collect();
    let source = SnapshotSource { kind, container: container.to_string() };
    write_snapshot(repo, &source, &records, redacted).expect("write_snapshot")
}

#[test]
fn second_container_keeps_the_first() {
    let s = Scratch::new("second");
    let eng = sync(&s.0, DocSourceKind::Confluence, "ENG", &[confluence("ENG", "Orders", "<p>o</p>")]);
    assert_eq!((eng.written, eng.kept_other, eng.replaced, eng.dropped_lines), (1, 0, 0, 0));
    let ops =
        sync(&s.0, DocSourceKind::Confluence, "OPS", &[confluence("OPS", "Billing", "<p>b</p>")]);
    assert_eq!(ops.path, s.manifest());
    assert_eq!((ops.written, ops.kept_other, ops.replaced, ops.dropped_lines), (1, 1, 0, 0));
    assert_eq!(s.paths(), ["confluence/ENG/orders.md", "confluence/OPS/billing.md"]);
    let leftovers: Vec<_> = std::fs::read_dir(s.0.join(".glia/docs-snapshot"))
        .expect("snapshot dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, ["manifest.jsonl"], "the tmp file is renamed away");
}

#[test]
fn same_container_replaces() {
    let s = Scratch::new("replace");
    sync(&s.0, DocSourceKind::Confluence, "ENG", &[confluence("ENG", "Orders", "<p>o</p>")]);
    sync(&s.0, DocSourceKind::Confluence, "OPS", &[confluence("OPS", "Billing", "<p>b</p>")]);
    let again =
        sync(&s.0, DocSourceKind::Confluence, "ENG", &[confluence("ENG", "Orders v2", "<p>o2</p>")]);
    assert_eq!((again.written, again.kept_other, again.replaced), (1, 1, 1));
    assert_eq!(s.paths(), ["confluence/ENG/orders-v2.md", "confluence/OPS/billing.md"]);
}

#[test]
fn other_sources_are_kept() {
    let s = Scratch::new("sources");
    let wiki = page(DocSourceKind::Wiki, "eng", "Setup", PageBody::Markdown("Run `make`.\n".into()));
    sync(&s.0, DocSourceKind::Wiki, "eng", &[wiki]);
    let w = sync(&s.0, DocSourceKind::Confluence, "ENG", &[confluence("ENG", "Setup", "<p>s</p>")]);
    assert_eq!((w.kept_other, w.replaced), (1, 0), "a wiki `eng` is not Confluence space `ENG`");
    let recs = s.records();
    assert_eq!(
        recs.iter().map(|r| r.rel_path.as_str()).collect::<Vec<_>>(),
        ["confluence/ENG/setup.md", "wiki/eng/setup.md"],
        "sorted by source tag, container, path"
    );
    assert_eq!(recs[1].provenance.kind, DocSourceKind::Wiki);
    assert_eq!(recs[1].provenance.container.as_deref(), Some("eng"));
    assert_eq!(recs[1].text, "# Setup\n\nRun `make`.\n", "markdown is taken as is");
}

#[test]
fn title_heading_is_not_duplicated() {
    let (rec, _) = record_from_page(&confluence(
        "OPS",
        "Billing",
        "<h2>Billing</h2><p>see <code>BillingService</code></p>",
    ));
    assert!(rec.text.starts_with("## Billing"), "{:?}", rec.text);
    assert!(!rec.text.contains("# Billing\n\n## Billing"), "{:?}", rec.text);
    assert_eq!(rec.rel_path, "confluence/OPS/billing.md");

    // A body without a heading gets the title as its H1 - exactly the pre-CE.4a text.
    let (rec, _) = record_from_page(&confluence("OPS", "Billing", "<p>see <code>X</code></p>"));
    assert_eq!(rec.text, "# Billing\n\nsee `X`");
    // A body opening with a different heading keeps the title H1 above it.
    let (rec, _) = record_from_page(&confluence("OPS", "Billing", "<h2>Refunds</h2><p>r</p>"));
    assert!(rec.text.starts_with("# Billing\n\n## Refunds"), "{:?}", rec.text);
    // `###` is not a level the chunker splits on, so it does not stand in for the title.
    let (rec, _) = record_from_page(&confluence("OPS", "Billing", "<h3>Billing</h3><p>r</p>"));
    assert!(rec.text.starts_with("# Billing\n\n### Billing"), "{:?}", rec.text);
}

#[test]
fn secrets_are_redacted() {
    let storage = "<p>token: ghp_0123456789abcdefghij0123</p><p>clone https://user:pw@host/x</p>";
    let (rec, n) = record_from_page(&confluence("ENG", "Access", storage));
    assert!(!rec.text.contains("ghp_0123456789abcdefghij0123"), "{:?}", rec.text);
    assert!(!rec.text.contains("user:pw@"), "{:?}", rec.text);
    assert!(rec.text.contains("token: ***"), "{:?}", rec.text);
    assert!(rec.text.contains("https://***@host/x"), "{:?}", rec.text);
    let converted = format!(
        "# Access\n\n{}",
        glia_doc_sources::confluence::storage_to_markdown(storage)
    );
    let (expected, expected_n) = redact_untrusted(&converted);
    assert!(n >= 2, "{n}");
    assert_eq!((rec.text.as_str(), n), (expected.as_str(), expected_n));

    // The count reaches the write's marker; a clean page counts 0.
    let s = Scratch::new("redact");
    let (clean, zero) = record_from_page(&confluence("ENG", "Clean", "<p>nothing here</p>"));
    assert_eq!(zero, 0);
    let source = SnapshotSource { kind: DocSourceKind::Confluence, container: "ENG".into() };
    let w = write_snapshot(&s.0, &source, &[rec, clean], n).expect("write");
    assert_eq!(w.written, 2);
    let stored = std::fs::read_to_string(s.manifest()).expect("manifest");
    assert!(!stored.contains("ghp_0123456789abcdefghij0123") && !stored.contains("user:pw@"));
}

#[test]
fn slug_hint_names_the_stem_and_each_source_its_tag() {
    let mut p = page(DocSourceKind::Wiki, "eng", "Setup", PageBody::Markdown("x\n".into()));
    p.slug_hint = Some("guides/setup".into());
    let (rec, _) = record_from_page(&p);
    assert_eq!(rec.rel_path, "wiki/eng/guides-setup.md");
    assert!(rec.text.starts_with("# Setup\n"), "the title stays human for the H1");
    let n = page(DocSourceKind::Notion, "db1", "Runbook", PageBody::Markdown("# Runbook\nr\n".into()));
    let (rec, _) = record_from_page(&n);
    assert_eq!(rec.rel_path, "notion/db1/runbook.md");
    assert_eq!(rec.text, "# Runbook\nr\n", "a markdown body opening with its title is kept as is");
    assert_eq!(rec.provenance.version.as_deref(), Some("1"));
    assert_eq!(rec.provenance.url.as_deref(), Some("https://x/db1/Runbook"));
}

#[test]
fn unparseable_lines_are_dropped_and_counted() {
    let s = Scratch::new("dropped");
    sync(&s.0, DocSourceKind::Confluence, "ENG", &[confluence("ENG", "Orders", "<p>o</p>")]);
    let mut text = std::fs::read_to_string(s.manifest()).expect("manifest");
    text.push_str("not json\n\n{\"rel_path\":1}\n");
    std::fs::write(s.manifest(), text).expect("rewrite");
    let w = sync(&s.0, DocSourceKind::Confluence, "OPS", &[confluence("OPS", "Billing", "<p>b</p>")]);
    assert_eq!((w.kept_other, w.dropped_lines), (1, 2));
    assert_eq!(s.paths(), ["confluence/ENG/orders.md", "confluence/OPS/billing.md"]);
}

#[test]
fn empty_or_foreign_records_are_refused() {
    let s = Scratch::new("refuse");
    sync(&s.0, DocSourceKind::Confluence, "ENG", &[confluence("ENG", "Orders", "<p>o</p>")]);
    let eng = SnapshotSource { kind: DocSourceKind::Confluence, container: "ENG".into() };
    let err = write_snapshot(&s.0, &eng, &[], 0).expect_err("an empty fetch never deletes a space");
    assert!(err.contains("refusing"), "{err}");
    let (ops_rec, _) = record_from_page(&confluence("OPS", "Billing", "<p>b</p>"));
    let err = write_snapshot(&s.0, &eng, &[ops_rec], 0).expect_err("an OPS record is not ENG's");
    assert!(err.contains("confluence/OPS/billing.md"), "{err}");
    assert_eq!(s.paths(), ["confluence/ENG/orders.md"], "a refused write leaves the manifest");
}

#[test]
fn repeated_paths_keep_one_record() {
    let s = Scratch::new("dupes");
    let w = sync(
        &s.0,
        DocSourceKind::Confluence,
        "ENG",
        &[confluence("ENG", "Orders", "<p>first</p>"), confluence("ENG", "orders", "<p>second</p>")],
    );
    assert_eq!(w.written, 1);
    let recs = s.records();
    assert_eq!(recs.len(), 1);
    assert!(recs[0].text.ends_with("first"), "the first of two new records is kept");
}
