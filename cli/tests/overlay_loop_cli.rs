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
            "[overlay] candidate file={c} stanzas=2 (route_prefix=0 wrapper=1 edge=1 constants=0 entrypoints=0) gap_links=0 errors=0"
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
            "[overlay] accept repo={r} added=1 (route_prefix=0 wrapper=1 edge=0 constants=0 entrypoints=0) removed=0 duplicates=0 file=.glia/overlay.toml dry_run=false surface=cli"
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
