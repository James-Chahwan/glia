//! CE.4b — a local GitHub / GitLab wiki checkout as a doc source:
//! `read_wiki_dir` reads its Markdown pages, and not its navigation files,
//! hidden entries or symlinks, into `Page`s the snapshot seam writes.
//!
//! Pre-fix (HEAD b0a91d9, target/debug/glia): `glia docs sync /tmp --source
//! notion` -> `error: unexpected argument '--source' found`, exit 2; there was
//! no way to ingest a wiki that lives beside the repo.

use std::path::{Path, PathBuf};

use glia_code_domain::{DocRecord, DocSourceKind};
use glia_doc_sources::wikidir::{DirStats, MAX_DEPTH, MAX_PAGE_BYTES, read_wiki_dir};
use glia_doc_sources::{PageBody, SnapshotSource, record_from_page, write_snapshot};

/// A scratch dir under the system temp dir, created fresh and removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("glia-ce4b-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, text).expect("write");
}

/// The acceptance wiki: four pages, one navigation file, a `.git` directory
/// and (on unix) a symlink out of the checkout.
fn acme_wiki(root: &Path) {
    write(&root.join("Home.md"), "Welcome.\n");
    write(&root.join("Order-Flow.md"), "Calls `OrderService.place`.\n");
    write(&root.join("guides/Setup.md"), "Run `make setup`.\n");
    write(&root.join("Setup.md"), "Top-level setup.\n");
    write(&root.join("_Sidebar.md"), "* [[Home]]\n* [[Order Flow]]\n");
    write(&root.join(".git/config"), "[core]\n\tbare = false\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hosts", root.join("hosts.md")).expect("symlink");
}

fn body(p: &glia_doc_sources::Page) -> &str {
    match &p.body {
        PageBody::Markdown(md) => md,
        PageBody::ConfluenceStorage(_) => panic!("a wiki page is markdown"),
    }
}

#[test]
fn reads_pages_not_navigation() {
    let s = Scratch::new("pages");
    let wiki = s.0.join("acme.wiki");
    acme_wiki(&wiki);
    let (pages, stats) = read_wiki_dir(&wiki, "acme-wiki", None).expect("read_wiki_dir");

    let titles: Vec<&str> = pages.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["Home", "Order Flow", "Setup", "Setup"]);
    let hints: Vec<&str> = pages
        .iter()
        .filter_map(|p| p.slug_hint.as_deref())
        .collect();
    assert_eq!(hints, ["Home", "Order-Flow", "Setup", "guides/Setup"]);
    assert!(
        pages
            .iter()
            .all(|p| p.kind == DocSourceKind::Wiki && p.container == "acme-wiki")
    );
    assert_eq!(body(&pages[1]), "Calls `OrderService.place`.\n");

    // No base url: the page's canonical file path.
    let canon = std::fs::canonicalize(&wiki).expect("canonical wiki");
    assert_eq!(
        pages[3].url,
        format!("file://{}", canon.join("guides/Setup.md").display())
    );

    let symlinks = usize::from(cfg!(unix));
    assert_eq!(
        stats,
        DirStats {
            files: 6 + symlinks,
            pages: 4,
            skipped_hidden: 1,
            skipped_underscore: 1,
            skipped_symlink: symlinks,
            ..DirStats::default()
        }
    );
    assert_eq!(stats.files, stats.pages + stats.skipped());

    // Two `Setup` pages keep their own manifest stems, and a human H1.
    let records: Vec<DocRecord> = pages.iter().map(|p| record_from_page(p).0).collect();
    let paths: Vec<&str> = records.iter().map(|r| r.rel_path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "wiki/acme-wiki/home.md",
            "wiki/acme-wiki/order-flow.md",
            "wiki/acme-wiki/setup.md",
            "wiki/acme-wiki/guides-setup.md",
        ]
    );
    assert_eq!(records[3].text, "# Setup\n\nRun `make setup`.\n");
    assert_eq!(
        records[1].text,
        "# Order Flow\n\nCalls `OrderService.place`.\n"
    );

    // The snapshot seam takes them as one wiki container.
    let repo = s.0.join("repo");
    let source = SnapshotSource {
        kind: DocSourceKind::Wiki,
        container: "acme-wiki".into(),
    };
    let w = write_snapshot(&repo, &source, &records, 0).expect("write_snapshot");
    assert_eq!((w.written, w.kept_other, w.replaced), (4, 0, 0));
}

#[test]
fn versions_are_content_hashes() {
    let s = Scratch::new("versions");
    acme_wiki(&s.0);
    let (before, _) = read_wiki_dir(&s.0, "acme-wiki", None).expect("first read");
    let (again, _) = read_wiki_dir(&s.0, "acme-wiki", None).expect("second read");
    let versions = |pages: &[glia_doc_sources::Page]| -> Vec<String> {
        pages.iter().map(|p| p.version.clone()).collect()
    };
    assert_eq!(
        versions(&before),
        versions(&again),
        "an unchanged wiki keeps every version"
    );

    write(
        &s.0.join("Order-Flow.md"),
        "Calls `OrderService.place` then `OrderService.ship`.\n",
    );
    let (after, _) = read_wiki_dir(&s.0, "acme-wiki", None).expect("third read");
    let changed: Vec<&str> = before
        .iter()
        .zip(&after)
        .filter(|(b, a)| b.version != a.version)
        .map(|(_, a)| a.title.as_str())
        .collect();
    assert_eq!(
        changed,
        ["Order Flow"],
        "only the edited page's version moves"
    );
    assert!(
        after
            .iter()
            .all(|p| p.version.len() == 16 && p.version.chars().all(|c| c.is_ascii_hexdigit()))
    );
}

#[test]
fn url_base_names_the_page() {
    let s = Scratch::new("url-base");
    write(&s.0.join("Order-Flow.md"), "x\n");
    write(&s.0.join("guides/First Steps.md"), "y\n");
    let (pages, _) =
        read_wiki_dir(&s.0, "acme", Some("https://gitlab.example/o/r/-/wikis/")).expect("read");
    let urls: Vec<&str> = pages.iter().map(|p| p.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://gitlab.example/o/r/-/wikis/Order-Flow",
            "https://gitlab.example/o/r/-/wikis/guides/First-Steps",
        ]
    );
    assert_eq!(pages[1].title, "First Steps");
}

#[test]
fn skips_large_non_utf8_deep_and_other_files() {
    let s = Scratch::new("skips");
    write(&s.0.join("Page.markdown"), "kept\n");
    write(&s.0.join("README.MD"), "kept too\n");
    write(&s.0.join("uploads/logo.png"), "png");
    std::fs::write(s.0.join("Latin1.md"), [b'c', b'a', b'f', 0xE9, b'\n']).expect("write latin1");
    let big = "a".repeat(usize::try_from(MAX_PAGE_BYTES).expect("fits") + 1);
    write(&s.0.join("Big.md"), &big);
    let mut deep = s.0.clone();
    for i in 0..MAX_DEPTH {
        deep.push(format!("d{i}"));
    }
    write(&deep.join("Deepest.md"), "at the depth limit\n");
    write(&deep.join("tooDeep/Lost.md"), "below it\n");

    let (pages, stats) = read_wiki_dir(&s.0, "w", None).expect("read");
    let titles: Vec<&str> = pages.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["Page", "README", "Deepest"]);
    assert_eq!(
        stats,
        DirStats {
            files: 7,
            pages: 3,
            skipped_not_markdown: 1,
            skipped_large: 1,
            skipped_non_utf8: 1,
            skipped_deep: 1,
            ..DirStats::default()
        }
    );
    assert_eq!(
        pages[2].slug_hint.as_deref(),
        Some("d0/d1/d2/d3/d4/d5/d6/d7/Deepest")
    );
}

#[test]
fn a_bad_dir_or_container_is_an_error() {
    let s = Scratch::new("errors");
    assert!(read_wiki_dir(&s.0.join("missing"), "w", None).is_err());
    write(&s.0.join("file.md"), "x\n");
    assert!(
        read_wiki_dir(&s.0.join("file.md"), "w", None).is_err(),
        "a file is not a wiki dir"
    );
    assert!(
        read_wiki_dir(&s.0, "a/b", None).is_err(),
        "a container is one path segment"
    );
    assert_eq!(
        glia_doc_sources::wikidir::default_container(&s.0).expect("default container"),
        s.0.file_name()
            .and_then(|n| n.to_str())
            .expect("UTF-8 name")
    );
}
