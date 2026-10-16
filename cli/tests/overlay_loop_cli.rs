//! CE.3e — `glia overlay propose|try|accept`, driving the real binary over
//! the Go sources of the substrate-gap fixture `go-overlay-data-wrapper`
//! laid out without its overlay file in a temp dir.
//!
//! `NewCollection` is rewritten to take a computed collection name
//! (`strings.ToLower(name)`), as engine/tests/overlay_loop.rs does: with the
//! fixture's literal `.Collection(name)` the CA.4 wrapper inference already
//! mints the entity, so the overlay `[[wrapper]]` would add nothing and the
//! round trip would test the inference instead of the loop.
//!
//! The `[overlay] candidate file=...`, `[overlay] propose ... surface=cli`,
//! `[overlay] try ... surface=cli` and `[overlay] accept ... surface=cli`
//! stderr lines are the fired_on markers; asserting them here makes them a
//! tested contract.
//!
//! DB.9a: a candidate may carry `[[constraint]]` rules. Baseline (HEAD
//! c2dd8d75): `glia overlay try <repo> --candidate rules.toml` (one invariant
//! with a `rule`) exits 2 and `glia overlay accept ... --dry-run` exits 1,
//! both with `candidate: [[constraint]]: a candidate carries only overlay
//! sections and entrypoints (...)`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CHAT_REPO_GO: &str = include_str!(
    "../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/chat_preview_repository.go"
);
const COLLECTION_GO: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/collection.go");
const FIXTURE_OVERLAY: &str =
    include_str!("../../bench/substrate-gap/fixtures/go-overlay-data-wrapper/.glia/overlay.toml");

/// An `[[edge]]` between two qnames that do not exist: binds nothing.
const USELESS_EDGE: &str =
    "[[edge]]\nfrom = \"nowhere::Caller\"\nto = \"nowhere::Callee\"\ncategory = \"CALLS\"\n";

fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    // Relay the markers so `-- --nocapture | grep '^\[overlay\] '` sees them.
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[overlay] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

/// A fresh temp root for one test.
fn tmp(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("glia-ce3e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    root
}

/// The fixture's Go sources under `<root>/repo`, `NewCollection` taking a
/// computed name (see the module doc); no `.glia/overlay.toml`.
fn go_repo(root: &Path) -> PathBuf {
    let collection = COLLECTION_GO
        .replace(
            "import (\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)",
            "import (\n\t\"strings\"\n\n\t\"go.mongodb.org/mongo-driver/mongo\"\n)",
        )
        .replace(".Collection(name)", ".Collection(strings.ToLower(name))");
    assert!(
        collection.contains("\"strings\"") && collection.contains("strings.ToLower(name)"),
        "the fixture's NewCollection moved"
    );
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    std::fs::write(repo.join("collection.go"), collection).expect("write");
    std::fs::write(repo.join("chat_preview_repository.go"), CHAT_REPO_GO).expect("write");
    repo
}

/// The fixture's `[[wrapper]]` stanza, the table alone.
fn wrapper_stanza() -> &'static str {
    let at = FIXTURE_OVERLAY
        .find("[[wrapper]]")
        .expect("the fixture overlay has a [[wrapper]]");
    &FIXTURE_OVERLAY[at..]
}

/// `<root>/cand.toml`: the wrapper (`wrapper#1`) then the useless edge
/// (`edge#1`).
fn candidate(root: &Path) -> PathBuf {
    let path = root.join("cand.toml");
    std::fs::write(&path, format!("{}\n{USELESS_EDGE}", wrapper_stanza())).expect("write");
    path
}

fn overlay_file(repo: &Path) -> PathBuf {
    repo.join(".glia/overlay.toml")
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_str(&text(&out.stdout)).expect("stdout is one JSON object")
}

#[test]
fn propose_try_accept_round_trip() {
    let root = tmp("round-trip");
    let repo = go_repo(&root);
    let (r, file) = (s(&repo), overlay_file(&repo));
    let cand = candidate(&root);
    let c = s(&cand);

    // propose: every row carries its id; the dead symbols come with source.
    let out = glia(&["overlay", "propose", r, "--json"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let p = json(&out);
    let rows = p["rows"].as_array().expect("rows");
    assert!(!rows.is_empty(), "{p}");
    for row in rows {
        let id = row["id"].as_str().expect("id");
        assert!(id.starts_with("gap:") && id.len() == 20, "{row}");
        assert_ne!(row["category"], "wrapped_sink", "{row}");
    }
    let dead = rows
        .iter()
        .find(|row| row["qname"] == "collection::NewCollection")
        .expect("NewCollection is a dead_symbol row");
    assert_eq!(dead["snippet"]["file"], "collection.go", "{dead}");
    assert!(
        p["guide"]
            .as_str()
            .expect("guide")
            .contains("docs/overlay.md")
    );
    assert!(
        stderr.lines().any(
            |l| l.starts_with(&format!("[overlay] propose repo={r} rows="))
                && l.ends_with(" ambiguous_root=0 surface=cli")
        ),
        "{stderr}"
    );
    assert!(!file.exists(), "propose writes no overlay");

    // The text form: the table, and the row's line marked in its snippet.
    let out = glia(&["overlay", "propose", r, "--category", "dead_symbol"]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(stdout.contains("## dead_symbol — "), "{stdout}");
    assert!(
        stdout.contains("| id | qname | at | tier | suggest |"),
        "{stdout}"
    );
    assert!(stdout.contains("```go\n"), "{stdout}");
    assert!(
        stdout.contains(
            ">15 | func NewCollection[T any](client *mongo.Client, database string, name string) *Collection[T] {"
        ),
        "{stdout}"
    );

    // try: the wrapper keeps, the edge drops.
    let out = glia(&["overlay", "try", r, "--candidate", c, "--json"]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let t = json(&out);
    let verdicts: Vec<(&str, &str)> = t["stanzas"]
        .as_array()
        .expect("stanzas")
        .iter()
        .map(|st| {
            (
                st["stanza"].as_str().expect("stanza"),
                st["verdict"].as_str().expect("verdict"),
            )
        })
        .collect();
    assert_eq!(verdicts, [("wrapper#1", "keep"), ("edge#1", "drop")], "{t}");
    assert_eq!(t["builds"], 4, "{t}");
    assert!(
        stderr.contains(&format!(
            "[overlay] candidate file={c} stanzas=2 (route_prefix=0 wrapper=1 edge=1 constants=0 entrypoints=0 constraint=0) gap_links=0 errors=0"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "[overlay] try repo={r} stanzas=2 builds=4 verdicts keep=1 review=0 drop=1 gaps 3→3 closed=0 surface=cli"
        )),
        "{stderr}"
    );
    assert!(!file.exists(), "try writes no overlay");

    // The text form: the totals, one row per stanza, the accept line.
    let out = glia(&["overlay", "try", r, "--candidate", c]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        stdout.contains("| node DATA_ENTITY | 0 | 1 | +1 |"),
        "{stdout}"
    );
    assert!(
        stdout.contains("| edge ACCESSES_DATA | 0 | 1 | +1 |"),
        "{stdout}"
    );
    assert!(
        stdout.contains("| wrapper#1 | keep | — | DATA_ENTITY +1 | ACCESSES_DATA +1 | — |"),
        "{stdout}"
    );
    assert!(
        stdout.contains("| edge#1 | drop | — | — | — | — |"),
        "{stdout}"
    );
    assert!(stdout.contains("--only wrapper#1"), "{stdout}");

    // --no-leave-one-out: two builds, the totals only.
    let out = glia(&[
        "overlay",
        "try",
        r,
        "--candidate",
        c,
        "--no-leave-one-out",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let quick = json(&out);
    assert_eq!(quick["builds"], 2, "{quick}");
    assert_eq!(quick["stanzas"], serde_json::json!([]), "{quick}");
    assert_eq!(quick["verdict"], t["verdict"]);

    // accept --dry-run: the diff, nothing written.
    let out = glia(&[
        "overlay",
        "accept",
        r,
        "--candidate",
        c,
        "--only",
        "wrapper#1",
        "--dry-run",
    ]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(stdout.contains("+[[wrapper]]"), "{stdout}");
    assert!(stdout.contains("dry run: "), "{stdout}");
    assert!(!file.exists(), "a dry run writes nothing");

    // accept: the wrapper goes in, the edge does not.
    let out = glia(&[
        "overlay",
        "accept",
        r,
        "--candidate",
        c,
        "--only",
        "wrapper#1",
    ]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("added: 1 (wrapper=1)"), "{stdout}");
    assert!(stdout.contains("wrote "), "{stdout}");
    assert!(
        stderr.contains(&format!(
            "[overlay] accept repo={r} added=1 (route_prefix=0 wrapper=1 edge=0 constants=0 entrypoints=0 constraint=0) removed=0 duplicates=0 file=.glia/overlay.toml dry_run=false surface=cli"
        )),
        "{stderr}"
    );
    let written = std::fs::read_to_string(&file).expect("accept wrote the overlay");
    assert!(written.starts_with("version = 1\n"), "{written}");
    assert!(
        written.contains("[[wrapper]]\ncall = \"NewCollection\""),
        "{written}"
    );
    assert!(
        !written.contains("[[edge]]"),
        "only wrapper#1 is written: {written}"
    );

    // The accepted overlay measures as a keep.
    let out = glia(&["gaps", r, "--overlay-delta", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let delta = json(&out);
    assert_eq!(delta["verdict"], "keep", "{delta}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn accept_without_input_exits_2() {
    let root = tmp("no-input");
    let repo = go_repo(&root);
    let out = glia(&["overlay", "accept", s(&repo)]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("nothing to accept"), "{stderr}");
    assert!(!overlay_file(&repo).exists());

    // --only names candidate stanzas: without --candidate it is a usage error.
    let out = glia(&["overlay", "accept", s(&repo), "--only", "wrapper#1"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));

    // A stanza the candidate does not have: a usage error, nothing written.
    let cand = candidate(&root);
    let out = glia(&[
        "overlay",
        "accept",
        s(&repo),
        "--candidate",
        s(&cand),
        "--only",
        "wrapper#9",
    ]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("wrapper#9") && stderr.contains("edge#1"),
        "{stderr}"
    );
    assert!(!overlay_file(&repo).exists());

    // The loop works on the overlay: the global --no-overlay is refused.
    let out = glia(&["--no-overlay", "overlay", "propose", s(&repo)]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--no-overlay"), "{stderr}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn refused_accept_exits_1() {
    let root = tmp("refused");
    let repo = go_repo(&root);
    let file = overlay_file(&repo);
    std::fs::create_dir_all(file.parent().expect("dir")).expect("mkdir");
    let before = "# team overlay\nversion = 1\n\n[constants]\nGATEWAY = \"/a\"\n";
    std::fs::write(&file, before).expect("write");

    // A key the loader rejects: it would drop the whole file.
    let bad = root.join("bad.toml");
    std::fs::write(
        &bad,
        wrapper_stanza().replace("flavor = \"nosql\"", "flavour = \"nosql\""),
    )
    .expect("write");
    let out = glia(&["overlay", "accept", s(&repo), "--candidate", s(&bad)]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("unknown field `flavour`"), "{stderr}");
    assert!(
        stderr.contains(&format!("[overlay] candidate file={} stanzas=1 ", s(&bad)))
            && stderr.contains(" errors=1"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("[overlay] accept "),
        "a refusal prints no accept marker: {stderr}"
    );
    assert_eq!(std::fs::read_to_string(&file).expect("read"), before);

    // The same candidate is a candidate error for try: exit 2.
    let out = glia(&["overlay", "try", s(&repo), "--candidate", s(&bad)]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));

    // A constant the file pins to another value: refused, nothing written.
    let conflict = root.join("conflict.toml");
    std::fs::write(&conflict, "[constants]\nGATEWAY = \"/b\"\n").expect("write");
    let out = glia(&["overlay", "accept", s(&repo), "--candidate", s(&conflict)]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("already pinned"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&file).expect("read"), before);
    std::fs::remove_dir_all(root).ok();
}

/// `<root>/<name>`: one `[[constraint]]` invariant `id` with `rule`, no
/// `version` (a candidate is checked as version 2).
fn rule_candidate(root: &Path, name: &str, id: &str, rule: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(
        &path,
        format!(
            "[[constraint]]\nid = \"{id}\"\nkind = \"invariant\"\nanchor = \"collection::NewCollection\"\ntext = \"t\"\nrule = '{rule}'\n"
        ),
    )
    .expect("write");
    path
}

/// DB.9a: a rule goes through accept (the file moves to `version = 2` in
/// place), an identical one is a duplicate, a same-id other body and a rule
/// `glia check` could not read are refused (exit 1, nothing written), an
/// `allow` is refused, and `try` takes a rule candidate.
#[test]
fn rule_candidates_accept_and_collide() {
    let root = tmp("rules");
    let repo = go_repo(&root);
    let (r, file) = (s(&repo), overlay_file(&repo));
    std::fs::create_dir_all(file.parent().expect("dir")).expect("mkdir");
    let base = "# team overlay\nversion = 1 # schema\n\n[constants]\nGATEWAY = \"/a\"\n";
    std::fs::write(&file, base).expect("write");
    // The tree has no ROUTE, so the rule holds whatever the Go call resolution.
    let rules = rule_candidate(
        &root,
        "rules.toml",
        "no-routes",
        "violation(r) :- node(r, \"ROUTE\").",
    );
    let c = s(&rules);

    let out = glia(&["overlay", "accept", r, "--candidate", c]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("added: 1 (constraint=1)"), "{stdout}");
    assert!(
        stderr.contains(&format!(
            "[overlay] accept repo={r} added=1 (route_prefix=0 wrapper=0 edge=0 constants=0 entrypoints=0 constraint=1) removed=0 duplicates=0 file=.glia/overlay.toml dry_run=false surface=cli"
        )),
        "{stderr}"
    );
    let written = std::fs::read_to_string(&file).expect("read");
    assert!(
        written
            .starts_with("# team overlay\nversion = 2 # schema\n\n[constants]\nGATEWAY = \"/a\"\n"),
        "{written}"
    );
    assert!(
        written.contains("[[constraint]]\nid = \"no-routes\""),
        "{written}"
    );

    // The same rule again: a duplicate, the file unchanged.
    let out = glia(&["overlay", "accept", r, "--candidate", c]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(stdout.contains("duplicates: 1"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&file).expect("read"), written);

    // The same id with another body: refused, nothing written.
    let other = rule_candidate(
        &root,
        "other.toml",
        "no-routes",
        "violation(e) :- node(e, \"ENDPOINT\").",
    );
    let out = glia(&["overlay", "accept", r, "--candidate", s(&other)]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("constraint \"no-routes\" is already declared"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&file).expect("read"), written);

    // A rule `glia check` could not read: refused at accept, nothing written.
    let rotten = rule_candidate(
        &root,
        "rotten.toml",
        "rotten",
        "violation(r) :- node(r, \"ROUTEE\").",
    );
    let out = glia(&["overlay", "accept", r, "--candidate", s(&rotten)]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("does not validate"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&file).expect("read"), written);

    // A waiver is a person's: refused, nothing written.
    let allow = root.join("allow.toml");
    std::fs::write(
        &allow,
        "[[constraint]]\nid = \"waive\"\nkind = \"allow\"\nfrom = \"web\"\nto = \"store\"\n",
    )
    .expect("write");
    let out = glia(&["overlay", "accept", r, "--candidate", s(&allow)]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("kind allow waives"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&file).expect("read"), written);

    // try takes a rule candidate.
    let out = glia(&["overlay", "try", r, "--candidate", c]);
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with(&format!("[overlay] candidate file={c} "))
                && l.ends_with("entrypoints=0 constraint=1) gap_links=0 errors=0")),
        "{stderr}"
    );
    std::fs::remove_dir_all(root).ok();
}

/// DB.9b's Python tree (engine/tests/overlay_loop_rules.rs) under
/// `<root>/py`: a PROJECT at the root and at web/, store/, scripts/;
/// `web::app::show` calls `store::db::load`, and the overlay forbids
/// scripts -> store (`scripts-no-store`).
fn py_rule_tree(root: &Path) -> PathBuf {
    let repo = root.join("py");
    let files: [(&str, &str); 9] = [
        ("pyproject.toml", "[project]\nname = \"py\"\n"),
        ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
        ("store/pyproject.toml", "[project]\nname = \"store\"\n"),
        ("scripts/pyproject.toml", "[project]\nname = \"scripts\"\n"),
        (
            "web/app.py",
            "from store.db import load\n\n\ndef show(k):\n    return load(k)\n",
        ),
        ("web/view.py", "def render(x):\n    return x\n"),
        (
            "store/db.py",
            "def load(k):\n    return k\n\n\ndef save(k, v):\n    return v\n",
        ),
        ("scripts/tool.py", "def run():\n    return 1\n"),
        (
            ".glia/overlay.toml",
            "version = 1\n\n[[constraint]]\nid = \"scripts-no-store\"\nkind = \"forbid_edge\"\nfrom = \"scripts\"\nto = \"store\"\norigin = \"human\"\n",
        ),
    ];
    for (rel, body) in files {
        let p = repo.join(rel);
        std::fs::create_dir_all(p.parent().expect("dir")).expect("mkdir");
        std::fs::write(p, body).expect("write");
    }
    repo
}

/// `<root>/rules7.toml`, DB.9b's seven-stanza candidate: `constraint#1`
/// holds, `#2` is violated by `web::app::show -> store::db::load`, `#3`
/// holds on an empty scope (`webs`), `#4` does not evaluate, `#5` holds,
/// `#6` binds no node (a stale `anchor`: without it, `from = "webs"` hangs
/// on the root PROJECT, DT.7a, and is a check error), and `edge#1` adds a
/// CALLS that breaks
/// `scripts-no-store`. Version-less.
fn seven_stanza_candidate(root: &Path) -> PathBuf {
    let path = root.join("rules7.toml");
    let text = r#"[[constraint]]
id = "web-no-save"
kind = "invariant"
text = "web never saves"
rule = "violation(a) :- edge(a, b, \"CALLS\"), in_scope(a, \"web\"), qname(b, \"store::db::save\")."

[[constraint]]
id = "web-no-store-calls"
kind = "invariant"
text = "web calls nothing in store"
rule = "violation(a, b) :- edge(a, b, \"CALLS\"), in_scope(a, \"web\"), in_scope(b, \"store\")."

[[constraint]]
id = "webs-no-store"
kind = "invariant"
text = "webs calls nothing in store"
rule = "violation(a, b) :- edge(a, b, \"CALLS\"), in_scope(a, \"webs\"), in_scope(b, \"store\")."

[[constraint]]
id = "rotten"
kind = "invariant"
text = "no routee"
rule = "violation(r) :- node(r, \"ROUTEE\")."

[[constraint]]
id = "view-no-store"
kind = "forbid_edge"
from = "web/view.py"
to = "store"
anchor = "web::view::render"

[[constraint]]
id = "orphan"
kind = "forbid_edge"
from = "webs"
to = "store"
anchor = "webs::gone::nothing"

[[edge]]
from = "scripts::tool::run"
to = "store::db::save"
category = "CALLS"
"#;
    std::fs::write(&path, text).expect("write");
    path
}

/// DB.9c: the text form of `try` shows the rule part: a `rules` column, the
/// per-stanza rule block with located hits, the `rules:` totals line and the
/// verdict split into its graph and rule parts; `--no-leave-one-out` keeps
/// the totals line. Pre-fix baseline (HEAD c675fe47): the 6-column table and
/// `verdict: review - the graph improved, but a gap category rose too`.
#[test]
fn try_shows_the_rule_part() {
    let root = tmp("rule-part");
    let repo = py_rule_tree(&root);
    let cand = seven_stanza_candidate(&root);
    let (r, c) = (s(&repo), s(&cand));
    let before = std::fs::read_to_string(overlay_file(&repo)).expect("read");

    let out = glia(&["overlay", "try", r, "--candidate", c]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    for want in [
        "| stanza | verdict | closes | nodes | edges | gaps | rules |\n|---|---|---|---|---|---|---|\n",
        "| constraint#1 | keep | — | — | — | — | holds |",
        "| constraint#2 | review | — | — | — | — | 1 violation(s) (fact 1; 1 blocking) |",
        "| constraint#3 | review | — | — | — | — | holds, empty scope webs |",
        "| constraint#4 | drop | — | — | — | — | error |",
        "| constraint#6 | drop | — | — | — | — | orphaned |",
        "| edge#1 | review | — | — | CALLS +1 | dead_symbol -1 | +1 -0 (0 blocking) |",
        "\n\nrules (glia check on the build with every stanza):\n- constraint#1 `web-no-save`: keep - holds on the code as it is\n",
        "- constraint#2 `web-no-store-calls`: review - flags 1 violation(s) in the code as it is (1 blocking: `glia check` exits 1 on them)\n    + rule `web::app::show, store::db::load` - fact at web/app.py:4\n",
        "- constraint#3 `webs-no-store`: review - holds, but scope `webs` holds no node: it may pass vacuously\n",
        "- constraint#4 `rotten`: drop - does not evaluate, `glia check` would exit 2: rotten (",
        "- constraint#6 `orphan`: drop - binds no node",
        "- edge#1: review - moves declared-rule violations\n    + forbid_edge `scripts-no-store` `scripts::tool::run -[CALLS]-> store::db::save` - heuristic, advisory at .glia/overlay.toml:",
        "\nbuilds: 3\nrules: holds 3, violated 1, error 1, orphaned 1; violations +2 -0 (1 blocking); errors: rotten\n",
        "verdict: review - graph part review (a declared rule's violations moved), rule part review\n",
        "--only constraint#1 --only constraint#5",
        "review before accepting: constraint#2, constraint#3, edge#1",
    ] {
        assert!(stdout.contains(want), "missing {want:?} in:\n{stdout}");
    }
    let rotten = stdout
        .lines()
        .find(|l| l.starts_with("- constraint#4 "))
        .expect("the rotten rule's line");
    assert!(rotten.contains("unknown node kind `ROUTEE`"), "{rotten}");
    assert!(
        !stdout.contains("a gap category rose too"),
        "a rule-driven review reads as a rule move: {stdout}"
    );
    assert!(
        stderr.contains(&format!(
            "[overlay] try-rules repo={r} rules=6 (holds=3 violated=1 error=1 orphaned=1 duplicate=0) empty_scopes=1 violations +2 -0 blocking=1 errors=1 surface=cli"
        )),
        "{stderr}"
    );

    // --no-leave-one-out: no stanza rows, and the totals line still names
    // every rule status.
    let out = glia(&["overlay", "try", r, "--candidate", c, "--no-leave-one-out"]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        stdout.contains("per-stanza attribution skipped (--no-leave-one-out)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "rules: holds 3, violated 1, error 1, orphaned 1; violations +2 -0 (1 blocking); errors: rotten\n"
        ),
        "{stdout}"
    );
    assert!(!stdout.contains("| stanza |"), "{stdout}");
    assert!(!stdout.contains("rules (glia check"), "{stdout}");
    assert!(
        stdout.contains(
            "verdict: review - graph part review (a declared rule's violations moved), rule part review"
        ),
        "{stdout}"
    );

    // --json: the rule part as the engine reports it.
    let out = glia(&["overlay", "try", r, "--candidate", c, "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let t = json(&out);
    assert_eq!(t["rules"]["rule_verdict"], "review", "{t}");
    assert_eq!(t["rules"]["graph_verdict"], "review", "{t}");

    // A rule-only candidate: the graph part is a drop, the verdict its rule
    // part.
    let one = root.join("one.toml");
    std::fs::write(
        &one,
        "[[constraint]]\nid = \"no-routes\"\nkind = \"invariant\"\nanchor = \"web::view::render\"\ntext = \"t\"\nrule = 'violation(r) :- node(r, \"ROUTE\").'\n",
    )
    .expect("write");
    let out = glia(&["overlay", "try", r, "--candidate", s(&one)]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        stdout.contains(
            "verdict: keep - graph part drop (no gap category fell and the graph did not grow), rule part keep\n"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("\nrules: holds 1; violations +0 -0 (0 blocking)\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("| constraint#1 | keep | — | — | — | — | holds |"),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(overlay_file(&repo)).expect("read"),
        before,
        "try writes no overlay"
    );
    std::fs::remove_dir_all(root).ok();
}
