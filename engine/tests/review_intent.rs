//! EB.7 — `glia review --title`: the review's intent section and the impact
//! rows ordered typed-links-first (LX[IDR, the existing order]).
//!
//! The tree: a Cargo crate whose `src/a.rs` `run` calls `src/b.rs` `bill`,
//! `src/c.rs` `other` and `src/d.rs` `misc`, and a spec-kit task list
//! `specs/001-billing/tasks.md` whose pending task `T012` names `src/b.rs`,
//! committed through the LE.1b git fixture; the working tree edits `run`.
//! A history snapshot (code-domain `write_history_with_messages`) holds past
//! commit messages for the BM25 text column.
//!
//! What is pinned:
//! (1) a title naming T012 routes to `src/b.rs` (idr 1.0, first in
//!     `intent.files`), and the impact rows located in `src/b.rs` move first,
//!     the rest keeping their order without a title;
//! (1b) a title whose words match an old subject but name no id reorders
//!     nothing (text never moves an impact row) and fills the text column;
//! (2) without a history snapshot the text is `absent` and T012 still routes;
//! (3) no title and an empty base..HEAD: no `intent`, the JSON identical to
//!     `review` over the same delta (the 0.5.2 shape);
//! (4) a commit in base..HEAD naming T012 is the default title (source
//!     `commit`), and its own sha is left out of the text index.
//!
//! (5) IDR reads the BASE tree: a task list the working tree moves and
//!     deletes still routes, defined at its base path.
//!
//! (1c), the three fusions' arithmetic, is a unit test in
//! `engine/src/review.rs` (`IntentSignals` is crate API).
//!
//! (6) EC.11: the intent fusion is set SELt2's calibrated pick (a record
//!     written by `calibrate::calibrate` over EC.5's two-cluster fixture with
//!     commit messages); with no record it is LX[IDR,P1], `selection_intent`
//!     saying which. Every fixture above has no record, so (1)-(5) pin EB.7's
//!     uncalibrated order unchanged.

mod git_fixture;

use git_fixture::GitRepo;
use glia_code_domain::snapshots::{
    HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, write_history_with_messages,
};
use glia_engine::calibrate::{CalibrateOptions, CalibrationRead, calibrate, read_calibration};
use glia_engine::delta::graph_delta_vs_rev;
use glia_engine::review::{
    INTENT_FUSION_REVIEW, MarkdownOptions, Review, ReviewArgs, render_markdown, review,
    review_vs_rev,
};

const CARGO: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub mod a;\npub mod b;\npub mod c;\npub mod d;\n";
const A: &str = "use crate::b::bill;\nuse crate::c::other;\nuse crate::d::misc;\n\npub fn run() {\n    other();\n    misc();\n    bill();\n}\n";
const A_EDITED: &str = "use crate::b::bill;\nuse crate::c::other;\nuse crate::d::misc;\n\npub fn run() {\n    other();\n    misc();\n    bill();\n    other();\n}\n";
const B: &str = "pub fn bill() {}\n\npub fn invoice() {\n    bill();\n}\n";
const C: &str = "pub fn other() {}\n\npub fn helper() {\n    other();\n}\n";
const D: &str = "pub fn misc() {}\n";
const TASKS: &str =
    "# Tasks\n\n- [ ] T012 billing export in src/b.rs\n- [x] T013 old work in src/d.rs\n";

/// The clean tree, committed on `main`, then `run` edited in the working tree.
fn fixture() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("Cargo.toml", CARGO);
    repo.write("src/lib.rs", LIB);
    repo.write("src/a.rs", A);
    repo.write("src/b.rs", B);
    repo.write("src/c.rs", C);
    repo.write("src/d.rs", D);
    repo.write("specs/001-billing/tasks.md", TASKS);
    repo.commit("clean");
    repo
}

fn sha(n: usize) -> String {
    format!("{n:02}{}", "0".repeat(38))
}

/// One snapshot row: (commit, message) over `files`.
fn row(c: String, t: i64, files: &[&str], subject: &str) -> (HistoryCommit, HistoryMessage) {
    let hc = HistoryCommit {
        c: c.clone(),
        t,
        files: files
            .iter()
            .map(|p| HistoryFile {
                p: p.to_string(),
                a: Some(1),
                d: Some(0),
                from: None,
            })
            .collect(),
    };
    let m = HistoryMessage {
        c,
        at: t,
        s: subject.to_string(),
        b: String::new(),
        br: None,
    };
    (hc, m)
}

/// Write the history snapshot into the working tree's `.glia/`, newest first.
fn snapshot(repo: &GitRepo, rows: Vec<(HistoryCommit, HistoryMessage)>) {
    let (commits, messages): (Vec<HistoryCommit>, Vec<HistoryMessage>) = rows.into_iter().unzip();
    let meta = HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new());
    write_history_with_messages(repo.root(), meta, &commits, &[], Some(&messages))
        .expect("snapshot");
}

/// The past subjects: one about invoices over src/c.rs + src/d.rs, and one
/// that co-changes src/a.rs with src/c.rs (so co-change, the default
/// ranking, puts c.rs's rows ahead of b.rs's without a title).
fn old_rows() -> Vec<(HistoryCommit, HistoryMessage)> {
    vec![
        row(
            sha(2),
            1_767_225_602,
            &["src/c.rs", "src/d.rs"],
            "invoice ledger rounding",
        ),
        row(
            sha(1),
            1_767_225_601,
            &["src/a.rs", "src/c.rs"],
            "initial shop skeleton",
        ),
    ]
}

fn with_title(t: Option<&str>) -> ReviewArgs {
    let mut a = ReviewArgs::default();
    a.title = t.map(str::to_string);
    a
}

fn rows(r: &Review) -> Vec<(String, Option<String>)> {
    r.impact
        .results
        .iter()
        .map(|b| (b.qname.clone(), b.file.clone()))
        .collect()
}

/// `before` with the rows located in `file` moved first, stably.
fn lifted(before: &[(String, Option<String>)], file: &str) -> Vec<(String, Option<String>)> {
    let (mut first, rest): (Vec<_>, Vec<_>) = before
        .iter()
        .cloned()
        .partition(|(_, f)| f.as_deref() == Some(file));
    first.extend(rest);
    first
}

#[test]
fn a_title_id_routes_and_lifts_its_files() {
    let repo = fixture();
    snapshot(&repo, old_rows());
    repo.write("src/a.rs", A_EDITED);

    let plain = review_vs_rev(repo.path(), "HEAD", &with_title(None)).expect("review");
    assert!(
        plain.intent.is_none(),
        "no title, empty base..HEAD: no intent"
    );
    let before = rows(&plain);
    let b_at = before
        .iter()
        .position(|(_, f)| f.as_deref() == Some("src/b.rs"))
        .expect("a b.rs row");
    assert!(
        b_at > 0,
        "the fixture puts a non-b.rs row first: {before:?}"
    );

    let r = review_vs_rev(
        repo.path(),
        "HEAD",
        &with_title(Some("T012 billing export")),
    )
    .expect("review");
    let intent = r.intent.as_ref().expect("intent with a title");
    assert_eq!(
        (intent.title.as_str(), intent.title_source),
        ("T012 billing export", "arg")
    );
    assert_eq!(intent.fusion, INTENT_FUSION_REVIEW);
    assert_eq!(intent.fusion, "LX[IDR,P1]");
    assert_eq!(intent.text, "present");
    let t012 = intent
        .ids
        .iter()
        .find(|i| i.id == "T012")
        .expect("T012 routed");
    assert_eq!(t012.state, Some("pending"));
    let first = intent.files.first().expect("a file");
    assert_eq!(
        (first.file.as_str(), first.idr, first.rank),
        ("src/b.rs", 1.0, 1)
    );
    assert_eq!(first.ids, ["T012"]);
    assert!(first.in_impact);
    assert!(
        intent.files.iter().all(|f| f.file != "src/a.rs"),
        "the changed file is no candidate"
    );
    assert_eq!(
        rows(&r),
        lifted(&before, "src/b.rs"),
        "b.rs rows first, the rest in order"
    );

    let md = render_markdown(&r, &MarkdownOptions::default());
    assert!(
        md.contains("### Intent\n\n_title (arg): T012 billing export_"),
        "{md}"
    );
    assert!(
        md.contains("| id | namespace | tier | route | defined at | state | files |"),
        "{md}"
    );
    assert!(
        md.contains("| # | file | idr | text | ppr | ids | in impact |"),
        "{md}"
    );
    let json = serde_json::to_value(&r).expect("json");
    assert_eq!(json["intent"]["fusion"], "LX[IDR,P1]");
    assert_eq!(json["intent"]["files"][0]["file"], "src/b.rs");
}

#[test]
fn text_never_reorders_impact() {
    let repo = fixture();
    snapshot(&repo, old_rows());
    repo.write("src/a.rs", A_EDITED);
    let plain = review_vs_rev(repo.path(), "HEAD", &with_title(None)).expect("review");
    let r =
        review_vs_rev(repo.path(), "HEAD", &with_title(Some("invoice ledger"))).expect("review");
    let intent = r.intent.as_ref().expect("intent");
    assert!(intent.ids.is_empty(), "{:?}", intent.ids);
    assert_eq!(intent.text, "present");
    let mut scored: Vec<(&str, bool)> = intent
        .files
        .iter()
        .filter(|f| f.text > 0.0)
        .map(|f| (f.file.as_str(), f.idr > 0.0))
        .collect();
    scored.sort_unstable();
    assert_eq!(
        scored,
        [("src/c.rs", false), ("src/d.rs", false)],
        "the old subject's files"
    );
    let plain_json = serde_json::to_value(&plain.impact).expect("json");
    let r_json = serde_json::to_value(&r.impact).expect("json");
    assert_eq!(
        plain_json, r_json,
        "the impact is byte-identical without an id"
    );
    let md = render_markdown(&r, &MarkdownOptions::default());
    assert!(
        !md.contains("| id | namespace |"),
        "no routed-ids table without an id:\n{md}"
    );
}

#[test]
fn no_snapshot_text_absent_ids_still_route() {
    let repo = fixture();
    repo.write("src/a.rs", A_EDITED);
    let r = review_vs_rev(
        repo.path(),
        "HEAD",
        &with_title(Some("T012 billing export")),
    )
    .expect("review");
    let intent = r.intent.as_ref().expect("intent");
    assert_eq!(intent.text, "absent");
    assert!(intent.files.iter().all(|f| f.text == 0.0));
    assert_eq!(intent.files[0].file, "src/b.rs");
    assert_eq!(intent.files[0].idr, 1.0);
    let md = render_markdown(&r, &MarkdownOptions::default());
    assert!(
        md.contains("_(no history messages: run glia history sync)_"),
        "{md}"
    );
}

#[test]
fn no_title_no_commits_is_the_plain_review() {
    let repo = fixture();
    snapshot(&repo, old_rows());
    repo.write("src/a.rs", A_EDITED);
    let r = review_vs_rev(repo.path(), "HEAD", &ReviewArgs::default()).expect("review");
    assert!(r.intent.is_none());
    let rev = graph_delta_vs_rev(repo.path(), "HEAD").expect("delta");
    let plain = review(&rev, &ReviewArgs::default()).expect("review");
    let (a, b) = (
        serde_json::to_string(&r).expect("json"),
        serde_json::to_string(&plain).expect("json"),
    );
    assert_eq!(a, b);
    assert!(!a.contains("\"intent\""));
    assert!(!render_markdown(&r, &MarkdownOptions::default()).contains("### Intent"));
}

#[test]
fn the_newest_commit_subject_is_the_default_title_and_never_its_own_text() {
    let repo = fixture();
    repo.write("src/d.rs", "pub fn misc() {}\n\npub fn zebra() {}\n");
    let pr = repo.commit("T012 zebra stripes");
    let mut rows = vec![row(
        pr.clone(),
        1_767_225_700,
        &["src/d.rs", "src/c.rs"],
        "T012 zebra stripes",
    )];
    rows.extend(old_rows());
    snapshot(&repo, rows);
    repo.write("src/a.rs", A_EDITED);
    let r = review_vs_rev(repo.path(), "HEAD~1", &ReviewArgs::default()).expect("review");
    let intent = r.intent.as_ref().expect("a commit in base..HEAD");
    assert_eq!(
        (intent.title.as_str(), intent.title_source),
        ("T012 zebra stripes", "commit")
    );
    assert_eq!(intent.text, "present");
    assert_eq!(intent.files[0].file, "src/b.rs");
    // its own message would score d.rs and c.rs on `zebra` / `stripes`: excluded
    assert!(
        intent.files.iter().all(|f| f.text == 0.0),
        "{:?}",
        intent.files
    );
}

#[test]
fn idr_reads_the_base_tree() {
    let repo = fixture();
    repo.git_mv("specs/001-billing/tasks.md", "specs/001-billing/done.md");
    repo.remove("specs/001-billing/done.md");
    repo.write("src/a.rs", A_EDITED);
    let r = review_vs_rev(repo.path(), "HEAD", &with_title(Some("T012"))).expect("review");
    let intent = r.intent.as_ref().expect("intent");
    let t012 = intent
        .ids
        .iter()
        .find(|i| i.id == "T012")
        .expect("T012 routed at the base");
    assert_eq!(
        t012.defined_at,
        Some(("specs/001-billing/tasks.md".to_string(), 3))
    );
    assert_eq!(
        (intent.files[0].file.as_str(), intent.files[0].idr),
        ("src/b.rs", 1.0)
    );
}

// ---- (6) EC.11: EC.5's two-cluster fixture (engine/tests/calibrate_step.rs) ----

/// Files per cluster.
const N: usize = 12;
/// Commits in the synthetic history.
const COMMITS: usize = 60;
/// Hub functions that call `h`.
const FILLERS: usize = 1000;

fn cluster_path(c: &str, i: usize) -> String {
    format!("{c}/{c}{i:02}.py")
}

/// Two clusters of `N` Python modules (`alpha/`, `zeta/`) around one hub.
fn clusters(repo: &GitRepo) {
    let mut hub = String::from("def h():\n    return 0\n\n");
    for i in 0..N {
        hub.push_str(&format!("from alpha.alpha{i:02} import alpha{i:02}_f\n"));
    }
    hub.push_str("\ndef r2():\n");
    for i in 0..N {
        hub.push_str(&format!("    alpha{i:02}_f()\n"));
    }
    hub.push_str("    return 0\n\n");
    for i in 0..N {
        hub.push_str(&format!("def g{i:02}():\n    return r2()\n\n"));
    }
    for i in 0..FILLERS {
        hub.push_str(&format!("def p{i:04}():\n    return h()\n\n"));
    }
    repo.write("hub.py", &hub);
    for c in ["alpha", "zeta"] {
        repo.write(&format!("{c}/__init__.py"), "");
    }
    for i in 0..N {
        repo.write(
            &cluster_path("alpha", i),
            &format!("import hub\n\ndef alpha{i:02}_f():\n    return {i}\n"),
        );
        repo.write(
            &cluster_path("zeta", i),
            &format!("from hub import h, g{i:02}\n\ndef zeta{i:02}_f():\n    h()\n    return g{i:02}()\n"),
        );
    }
}


/// The clusters fixture's history with messages: even commits touch alpha
/// modules, odd ones zeta modules, each subject naming its cluster.
fn text_history(root: &std::path::Path) {
    let mut commits = Vec::new();
    let mut messages = Vec::new();
    for i in 0..COMMITS {
        let c = if i.is_multiple_of(2) { "alpha" } else { "zeta" };
        let size = 2 + (i / 2) % 5;
        let start = (i * 5) % N;
        let sha = format!("{:040x}", i + 1);
        let t = 1_700_000_000 + i as i64 * 3600;
        commits.push(HistoryCommit {
            c: sha.clone(),
            t,
            files: (0..size)
                .map(|j| HistoryFile { p: cluster_path(c, (start + j) % N), a: Some(1), d: Some(0), from: None })
                .collect(),
        });
        messages.push(HistoryMessage { c: sha, at: t, s: format!("{c} cluster work {i}"), b: String::new(), br: None });
    }
    commits.reverse();
    messages.reverse();
    let meta = HistoryMeta::new(format!("{:040x}", COMMITS), COMMITS, None, String::new());
    write_history_with_messages(root, meta, &commits, &[], Some(&messages)).expect("write history");
}

#[test]
fn review_intent_follows_the_selt_pick() {
    let g = GitRepo::init();
    clusters(&g);
    g.commit("clean");
    text_history(g.root());
    let built = glia_engine::generate_one(g.path()).expect("build");
    g.write("alpha/alpha03.py", "import hub\n\ndef alpha03_f():\n    return 33\n");
    let mut args = ReviewArgs::default();
    args.title = Some("alpha cluster work".into());

    // Uncalibrated: EB.7's LX[IDR,P1], the fallback with its reason.
    let u = review_vs_rev(g.path(), "HEAD", &args).expect("review");
    let sel = u.selection_intent.as_ref().expect("selection_intent");
    assert_eq!(
        (sel.set.as_str(), sel.shape.as_str(), sel.pick.as_str(), sel.source, sel.reason),
        ("SELt2", "k1", "LX[IDR,P1]", "fallback", Some("absent"))
    );
    let ui = u.intent.as_ref().expect("intent");
    assert_eq!(ui.fusion, "LX[IDR,P1]");
    let md = render_markdown(&u, &MarkdownOptions::default());
    assert!(md.contains("_title (arg): alpha cluster work_\n\n_ranked by LX[IDR,P1] (uncalibrated: absent)_"), "{md}");

    // Calibrated: the record's SELt2 k1 pick orders the intent files.
    let mut opts = CalibrateOptions::default();
    opts.groups = vec!["review"];
    calibrate(g.root(), &built.merged, &opts).expect("calibrate");
    let CalibrationRead::Loaded(cal) = read_calibration(g.root()) else {
        panic!("record not loaded");
    };
    let want = cal.sets.iter().find(|s| s.set == "SELt2" && s.shape == "k1").map(|s| s.pick.clone()).expect("SELt2 k1");
    let c = review_vs_rev(g.path(), "HEAD", &args).expect("review");
    let sel = c.selection_intent.as_ref().expect("selection_intent");
    assert_eq!(
        (sel.set.as_str(), sel.shape.as_str(), sel.pick.as_str(), sel.source, sel.reason),
        ("SELt2", "k1", want.as_str(), "calibrated", None)
    );
    assert_eq!(sel.test_commits, COMMITS);
    let ci = c.intent.as_ref().expect("intent");
    assert_eq!(ci.fusion, want);
    assert!(!ci.files.is_empty());
    assert!(ci.files.iter().all(|f| f.file != "alpha/alpha03.py"), "the changed file is no candidate");
    // the title's cluster leads the calibrated list
    assert!(ci.files[0].file.starts_with("alpha/"), "{:?}", ci.files);
    let v = serde_json::to_value(&c).expect("json");
    assert_eq!(v["selection_intent"]["source"], "calibrated", "{v}");
    assert_eq!(v["intent"]["fusion"], want.as_str());
    let md = render_markdown(&c, &MarkdownOptions::default());
    let line = format!("_ranked by {} (calibrated on {COMMITS} commits, shape k1)_", want.replace('*', "\\*"));
    assert!(md.contains(&line), "{line}\n{md}");
    // The impact rows: EB.7's re-sort (IDR only), unchanged by the pick.
    assert_eq!(
        serde_json::to_value(&u.impact.results).expect("json"),
        serde_json::to_value(&c.impact.results).expect("json")
    );
}
