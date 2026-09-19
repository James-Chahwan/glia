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
