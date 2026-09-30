//! CE.4b — `glia docs sync --source dir`: a local GitHub / GitLab wiki
//! checkout, beside the repo, synced into the repo's doc snapshot and linked to
//! the code its pages name. Drives the real binary.
//!
//! Safety as in `docs_stub_cli.rs`: the child runs in a fresh scratch dir (no
//! developer `./.env`), with `CONFLUENCE_*` removed, and the one Confluence
//! sync here is aimed at the loopback stub.
//!
//! The `[docs] sync source=dir ...` stderr line is the CE.4b fired_on marker;
//! it is relayed so
//! `cargo test -p glia-cli --test docs_sources_cli -- --nocapture | grep '\[docs\] sync source=dir'`
//! sees it.
//!
//! Pre-fix (HEAD b0a91d9): `glia docs sync /tmp --source notion` ->
//! `error: unexpected argument '--source' found`, exit 2, and `--space` was
//! clap-required for every sync.
//!
//! CE.4d adds `--source mediawiki` (`mediawiki_sync_then_build`), a MediaWiki
//! Action API played by the same loopback stub; its fired_on marker is the
//! `[docs] sync source=mediawiki ...` line. Pre-fix (HEAD c6726b5):
//! `glia docs sync /tmp --source mediawiki` -> `error: invalid value
//! 'mediawiki' for '--source <SOURCE>'`, exit 2.

// The acceptance wiki holds a symlink out of the checkout.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glia_doc_sources::stub::{Canned, StubServer};

/// A scratch dir under the system temp dir, created fresh and removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("glia-docs-sources-{}-{test}", std::process::id()));
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

fn glia(cwd: &Path, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .current_dir(cwd)
        .env_remove("CONFLUENCE_SITE")
        .env_remove("CONFLUENCE_EMAIL")
        .env_remove("CONFLUENCE_TOKEN")
        .env_remove("MEDIAWIKI_TOKEN")
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[docs] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

/// A repo with `svc/orders.py` and, beside it (not inside it, as a
/// `<repo>.wiki.git` clone sits), the acceptance wiki: four pages, one
/// navigation file, a `.git` directory and a symlink out of the checkout.
fn repo_and_wiki(root: &Path) -> (PathBuf, PathBuf) {
    let repo = root.join("repo");
    write(
        &repo.join("svc/orders.py"),
        "class OrderService:\n    def place(self):\n        ...\n",
    );
    let wiki = root.join("repo.wiki");
    write(&wiki.join("Home.md"), "Welcome.\n");
    write(
        &wiki.join("Order-Flow.md"),
        "The `OrderService` takes orders. Calls `OrderService.place`.\n",
    );
    write(&wiki.join("guides/Setup.md"), "Run `make setup`.\n");
    write(&wiki.join("Setup.md"), "Top-level setup.\n");
    write(&wiki.join("_Sidebar.md"), "* [[Home]]\n");
    write(&wiki.join(".git/config"), "[core]\n");
    std::os::unix::fs::symlink("/etc/hosts", wiki.join("hosts.md")).expect("symlink");
    (repo, wiki)
}

fn manifest_paths(repo: &Path) -> Vec<String> {
    let manifest = std::fs::read_to_string(repo.join(".glia/docs-snapshot/manifest.jsonl"))
        .expect("manifest written");
    manifest
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("a DocRecord line");
            v["rel_path"].as_str().unwrap_or("").to_string()
        })
        .collect()
}

#[test]
fn dir_sync_then_build_links_docs() {
    let scratch = Scratch::new("dir-sync");
    let (repo, wiki) = repo_and_wiki(&scratch.0);
    let (repo_arg, wiki_arg) = (repo.to_str().expect("UTF-8"), wiki.to_str().expect("UTF-8"));

    let out = glia(
        &scratch.0,
        &[
            "docs",
            "sync",
            repo_arg,
            "--source",
            "dir",
            "--path",
            wiki_arg,
            "--container",
            "acme-wiki",
        ],
    );
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(
        out.status.success(),
        "docs sync exited {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status
    );
    assert!(
        stderr.contains(&format!(
            "[docs] sync source=dir path={wiki_arg} container=acme-wiki files=7 pages=4 kept=4 skipped=3"
        )),
        "fired_on marker missing:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "[docs] snapshot source=wiki container=acme-wiki records=4 kept_other=0 replaced=0 redacted=0 -> .glia/docs-snapshot/manifest.jsonl"
        ),
        "stderr:\n{stderr}"
    );
    assert!(
        stdout.contains("synced 4 page(s) from wiki dir"),
        "stdout:\n{stdout}"
    );
    assert_eq!(
        manifest_paths(&repo),
        [
            "wiki/acme-wiki/guides-setup.md",
            "wiki/acme-wiki/home.md",
            "wiki/acme-wiki/order-flow.md",
            "wiki/acme-wiki/setup.md",
        ]
    );

    let out = glia(&scratch.0, &["analyze", repo_arg, "--format", "json"]);
    assert!(
        out.status.success(),
        "analyze exited {:?}\nstderr:\n{}",
        out.status,
        text(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("analyze stdout is JSON");
    let nodes = v["nodes"].as_array().expect("nodes array");
    let edges = v["edges"].as_array().expect("edges array");
    let node = |kind: &str, qname: &str| {
        nodes
            .iter()
            .find(|n| n["kind_name"] == kind && n["qname"] == qname)
            .unwrap_or_else(|| {
                panic!(
                    "no {kind} {qname}; qnames: {:?}",
                    nodes.iter().map(|n| &n["qname"]).collect::<Vec<_>>()
                )
            })
    };
    let space = node("DOC_SPACE", "docspace::wiki::acme-wiki");
    let section = node(
        "DOC_SECTION",
        "docs::wiki::acme-wiki::order-flow::order-flow",
    );
    // The two `Setup` pages stay two sections.
    node("DOC_SECTION", "docs::wiki::acme-wiki::setup::setup");
    node("DOC_SECTION", "docs::wiki::acme-wiki::guides-setup::setup");
    assert!(
        edges.iter().any(|e| e["category"] == "CONTAINS"
            && e["from"] == space["id"]
            && e["to"] == section["id"]),
        "the wiki space contains the Order Flow section"
    );
    let documents = |qname: &str| {
        let target = node(
            if qname.ends_with("place") {
                "METHOD"
            } else {
                "CLASS"
            },
            qname,
        );
        edges.iter().any(|e| {
            e["category"] == "DOCUMENTS" && e["from"] == section["id"] && e["to"] == target["id"]
        })
    };
    assert!(
        documents("svc::orders::OrderService"),
        "no Order Flow -DOCUMENTS-> OrderService"
    );
    // `OrderService.place` is a qualified mention: it binds the method.
    assert!(
        documents("svc::orders::OrderService::place"),
        "no Order Flow -DOCUMENTS-> OrderService::place"
    );
}

#[test]
fn confluence_still_needs_space() {
    let scratch = Scratch::new("needs-space");
    let out = glia(
        &scratch.0,
        &["docs", "sync", scratch.0.to_str().expect("UTF-8")],
    );
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "stderr:\n{stderr}");
    assert!(
        stderr.contains("--space"),
        "the error names --space:\n{stderr}"
    );
    assert!(
        !scratch.0.join(".glia/docs-snapshot").exists(),
        "a refused sync writes nothing"
    );
}

#[test]
fn source_flags_do_not_cross() {
    let scratch = Scratch::new("cross");
    let (repo, wiki) = repo_and_wiki(&scratch.0);
    let (repo_arg, wiki_arg) = (repo.to_str().expect("UTF-8"), wiki.to_str().expect("UTF-8"));
    let cases: [(&[&str], &str); 11] = [
        (
            &["--source", "dir"],
            "--path <DIR> is required with --source dir",
        ),
        (
            &["--source", "dir", "--path", wiki_arg, "--space", "K"],
            "--space applies only to --source confluence",
        ),
        (
            &["--space", "K", "--path", wiki_arg],
            "--path applies only to --source dir",
        ),
        (
            &["--source", "dir", "--path", wiki_arg, "--container", "a/b"],
            "--container",
        ),
        (&["--source", "notion"], "invalid value 'notion'"),
        (
            &["--source", "mediawiki", "--namespace", "0"],
            "--api <URL> is required with --source mediawiki",
        ),
        (
            &[
                "--source",
                "mediawiki",
                "--api",
                "https://w.example/w/api.php",
            ],
            "one of --namespace <N> or --category <NAME> is required",
        ),
        (
            &[
                "--source",
                "mediawiki",
                "--api",
                "http://wiki.example/w/api.php",
                "--namespace",
                "0",
            ],
            "refusing plain http://",
        ),
        (
            &[
                "--source",
                "dir",
                "--path",
                wiki_arg,
                "--api",
                "https://w.example/w/api.php",
            ],
            "--api applies only to --source mediawiki",
        ),
        (
            &["--space", "K", "--container", "c"],
            "--container applies only to --source dir or --source mediawiki",
        ),
        (
            &[
                "--source",
                "mediawiki",
                "--api",
                "https://w.example/w/api.php",
                "--namespace",
                "0",
                "--category",
                "Runbooks",
            ],
            "cannot be used with",
        ),
    ];
    for (extra, expect) in cases {
        let mut args = vec!["docs", "sync", repo_arg];
        args.extend_from_slice(extra);
        let out = glia(&scratch.0, &args);
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}\nstderr:\n{stderr}");
        assert!(
            stderr.contains(expect),
            "{args:?}: expected {expect:?} in\n{stderr}"
        );
    }
    assert!(
        !repo.join(".glia/docs-snapshot").exists(),
        "no refused sync wrote a snapshot"
    );

    // A wiki with no pages is an error, not an emptied container.
    let empty = scratch.0.join("empty.wiki");
    write(&empty.join("_Sidebar.md"), "* [[Home]]\n");
    let out = glia(
        &scratch.0,
        &[
            "docs",
            "sync",
            repo_arg,
            "--source",
            "dir",
            "--path",
            empty.to_str().expect("UTF-8"),
        ],
    );
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{stderr}");
    assert!(
        stderr.contains("[docs] sync source=dir")
            && stderr.contains("container=empty.wiki files=1 pages=0 kept=0 skipped=1"),
        "the container defaults to the directory's name:\n{stderr}"
    );
    assert!(
        stderr.contains("no Markdown pages under"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn dir_and_confluence_coexist() {
    let scratch = Scratch::new("coexist");
    let (repo, wiki) = repo_and_wiki(&scratch.0);
    let (repo_arg, wiki_arg) = (repo.to_str().expect("UTF-8"), wiki.to_str().expect("UTF-8"));

    let page = serde_json::json!({ "results": [{
        "id": "1",
        "title": "Ordering",
        "version": { "number": 2 },
        "body": { "storage": { "value": "<p>See <code>OrderService</code>.</p>" } },
        "_links": { "webui": "/spaces/ENG/pages/1" },
    }]});
    let stub = StubServer::start(vec![Canned::ok(page.to_string())]).expect("stub binds");
    let origin = stub.origin();
    let out = glia(
        &scratch.0,
        &[
            "docs", "sync", repo_arg, "--space", "ENG", "--site", &origin, "--email", "e",
            "--token", "t",
        ],
    );
    stub.finish();
    assert!(
        out.status.success(),
        "confluence sync stderr:\n{}",
        text(&out.stderr)
    );

    // `--include` scopes a dir sync by title exactly as it scopes a space.
    let out = glia(
        &scratch.0,
        &[
            "docs",
            "sync",
            repo_arg,
            "--source",
            "dir",
            "--path",
            wiki_arg,
            "--include",
            "order*",
        ],
    );
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "dir sync stderr:\n{stderr}");
    assert!(
        stderr.contains(&format!(
            "[docs] sync source=dir path={wiki_arg} container=repo.wiki files=7 pages=4 kept=1 skipped=3"
        )),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "[docs] snapshot source=wiki container=repo.wiki records=1 kept_other=1 replaced=0"
        ),
        "the Confluence space is kept:\n{stderr}"
    );
    assert_eq!(
        manifest_paths(&repo),
        ["confluence/ENG/ordering.md", "wiki/repo.wiki/order-flow.md"]
    );

    let manifest =
        std::fs::read_to_string(repo.join(".glia/docs-snapshot/manifest.jsonl")).expect("manifest");
    let wiki_rec: serde_json::Value =
        serde_json::from_str(manifest.lines().nth(1).expect("two lines")).expect("a DocRecord");
    let canon = std::fs::canonicalize(&wiki).expect("canonical wiki");
    assert_eq!(wiki_rec["provenance"]["kind"], "Wiki");
    assert_eq!(wiki_rec["provenance"]["container"], "repo.wiki");
    assert_eq!(
        wiki_rec["provenance"]["url"],
        format!("file://{}", canon.join("Order-Flow.md").display())
    );

    // A second dir sync of the same wiki replaces its own container only.
    let out = glia(
        &scratch.0,
        &[
            "docs", "sync", repo_arg, "--source", "dir", "--path", wiki_arg,
        ],
    );
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "re-sync stderr:\n{stderr}");
    assert!(
        stderr.contains(
            "[docs] snapshot source=wiki container=repo.wiki records=4 kept_other=1 replaced=1"
        ),
        "stderr:\n{stderr}"
    );
    assert_eq!(manifest_paths(&repo).len(), 5);
}

/// The Order Flow page's wikitext: a `== Flow ==` section naming the class.
const ORDER_FLOW: &str = "== Flow ==\n<code>OrderService</code> places orders.\n";

/// One `formatversion=2` `query.pages` entry (mediawiki.org API:Query /
/// API:Revisions / API:Info shape).
fn wiki_page(pageid: u64, title: &str, revid: u64, content: &str) -> serde_json::Value {
    let url = format!("https://ops.example/wiki/{}", title.replace(' ', "_"));
    serde_json::json!({
        "pageid": pageid,
        "ns": 0,
        "title": title,
        "contentmodel": "wikitext",
        "pagelanguage": "en",
        "touched": "2026-09-01T00:00:00Z",
        "lastrevid": revid,
        "length": content.len(),
        "fullurl": url,
        "canonicalurl": url,
        "revisions": [{
            "revid": revid,
            "parentid": 0,
            "timestamp": "2026-09-01T00:00:00Z",
            "slots": { "main": {
                "contentmodel": "wikitext",
                "contentformat": "text/x-wiki",
                "content": content,
            }},
        }],
    })
}

#[test]
fn mediawiki_sync_then_build() {
    let scratch = Scratch::new("mediawiki");
    let (repo, _) = repo_and_wiki(&scratch.0);
    let repo_arg = repo.to_str().expect("UTF-8");

    let first = serde_json::json!({
        "continue": { "gapcontinue": "Zeta", "continue": "gapcontinue||" },
        "query": { "pages": [
            wiki_page(1, "Order Flow", 11, ORDER_FLOW),
            wiki_page(2, "Deploy", 12, "Run the deploy.\n"),
        ]},
    });
    let last = serde_json::json!({
        "batchcomplete": true,
        "query": { "pages": [wiki_page(3, "Zeta", 13, "Last page.\n")] },
    });
    let stub = StubServer::start(vec![
        Canned::ok(first.to_string()),
        Canned::ok(last.to_string()),
    ])
    .expect("stub binds");
    let origin = stub.origin();
    let api = format!("{origin}/w/api.php");
    let out = glia(
        &scratch.0,
        &[
            "docs",
            "sync",
            repo_arg,
            "--source",
            "mediawiki",
            "--api",
            &api,
            "--namespace",
            "0",
            "--container",
            "ops-wiki",
        ],
    );
    let seen = stub.finish();
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(
        out.status.success(),
        "docs sync exited {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status
    );
    assert!(
        stderr.contains(&format!(
            "[docs] sync source=mediawiki api={origin} selection=ns:0 fetched=3 kept=3 skipped=0 requests=2 retries=0"
        )),
        "fired_on marker missing:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "[docs] wikitext pages=3 headings=1 code_blocks=0 inline_code=1 links=0 templates_dropped=0 unbalanced=0"
        ),
        "CE.4c's wikitext line follows:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "[docs] snapshot source=wiki container=ops-wiki records=3 kept_other=0 replaced=0 redacted=0"
        ),
        "stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("/w/api.php"),
        "the marker carries no path:\n{stderr}"
    );
    assert_eq!(seen.len(), 2);
    assert!(
        seen[1].target.contains("&gapcontinue=Zeta"),
        "{}",
        seen[1].target
    );
    assert!(
        seen.iter().all(|r| r
            .header("user-agent")
            .is_some_and(|ua| ua.starts_with("glia/"))
            && r.header("authorization").is_none()),
        "a glia User-Agent, and no token when none was given"
    );
    assert_eq!(
        manifest_paths(&repo),
        [
            "wiki/ops-wiki/deploy.md",
            "wiki/ops-wiki/order-flow.md",
            "wiki/ops-wiki/zeta.md",
        ]
    );

    let out = glia(&scratch.0, &["analyze", repo_arg, "--format", "json"]);
    assert!(
        out.status.success(),
        "analyze exited {:?}\nstderr:\n{}",
        out.status,
        text(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("analyze stdout is JSON");
    let nodes = v["nodes"].as_array().expect("nodes array");
    let edges = v["edges"].as_array().expect("edges array");
    let node = |kind: &str, qname: &str| {
        nodes
            .iter()
            .find(|n| n["kind_name"] == kind && n["qname"] == qname)
            .unwrap_or_else(|| {
                panic!(
                    "no {kind} {qname}; qnames: {:?}",
                    nodes.iter().map(|n| &n["qname"]).collect::<Vec<_>>()
                )
            })
    };
    let flow = node("DOC_SECTION", "docs::wiki::ops-wiki::order-flow::flow");
    let class = node("CLASS", "svc::orders::OrderService");
    assert!(
        edges.iter().any(|e| e["category"] == "DOCUMENTS"
            && e["from"] == flow["id"]
            && e["to"] == class["id"]),
        "no Order Flow § Flow -DOCUMENTS-> OrderService"
    );
}
