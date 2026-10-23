//! EB.5a: `glia_engine::intent::defs::read_defs`, id definitions read at one
//! revision from the docs a build ingests, `[ids] markdown` globs, JSON
//! trackers and the Tier-4 docs snapshot.

use std::path::Path;
use std::process::Command;

use glia_code_domain::glia_config::parse_str;
use glia_code_domain::{DocProvenance, DocRecord, DocSourceKind};
use glia_core::RepoId;
use glia_engine::intent::defs::{read_defs, DefRow, IdDefs, RowState, Tree};
use glia_engine::intent::ids::IdVocab;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

const TASKS: &str = "# Tasks: billing

## Phase 1

- [ ] T012 Add invoice export in src/billing/export.rs
  also touches `src/billing/{model,view}.rs`
- [x] T013 Wire the export route
- [ ] T015 (was T009) split from T010

```text
- [ ] T099 not a task
```

## T014: retry
";

const REQS: &str = "# Requirements

| ID | What | Where |
|----|------|-------|
| FR-001 | Users export invoices | src/billing |
| FR-001 rollout | later | - |
| **FR-002** | Admins void invoices | src/admin |
";

const TRACKER: &str = r#"{"items": [
  {"id": "OPS-7", "title": "rotate keys", "files": ["src/a.rs (new)"], "status": "done"},
  {"id": "not an id", "files": "x"},
  {"id": "OPS-8", "files": "src/b.rs", "status": "open"}
]}"#;

/// The fixture repo, committed; returns its HEAD sha.
fn fixture(d: &Path) -> String {
    git(d, &["init", "-q", "-b", "main"]);
    put(d, "specs/003-billing/tasks.md", TASKS);
    put(d, "docs/requirements.md", REQS);
    put(d, "README.md", "# Shop\n\n- [ ] OPS-1: readme task in `src/shop.rs`\n");
    put(d, "src/notes.md", "- [ ] T050 outside the doc rules\n");
    put(d, "dev-notes/plan.md", "- [ ] T060 a planned task in src/plan.rs\n");
    put(d, "ops/tasks.json", TRACKER);
    put(d, "ops/broken.json", "{not json");
    put(d, "src/billing/export.rs", "pub fn export() {}\n");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "init"]);
    git(d, &["rev-parse", "HEAD"])
}

fn vocab(markdown: bool) -> IdVocab {
    let md = if markdown { "markdown = [\"dev-notes/plan.md\"]\n" } else { "" };
    let text = format!(
        "version = 1\n[ids]\n{md}[[ids.tracker]]\npath = \"ops/*.json\"\nlist = \"items\"\nfiles = \"files\"\nstate = \"status\"\ndone = [\"done\"]\n"
    );
    let loaded = parse_str(&text);
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    IdVocab::from_config(Some(&loaded))
}

fn row<'a>(defs: &'a IdDefs, id: &str) -> &'a DefRow {
    defs.rows.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no row for {id}: {:#?}", defs.rows))
}

#[test]
fn reads_task_rows_tables_headings_and_trackers() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let defs = read_defs(&Tree::Work(d), &[], &vocab(false), None);
    defs.report("fixture");
    assert_eq!(defs.tree_error, None);

    let ids: Vec<(&str, &str, i64)> = defs.rows.iter().map(|r| (r.page.as_str(), r.id.as_str(), r.line)).collect();
    assert_eq!(
        ids,
        [
            ("README.md", "OPS-1", 3),
            ("docs/requirements.md", "FR-001", 5),
            ("docs/requirements.md", "FR-002", 7),
            ("ops/tasks.json", "OPS-7", 2),
            ("ops/tasks.json", "OPS-8", 4),
            ("specs/003-billing/tasks.md", "T012", 5),
            ("specs/003-billing/tasks.md", "T013", 7),
            ("specs/003-billing/tasks.md", "T015", 8),
            ("specs/003-billing/tasks.md", "T014", 14),
        ]
    );

    let t12 = row(&defs, "T012");
    assert_eq!(t12.state, RowState::Pending);
    assert_eq!(t12.family, "T");
    assert_eq!(t12.scope.as_deref(), Some("specs/003-billing"));
    assert!(t12.names.iter().any(|n| n == "src/billing/export.rs"), "{:?}", t12.names);
    assert!(t12.names.iter().any(|n| n == "src/billing/{model,view}.rs"), "the block's indented line: {:?}", t12.names);
    assert_eq!(t12.head, "- [ ]   Add invoice export in src/billing/export.rs");
    assert_eq!(t12.heading_words, "tasks.md phase 1 tasks: billing (top) ");
    assert_eq!(t12.nearest_heading, "phase 1");
    assert_eq!(row(&defs, "T013").state, RowState::Done);
    // rank_eval's subject rule: `split from` takes the id just before it.
    assert_eq!(
        row(&defs, "T015").lineage,
        [("T015".to_string(), "T009".to_string(), "was"), ("T009".to_string(), "T010".to_string(), "split-from")]
    );
    assert_eq!(row(&defs, "T014").state, RowState::None);
    assert_eq!(row(&defs, "T014").nearest_heading, "t014: retry");
    assert!(defs.rows.iter().all(|r| r.id != "T099"), "a fenced task defines nothing");
    assert!(defs.rows.iter().all(|r| r.id != "T050"), "src/notes.md is outside the doc rules");
    assert!(defs.rows.iter().all(|r| r.id != "T060"), "dev-notes/ is read only through [ids] markdown");
    assert_eq!(defs.label_rows, 1, "`| FR-001 rollout |` is a label row");
    assert_eq!(row(&defs, "FR-001").scope, None);

    let ops7 = row(&defs, "OPS-7");
    assert_eq!((ops7.state, ops7.head.as_str()), (RowState::Done, "rotate keys"));
    assert_eq!(ops7.names, ["src/a.rs (new)"]);
    assert_eq!(row(&defs, "OPS-8").state, RowState::Pending);
    assert_eq!(row(&defs, "OPS-8").names, ["src/b.rs"]);
    assert_eq!((defs.tracker_rows, defs.trackers_unreadable), (2, 1));
    assert_eq!((defs.pending(), defs.done(), defs.lineage()), (4, 2, 2));
    assert_eq!(defs.pages, 3);
    assert_eq!(defs.rev, "worktree");
}

#[test]
fn rev_and_work_trees_read_the_same_definitions() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    let sha = fixture(d);
    let v = vocab(true);
    let work = read_defs(&Tree::Work(d), &[], &v, None);
    let rev = read_defs(&Tree::Rev { repo: d, sha: sha.clone() }, &[], &v, None);
    rev.report("fixture");
    assert_eq!(work.rows, rev.rows);
    assert_eq!(
        (work.label_rows, work.tracker_rows, work.trackers_unreadable, work.pages),
        (rev.label_rows, rev.tracker_rows, rev.trackers_unreadable, rev.pages)
    );
    assert_eq!(rev.rev, sha[..12]);
    // The [ids] markdown glob admits a file the doc include rule does not.
    let t60 = row(&rev, "T060");
    assert_eq!((t60.page.as_str(), t60.state), ("dev-notes/plan.md", RowState::Pending));
    assert!(rev.rows.iter().all(|r| r.id != "T050"));

    // A later working-tree edit is not in the rev's tree.
    put(d, "docs/requirements.md", &format!("{REQS}| FR-003 | new | src/x |\n"));
    let work = read_defs(&Tree::Work(d), &[], &v, None);
    let rev = read_defs(&Tree::Rev { repo: d, sha }, &[], &v, None);
    assert!(work.rows.iter().any(|r| r.id == "FR-003"));
    assert!(rev.rows.iter().all(|r| r.id != "FR-003"));
}

#[test]
fn an_unknown_rev_is_a_tree_error_not_a_panic() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let defs = read_defs(&Tree::Rev { repo: d, sha: "0".repeat(40) }, &[], &vocab(false), None);
    assert!(defs.tree_error.is_some());
    assert!(defs.rows.is_empty());
}

#[test]
fn project_root_specs_are_scoped_under_their_root() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    git(d, &["init", "-q", "-b", "main"]);
    put(d, "web/specs/004-cart/tasks.md", "- [ ] T001 cart in web/src/cart.ts\n");
    put(d, "web/package.json", "{\"name\":\"web\"}\n");
    let defs = read_defs(&Tree::Work(d), &["web"], &IdVocab::default(), None);
    assert_eq!(row(&defs, "T001").scope.as_deref(), Some("web/specs/004-cart"));
    let none = read_defs(&Tree::Work(d), &[], &IdVocab::default(), None);
    assert!(none.rows.is_empty(), "without the PROJECT root the doc is not ingested");
}

#[test]
fn snapshot_sections_define_ids() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    git(d, &["init", "-q", "-b", "main"]);
    put(d, "app.py", "def place(o):\n    return o\n");
    let record = DocRecord {
        rel_path: "confluence/SHOP/Plan.md".to_string(),
        text: "# Plan\n\n## Work\n\n- [ ] PAY-12: price orders in app.py\n".to_string(),
        provenance: DocProvenance {
            kind: DocSourceKind::Confluence,
            url: None,
            container: Some("SHOP".to_string()),
            version: Some("1".to_string()),
        },
    };
    let line = serde_json::to_string(&record).expect("serialise");
    put(d, ".glia/docs-snapshot/manifest.jsonl", &format!("{line}\n"));
    let built = glia_engine::generate_one(d.to_str().expect("utf-8")).expect("build");
    let repo = RepoId(*built.repo_labels.keys().next().expect("one repo"));
    let defs = read_defs(&Tree::Work(d), &[], &IdVocab::default(), Some((&built.merged, repo)));
    defs.report("snapshot");
    assert_eq!(defs.snapshot_sections, 2);
    let r = row(&defs, "PAY-12");
    assert_eq!((r.page.as_str(), r.line), ("confluence/SHOP/Plan.md", 5));
    assert_eq!(r.heading_words, "plan.md work plan (top) ");
    assert!(r.names.iter().any(|n| n == "app.py"));
}
