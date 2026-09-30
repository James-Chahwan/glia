//! LA.11 — `glia docs sync` / `glia docs push` end to end, driving the real
//! binary against the loopback Confluence stub (`glia_doc_sources::stub`).
//!
//! Safety: the child always runs with `current_dir` = a fresh scratch dir (so a
//! developer's `./.env` is never read) and with `CONFLUENCE_*` removed from its
//! env, and every credential is passed as a flag aimed at the stub — a test can
//! never reach a real site with real credentials.
//!
//! The `[docs] origin=…` stderr line is the LA.11 fired_on marker; it is relayed
//! so `cargo test -p glia-cli --test docs_stub_cli -- --nocapture | grep '\[docs\] origin='`
//! sees it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glia_doc_sources::stub::{Canned, StubServer};

/// A scratch dir under the system temp dir, created fresh and removed on drop.
/// (`cli` has no dev-dependencies, so no `tempfile`.)
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("glia-docs-stub-{}-{test}", std::process::id()));
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

#[test]
fn docs_sync_through_stub_feeds_documents_edge() {
    let scratch = Scratch::new("sync");
    let repo = scratch.0.join("repo");
    std::fs::create_dir_all(repo.join("shop")).expect("mkdir shop");
    std::fs::write(
        repo.join("shop/orders.py"),
        "def place_order(order):\n    return order\n",
    )
    .expect("write orders.py");
    let repo_arg = repo.to_str().expect("scratch path is UTF-8");

    let page = serde_json::json!({ "results": [{
        "id": "1",
        "title": "Ordering",
        "version": { "number": 4 },
        "body": { "storage": { "value": "<p>Submit with <code>place_order</code> once the cart is valid.</p>" } },
        "_links": { "webui": "/spaces/K/pages/1" },
    }]});
    let stub = StubServer::start(vec![Canned::ok(page.to_string())]).expect("stub binds");
    let origin = stub.origin();
    let out = glia(
        &scratch.0,
        &[
            "docs", "sync", repo_arg, "--space", "K", "--site", &origin, "--email", "e", "--token",
            "t",
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
        stdout.contains("synced 1 page(s) from space K"),
        "stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("[docs] sync space=K fetched=1 kept=1 include=0 exclude=0"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("[docs] origin={origin} (plain http, loopback)")),
        "fired_on marker missing:\n{stderr}"
    );
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert!(
        seen[0]
            .target
            .starts_with("/wiki/rest/api/space/K/content/page?"),
        "{}",
        seen[0].target
    );
    // base64("e:t")
    assert_eq!(seen[0].header("authorization"), Some("Basic ZTp0"));
    assert!(
        repo.join(".glia/docs-snapshot/manifest.jsonl").is_file(),
        "snapshot manifest written"
    );
    assert!(
        stderr.contains(
            "[docs] snapshot source=confluence container=K records=1 kept_other=0 replaced=0 redacted=0 -> .glia/docs-snapshot/manifest.jsonl"
        ),
        "CE.4a snapshot marker missing:\n{stderr}"
    );

    // The snapshot feeds the offline build: DOC_SPACE + DOC_SECTION + a
    // DOCUMENTS link from the page to the function its code span names.
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
    let kinds = |k: &str| nodes.iter().filter(|n| n["kind_name"] == k).count();
    assert!(kinds("DOC_SPACE") >= 1, "no DOC_SPACE node");
    assert!(kinds("DOC_SECTION") >= 1, "no DOC_SECTION node");
    let target = nodes
        .iter()
        .find(|n| n["qname"] == "shop::orders::place_order")
        .unwrap_or_else(|| {
            panic!(
                "place_order node missing; qnames: {:?}",
                nodes.iter().map(|n| &n["qname"]).collect::<Vec<_>>()
            )
        });
    let documented = edges.iter().any(|e| {
        e["category"] == "DOCUMENTS"
            && e["to"] == target["id"]
            && nodes
                .iter()
                .any(|n| n["id"] == e["from"] && n["kind_name"] == "DOC_SECTION")
    });
    assert!(
        documented,
        "no DOC_SECTION -DOCUMENTS-> shop::orders::place_order edge"
    );
}

/// `glia docs sync <repo> --space <space>` against a stub serving `page`.
fn sync_space(scratch: &Path, repo: &str, space: &str, page: serde_json::Value) -> (Output, String) {
    let stub = StubServer::start(vec![Canned::ok(serde_json::json!({ "results": [page] }).to_string())])
        .expect("stub binds");
    let origin = stub.origin();
    let out = glia(
        scratch,
        &["docs", "sync", repo, "--space", space, "--site", &origin, "--email", "e", "--token", "t"],
    );
    stub.finish();
    let stderr = text(&out.stderr);
    assert!(
        out.status.success(),
        "docs sync {space} exited {:?}\nstderr:\n{stderr}",
        out.status
    );
    (out, stderr)
}

/// CE.4a: syncing a second space keeps the first space's records (HEAD: the
/// second sync overwrote the manifest, leaving 1 record), and a page whose
/// body opens with its own title builds ONE DOC_SECTION with one CONTAINS
/// edge (HEAD: `docs::OPS::billing::billing` twice, NodeId 733198665780254940,
/// with two identical CONTAINS edges from `docspace::confluence::OPS`).
#[test]
fn docs_sync_second_space_keeps_the_first() {
    let scratch = Scratch::new("two-spaces");
    let repo = scratch.0.join("repo");
    std::fs::create_dir_all(repo.join("shop")).expect("mkdir shop");
    std::fs::write(
        repo.join("shop/orders.py"),
        "def place_order(order):\n    return order\n\nclass BillingService:\n    pass\n",
    )
    .expect("write orders.py");
    let repo_arg = repo.to_str().expect("scratch path is UTF-8");

    let page = |id: &str, title: &str, storage: &str, space: &str| {
        serde_json::json!({
            "id": id,
            "title": title,
            "version": { "number": 1 },
            "body": { "storage": { "value": storage } },
            "_links": { "webui": format!("/spaces/{space}/pages/{id}") },
        })
    };
    sync_space(
        &scratch.0,
        repo_arg,
        "ENG",
        page("1", "Orders", "<p>Call <code>place_order</code>.</p>", "ENG"),
    );
    let (_, stderr) = sync_space(
        &scratch.0,
        repo_arg,
        "OPS",
        page("2", "Billing", "<h2>Billing</h2><p>see <code>BillingService</code></p>", "OPS"),
    );
    assert!(
        stderr.contains(
            "[docs] snapshot source=confluence container=OPS records=1 kept_other=1 replaced=0 redacted=0 -> .glia/docs-snapshot/manifest.jsonl"
        ),
        "stderr:\n{stderr}"
    );
    let manifest = std::fs::read_to_string(repo.join(".glia/docs-snapshot/manifest.jsonl"))
        .expect("manifest written");
    let paths: Vec<String> = manifest
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("a DocRecord line");
            v["rel_path"].as_str().unwrap_or("").to_string()
        })
        .collect();
    assert_eq!(paths, ["confluence/ENG/orders.md", "confluence/OPS/billing.md"]);

    let out = glia(&scratch.0, &["analyze", repo_arg, "--format", "json"]);
    assert!(out.status.success(), "analyze stderr:\n{}", text(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("analyze stdout is JSON");
    let nodes = v["nodes"].as_array().expect("nodes array");
    let edges = v["edges"].as_array().expect("edges array");
    let mut ids: Vec<u64> = nodes.iter().filter_map(|n| n["id"].as_u64()).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "every node id is distinct");
    let billing: Vec<&serde_json::Value> = nodes
        .iter()
        .filter(|n| n["kind_name"] == "DOC_SECTION" && n["qname"] == "docs::OPS::billing::billing")
        .collect();
    assert_eq!(billing.len(), 1, "one billing section");
    let into_billing: Vec<&serde_json::Value> = edges
        .iter()
        .filter(|e| e["category"] == "CONTAINS" && e["to"] == billing[0]["id"])
        .collect();
    assert_eq!(into_billing.len(), 1, "one CONTAINS edge into it");
    assert!(
        nodes.iter().any(|n| n["id"] == into_billing[0]["from"] && n["qname"] == "docspace::confluence::OPS"),
        "the OPS space contains it"
    );
    assert!(
        nodes.iter().any(|n| n["kind_name"] == "DOC_SECTION" && n["qname"] == "docs::ENG::orders::orders"),
        "the ENG space's page survives the OPS sync"
    );
}

#[test]
fn docs_push_markdown_through_stub_creates_page() {
    let scratch = Scratch::new("push");
    let note = scratch.0.join("note.md");
    std::fs::write(
        &note,
        "# Ordering\n\nCall `place_order` once the cart is valid.\n",
    )
    .expect("write note.md");

    let created = serde_json::json!({
        "id": "77",
        "title": "T",
        "version": { "number": 1 },
        "_links": { "webui": "/spaces/K/pages/77" },
    });
    let stub = StubServer::start(vec![Canned::ok(created.to_string())]).expect("stub binds");
    let origin = stub.origin();
    let out = glia(
        &scratch.0,
        &[
            "docs",
            "push",
            "--space",
            "K",
            "--title",
            "T",
            "--file",
            note.to_str().expect("UTF-8"),
            "--markdown",
            "--site",
            &origin,
            "--email",
            "e",
            "--token",
            "t",
        ],
    );
    let seen = stub.finish();
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(
        out.status.success(),
        "docs push exited {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status
    );
    assert!(
        stdout.starts_with("created page 77 (v1)"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("{origin}/wiki/spaces/K/pages/77")),
        "stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("[docs] push markdown→storage:"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("[docs] origin={origin} (plain http, loopback)")),
        "fired_on marker missing:\n{stderr}"
    );

    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].target, "/wiki/rest/api/content");
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).expect("POST body is JSON");
    assert_eq!(body["space"]["key"], "K");
    assert_eq!(body["title"], "T");
    let storage = body["body"]["storage"]["value"]
        .as_str()
        .expect("storage value");
    assert!(
        storage.contains("<h1>"),
        "markdown heading not converted: {storage}"
    );
    assert!(
        storage.contains("<code>place_order</code>"),
        "inline code not converted: {storage}"
    );
}
