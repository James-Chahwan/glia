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
