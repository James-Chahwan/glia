//! EB.6: `glia_engine::intent::idr`, typed id routing (rank_eval's round-2
//! IDR): a text's ids -> their defining rows (lineage and families included)
//! -> the files those rows name, plus the code whose comments cite the id;
//! the conventional-commit scope route when no id resolves; read at one
//! revision; recent scopes from the history snapshot's subjects.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use glia_code_domain::glia_config::parse_str;
use glia_code_domain::snapshots::{
    read_history, read_history_messages, write_history_with_messages, HistoryCommit, HistoryFile, HistoryMessage,
    HistoryMeta,
};
use glia_engine::intent::defs::Tree;
use glia_engine::intent::idr::{
    comment_part, conventional_scopes, recent_scopes, recent_subjects, route, Idr, IdrInputs,
};
use glia_engine::intent::ids::IdVocab;
use glia_engine::intent::paths::rename_pairs;
use glia_engine::intent::text::Candidates;

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

fn vocab() -> IdVocab {
    let loaded = parse_str("version = 1\n[ids]\npatterns = [\"T{N}\", \"{U}-{N}\", \"{U}.{N}{l?}\"]\n");
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    IdVocab::from_config(Some(&loaded))
}

const SRC: &[&str] = &[
    "src/billing/api.rs",
    "src/billing/export.rs",
    "src/billing/legacy.rs",
    "src/billing/model.rs",
    "src/billing/new.rs",
    "src/billing/plan_impl.rs",
    "src/billing/route.rs",
    "src/billing/tweak.rs",
    "src/pk/a.rs",
    "src/pk/b.rs",
    "src/search/tweak.rs",
];

const BILLING_TASKS: &str = "# Tasks\n\n\
- [ ] T012 add the export in `src/billing/export.rs` and src/billing/model.rs\n\
- [x] T013 wire the route in src/billing/route.rs\n\
- [x] T009 old export plan in src/billing/legacy.rs\n\
- [ ] T015 (was T009) new export plan in src/billing/plan_impl.rs\n\
- [ ] T030 billing tweak in src/billing/tweak.rs\n";

const SEARCH_TASKS: &str = "# Tasks\n\n- [ ] T030 search tweak in src/search/tweak.rs\n";

const PK_PLAN: &str = "# Packets\n\n- [ ] PK.3a first half in src/pk/a.rs\n- [ ] PK.3b second half in src/pk/b.rs\n";

/// The spec-kit fixture, committed; returns the commit sha.
fn fixture(d: &Path) -> String {
    git(d, &["init", "-q", "-b", "main"]);
    put(d, "specs/001-billing/tasks.md", BILLING_TASKS);
    put(d, "specs/002-search/tasks.md", SEARCH_TASKS);
    put(d, "docs/packets.md", PK_PLAN);
    put(d, "src/billing/export.rs", "// implements T012\npub fn export() {}\n");
    put(d, "src/billing/model.rs", "pub struct Model;\n");
    put(d, "src/billing/api.rs", "/* see T012 */\npub fn api() {}\n");
    put(d, "src/billing/route.rs", "pub fn route() { let s = \"T012\"; let t012 = 1; }\n");
    put(d, "src/billing/legacy.rs", "pub fn legacy() {}\n");
    put(d, "src/billing/plan_impl.rs", "pub fn plan() {}\n");
    put(d, "src/billing/tweak.rs", "// T030 billing side\n");
    put(d, "src/search/tweak.rs", "// T030 search side\n");
    put(d, "src/billing/new.rs", "pub fn new() {}\n");
    put(d, "src/pk/a.rs", "pub fn a() {}\n");
    put(d, "src/pk/b.rs", "pub fn b() {}\n");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "init"]);
    git(d, &["rev-parse", "HEAD"])
}

fn cands(paths: &[&str]) -> Candidates {
    let mut c = Candidates::default();
    c.paths = paths.iter().map(|p| p.to_string()).collect();
    c.paths.sort();
    c.deg = vec![1.0; c.paths.len()];
    c.files = (0..c.paths.len() as u32).collect();
    c
}

/// Route `text` over `tree`; returns the routing and candidates.
fn run(tree: &Tree, c: &Candidates, text: &str, recent: &[String]) -> Idr {
    let v = vocab();
    let inputs = IdrInputs::read(tree, &[], &v, c, &[]);
    let side = inputs.side(c, &v, tree).labelled("fixture", "test");
    route(text, &side, recent)
}

/// (path, idr) of every file with a score, path order.
fn scored(c: &Candidates, x: &Idr) -> Vec<(String, f64)> {
    c.paths.iter().zip(&x.idr).filter(|(_, w)| **w > 0.0).map(|(p, w)| (p.clone(), *w)).collect()
}

fn s(p: &str, w: f64) -> (String, f64) {
    (p.to_string(), w)
}

#[test]
fn a_title_id_reaches_named_and_citing_files() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    let x = run(&Tree::Work(d), &c, "fix: finish T012 export", &[]);
    assert!(x.has_id);
    // export.rs is named (1.0) and cites T012 in a comment (0.5); model.rs is
    // named; api.rs cites it; route.rs only holds it in a string literal
    assert_eq!(scored(&c, &x), [s("src/billing/api.rs", 0.5), s("src/billing/export.rs", 1.5), s("src/billing/model.rs", 1.0)]);
    let i = c.position("src/billing/export.rs").expect("export");
    assert_eq!((x.named[i], x.cited[i]), (1.0, 0.5));
    assert_eq!(x.ids.len(), 1);
    let r = &x.ids[0];
    assert_eq!((r.id.as_str(), r.route, r.tier, r.state), ("T012", "unique", Some("derived"), Some("pending")));
    assert_eq!(r.defined_at, Some(("specs/001-billing/tasks.md".to_string(), 3)));
    assert_eq!((r.named_files, r.cited_files), (2, 2));
}

#[test]
fn a_done_row_weighs_half() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    let x = run(&Tree::Work(d), &c, "T013 follow-up", &[]);
    assert_eq!(scored(&c, &x), [s("src/billing/route.rs", 0.5)]);
    assert_eq!(x.ids[0].state, Some("done"));
}

#[test]
fn an_id_in_two_specs_routes_by_line_scope() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    // the citing file's own scope decides each comment: only search's counts
    let x = run(&Tree::Work(d), &c, "T030 per 002-search", &[]);
    assert_eq!(x.ids[0].route, "line-scope");
    assert_eq!(scored(&c, &x), [s("src/search/tweak.rs", 1.5)]);
    let y = run(&Tree::Work(d), &c, "T030 per 001-billing", &[]);
    assert_eq!(scored(&c, &y), [s("src/billing/tweak.rs", 1.5)]);
}

#[test]
fn an_undefined_id_is_listed_and_reaches_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    let x = run(&Tree::Work(d), &c, "handle UTF-8 input", &[]);
    assert!(!x.has_id);
    assert!(x.idr.iter().all(|w| *w == 0.0));
    assert_eq!(x.ids.len(), 1);
    assert_eq!((x.ids[0].id.as_str(), x.ids[0].route, x.ids[0].ns.as_deref()), ("UTF-8", "undefined", None));
}

#[test]
fn reading_at_the_commit_ignores_a_row_the_working_tree_added() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    let sha = fixture(d);
    put(d, "specs/001-billing/tasks.md", &format!("{BILLING_TASKS}- [ ] T040 new thing in src/billing/new.rs\n"));
    let c = cands(SRC);
    let work = run(&Tree::Work(d), &c, "T040 start", &[]);
    assert_eq!(scored(&c, &work), [s("src/billing/new.rs", 1.0)]);
    let rev = run(&Tree::Rev { repo: d, sha }, &c, "T040 start", &[]);
    assert!(!rev.has_id);
    assert_eq!(rev.ids[0].route, "undefined");
    assert!(rev.idr.iter().all(|w| *w == 0.0));
    // the commit's own rows and comments read the same at the commit
    let sha2 = git(d, &["rev-parse", "HEAD"]);
    let x = run(&Tree::Rev { repo: d, sha: sha2 }, &c, "fix: finish T012 export", &[]);
    assert_eq!(scored(&c, &x), [s("src/billing/api.rs", 0.5), s("src/billing/export.rs", 1.5), s("src/billing/model.rs", 1.0)]);
}

#[test]
fn a_family_reaches_its_members_and_a_bare_prefix_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    let x = run(&Tree::Work(d), &c, "PK.3 and T01", &[]);
    assert_eq!(scored(&c, &x), [s("src/pk/a.rs", 1.0), s("src/pk/b.rs", 1.0)]);
    let routes: Vec<(&str, &str)> = x.ids.iter().map(|r| (r.id.as_str(), r.route)).collect();
    assert_eq!(routes, [("PK.3a", "family"), ("PK.3b", "family"), ("T01", "undefined")]);
}

#[test]
fn lineage_routes_an_old_id_to_its_successor() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    // T009 (done) and T015 (was T009) are one lineage class
    let x = run(&Tree::Work(d), &c, "revisit T009", &[]);
    assert_eq!(scored(&c, &x), [s("src/billing/legacy.rs", 0.5), s("src/billing/plan_impl.rs", 1.0)]);
}

#[test]
fn recent_scopes_pick_the_scope_the_last_subjects_named() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    let sha = |n: usize| format!("{n:02}{}", "0".repeat(38));
    let commit = |n: usize, files: &[&str], subj: &str| {
        let hc = HistoryCommit {
            c: sha(n),
            t: 1_767_225_600 + n as i64,
            files: files.iter().map(|p| HistoryFile { p: p.to_string(), a: Some(1), d: Some(0), from: None }).collect(),
        };
        let m = HistoryMessage { c: sha(n), at: hc.t, s: subj.to_string(), b: String::new(), br: None };
        (hc, m)
    };
    // newest first; commit 3 touches one candidate file (no test commit)
    let rows = vec![
        commit(3, &["src/billing/tweak.rs"], "T013 tiny"),
        commit(2, &["src/search/tweak.rs", "src/billing/tweak.rs"], "T030 per 002-search"),
        commit(1, &["src/billing/export.rs", "src/billing/model.rs"], "T012 export"),
    ];
    let (commits, messages): (Vec<HistoryCommit>, Vec<HistoryMessage>) = rows.into_iter().unzip();
    let meta = HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new());
    write_history_with_messages(d, meta, &commits, &[], Some(&messages)).expect("snapshot");
    let snap = read_history(d).expect("history");
    let msgs = read_history_messages(d, &snap).expect("messages");
    let subjects = recent_subjects(&snap, &msgs, &c, &BTreeSet::new());
    assert_eq!(subjects, ["T030 per 002-search", "T012 export"]);
    let v = vocab();
    let tree = Tree::Work(d);
    let inputs = IdrInputs::read(&tree, &[], &v, &c, &rename_pairs(&snap));
    let recent = recent_scopes(&subjects, &inputs.ns, &v);
    assert_eq!(recent, ["specs/002-search", "specs/001-billing"]);
    let side = inputs.side(&c, &v, &tree).labelled("fixture", "test");
    let x = route("T030 again", &side, &recent);
    assert_eq!(x.ids[0].route, "recent-scope");
    assert_eq!(scored(&c, &x), [s("src/search/tweak.rs", 1.5)]);
    // the answer's own commit excluded: the newest test subject is gone
    let ex: BTreeSet<String> = [sha(2)].into_iter().collect();
    assert_eq!(recent_subjects(&snap, &msgs, &c, &ex), ["T012 export"]);
}

const SCOPE_SRC: &[&str] = &["src/billing/export.rs", "src/billing/old.rs", "src/other.rs"];

fn scope_fixture(d: &Path) {
    git(d, &["init", "-q", "-b", "main"]);
    put(d, "features/billing/plan.md", "# Plan\n\n- [ ] write src/billing/export.rs\n- [x] done in src/billing/old.rs\n");
    put(d, "node_modules/billing/x.md", "- [ ] src/other.rs\n");
    put(d, "docs/tasks.md", "# Tasks\n\n- [ ] T101 other work in src/other.rs\n");
    for p in SCOPE_SRC {
        put(d, p, "fn x() {}\n");
    }
}

#[test]
fn the_scope_route_runs_only_when_no_id_resolves() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    scope_fixture(d);
    let c = cands(SCOPE_SRC);
    let x = run(&Tree::Work(d), &c, "feat(billing): add export", &[]);
    assert!(x.has_id);
    assert_eq!(scored(&c, &x), [s("src/billing/export.rs", 1.0), s("src/billing/old.rs", 0.5)]);
    assert_eq!((x.ids[0].id.as_str(), x.ids[0].route), ("scope:billing", "scope-dir"));
    let y = run(&Tree::Work(d), &c, "feat(billing): add export for T101", &[]);
    assert_eq!(scored(&c, &y), [s("src/other.rs", 1.0)]);
    // a scope with no docs is no id
    let z = run(&Tree::Work(d), &c, "feat(search): nothing", &[]);
    assert!(!z.has_id && z.ids.is_empty());
}

#[test]
fn a_text_without_ids_or_scopes_routes_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    fixture(d);
    let c = cands(SRC);
    let x = run(&Tree::Work(d), &c, "tidy the export code", &[]);
    assert!(!x.has_id && x.ids.is_empty());
    assert_eq!(x.idr, vec![0.0; c.len()]);
}

#[test]
fn the_harness_helpers() {
    assert_eq!(conventional_scopes("feat(billing, Search-2)!: x"), ["billing", "search-2"]);
    assert!(conventional_scopes("feat(ab): x").is_empty());
    assert!(conventional_scopes("fix billing(x): y").is_empty());
    assert!(conventional_scopes("feat(billing) x").is_empty());
    assert_eq!(comment_part("let a = 1; // T012"), Some("// T012"));
    assert_eq!(comment_part("  # T012 note"), Some("# T012 note"));
    assert_eq!(comment_part("let s = \"T012\";"), None);
}
