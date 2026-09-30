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
        PageBody::ConfluenceStorage(_) | PageBody::Wikitext(_) => {
            panic!("a wiki `.md` page is markdown")
        }
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

/// CE.4c — a wiki checkout's `.mediawiki` / `.wiki` pages are read as wikitext
/// and converted to markdown that keeps their headings and code spans.
///
/// Pre-fix (HEAD b0882a8, target/debug/glia): `glia docs sync <repo> --source
/// dir --path <wiki>` over a wiki holding only `Deploy.mediawiki` ->
/// `[docs] sync source=dir ... files=1 pages=0 kept=0 skipped=1` and `error: no
/// Markdown pages under <wiki>`, exit 1: the page was skipped as not Markdown.
#[test]
fn mediawiki_pages_convert() {
    let s = Scratch::new("mediawiki");
    write(
        &s.0.join("Deploy.mediawiki"),
        "== Deploy ==\n<code>deploy_all</code>\n",
    );
    write(&s.0.join("Home.md"), "Welcome.\n");
    let (pages, stats) = read_wiki_dir(&s.0, "acme-wiki", None).expect("read_wiki_dir");

    let titles: Vec<&str> = pages.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["Deploy", "Home"]);
    assert!(
        matches!(&pages[0].body, PageBody::Wikitext(t) if t.starts_with("== Deploy ==")),
        "a .mediawiki page keeps its wikitext until record_from_page"
    );
    assert_eq!((stats.files, stats.pages, stats.wikitext_pages), (2, 2, 1));

    // The converted body opens with the title's own `##` heading, so no H1 is
    // prepended; the code span survives for the linker.
    let (record, redacted) = record_from_page(&pages[0]);
    assert_eq!(record.rel_path, "wiki/acme-wiki/deploy.md");
    assert_eq!(record.text, "## Deploy\n`deploy_all`\n");
    assert_eq!(redacted, 0);

    // The marker read_wiki_dir printed on stderr, counting the wikitext page only.
    assert_eq!(
        stats.wikitext_marker().as_deref(),
        Some(
            "[docs] wikitext pages=1 headings=1 code_blocks=0 inline_code=1 links=0 templates_dropped=0 unbalanced=0"
        )
    );
}

#[test]
fn wiki_extension_pages_sum_their_stats() {
    let s = Scratch::new("wiki-ext");
    write(
        &s.0.join("guides/Runbook.WIKI"),
        "{{Infobox|svc}}\n== Restart ==\n<syntaxhighlight lang=\"bash\">\n# drain first\nsystemctl restart api\n</syntaxhighlight>\nSee [[Deploy|the deploy page]].\n",
    );
    write(
        &s.0.join("Deploy.wiki"),
        "= Deploy =\nRun <tt>deploy_all</tt> {{unclosed\n",
    );
    let (pages, stats) = read_wiki_dir(&s.0, "ops", None).expect("read_wiki_dir");
    assert_eq!(stats.wikitext_pages, 2);
    assert_eq!(
        stats.wikitext_marker().as_deref(),
        Some(
            "[docs] wikitext pages=2 headings=2 code_blocks=1 inline_code=1 links=1 templates_dropped=2 unbalanced=1"
        )
    );
    let records: Vec<DocRecord> = pages.iter().map(|p| record_from_page(p).0).collect();
    // `= Deploy =` is the title's own H1; the runbook's `== Restart ==` is not
    // its title, so the title H1 is prepended. The `#` comment stays inside the
    // fence, where the chunker does not read it as a heading.
    assert_eq!(records[0].text, "# Deploy\nRun `deploy_all`\n");
    assert_eq!(
        records[1].text,
        "# Runbook\n\n## Restart\n```bash\n# drain first\nsystemctl restart api\n```\nSee the deploy page.\n"
    );
    assert_eq!(records[1].rel_path, "wiki/ops/guides-runbook.md");

    // A Markdown-only wiki prints no wikitext marker.
    let md_only = Scratch::new("md-only");
    write(&md_only.0.join("Home.md"), "x\n");
    let (_, stats) = read_wiki_dir(&md_only.0, "w", None).expect("read");
    assert_eq!(stats.wikitext_marker(), None);
}
