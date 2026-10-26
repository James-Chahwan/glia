//! FB.12b: scope-level conventions (FB.12a's CONV entries of kind
//! `convention`) in the answers an agent works from. `pack` lists the
//! conventions governing its packed nodes' files as a `## conventions`
//! section and in `Pack::conventions`; `review` lists those governing the
//! changed files as `Review::conventions` and a `### Conventions` markdown
//! table, never blocking. A file is governed when it lies under one of a
//! convention's `paths` (`/`-boundary prefix, `.` every file); the longest
//! matching path sorts first, then `(source, id)`. An overlay convention is
//! tier `declared`, a cell-API one `heuristic`.
//!
//! fired_on: the `[pack] query=..` line's ` conventions=<n>` field and the
//! `[review] conventions=<n>` line (only when one is listed).

use crate::git_fixture;

use git_fixture::GitRepo;
use glia_engine::generate_one;
use glia_engine::pack::{Pack, PackArgs, pack};
use glia_engine::review::{MarkdownOptions, ReviewArgs, render_markdown, review_vs_rev};

const PACKAGE: &str = "{\"name\": \"shop\"}\n";
const CONFIRM: &str =
    "export function confirmDialog(msg: string): boolean {\n  return msg.length > 0;\n}\n";
const CONFIRM_EDITED: &str =
    "export function confirmDialog(msg: string): boolean {\n  return msg.length > 1;\n}\n";
const LEGACY: &str = "export function legacyPopup(n: number): number {\n  return n + 1;\n}\n";
const UTIL: &str = "export function padLeft(s: string): string {\n  return ' ' + s;\n}\n";
const OVERLAY: &str = "version = 1

[[convention]]
id = \"modal-centre\"
scope = \"src/modals\"
text = \"modals are centred\"
by = \"design\"

[[convention]]
id = \"legacy-frozen\"
scope = \"src/modals/legacy\"
text = \"legacy modals are frozen: fix, never extend\"
";

/// The tree: a root package, `src/modals/confirm.ts`,
/// `src/modals/legacy/old.ts`, `src/other/util.ts`, the two overlay
/// conventions, and `cells` (when given) as `.glia/cells.jsonl`.
fn tree(dir: &std::path::Path, cells: Option<&str>) {
    let files = [
        ("package.json", PACKAGE),
        ("src/modals/confirm.ts", CONFIRM),
        ("src/modals/legacy/old.ts", LEGACY),
        ("src/other/util.ts", UTIL),
        (".glia/overlay.toml", OVERLAY),
    ];
    for (rel, text) in files {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    if let Some(c) = cells {
        std::fs::write(dir.join(".glia/cells.jsonl"), c).unwrap();
    }
}

/// The pack of `query` with only its seed packed (so which files are
/// packed is exact).
fn pack_seed(dir: &std::path::Path, query: &str) -> Pack {
    let r = generate_one(dir.to_str().unwrap()).unwrap();
    let mut args = PackArgs::default();
    args.candidates = 1;
    pack(&r.merged, query, &args)
}

fn ids(p: &Pack) -> Vec<(&str, &str, &str, usize)> {
    p.conventions
        .iter()
        .map(|c| (c.source.as_str(), c.id.as_str(), c.tier, c.files))
        .collect()
}

#[test]
fn pack_lists_the_conventions_of_its_files() {
    let tmp = tempfile::tempdir().unwrap();
    tree(tmp.path(), None);
    let p = pack_seed(tmp.path(), "confirmDialog");
    assert_eq!(
        p.nodes
            .iter()
            .map(|n| n.file.as_deref())
            .collect::<Vec<_>>(),
        [Some("src/modals/confirm.ts")]
    );
    assert_eq!(ids(&p), [("overlay", "modal-centre", "declared", 1)]);
    assert_eq!(p.conventions[0].scope, "src/modals");
    assert_eq!(
        p.conventions[0].decl.as_deref(),
        Some(".glia/overlay.toml:3")
    );
    assert!(
        p.text.ends_with(
            "\n## conventions\n- modals are centred (scope src/modals, overlay/modal-centre)\n"
        ),
        "{}",
        p.text
    );
    assert!(p.used_tokens <= p.budget_tokens);
    assert_eq!(p.bytes, p.text.len());
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(
        json["conventions"][0]["text"], "modals are centred",
        "{json}"
    );
}

#[test]
fn a_pack_elsewhere_has_no_section_and_no_key() {
    let tmp = tempfile::tempdir().unwrap();
    tree(tmp.path(), None);
    let p = pack_seed(tmp.path(), "padLeft");
    assert_eq!(
        p.nodes
            .iter()
            .map(|n| n.file.as_deref())
            .collect::<Vec<_>>(),
        [Some("src/other/util.ts")]
    );
    assert!(p.conventions.is_empty());
    assert!(!p.text.contains("## conventions"), "{}", p.text);
    let json = serde_json::to_value(&p).unwrap();
    assert!(json.get("conventions").is_none(), "{json}");
}

/// A nested convention sorts before its parent's for a file under both.
#[test]
fn the_nested_convention_sorts_first() {
    let tmp = tempfile::tempdir().unwrap();
    tree(tmp.path(), None);
    let p = pack_seed(tmp.path(), "legacyPopup");
    assert_eq!(
        ids(&p),
        [
            ("overlay", "legacy-frozen", "declared", 1),
            ("overlay", "modal-centre", "declared", 1)
        ]
    );
    assert!(
        p.text.ends_with(
            "## conventions\n- legacy modals are frozen: fix, never extend (scope src/modals/legacy, overlay/legacy-frozen)\n- modals are centred (scope src/modals, overlay/modal-centre)\n"
        ),
        "{}",
        p.text
    );
}

/// Every convention a default pack lists governs one of its packed files,
/// and its text is part of the measured budget.
#[test]
fn a_default_pack_lists_only_conventions_of_packed_files() {
    let tmp = tempfile::tempdir().unwrap();
    tree(tmp.path(), None);
    let r = generate_one(tmp.path().to_str().unwrap()).unwrap();
    let p = pack(&r.merged, "confirmDialog", &PackArgs::default());
    let files: Vec<&str> = p.nodes.iter().filter_map(|n| n.file.as_deref()).collect();
    assert!(!p.conventions.is_empty());
    for c in &p.conventions {
        let under = files
            .iter()
            .filter(|f| f.starts_with(&format!("{}/", c.scope)))
            .count();
        assert!(under >= 1 && c.files >= 1, "{c:?} over {files:?}");
    }
    assert!(p.used_tokens <= p.budget_tokens);
}

/// A cell-API convention (Engram's write path) with only a `scope` is
/// tier heuristic; `.` governs every file.
#[test]
fn an_api_convention_is_heuristic() {
    let cells = r#"{"qname":"project:.","cell":"CONV","entry":{"source":"api","id":"engram:7","kind":"convention","scope":".","text":"every export is typed"}}"#;
    let tmp = tempfile::tempdir().unwrap();
    tree(tmp.path(), Some(cells));
    let p = pack_seed(tmp.path(), "padLeft");
    assert_eq!(ids(&p), [("api", "engram:7", "heuristic", 1)]);
    assert!(
        p.text
            .ends_with("## conventions\n- every export is typed (scope ., api/engram:7)\n"),
        "{}",
        p.text
    );
    let p = pack_seed(tmp.path(), "confirmDialog");
    assert_eq!(
        ids(&p),
        [
            ("overlay", "modal-centre", "declared", 1),
            ("api", "engram:7", "heuristic", 1)
        ]
    );
}

#[test]
fn review_lists_the_conventions_of_the_changed_files() {
    let repo = GitRepo::init();
    tree(repo.root(), None);
    repo.commit("base");
    repo.write("src/modals/confirm.ts", CONFIRM_EDITED);
    let r = review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review");
    assert!(!r.blocking);
    let rows: Vec<(&str, &str, &[String])> = r
        .conventions
        .iter()
        .map(|c| (c.id.as_str(), c.tier, c.files.as_slice()))
        .collect();
    assert_eq!(
        rows,
        [(
            "modal-centre",
            "declared",
            &["src/modals/confirm.ts".to_string()][..]
        )]
    );
    assert_eq!(r.conventions[0].by.as_deref(), Some("design"));
    let md = render_markdown(&r, &MarkdownOptions::default());
    assert!(
        md.contains(
            "### Conventions\n\n| convention | scope | files changed | source |\n|---|---|---|---|\n| modals are centred | `src/modals` | 1 | overlay/modal-centre (declared) at .glia/overlay.toml:3 |"
        ),
        "{md}"
    );
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["conventions"][0]["id"], "modal-centre");
}

#[test]
fn a_review_outside_every_scope_has_no_conventions() {
    let repo = GitRepo::init();
    tree(repo.root(), None);
    repo.commit("base");
    repo.write("src/other/util.ts", &UTIL.replace("' '", "'  '"));
    let r = review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review");
    assert!(r.conventions.is_empty());
    let md = render_markdown(&r, &MarkdownOptions::default());
    assert!(!md.contains("### Conventions"), "{md}");
    let json = serde_json::to_value(&r).unwrap();
    assert!(json.get("conventions").is_none());
}

/// A removed file is a changed file too, governed at its base path; a moved
/// one is read at the path its changed rows carry (diff_impact's: the new
/// one), so a move out of a scope lists none of that scope's conventions.
#[test]
fn a_removed_or_moved_file_keeps_its_conventions() {
    let repo = GitRepo::init();
    tree(repo.root(), None);
    repo.commit("base");
    repo.remove("src/modals/legacy/old.ts");
    repo.git_mv("src/modals/confirm.ts", "src/other/confirm.ts");
    let r = review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review");
    let rows: Vec<(&str, Vec<&str>)> = r
        .conventions
        .iter()
        .map(|c| (c.id.as_str(), c.files.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        rows,
        [
            ("legacy-frozen", vec!["src/modals/legacy/old.ts"]),
            ("modal-centre", vec!["src/modals/legacy/old.ts"]),
        ]
    );
    assert!(!r.blocking);
    assert!(
        r.changed
            .iter()
            .any(|n| n.file.as_deref() == Some("src/other/confirm.ts")),
        "{:?}",
        r.changed
    );
}
