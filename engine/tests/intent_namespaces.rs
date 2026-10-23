//! EB.5b: `glia_engine::intent::defs::Namespaces`, bare-id namespacing and the
//! IDR resolution cascade (rank_eval `KNs::build` / `KNs::resolve` /
//! `KNs::siblings`, `r2_pick`, `KLin`), over EB.5a-style fixture trees.

use std::path::Path;
use std::process::Command;

use glia_code_domain::glia_config::parse_str;
use glia_engine::intent::defs::{line_scopes, read_defs, Ctx, IdDefs, Namespaces, Resolved, RouteCounts, Tree};
use glia_engine::intent::ids::IdVocab;

fn git(dir: &Path, args: &[&str]) {
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
}

fn vocab() -> IdVocab {
    let loaded = parse_str("version = 1\n[ids]\npatterns = [\"T{N3}\", \"{U}-{N}\", \"F{N}\", \"D{N}\"]\n");
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    IdVocab::from_config(Some(&loaded))
}

/// The defs of a tree holding `files`, and the same defs with the rows
/// enumerated in reverse (the order-independence check).
fn defs_of(files: &[(&str, &str)]) -> (IdDefs, IdDefs) {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = tmp.path();
    git(d, &["init", "-q", "-b", "main"]);
    for (rel, text) in files {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, text).expect("write");
    }
    let defs = read_defs(&Tree::Work(d), &[], &vocab(), None);
    assert_eq!(defs.tree_error, None);
    let mut rev = defs.clone();
    rev.rows.reverse();
    (defs, rev)
}

fn local() -> Vec<String> {
    vocab().local_words
}

/// Both namespace builds, which must agree.
fn both(files: &[(&str, &str)]) -> (Namespaces, Namespaces) {
    let (a, b) = defs_of(files);
    let (na, nb) = (a.namespaces(&local()), b.namespaces(&local()));
    na.report("fixture");
    (na, nb)
}

/// Resolve `id` under `cx` in both builds; they must agree.
fn res(ns: &(Namespaces, Namespaces), id: &str, cx: &Ctx) -> Result<(String, &'static str, &'static str), &'static str> {
    let f = |r: Result<Resolved, &'static str>| r.map(|r| (r.ns, r.tier, r.route));
    let (a, b) = (f(ns.0.resolve(id, cx)), f(ns.1.resolve(id, cx)));
    assert_eq!(a, b, "reverse enumeration changed the resolution of {id}");
    a
}

const SPEC_A: &str = "# Tasks\n\n- [ ] T001 build the a side\n";
const SPEC_X: &str = "# Tasks\n\n- [ ] T001 build the x side\n";

#[test]
fn the_cascade_own_scope_line_scope_recent_newest() {
    let ns = both(&[
        ("specs/001-a/tasks.md", SPEC_A),
        ("specs/002-x/tasks.md", SPEC_X),
        ("specs/002-x/plan.md", "# Plan\n\nWe do T001 first.\n"),
    ]);
    let a = "specs/001-a/tasks.md".to_string();
    let x = "specs/002-x/tasks.md".to_string();

    let mut cx = Ctx::default();
    cx.own_page = Some("specs/001-a/tasks.md");
    assert_eq!(res(&ns, "T001", &cx), Ok((a.clone(), "fact", "own-page")));

    let mut cx = Ctx::default();
    cx.own_page = Some("specs/002-x/plan.md");
    cx.own_scope = Some("specs/002-x");
    assert_eq!(res(&ns, "T001", &cx), Ok((x.clone(), "derived", "own-scope")));

    let line = "finish T001 for spec 002";
    let mut cx = Ctx::default();
    cx.line_scopes = line_scopes(line, &ns.0);
    cx.text = line;
    assert_eq!(cx.line_scopes, ["specs/002-x"]);
    assert_eq!(res(&ns, "T001", &cx), Ok((x.clone(), "derived", "line-scope")));
    assert_eq!(line_scopes("see specs/001-a/tasks.md and 002-x", &ns.0), ["specs/001-a", "specs/002-x"]);

    // both pending: recent, then newest
    let recent = vec!["specs/001-a".to_string()];
    let mut cx = Ctx::default();
    cx.recent = &recent;
    assert_eq!(res(&ns, "T001", &cx), Ok((a.clone(), "heuristic", "recent-scope")));
    assert_eq!(res(&ns, "T001", &Ctx::default()), Ok((x.clone(), "heuristic", "newest-scope")));
    assert_eq!(res(&ns, "T777", &Ctx::default()), Err("undefined"));
}

#[test]
fn one_open_row_wins() {
    let ns = both(&[
        ("specs/001-a/tasks.md", "# Tasks\n\n- [x] T001 build the a side\n"),
        ("specs/002-x/tasks.md", SPEC_X),
    ]);
    assert_eq!(
        res(&ns, "T001", &Ctx::default()),
        Ok(("specs/002-x/tasks.md".to_string(), "derived", "open-row"))
    );
}

#[test]
fn recent_and_newest_pick_only_in_the_pending_pool() {
    let ns = both(&[
        ("specs/001-a/tasks.md", "# Tasks\n\n- [x] T001 a\n"),
        ("specs/002-b/tasks.md", "# Tasks\n\n- [ ] T001 b\n"),
        ("specs/003-c/tasks.md", "# Tasks\n\n- [ ] T001 c\n"),
    ]);
    let r1 = vec!["specs/001-a".to_string()];
    let mut cx = Ctx::default();
    cx.recent = &r1;
    assert_eq!(res(&ns, "T001", &cx), Ok(("specs/003-c/tasks.md".to_string(), "heuristic", "newest-scope")));
    let r2 = vec!["specs/002-b".to_string()];
    let mut cx = Ctx::default();
    cx.recent = &r2;
    assert_eq!(res(&ns, "T001", &cx), Ok(("specs/002-b/tasks.md".to_string(), "heuristic", "recent-scope")));
}

#[test]
fn findings_series_are_local_and_restated_series_merge() {
    let findings = "# Review findings\n\n- F1: a\n- F2: b\n- F3: c\n- F4: d\n- F5: e\n";
    let decisions = "# Decisions\n\n- D1: use postgres for storage\n- D2: keep the api versioned\n- D3: one queue per tenant\n- D4: ship weekly\n";
    let ns = both(&[
        ("specs/001-a/review-findings.md", findings),
        ("specs/001-a/spec.md", "# Spec\n"),
        ("specs/002-x/plan.md", "# Plan\n"),
        ("docs/decisions.md", decisions),
        ("specs/002-x/decisions.md", decisions),
    ]);
    assert_eq!(ns.0.local, 1, "the findings series");
    assert_eq!(ns.0.merges, 1, "the restated decisions");

    let mut cx = Ctx::default();
    cx.own_page = Some("specs/002-x/plan.md");
    cx.own_scope = Some("specs/002-x");
    assert_eq!(res(&ns, "F3", &cx), Err("local-only"));
    let mut cx = Ctx::default();
    cx.own_page = Some("specs/001-a/spec.md");
    cx.own_scope = Some("specs/001-a");
    assert_eq!(
        res(&ns, "F3", &cx),
        Ok(("specs/001-a/review-findings.md".to_string(), "derived", "unique"))
    );

    let merged = "specs/002-x/decisions.md".to_string();
    for page in ["docs/decisions.md", "specs/002-x/decisions.md"] {
        let mut cx = Ctx::default();
        cx.own_page = Some(page);
        assert_eq!(res(&ns, "D2", &cx), Ok((merged.clone(), "fact", "own-page")), "{page}");
    }
    assert_eq!(res(&ns, "D2", &Ctx::default()), Ok((merged, "derived", "unique")));
}

#[test]
fn global_and_unique_series() {
    let ns = both(&[
        ("docs/requirements.md", "# Reqs\n\n- FR-001: users pay\n- FR-002: admins void\n"),
        ("specs/001-a/spec.md", "# Spec\n\n- FR-002: a local restatement elsewhere\n"),
    ]);
    assert_eq!(res(&ns, "FR-001", &Ctx::default()), Ok(("docs/requirements.md".to_string(), "derived", "unique")));
    assert_eq!(res(&ns, "FR-002", &Ctx::default()), Ok(("docs/requirements.md".to_string(), "derived", "global")));
}

#[test]
fn a_weak_mention_takes_its_same_line_siblings_namespace() {
    let ns = both(&[
        ("specs/001-a/decisions.md", "# Decisions\n\n- D1: alpha\n- D2: bravo\n- D3: charlie\n- D4: delta\n- D5: echo\n"),
        ("specs/001-a/notes.md", "# Notes\n\n- D5: other five\n- D6: six\n"),
        ("specs/002-x/decisions.md", "# Decisions\n\n- D1: zulu\n- D2: yankee\n- D3: xray\n- D4: whisky\n- D5: victor\n"),
    ]);
    assert_eq!(ns.0.merges, 0);
    let line = "D2/D5 in spec 001";
    let v = vocab();
    let ments = v.mentions(line);
    let mut cx = Ctx::default();
    cx.line_scopes = line_scopes(line, &ns.0);
    cx.text = line;
    let mut counts = RouteCounts::default();
    let got: Vec<_> = ns.0.resolve_line(line, &ments, &cx).into_iter().map(|r| {
        counts.add(&r);
        r.map(|r| (r.id, r.ns, r.tier, r.route))
    }).collect();
    let dec = "specs/001-a/decisions.md".to_string();
    assert_eq!(
        got,
        [
            Ok(("D2".to_string(), dec.clone(), "derived", "line-scope")),
            Ok(("D5".to_string(), dec, "derived", "sibling")),
        ]
    );
    assert_eq!(counts.render(), "line-scope:1,sibling:1");
    // D5 alone would have gone to the newest spec
    assert_eq!(
        res(&ns, "D5", &cx),
        Ok(("specs/002-x/decisions.md".to_string(), "heuristic", "newest-scope"))
    );
    let again: Vec<_> = ns.1.resolve_line(line, &ments, &cx).into_iter().map(|r| r.map(|r| r.ns)).collect();
    assert_eq!(again.len(), 2);
}

#[test]
fn lineage_merges_one_to_one_and_redirects_splits() {
    let tasks = "# Tasks\n\n- [ ] T015 (was T009) do the thing\n- [x] T009 the old thing\n- [ ] T020 split work\n  T020 split into T021\n  T020 split into T022\n- [ ] T021 part one\n- [ ] T022 part two\n";
    let ns = both(&[("specs/004-l/tasks.md", tasks)]);
    let k = |id: &str| Namespaces::key("specs/004-l/tasks.md", id);
    for n in [&ns.0, &ns.1] {
        assert_eq!(n.expand(&k("T009")), [k("T009"), k("T015")]);
        assert_eq!(n.expand(&k("T015")), [k("T009"), k("T015")]);
        assert_eq!(n.expand(&k("T020")), [k("T020"), k("T021"), k("T022")]);
        assert_eq!(n.expand(&k("T021")), [k("T021")]);
        assert_eq!(n.lineage_merges, 1);
        assert_eq!(n.lineage_redirects, 2);
        assert_eq!(n.lineage_capped, 0);
    }
    // T009 expands to T015's rows
    let rows: Vec<i64> = ns.0.expand(&k("T009")).iter().flat_map(|key| {
        let (nsn, id) = key.rsplit_once('|').expect("key");
        ns.0.rows.get(&(nsn.to_string(), id.to_string())).cloned().unwrap_or_default()
    }).map(|i| i as i64).collect();
    assert_eq!(rows.len(), 2);
}
