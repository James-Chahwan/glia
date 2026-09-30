//! CC.11a: co-change suggestions, pairwise (ROSE). For the files of a change,
//! the files that usually change with them: the CO_CHANGES edges (LF.5b)
//! with the query file at either end, read DIRECTIONALLY — confidence is the
//! pair's co-changes over the query file's own commits (its module churn),
//! not LF.5b's symmetric ratio over the rarer file — plus the support and
//! whether any static link joins the pair (LF.5c's link test).
//!
//! The history snapshot is synthetic (`code_domain::snapshots::write_history`,
//! no git), so every count is exact: svc/admin.py has 45 commits — 3 together
//! with svc/report.py (report's only 3), 20 together with web/page.ts (page's
//! only 20) and 22 alone (a one-file commit counts toward churn, never toward
//! a pair). svc/admin.py imports svc/report.py; nothing links web/page.ts to
//! svc/admin.py.

use std::path::Path;

use glia_code_domain::edge_category;
use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
use glia_engine::cochange::{Cochange, CochangeArgs, cochange};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const ADMIN_PY: &str = "from svc.report import format_report\n\n\ndef admin_summary(rows):\n    return format_report(rows)\n";
const REPORT_PY: &str = "def format_report(rows):\n    return \", \".join(rows)\n";
const PAGE_TS: &str =
    "export function renderPage(title: string): string {\n  return `<h1>${title}</h1>`;\n}\n";
const README: &str = "# demo\n";

const ADMIN: &str = "svc/admin.py";
const REPORT: &str = "svc/report.py";
const PAGE: &str = "web/page.ts";

const T0: i64 = 1_767_225_600;

/// Commits, newest first: `n` commits per group touching every file of it,
/// each with its own sha and time.
fn commits(groups: &[(&[&str], usize)]) -> Vec<HistoryCommit> {
    let mut out = Vec::new();
    let mut i = 0usize;
    for (files, n) in groups {
        for _ in 0..*n {
            i += 1;
            out.push(HistoryCommit {
                c: format!("{i:03}{}", "0".repeat(37)),
                t: T0 + i64::try_from(i).expect("small") * 3_600,
                files: files
                    .iter()
                    .map(|p| HistoryFile {
                        p: (*p).to_string(),
                        a: Some(1),
                        d: Some(0),
                        from: None,
                    })
                    .collect(),
            });
        }
    }
    out.reverse();
    out
}

/// The acceptance history: admin 45 commits (3 with report, 20 with page, 22
/// alone).
fn history() -> Vec<HistoryCommit> {
    commits(&[(&[ADMIN, REPORT], 3), (&[ADMIN, PAGE], 20), (&[ADMIN], 22)])
}

/// A temp tree of the four files, with a history snapshot of `history` when
/// it is `Some`.
fn tree(history: Option<&[HistoryCommit]>) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for (p, text) in [
        (ADMIN, ADMIN_PY),
        (REPORT, REPORT_PY),
        (PAGE, PAGE_TS),
        ("README.md", README),
    ] {
        let path = d.path().join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    if let Some(h) = history {
        let head = h.first().map_or_else(|| "0".repeat(40), |c| c.c.clone());
        write_history(
            d.path(),
            HistoryMeta::new(head, 2000, None, String::new()),
            h,
            &[],
        )
        .expect("write snapshot");
    }
    d
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).expect("build").merged
}

fn q(files: &[&str]) -> Vec<String> {
    files.iter().map(|f| (*f).to_string()).collect()
}

/// Default floors (300 per mille, support 3, top 20), with `min_confidence`.
fn floor(min_confidence_permille: u32) -> CochangeArgs {
    let mut a = CochangeArgs::default();
    a.min_confidence_permille = min_confidence_permille;
    a
}

/// `(file, confidence_permille, link)` per row, in answer order.
fn rows(c: &Cochange) -> Vec<(&str, u32, &str)> {
    c.rows
        .iter()
        .map(|r| (r.file.as_str(), r.confidence_permille, r.link))
        .collect()
}

/// Both pairs are CO_CHANGES edges (LF.5b kept them: each is 1000 per mille of
/// its rarer file), and the defaults are the spec's floors.
#[test]
fn history_and_defaults() {
    let d = tree(Some(&history()));
    let m = build(d.path());
    assert_eq!(
        m.all_edges()
            .filter(|e| e.category == edge_category::CO_CHANGES)
            .count(),
        2
    );
    let a = CochangeArgs::default();
    assert_eq!(
        (
            a.min_confidence_permille,
            a.min_support,
            a.top,
            a.unlinked_only
        ),
        (300, 3, 20, false)
    );
}

/// report -> admin is a strong rule (3 / 3); admin -> report is noise (3 / 45 =
/// 66, under the default floor) while admin -> page (20 / 45 = 444) is listed
/// with no static link.
#[test]
fn direction_matters() {
    let d = tree(Some(&history()));
    let m = build(d.path());

    let r = cochange(&m, &q(&[REPORT]), &CochangeArgs::default());
    assert_eq!(r.query_files, [REPORT]);
    assert!(r.unmapped.is_empty());
    assert!(r.absence.is_none());
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    let row = &r.rows[0];
    assert_eq!(row.file, ADMIN);
    assert_eq!(row.module_qname, "svc::admin");
    assert_eq!(row.antecedent, [REPORT]);
    assert_eq!(
        (row.support, row.antecedent_commits, row.confidence_permille),
        (3, 3, 1000)
    );
    assert_eq!(
        (row.link, row.source, row.tier),
        ("direct", "pairwise", "heuristic")
    );
    assert_eq!(row.note, None, "a linked pair carries no note");

    let r = cochange(&m, &q(&[ADMIN]), &CochangeArgs::default());
    assert_eq!(rows(&r), [(PAGE, 444, "none")]);
    let row = &r.rows[0];
    assert_eq!(row.antecedent, [ADMIN]);
    assert_eq!((row.support, row.antecedent_commits), (20, 45));
    assert_eq!(
        row.note.as_deref(),
        Some("no static link joins them (a blind spot, or coupling outside code)")
    );

    // Floor 0 shows the filtered converse: 3 / 45 = 66, and it is linked.
    let r = cochange(&m, &q(&[ADMIN]), &floor(0));
    assert_eq!(rows(&r), [(PAGE, 444, "none"), (REPORT, 66, "direct")]);
    assert_eq!((r.rows[1].support, r.rows[1].antecedent_commits), (3, 45));
}

/// unlinked_only keeps the rows no static link explains: web/page.ts only.
#[test]
fn unlinked_only() {
    let d = tree(Some(&history()));
    let m = build(d.path());
    for min in [0, 300] {
        let mut a = floor(min);
        a.unlinked_only = true;
        let r = cochange(&m, &q(&[ADMIN]), &a);
        assert_eq!(rows(&r), [(PAGE, 444, "none")], "floor {min}");
    }
    // The report -> admin rule is linked, so unlinked_only empties it.
    let mut a = CochangeArgs::default();
    a.unlinked_only = true;
    let r = cochange(&m, &q(&[REPORT]), &a);
    assert!(r.rows.is_empty());
    assert_eq!(r.absence.as_ref().map(|a| a.reason), Some("no_match"));
}

/// A query file with no MODULE (a doc, a path that does not exist) is listed
/// as unmapped, never guessed; the mapped files still answer.
#[test]
fn unmapped_file_listed() {
    let d = tree(Some(&history()));
    let m = build(d.path());
    let r = cochange(
        &m,
        &q(&["README.md", REPORT, "gone/missing.py", "./svc/report.py"]),
        &CochangeArgs::default(),
    );
    assert_eq!(r.query_files, ["README.md", "gone/missing.py", REPORT]);
    assert_eq!(r.unmapped, ["README.md", "gone/missing.py"]);
    assert_eq!(rows(&r), [(ADMIN, 1000, "direct")]);
}

/// Two query files that both predict svc/admin.py: one row, the antecedent
/// with the best confidence (a tie at 1000, broken by support: page's 20 over
/// report's 3), and the link is the best over EVERY query file of the repo
/// (admin imports report: direct, though page has no link).
#[test]
fn one_row_per_consequent() {
    let d = tree(Some(&history()));
    let m = build(d.path());
    let r = cochange(&m, &q(&[REPORT, PAGE]), &CochangeArgs::default());
    assert_eq!(rows(&r), [(ADMIN, 1000, "direct")]);
    let row = &r.rows[0];
    assert_eq!(row.antecedent, [PAGE]);
    assert_eq!((row.support, row.antecedent_commits), (20, 20));
    assert_eq!(row.note, None);
}

/// No snapshot: no CO_CHANGES edge at all, so the absence says there is no
/// history (and how to get one) rather than "no match".
#[test]
fn no_history_absence() {
    let d = tree(None);
    let m = build(d.path());
    let r = cochange(&m, &q(&[ADMIN]), &CochangeArgs::default());
    assert!(r.rows.is_empty());
    assert!(r.unmapped.is_empty(), "admin.py is still a MODULE");
    let a = r.absence.expect("absence");
    assert_eq!((a.tier, a.reason), ("FACT", "no_history"));
    assert!(a.note.contains("glia history sync"), "{}", a.note);
    assert_eq!(a.mechanisms, ["CO_CHANGES"]);
}

/// History exists but no rule passes the floors: `no_match`, naming them.
#[test]
fn floors_absence() {
    let d = tree(Some(&history()));
    let m = build(d.path());
    let mut a = CochangeArgs::default();
    a.min_support = 4;
    let r = cochange(&m, &q(&[REPORT]), &a);
    assert!(r.rows.is_empty());
    let abs = r.absence.expect("absence");
    assert_eq!(abs.reason, "no_match");
    assert!(
        abs.note.contains("min_support=4") && abs.note.contains("min_confidence_permille=300"),
        "{}",
        abs.note
    );
    // Only unmapped files: no_match too, and the file is listed.
    let r = cochange(&m, &q(&["README.md"]), &CochangeArgs::default());
    assert_eq!(r.unmapped, ["README.md"]);
    assert_eq!(r.absence.map(|a| a.reason), Some("no_match"));
    // top cuts.
    let mut a = floor(0);
    a.top = 1;
    assert_eq!(rows(&cochange(&m, &q(&[ADMIN]), &a)), [(PAGE, 444, "none")]);
}

/// Two builds of one tree, and the query in any order, give the same bytes.
#[test]
fn deterministic() {
    let d = tree(Some(&history()));
    let json = |files: &[&str]| {
        let m = build(d.path());
        serde_json::to_string(&cochange(&m, &q(files), &floor(0))).expect("json")
    };
    let first = json(&[ADMIN, "README.md"]);
    assert_eq!(first, json(&[ADMIN, "README.md"]));
    assert_eq!(first, json(&["README.md", ADMIN]));
    assert!(first.contains("\"unmapped\":[\"README.md\"]"), "{first}");
    assert!(first.contains("\"file\":\"web/page.ts\""), "{first}");
}

// ---------------------------------------------------------------------------
// CC.11b: multi-antecedent rules from the history snapshot's commits, and the
// working tree's change against a git rev.
//
// The acceptance history: a.py + a_test.py + db/migrate.sql together 4 times,
// a.py + b.py 6 times, a_test.py alone twice (12 commits). a.py has 10
// commits, a_test.py 6, b.py 6; db/migrate.sql is no MODULE. a.py imports
// b.py and a_test.py imports a.py; nothing links the migration.

mod git_fixture;

use glia_engine::GenerateResult;
use glia_engine::cochange::{cochange_multi, cochange_vs_rev};

const MA_PY: &str = "from b import helper\n\n\ndef run():\n    return helper()\n";
const MA_TEST_PY: &str = "from a import run\n\n\ndef test_run():\n    assert run() == 1\n";
const MB_PY: &str = "def helper():\n    return 1\n";
const MIGRATE_SQL: &str = "ALTER TABLE t ADD COLUMN c INT;\n";

const MA: &str = "a.py";
const MA_TEST: &str = "a_test.py";
const MB: &str = "b.py";
const MIGRATE: &str = "db/migrate.sql";

/// The multi acceptance tree's files.
fn multi_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (MA, MA_PY),
        (MA_TEST, MA_TEST_PY),
        (MB, MB_PY),
        (MIGRATE, MIGRATE_SQL),
    ]
}

/// The multi acceptance history (module comment).
fn multi_history() -> Vec<HistoryCommit> {
    commits(&[
        (&[MA, MA_TEST, MIGRATE], 4),
        (&[MA, MB], 6),
        (&[MA_TEST], 2),
    ])
}

/// Write `files` under `root`, creating parent dirs.
fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (p, text) in files {
        let path = root.join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
}

/// Write `history` as `root`'s history snapshot.
fn write_snapshot(root: &Path, history: &[HistoryCommit]) {
    let head = history
        .first()
        .map_or_else(|| "0".repeat(40), |c| c.c.clone());
    write_history(
        root,
        HistoryMeta::new(head, 2000, None, String::new()),
        history,
        &[],
    )
    .expect("write snapshot");
}

/// A temp tree of `files` with `history` as its snapshot.
fn tree_of(files: &[(&str, &str)], history: &[HistoryCommit]) -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    write_files(d.path(), files);
    write_snapshot(d.path(), history);
    d
}

fn generate(root: &Path) -> GenerateResult {
    generate_one(&root.to_string_lossy()).expect("build")
}

/// cochange_multi over a build's graph and repo roots.
fn multi(g: &GenerateResult, files: &[&str], args: &CochangeArgs) -> Cochange {
    cochange_multi(&g.merged, &g.repo_roots, &q(files), args)
}

/// One row as `(file, antecedent, support, antecedent_commits,
/// confidence_permille, source)`.
type RuleRow<'a> = (&'a str, Vec<&'a str>, u32, u32, u32, &'a str);

/// Every row, in answer order.
fn rules(c: &Cochange) -> Vec<RuleRow<'_>> {
    c.rows
        .iter()
        .map(|r| {
            (
                r.file.as_str(),
                r.antecedent.iter().map(String::as_str).collect(),
                r.support,
                r.antecedent_commits,
                r.confidence_permille,
                r.source,
            )
        })
        .collect()
}

/// Changing a.py AND a_test.py together predicts the migration every time
/// (4 / 4): a rule no pair shows — the migration is no MODULE, so there is no
/// CO_CHANGES edge to it — ranked above the pairwise a.py -> b.py (6 / 10).
#[test]
fn multi_antecedent_beats_pairwise() {
    let d = tree_of(&multi_files(), &multi_history());
    let g = generate(d.path());

    let r = multi(&g, &[MA_TEST, MA], &CochangeArgs::default());
    assert_eq!(r.query_files, [MA, MA_TEST]);
    assert!(r.unmapped.is_empty(), "{:?}", r.unmapped);
    assert!(r.absence.is_none());
    assert_eq!(
        rules(&r),
        [
            (MIGRATE, vec![MA, MA_TEST], 4, 4, 1000, "multi"),
            (MB, vec![MA], 6, 10, 600, "pairwise"),
        ]
    );
    let migrate = &r.rows[0];
    assert_eq!(migrate.module_qname, "", "the migration is no MODULE");
    assert_eq!((migrate.link, migrate.tier), ("none", "heuristic"));
    assert_eq!(
        migrate.note.as_deref(),
        Some("no static link joins them (a blind spot, or coupling outside code)")
    );
    let b = &r.rows[1];
    assert_eq!((b.module_qname.as_str(), b.link), ("b", "direct"));

    // The pairwise answer over the same graph cannot see the migration.
    let p = cochange(&g.merged, &q(&[MA, MA_TEST]), &CochangeArgs::default());
    assert_eq!(rules(&p), [(MB, vec![MA], 6, 10, 600, "pairwise")]);

    // Deterministic: another build, the query in another order, same bytes.
    let again = generate(d.path());
    let json = |c: &Cochange| serde_json::to_string(c).expect("json");
    assert_eq!(
        json(&r),
        json(&multi(
            &again,
            &[MA, MA_TEST, "./a.py"],
            &CochangeArgs::default()
        ))
    );
}

/// Three query files: the whole set co-changed in no commit, but its
/// one-file-smaller subset {a.py, a_test.py} did, and still predicts the
/// migration; the subset {a.py, b.py} predicts nothing outside the query.
#[test]
fn one_file_smaller_subsets() {
    let d = tree_of(&multi_files(), &multi_history());
    let g = generate(d.path());
    let r = multi(&g, &[MA, MA_TEST, MB], &CochangeArgs::default());
    assert_eq!(
        rules(&r),
        [(MIGRATE, vec![MA, MA_TEST], 4, 4, 1000, "multi")]
    );
    // The floors apply to multi rules too.
    let mut a = CochangeArgs::default();
    a.min_support = 5;
    let r = multi(&g, &[MA, MA_TEST, MB], &a);
    assert!(r.rows.is_empty(), "{:?}", r.rows);
    let abs = r.absence.expect("absence");
    assert_eq!(abs.reason, "no_match");
    assert!(abs.note.contains("min_support=5"), "{}", abs.note);
}

/// A commit that renamed old.py to new.py, and older commits touching
/// old.py: those count for new.py, its path today.
#[test]
fn renamed_file_counts_under_its_current_path() {
    const NEW: &str = "svc/new.py";
    const OLD: &str = "svc/old.py";
    const OTHER: &str = "svc/other.py";
    const DEPLOY: &str = "deploy/app.yaml";
    let mut history = vec![HistoryCommit {
        c: format!("900{}", "0".repeat(37)),
        t: T0 + 90_000,
        files: vec![HistoryFile {
            p: NEW.to_string(),
            a: Some(0),
            d: Some(0),
            from: Some(OLD.to_string()),
        }],
    }];
    history.extend(commits(&[(&[OLD, OTHER, DEPLOY], 3)]));
    let d = tree_of(
        &[
            (NEW, "def fresh():\n    return 1\n"),
            (OTHER, "def other():\n    return 2\n"),
            (DEPLOY, "name: app\nreplicas: 2\n"),
        ],
        &history,
    );
    let g = generate(d.path());
    let r = multi(&g, &[NEW, OTHER], &CochangeArgs::default());
    assert_eq!(rules(&r), [(DEPLOY, vec![NEW, OTHER], 3, 3, 1000, "multi")]);
    assert!(
        r.rows.iter().all(|row| row.file != OLD),
        "the old path is folded, never a consequent: {:?}",
        r.rows
    );
}

/// Nine query files use the whole set only (no one-file-smaller subset), so
/// a rule only a subset holds is lost; eight use all nine antecedents. Here
/// f1..f8 and cfg/settings.yaml co-changed 4 times; f9.py never did.
#[test]
fn lattice_cap() {
    let names: Vec<String> = (1..=9).map(|i| format!("pkg/f{i}.py")).collect();
    let sources: Vec<String> = (1..=9)
        .map(|i| format!("def f{i}():\n    return {i}\n"))
        .collect();
    let mut files: Vec<(&str, &str)> = names
        .iter()
        .zip(&sources)
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    files.push(("cfg/settings.yaml", "debug: false\n"));
    let together: Vec<&str> = names[..8]
        .iter()
        .map(String::as_str)
        .chain(["cfg/settings.yaml"])
        .collect();
    let d = tree_of(&files, &commits(&[(&together, 4)]));
    let g = generate(d.path());

    // f2..f9 (8 files): Q \ {f9} = f2..f8 co-changed 4 times, with f1 and the
    // yaml. f1 is also a pairwise rule (f2 -> f1, 4 / 4): the tie on
    // (confidence, support) goes to the larger antecedent.
    let eight: Vec<&str> = names[1..].iter().map(String::as_str).collect();
    let r = multi(&g, &eight, &CochangeArgs::default());
    let seven: Vec<&str> = names[1..8].iter().map(String::as_str).collect();
    assert_eq!(
        rules(&r),
        [
            ("cfg/settings.yaml", seven.clone(), 4, 4, 1000, "multi"),
            ("pkg/f1.py", seven, 4, 4, 1000, "multi"),
        ]
    );

    // f1..f9 (9 files): the whole set only, which never co-changed; every
    // other file is a query file, so there is no pairwise rule either.
    let nine: Vec<&str> = names.iter().map(String::as_str).collect();
    let r = multi(&g, &nine, &CochangeArgs::default());
    assert!(r.rows.is_empty(), "{:?}", r.rows);
    assert_eq!(r.absence.map(|a| a.reason), Some("no_match"));
}

/// No snapshot and no CO_CHANGES edge: `no_history`, as for the pairwise
/// answer.
#[test]
fn multi_without_history() {
    let d = tempfile::tempdir().expect("tempdir");
    write_files(d.path(), &multi_files());
    let g = generate(d.path());
    let r = multi(&g, &[MA, MA_TEST], &CochangeArgs::default());
    assert!(r.rows.is_empty());
    assert_eq!(r.absence.map(|a| a.reason), Some("no_history"));
}

/// The working tree's change against HEAD is the query: a.py and a_test.py
/// edited (the untracked history snapshot under .glia/ is no change).
#[test]
fn cochange_vs_rev_uses_the_working_tree_change() {
    let repo = git_fixture::GitRepo::init();
    for (p, text) in multi_files() {
        repo.write(p, text);
    }
    repo.commit("init");
    write_snapshot(repo.root(), &multi_history());

    // A clean tree: no change, so no query file and an absence.
    let clean = cochange_vs_rev(repo.path(), "HEAD", &CochangeArgs::default()).expect("clean");
    assert!(clean.query_files.is_empty(), "{:?}", clean.query_files);
    let abs = clean.absence.expect("absence");
    assert_eq!(abs.reason, "no_match");
    assert!(abs.note.contains("no change against HEAD"), "{}", abs.note);

    repo.write(MA, &format!("{MA_PY}# edited\n"));
    repo.write(MA_TEST, &format!("{MA_TEST_PY}# edited\n"));
    let r = cochange_vs_rev(repo.path(), "HEAD", &CochangeArgs::default()).expect("answer");
    assert_eq!(r.query_files, [MA, MA_TEST]);
    assert_eq!(
        rules(&r),
        [
            (MIGRATE, vec![MA, MA_TEST], 4, 4, 1000, "multi"),
            (MB, vec![MA], 6, 10, 600, "pairwise"),
        ]
    );

    // An untracked new file is part of the change too.
    repo.write("c.py", "def c():\n    return 3\n");
    let r = cochange_vs_rev(repo.path(), "HEAD", &CochangeArgs::default()).expect("answer");
    assert_eq!(r.query_files, [MA, MA_TEST, "c.py"]);

    // So are a deletion and both paths of a (staged) rename.
    repo.remove(MB);
    repo.git_mv(MIGRATE, "db/v2.sql");
    let r = cochange_vs_rev(repo.path(), "HEAD", &CochangeArgs::default()).expect("answer");
    assert_eq!(
        r.query_files,
        [MA, MA_TEST, MB, "c.py", MIGRATE, "db/v2.sql"]
    );

    assert!(cochange_vs_rev(repo.path(), "no-such-rev", &CochangeArgs::default()).is_err());
}
