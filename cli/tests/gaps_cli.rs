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
