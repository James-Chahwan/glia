//! LF.2c — `glia gaps`, driving the real binary over probe r1 (a web client
//! whose hand-rolled `request()` wrapper hides the path from the extractor,
//! and a flask API) laid out under one root.
//!
//! The `[gaps] rows=... surface=cli` and `[overlay] N rules, +M edges,
//! orphans K→J, gaps G0→G1, verdict=<v>` stderr lines are the fired_on
//! markers; asserting them here makes them a tested contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CLIENT_TS: &str = "export function request(method: string, path: string) {\n  return fetch(path, { method });\n}\n\nexport async function loadUsers() {\n  return request('GET', '/users');\n}\n";
const API_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n\n\n@app.route(\"/orders\", methods=[\"GET\"])\ndef list_orders():\n    return []\n";

fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    // Relay the markers so `-- --nocapture | grep '^\[gaps\] '` sees them.
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[gaps] ") || line.starts_with("[overlay] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

/// Probe r1 under a fresh temp root: `web/src/client.ts` + `api/app.py`.
fn r1(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("glia-lf2c-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "web/src/client.ts", CLIENT_TS);
    write(&root, "api/app.py", API_PY);
    root
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

#[test]
fn gaps_json_reports_the_unresolved_sink() {
    let root = r1("json");
    let out = glia(&["gaps", s(&root), "--json"]);
    std::fs::remove_dir_all(&root).ok();
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");

    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON object");
    let keys: Vec<&str> = v
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        keys.contains(&"counts") && keys.contains(&"rows"),
        "{stdout}"
    );
    let rows = v["rows"].as_array().expect("rows");
    let unresolved: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r["category"] == "unresolved_endpoint")
        .collect();
    assert_eq!(unresolved.len(), 1, "{stdout}");
    assert_eq!(unresolved[0]["qname"], "endpoint:GET:<unresolved>");
    assert_eq!(unresolved[0]["detail"], "owner=web::src::client::request");
    assert_eq!(v["counts"]["unresolved_endpoint"], 1, "{stdout}");

    assert!(
        stderr.lines().any(|l| l.starts_with("[gaps] rows=4 (")
            && l.contains("unresolved_endpoint=1 wrapped_sink=0 unpaired_route=2")
            && l.ends_with(" surface=cli")),
        "{stderr}"
    );
}

#[test]
fn gaps_table_and_category_filter() {
    let root = r1("table");
    let out = glia(&[
        "gaps",
        s(&root),
        "--category",
        "unpaired_route",
        "--top-k",
        "1",
    ]);
    std::fs::remove_dir_all(&root).ok();
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("## unpaired_route — 2 (top 1)"), "{stdout}");
    assert!(
        stdout
            .contains("| `GET /users` | ROUTE | api/app.py:7 | heuristic | route_prefix\\|edge |"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("## unresolved_endpoint"),
        "filtered out: {stdout}"
    );
    assert!(
        stdout.contains("totals: unpaired_endpoint=0 ambiguous_endpoint=0 unresolved_endpoint=1"),
        "{stdout}"
    );
}

#[test]
fn unknown_category_exits_2() {
    let root = r1("nope");
    let out = glia(&["gaps", s(&root), "--category", "nope"]);
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
}

#[test]
fn overlay_delta_prints_the_accept_line() {
    let root = r1("delta");
    write(
        &root,
        "web/.glia/overlay.toml",
        "version = 1\n\n[[edge]]\nfrom = \"endpoint:GET:<unresolved>\"\nto = \"GET /users\"\ncategory = \"HTTP_CALLS\"\n",
    );
    let (web, api) = (root.join("web"), root.join("api"));
    let out = glia(&[
        "gaps",
        s(&web),
        "--with",
        s(&api),
        "--overlay-delta",
        "--json",
    ]);
    std::fs::remove_dir_all(&root).ok();
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let d: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON object");
    assert_eq!(
        (d["orphans_without"].as_u64(), d["orphans_with"].as_u64()),
        (Some(1), Some(0)),
        "{stdout}"
    );
    assert_eq!(d["added_by_category"]["HTTP_CALLS"], 1, "{stdout}");
    assert_eq!(d["verdict"], "keep", "{stdout}");
    // The graph gaps: dead_symbol 1 + unpaired_route 2 + unresolved_endpoint 1
    // without; the stanza pairs the sink with GET /users.
    assert_eq!(
        (
            &d["without"]["gaps_by_category"]["unresolved_endpoint"],
            &d["with"]["gaps_by_category"]["unresolved_endpoint"],
            &d["without"]["gaps_by_category"]["unpaired_route"],
            &d["with"]["gaps_by_category"]["unpaired_route"],
        ),
        (
            &serde_json::json!(1),
            &serde_json::json!(0),
            &serde_json::json!(2),
            &serde_json::json!(1)
        ),
        "{stdout}"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l == "[overlay] 1 rules, +1 edges, orphans 1→0, gaps 4→2, verdict=keep"),
        "{stderr}"
    );
}

/// CE.3a: the go-overlay-data-wrapper fixture, its NewCollection body changed
/// to `.Collection(strings.ToLower(name))` so CA.4 infers no wrapper (no bare
/// parameter reaches the driver): the overlay [[wrapper]] mints one
/// DATA_ENTITY and its ACCESSES_DATA edge, no orphan and no graph gap moves,
/// and the verdict keeps it (pre-CE.3a: `[overlay] 1 rules, +1 edges,
/// orphans 0→0` and "orphans did not fall").
#[test]
fn overlay_delta_keeps_a_data_wrapper_that_pairs_no_orphan() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/go-overlay-data-wrapper");
    let read = |rel: &str| std::fs::read_to_string(fixture.join(rel)).expect("fixture file");
    let collection = read("collection.go")
        .replace(
            "import (\n\t\"go.mongodb.org",
            "import (\n\t\"strings\"\n\n\t\"go.mongodb.org",
        )
        .replace(".Collection(name)}", ".Collection(strings.ToLower(name))}");
    assert!(
        collection.contains("strings.ToLower(name)") && collection.contains("\"strings\""),
        "{collection}"
    );
    let root = std::env::temp_dir().join(format!("glia-ce3a-cli-gowrap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "collection.go", &collection);
    write(
        &root,
        "chat_preview_repository.go",
        &read("chat_preview_repository.go"),
    );
    write(&root, ".glia/overlay.toml", &read(".glia/overlay.toml"));

    let json = glia(&["gaps", s(&root), "--overlay-delta", "--json"]);
    let table = glia(&["gaps", s(&root), "--overlay-delta"]);
    std::fs::remove_dir_all(&root).ok();
    let (stdout, stderr) = (text(&json.stdout), text(&json.stderr));
    assert_eq!(json.status.code(), Some(0), "{stderr}");
    assert!(
        !stderr.lines().any(|l| l.starts_with("[wrappers] inferred")),
        "CA.4 infers nothing from the ToLower body: {stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l == "[overlay] 1 rules, +1 edges, orphans 0→0, gaps 3→3, verdict=keep"),
        "{stderr}"
    );
    let d: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON object");
    assert_eq!(
        d["nodes_added_by_kind"],
        serde_json::json!({"DATA_ENTITY": 1}),
        "{stdout}"
    );
    assert_eq!(
        d["added_by_category"],
        serde_json::json!({"ACCESSES_DATA": 1}),
        "{stdout}"
    );
    assert_eq!(d["verdict"], "keep", "{stdout}");

    let out = text(&table.stdout);
    assert_eq!(table.status.code(), Some(0), "{}", text(&table.stderr));
    for line in [
        "| gaps | 3 | 3 |",
        "- node kind DATA_ENTITY: 0→1 (+1)",
        "- edge category ACCESSES_DATA: 0→1 (+1)",
        "verdict: keep - a gap category fell or the graph grew, none rose",
    ] {
        assert!(out.lines().any(|l| l == line), "{line:?} in {out}");
    }
    assert!(
        !out.lines().any(|l| l.starts_with("- gap category")),
        "no gap category moved: {out}"
    );
}

/// CD.3b's suspected_edges fixture (engine/tests/suspected_edges.rs) under a
/// fresh temp root: an Angular client (`web/`) whose `list()` / `count()`
/// pair through the HTTP resolver, so (ENDPOINT, HTTP_CALLS, ROUTE) is
/// learned, and whose `markAllRead()` posts through a class-field
/// `${environment.apiUrl}` base the resolver cannot pair; an Express API
/// (`api/`) serving all three, the read-all routes behind a `/:tenant` path
/// parameter the mount-segment fold (CB.23) never strips.
fn suspected(tag: &str) -> PathBuf {
    const SERVICE_TS: &str = "import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { environment } from '../environments/environment';

@Injectable({ providedIn: 'root' })
export class NotificationsApi {
  private readonly base = `${environment.apiUrl}/notifications`;
  constructor(private http: HttpClient) {}

  markAllRead() {
    return this.http.post(`${this.base}/read-all`, {});
  }

  list() {
    return this.http.get('/notifications');
  }

  count() {
    return this.http.get('/notifications/count');
  }
}
";
    const ENVIRONMENT_TS: &str =
        "export const environment = { production: false, apiUrl: 'http://localhost:8080/api' };\n";
    const WEB_PACKAGE: &str = "{\"name\":\"web\",\"dependencies\":{\"@angular/core\":\"17.0.0\",\"@angular/common\":\"17.0.0\"}}\n";
    const ROUTES_TS: &str = "import express from 'express';
const router = express.Router();

function markAll(req, res) { res.json({}); }
function getOne(req, res) { res.json({}); }
function listAll(req, res) { res.json([]); }
function countAll(req, res) { res.json(0); }

router.post('/:tenant/notifications/read-all', markAll);
router.get('/:tenant/notifications/read-all', getOne);
router.get('/notifications', listAll);
router.get('/notifications/count', countAll);

export default router;
";
    const API_PACKAGE: &str = "{\"name\":\"api\",\"dependencies\":{\"express\":\"4.18.0\"}}\n";

    let root = std::env::temp_dir().join(format!("glia-cd3c-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "web/src/notifications.service.ts", SERVICE_TS);
    write(&root, "web/environments/environment.ts", ENVIRONMENT_TS);
    write(&root, "web/package.json", WEB_PACKAGE);
    write(&root, "api/src/routes.ts", ROUTES_TS);
    write(&root, "api/package.json", API_PACKAGE);
    root
}

/// The fenced ```toml blocks of `stdout`, each without its fences.
fn toml_blocks(stdout: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut open: Option<Vec<&str>> = None;
    for line in stdout.lines() {
        match open.as_mut() {
            None if line == "```toml" => open = Some(Vec::new()),
            Some(body) if line == "```" => {
                blocks.push(body.join("\n") + "\n");
                open = None;
            }
            Some(body) => body.push(line),
            None => {}
        }
    }
    assert!(open.is_none(), "an unclosed fence: {stdout}");
    blocks
}

/// CD.3c: after the suspected_edge table, one ```toml block per row holding
/// the row's `draft` verbatim — a `# gap: <id>` comment, then one `[[edge]]`
/// — under a `paste into <repo>/.glia/overlay.toml` header. Pasted (after
/// `version = 1`), it is the overlay `--overlay-delta` keeps. Pre-CD.3c:
/// the table only, no block.
#[test]
fn suspected_edge_prints_stanza() {
    let root = suspected("stanza");
    let out = glia(&["gaps", s(&root), "--category", "suspected_edge"]);
    let json = glia(&["gaps", s(&root), "--category", "suspected_edge", "--json"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(json.status.code(), Some(0), "{}", text(&json.stderr));
    assert!(stdout.contains("## suspected_edge — 1"), "{stdout}");
    assert!(
        stderr.lines().any(|l| l.starts_with("[gaps] rows=1 (")
            && l.contains(" suspected_edge=1 ")
            && l.ends_with(" surface=cli")),
        "{stderr}"
    );

    let overlay = root.join(".glia/overlay.toml");
    let header = format!(
        "paste into {} (heuristic - check it, then try it with --overlay-delta)",
        overlay.display()
    );
    let lines: Vec<&str> = stdout.lines().collect();
    let at = lines.iter().position(|l| *l == header);
    assert!(at.is_some(), "{header:?} in {stdout}");
    assert_eq!(
        lines.get(at.unwrap_or_default() + 1).copied(),
        Some("(no such file yet: a new overlay starts with `version = 1`)"),
        "{stdout}"
    );
    let table_row = lines.iter().position(|l| l.contains("`endpoint:POST:"));
    assert!(
        table_row.is_some_and(|t| Some(t) < at),
        "the stanza follows the table: {stdout}"
    );

    let blocks = toml_blocks(&stdout);
    assert_eq!(blocks.len(), 1, "one block per suspected row: {stdout}");
    let block = &blocks[0];
    let first = block.lines().next().unwrap_or_default();
    let hex = first.strip_prefix("# gap: gap:").unwrap_or_default();
    assert!(
        hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "{block}"
    );
    assert_eq!(block.lines().nth(1), Some("[[edge]]"), "{block}");
    for line in [
        "category = \"HTTP_CALLS\"",
        "from = \"endpoint:POST:${…}/read-all @web\"",
        "to = \"POST /:tenant/notifications/read-all @api\"",
    ] {
        assert!(block.lines().any(|l| l == line), "{line:?} in {block}");
    }
    assert!(
        !block.lines().any(|l| l.starts_with("gap = ")),
        "the id rides in a comment: {block}"
    );

    // Verbatim: the block is the row's JSON `draft`, and its comment names
    // the row's id.
    let v: serde_json::Value =
        serde_json::from_str(&text(&json.stdout)).expect("stdout is one JSON object");
    let row = &v["rows"][0];
    assert_eq!(row["category"], "suspected_edge", "{v}");
    assert_eq!(row["draft"].as_str(), Some(block.as_str()), "{v}");
    assert_eq!(
        Some(first),
        row["id"]
            .as_str()
            .map(|id| format!("# gap: {id}"))
            .as_deref(),
        "{v}"
    );

    // Pasted as told, the stanza binds: the orphan pairs, the row goes, and
    // the overlay is kept.
    write(
        &root,
        ".glia/overlay.toml",
        &format!("version = 1\n\n{block}"),
    );
    let delta = glia(&["gaps", s(&root), "--overlay-delta", "--json"]);
    let again = glia(&["gaps", s(&root), "--category", "suspected_edge"]);
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(delta.status.code(), Some(0), "{}", text(&delta.stderr));
    let d: serde_json::Value = serde_json::from_str(&text(&delta.stdout)).expect("one JSON object");
    assert_eq!(d["added_by_category"]["HTTP_CALLS"], 1, "{d}");
    assert_eq!(
        (
            &d["without"]["gaps_by_category"]["suspected_edge"],
            &d["with"]["gaps_by_category"]["suspected_edge"]
        ),
        (&serde_json::json!(1), &serde_json::json!(0)),
        "{d}"
    );
    assert_eq!(d["verdict"], "keep", "{d}");
    let again = text(&again.stdout);
    assert!(
        !again.contains("## suspected_edge") && !again.contains("```toml"),
        "nothing left to propose: {again}"
    );
}

/// CE.3a: the table gains an `id` column first, one `gap:<16 hex>` per row.
#[test]
fn gaps_table_leads_with_the_id() {
    let root = r1("ids");
    let out = glia(&["gaps", s(&root), "--category", "unresolved_endpoint"]);
    std::fs::remove_dir_all(&root).ok();
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        stdout.contains("| id | qname | kind | at | tier | suggest | detail |"),
        "{stdout}"
    );
    let row = stdout
        .lines()
        .find(|l| l.contains("`endpoint:GET:<unresolved>`"))
        .expect("the sink's row");
    let id = row
        .strip_prefix("| gap:")
        .and_then(|r| r.split_once(' '))
        .map(|(hex, _)| hex)
        .unwrap_or_default();
    assert!(
        id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "{row}"
    );
}
