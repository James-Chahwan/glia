//! EB.8 — `glia_engine::find::find_files`: the files a free-text query is
//! about, in one repo, ranked LX[IDR,T1s] (typed id links first, then the
//! query's BM25 over past commit messages, then the file's degree, then path).
//!
//! The tree: a Cargo crate (`src/a.rs` .. `src/f.rs`) and a spec-kit task
//! list `specs/001-billing/tasks.md` whose pending task `T012` names
//! `src/b.rs`, committed through the LE.1b git fixture. A history snapshot
//! (code-domain `write_history_with_messages`) holds past commit messages.
//!
//! What is pinned:
//! (1) `T012 export`: the T012-named file first (idr 1.0, `ids = [T012]`),
//!     then the files of the past commits whose messages say `export`;
//! (2) the text-tied files go by degree, then path;
//! (3) a query with no id ranks the matching commits' files by BM25;
//! (4) no snapshot and no id: a FACT `no_match` absence naming both causes,
//!     no caveats; a snapshot whose messages do not match says so;
//! (5) two repos: an error;
//! (6) two builds give the same answer, byte for byte;
//! (7) EC.13: the answer carries its `selection` (set SELf2t, shape `find`):
//!     with no calibration record the fallback LX[IDR,T1s] (`absent`), rows
//!     unchanged; `select = false` is the same order as an explicit choice;
//!     the JSON gains `selection` after `absence`.

mod git_fixture;

use git_fixture::GitRepo;
use glia_code_domain::snapshots::{
    HistoryCommit, HistoryFile, HistoryMessage, HistoryMeta, write_history_with_messages,
};
use glia_engine::GenerateResult;
use glia_engine::find::{
    FIND_FILES_FUSION, FindFilesOptions, FoundFile, find_files, find_files_with, find_files_with_ids,
};

const CARGO: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub mod a;\npub mod b;\npub mod c;\npub mod d;\npub mod e;\npub mod f;\n";
const A: &str = "use crate::b::bill;\nuse crate::c::other;\nuse crate::d::{misc, misc_two, misc_three};\n\npub fn run() {\n    other();\n    misc();\n    misc_two();\n    misc_three();\n    bill();\n}\n";
const B: &str = "pub fn bill() {}\n";
const C: &str = "pub fn other() {}\n";
const D: &str = "pub fn misc() {}\n\npub fn misc_two() {\n    misc();\n}\n\npub fn misc_three() {\n    misc();\n    misc_two();\n}\n";
const E: &str = "pub fn e() {}\n";
const F: &str = "pub fn f() {}\n";
const TASKS: &str =
    "# Tasks\n\n- [ ] T012 billing export in src/b.rs\n- [x] T013 old work in src/d.rs\n";

fn fixture() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("Cargo.toml", CARGO);
    repo.write("src/lib.rs", LIB);
    for (p, t) in [
        ("src/a.rs", A),
        ("src/b.rs", B),
        ("src/c.rs", C),
        ("src/d.rs", D),
        ("src/e.rs", E),
        ("src/f.rs", F),
    ] {
        repo.write(p, t);
    }
    repo.write("specs/001-billing/tasks.md", TASKS);
    repo.commit("clean");
    repo
}

fn sha(n: usize) -> String {
    format!("{n:02}{}", "0".repeat(38))
}

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

/// The past commits, newest first: two `export` subjects of equal length
/// (so their files tie on text), one invoice subject, the skeleton.
fn snapshot(repo: &GitRepo) {
    let rows = vec![
        row(sha(4), 1_767_225_604, &["src/e.rs"], "invoice totals"),
        row(sha(3), 1_767_225_603, &["src/c.rs", "src/d.rs"], "invoice export rounding"),
        row(sha(2), 1_767_225_602, &["src/e.rs", "src/f.rs"], "export ledger cleanup"),
        row(sha(1), 1_767_225_601, &["src/a.rs", "src/b.rs"], "initial shop skeleton"),
    ];
    let (commits, messages): (Vec<HistoryCommit>, Vec<HistoryMessage>) = rows.into_iter().unzip();
    let meta = HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new());
    write_history_with_messages(repo.root(), meta, &commits, &[], Some(&messages))
        .expect("snapshot");
}

fn build(repo: &GitRepo) -> GenerateResult {
    glia_engine::generate_one(repo.path()).expect("build")
}

fn ask(b: &GenerateResult, q: &str) -> glia_engine::absence::Answer<FoundFile> {
    find_files(&b.merged, &b.repo_roots, q, &FindFilesOptions::default()).expect("answer")
}

fn files(rows: &[FoundFile]) -> Vec<&str> {
    rows.iter().map(|r| r.file.as_str()).collect()
}

#[test]
fn an_id_ranks_its_file_first_then_text_matches() {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    let (a, ids) = find_files_with_ids(&b.merged, &b.repo_roots, "T012 export", &FindFilesOptions::default())
        .expect("answer");
    assert_eq!(FIND_FILES_FUSION, "LX[IDR,T1s]");
    assert!(a.absence.is_none(), "{:?}", a.absence);
    let first = &a.results[0];
    assert_eq!((first.rank, first.file.as_str(), first.idr), (1, "src/b.rs", 1.0));
    assert_eq!(first.ids, ["T012"]);
    let t012 = ids.iter().find(|r| r.id == "T012").expect("T012 routed");
    assert_eq!(t012.state, Some("pending"));
    // the rest: the files of the two `export` commits, text only
    let rest: Vec<&str> = files(&a.results[1..]);
    let mut want = vec!["src/c.rs", "src/d.rs", "src/e.rs", "src/f.rs"];
    let mut got = rest.clone();
    got.sort();
    want.sort();
    assert_eq!(got, want, "{:?}", a.results);
    assert!(a.results[1..].iter().all(|r| r.idr == 0.0 && r.text > 0.0 && r.ids.is_empty()));
    assert!(a.results.iter().enumerate().all(|(i, r)| r.rank == i + 1));
    // the skeleton's files never matched
    assert!(!rest.contains(&"src/a.rs"));
}

#[test]
fn text_ties_go_by_degree_then_path() {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    let a = ask(&b, "T012 export");
    let tied = &a.results[1..];
    // the two export subjects have equal length: every text score is equal
    assert!(tied.windows(2).all(|w| w[0].text == w[1].text), "{tied:?}");
    let by = |p: &str| tied.iter().find(|r| r.file == p).expect(p).clone();
    let (c, d, e, f) = (by("src/c.rs"), by("src/d.rs"), by("src/e.rs"), by("src/f.rs"));
    assert!(d.degree > c.degree, "the fixture gives d.rs more edges: {d:?} vs {c:?}");
    assert_eq!(e.degree, f.degree, "e.rs and f.rs have one function each");
    // degree descending, then path ascending
    let mut want: Vec<FoundFile> = tied.to_vec();
    want.sort_by(|x, y| y.degree.total_cmp(&x.degree).then(x.file.cmp(&y.file)));
    assert_eq!(files(tied), files(&want));
    let pos = |p: &str| tied.iter().position(|r| r.file == p).expect(p);
    assert!(pos("src/d.rs") < pos("src/c.rs"), "degree beats path: {tied:?}");
    assert!(pos("src/e.rs") < pos("src/f.rs"), "equal degree: path: {tied:?}");
}

#[test]
fn a_query_without_ids_ranks_by_bm25() {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    let (a, ids) = find_files_with_ids(&b.merged, &b.repo_roots, "invoice rounding bug", &FindFilesOptions::default())
        .expect("answer");
    assert!(ids.is_empty(), "{ids:?}");
    // c.rs / d.rs match two tokens (sha 3), e.rs one (sha 4)
    let got = files(&a.results);
    assert_eq!(got.len(), 3, "{:?}", a.results);
    assert_eq!(got[2], "src/e.rs");
    let mut top: Vec<&str> = got[..2].to_vec();
    top.sort();
    assert_eq!(top, ["src/c.rs", "src/d.rs"]);
    assert!(a.results[0].text > a.results[2].text);
    assert!(a.results.iter().all(|r| r.idr == 0.0));
    // top_k cuts
    let mut o = FindFilesOptions::default();
    o.top_k = 1;
    let one = find_files(&b.merged, &b.repo_roots, "invoice rounding bug", &o).expect("answer");
    assert_eq!(files(&one.results), files(&a.results[..1]));
}

#[test]
fn no_snapshot_and_no_id_is_an_absence_naming_both() {
    let repo = fixture();
    let b = build(&repo);
    let a = ask(&b, "rounding of the ledger");
    assert!(a.results.is_empty());
    let ab = a.absence.expect("absence");
    assert_eq!((ab.tier, ab.reason), ("FACT", "no_match"));
    assert!(ab.note.contains("no id in the query resolved to a defining row"), "{}", ab.note);
    assert!(ab.note.contains("no history messages (run glia history sync)"), "{}", ab.note);
    assert!(ab.caveats.is_empty() && ab.mechanisms.is_empty());

    // without a snapshot an id still routes
    let t = ask(&b, "T012 export");
    assert_eq!(files(&t.results), ["src/b.rs"]);
    assert_eq!(t.results[0].text, 0.0);

    snapshot(&repo);
    let b = build(&repo);
    let m = ask(&b, "zebra quagga");
    let note = m.absence.expect("absence").note;
    assert!(note.contains("no past commit message matches the query"), "{note}");
    assert!(!note.contains("history sync"), "{note}");
}

#[test]
fn two_repos_are_refused() {
    let one = fixture();
    let two = fixture();
    let b = glia_engine::generate_many(&[one.path().to_string(), two.path().to_string()]).expect("build");
    let err = find_files(&b.merged, &b.repo_roots, "T012", &FindFilesOptions::default())
        .expect_err("two repos");
    assert_eq!(err, "find --files ranks one repo's files; got 2 repos");
}

#[test]
fn two_builds_answer_identically() {
    let repo = fixture();
    snapshot(&repo);
    let (b1, b2) = (build(&repo), build(&repo));
    for q in ["T012 export", "invoice rounding bug", "nothing here"] {
        let x = serde_json::to_string(&ask(&b1, q)).expect("json");
        let y = serde_json::to_string(&ask(&b2, q)).expect("json");
        assert_eq!(x, y, "{q}");
    }
}

/// IDR reads the WORKING tree (review reads its base), and a past commit
/// credits a file only under the path it touched (no rename fold, the
/// harness's `git log --no-renames`).
#[test]
fn the_working_tree_is_read_and_renames_are_not_folded() {
    let repo = fixture();
    snapshot(&repo);
    repo.git_mv("src/f.rs", "src/g.rs");
    repo.write("src/lib.rs", &LIB.replace("pub mod f;", "pub mod g;"));
    repo.remove("specs/001-billing/tasks.md");
    let b = build(&repo);
    let (a, ids) = find_files_with_ids(&b.merged, &b.repo_roots, "T012 export", &FindFilesOptions::default())
        .expect("answer");
    assert!(a.results.iter().all(|r| r.idr == 0.0), "{:?}", a.results);
    assert!(ids.iter().all(|r| r.ns.is_none()), "T012 is undefined in the working tree: {ids:?}");
    let got = files(&a.results);
    assert!(!got.contains(&"src/g.rs") && !got.contains(&"src/f.rs"), "{got:?}");
    assert!(got.contains(&"src/e.rs"), "{got:?}");
}

#[test]
fn the_answer_carries_its_selection() {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    let a = ask(&b, "T012 export");
    let s = a.selection.as_ref().expect("selection");
    assert_eq!(
        (s.set.as_str(), s.shape.as_str(), s.pick.as_str(), s.source, s.reason, s.test_commits),
        ("SELf2t", "find", FIND_FILES_FUSION, "fallback", Some("absent"), 0)
    );
    let mut o = FindFilesOptions::default();
    o.select = false;
    let e = find_files(&b.merged, &b.repo_roots, "T012 export", &o).expect("answer");
    let es = e.selection.as_ref().expect("selection");
    assert_eq!((es.pick.as_str(), es.source, es.reason), (FIND_FILES_FUSION, "explicit", None));
    assert_eq!(e.results, a.results);
    let json = serde_json::to_string(&a).expect("json");
    assert!(
        json.contains("\"absence\":null,\"selection\":{\"set\":\"SELf2t\",\"shape\":\"find\",\"pick\":\"LX[IDR,T1s]\",\"source\":\"fallback\",\"reason\":\"absent\",\"test_commits\":0}}"),
        "{json}"
    );
    // an absence carries it too
    let none = ask(&b, "zebra quagga");
    assert!(none.results.is_empty() && none.absence.is_some());
    assert_eq!(none.selection.as_ref().map(|s| s.source), Some("fallback"));
}

// ---- EG.7: the rank gate's seam `find_files_with` ---------------------------

fn history_of(repo: &GitRepo) -> glia_engine::review::IntentHistory {
    let snap = glia_code_domain::snapshots::read_history(repo.root()).expect("snapshot");
    let msgs = glia_code_domain::snapshots::read_history_messages(repo.root(), &snap);
    glia_engine::review::IntentHistory::new(snap, msgs, std::collections::BTreeSet::new())
}

/// (8) `find_files_with(.., None, None)` is `find_files`, byte for byte; the
/// root's own snapshot given in memory answers the same rows; a forced pick
/// is the same order as `select = false`'s explicit choice.
#[test]
fn the_seam_is_find_files() {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    let opts = FindFilesOptions::default();
    for q in ["T012 export", "export ledger", "nothing here"] {
        let live = find_files(&b.merged, &b.repo_roots, q, &opts).expect("answer");
        let seam = find_files_with(&b.merged, &b.repo_roots, q, &opts, None, None).expect("answer");
        assert_eq!(
            serde_json::to_string(&live).expect("json"),
            serde_json::to_string(&seam).expect("json"),
            "{q}"
        );
        let hist = history_of(&repo);
        let mem = find_files_with(&b.merged, &b.repo_roots, q, &opts, Some(&hist), None).expect("answer");
        assert_eq!(live.results, mem.results, "{q}");
        let forced =
            find_files_with(&b.merged, &b.repo_roots, q, &opts, Some(&hist), Some(FIND_FILES_FUSION)).expect("answer");
        assert_eq!(live.results, forced.results, "{q}");
        assert_eq!(forced.selection.as_ref().map(|s| s.source), Some("explicit"));
    }
}

/// (9) Leak-free: a history whose `exclude` names a commit answers exactly as
/// the history with that commit (and its message) removed.
#[test]
fn an_excluded_commit_reads_as_absent() {
    let repo = fixture();
    snapshot(&repo);
    let b = build(&repo);
    let opts = FindFilesOptions::default();
    let mut excl = history_of(&repo);
    excl.exclude.insert(sha(2));
    let mut cut = history_of(&repo);
    let i = cut.snapshot.commits.iter().position(|c| c.c == sha(2)).expect("sha 2");
    cut.snapshot.commits.remove(i);
    if let Some(m) = cut.messages.as_mut() {
        m.remove(i);
    }
    let a = find_files_with(&b.merged, &b.repo_roots, "export ledger", &opts, Some(&excl), None).expect("answer");
    let c = find_files_with(&b.merged, &b.repo_roots, "export ledger", &opts, Some(&cut), None).expect("answer");
    assert_eq!(a.results, c.results);
    // and the excluded commit's files lose its text
    let full = find_files_with(&b.merged, &b.repo_roots, "export ledger", &opts, Some(&history_of(&repo)), None)
        .expect("answer");
    assert_ne!(files(&full.results), files(&a.results));
}
