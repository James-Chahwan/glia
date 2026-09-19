//! LE.2 — `diff_impact`: the changed nodes of a change (a git rev's graph
//! delta, or a pasted unified diff) seeding ONE multi-seed, located, ranked
//! blast radius, each row attributed to the seed whose wave reached it.
//!
//! The tree: `shop/a.py` (`place` calls a multi-line `price`) and `shop/b.py`
//! (`checkout` calls `place`), committed through the LE.1b git fixture. The
//! engine prints `[diff-impact] mode=.. base=.. changed=.. seeds=..` per
//! answer; `cli/tests/diff_impact_cli.rs` asserts that line.

mod git_fixture;

use git_fixture::GitRepo;
use glia_engine::diff_impact::{ChangedNode, DiffImpact, diff_impact_from_diff, diff_impact_vs_rev};
use glia_engine::{BlastOptions, generate_one};
use glia_graph::Reach;

const A_PY: &str = "def price(o):\n    total = o\n    total = total + 0\n    total = total * 1\n    return total\n\n\n\
def place(o):\n    return price(o)\n";
/// `A_PY` with one line of `price`'s body edited.
const A_PY_EDITED: &str = "def price(o):\n    total = o or 0\n    total = total + 0\n    total = total * 1\n    return total\n\n\n\
def place(o):\n    return price(o)\n";
/// `A_PY` with two lines of `price`'s body deleted and none added.
const A_PY_SHORTER: &str = "def price(o):\n    total = o\n    return total\n\n\n\
def place(o):\n    return price(o)\n";
/// `A_PY` with the call in `place` removed.
const A_PY_NO_CALL: &str = "def price(o):\n    total = o\n    total = total + 0\n    total = total * 1\n    return total\n\n\n\
def place(o):\n    return o\n";
/// `A_PY_EDITED` plus a module-level import: the module's own text changes.
const A_PY_EDITED_IMPORT: &str = "import os\n\n\ndef price(o):\n    total = o or 0\n    total = total + 0\n    total = total * 1\n    return total\n\n\n\
def place(o):\n    return price(o)\n";
const B_PY: &str = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n";

const PRICE: &str = "shop::a::price";
const PLACE: &str = "shop::a::place";
const CHECKOUT: &str = "shop::b::checkout";
const MODULE_A: &str = "shop::a";

/// The two-file shop, committed on `main`.
fn committed_shop() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("shop/a.py", A_PY);
    repo.write("shop/b.py", B_PY);
    repo.commit("shop");
    repo
}

fn opts(direction: Reach) -> BlastOptions {
    let mut o = BlastOptions::default();
    o.direction = direction;
    o
}

fn changed<'a>(d: &'a DiffImpact, qname: &str) -> &'a ChangedNode {
    d.changed
        .iter()
        .find(|c| c.qname == qname)
        .unwrap_or_else(|| panic!("no changed row {qname} in {:#?}", d.changed))
}

/// `(qname, depth, seed)` per impact row, sorted by qname.
fn rows(d: &DiffImpact) -> Vec<(String, usize, String)> {
    let mut out: Vec<(String, usize, String)> = d
        .impact
        .results
        .iter()
        .map(|r| (r.qname.clone(), r.depth, r.seed.clone()))
        .collect();
    out.sort();
    out
}

fn seed_qnames(d: &DiffImpact) -> Vec<String> {
    d.impact.seeds.iter().map(|s| s.qname.clone()).collect()
}

/// (1) An edit inside `price` seeds `price` alone (its module is changed,
/// never a seed) and the backward radius names both callers, each attributed
/// to `price`.
#[test]
fn modified_leaf_impacts_callers() {
    let repo = committed_shop();
    repo.write("shop/a.py", A_PY_EDITED);
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &opts(Reach::Backward)).expect("diff impact");
    assert_eq!(d.base.as_deref(), Some("HEAD"));
    let price = changed(&d, PRICE);
    assert_eq!((price.change, price.seed), ("modified", true), "{price:#?}");
    assert_eq!(price.file.as_deref(), Some("shop/a.py"));
    assert_eq!(price.line, Some(1), "1-based: {price:#?}");
    for c in d.changed.iter().filter(|c| c.kind == "MODULE") {
        assert!(!c.seed, "a module is never a seed beside a finer one: {c:#?}");
    }
    assert_eq!(seed_qnames(&d), [PRICE]);
    // Sorted by qname: `shop::a::place` before `shop::b::checkout`.
    assert_eq!(
        rows(&d),
        [(PLACE.to_string(), 1, PRICE.to_string()), (CHECKOUT.to_string(), 2, PRICE.to_string())]
    );
    let place = d.impact.results.iter().find(|r| r.qname == PLACE).expect("place row");
    assert_eq!((place.file.as_deref(), place.line), (Some("shop/a.py"), Some(8)));
    assert!(d.edges_added.is_empty() && d.edges_removed.is_empty(), "{d:#?}");
    assert!(d.unresolved_diff_files.is_empty());
    assert!(d.impact.absence.is_none());
}

/// (2) Deleting the call in `place` removes the CALLS edge; its surviving
/// ends are seeds, so the caller that lost a call is in the answer.
#[test]
fn removed_call_seeds_the_caller() {
    let repo = committed_shop();
    repo.write("shop/a.py", A_PY_NO_CALL);
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &opts(Reach::Both)).expect("diff impact");
    let removed: Vec<(&str, &str, &str)> = d
        .edges_removed
        .iter()
        .map(|e| (e.category, e.from_qname.as_str(), e.to_qname.as_str()))
        .collect();
    assert_eq!(removed, [("CALLS", PLACE, PRICE)], "{:#?}", d.edges_removed);
    assert!(d.edges_added.is_empty(), "{:#?}", d.edges_added);
    let place = changed(&d, PLACE);
    assert!(place.seed, "{place:#?}");
    let price = changed(&d, PRICE);
    assert_eq!((price.change, price.seed), ("edge_endpoint", true), "{price:#?}");
    let seeds = seed_qnames(&d);
    assert!(seeds.contains(&PLACE.to_string()) && seeds.contains(&PRICE.to_string()), "{seeds:?}");
    // `checkout` still calls `place`: in radius, attributed to `place`.
    let checkout = d.impact.results.iter().find(|r| r.qname == CHECKOUT).expect("checkout row");
    assert_eq!((checkout.depth, checkout.seed.as_str()), (1, PLACE));
}

/// (3) The `git diff` of case (1), pasted over the working tree's graph,
/// answers with the same impact rows as the rev mode.
#[test]
fn pasted_diff_matches_rev_mode() {
    let repo = committed_shop();
    repo.write("shop/a.py", A_PY_EDITED);
    let rev = diff_impact_vs_rev(repo.path(), "HEAD", &opts(Reach::Backward)).expect("rev mode");
    let text = String::from_utf8_lossy(&repo.git(&["diff"]).stdout).into_owned();
    assert!(text.contains("+++ b/shop/a.py"), "{text}");
    let after = generate_one(repo.path()).expect("build the working tree");
    let pasted = diff_impact_from_diff(&after.merged, &text, &opts(Reach::Backward));
    assert_eq!(pasted.base, None);
    assert_eq!(rows(&pasted), rows(&rev));
    assert_eq!(seed_qnames(&pasted), [PRICE]);
    let hit = changed(&pasted, PRICE);
    assert_eq!((hit.change, hit.seed), ("diff_hit", true), "{hit:#?}");
    assert!(pasted.unresolved_diff_files.is_empty(), "{:?}", pasted.unresolved_diff_files);
}

/// (4) A hunk that only deletes lines: the rev mode still seeds `price` (its
/// text changed); the pasted diff has no added line to place, so it names
/// the file in `unresolved_diff_files` instead of answering "nothing changed".
#[test]
fn deletion_only_hunk_still_seeds_in_rev_mode() {
    let repo = committed_shop();
    repo.write("shop/a.py", A_PY_SHORTER);
    let rev = diff_impact_vs_rev(repo.path(), "HEAD", &opts(Reach::Backward)).expect("rev mode");
    assert_eq!(seed_qnames(&rev), [PRICE]);
    assert_eq!(changed(&rev, PRICE).change, "modified");
    assert!(!rev.impact.results.is_empty());

    let text = String::from_utf8_lossy(&repo.git(&["diff"]).stdout).into_owned();
    assert!(!text.lines().any(|l| l.starts_with('+') && !l.starts_with("+++")), "{text}");
    let after = generate_one(repo.path()).expect("build the working tree");
    let pasted = diff_impact_from_diff(&after.merged, &text, &opts(Reach::Backward));
    assert_eq!(pasted.unresolved_diff_files, ["shop/a.py"]);
    assert!(pasted.changed.is_empty(), "{:#?}", pasted.changed);
    assert!(pasted.impact.results.is_empty() && pasted.impact.seeds.is_empty());
    let absence = pasted.impact.absence.as_ref().expect("absence");
    assert_eq!(absence.reason, "no_match");
    assert!(absence.note.contains("the diff resolved to no node"), "{}", absence.note);
    assert!(absence.note.contains("shop/a.py"), "{}", absence.note);
}

/// (5) A clean tree: nothing changed, no radius, and the absence says so.
#[test]
fn empty_delta_has_absence() {
    let repo = committed_shop();
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &BlastOptions::default()).expect("diff impact");
    assert!(d.changed.is_empty(), "{:#?}", d.changed);
    assert!(d.impact.results.is_empty() && d.impact.seeds.is_empty());
    let absence = d.impact.absence.as_ref().expect("absence on a clean tree");
    assert_eq!(absence.reason, "no_match");
    assert_eq!(absence.note, "no graph change vs HEAD");
}

/// A module whose own text changed (a new import) beside an edited function
/// in the same file is changed, not a seed: the function carries the edges.
/// The same holds for the module-level line of a pasted diff.
#[test]
fn module_seed_dropped_beside_a_finer_seed() {
    let repo = committed_shop();
    repo.write("shop/a.py", A_PY_EDITED_IMPORT);
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &opts(Reach::Backward)).expect("rev mode");
    let module = changed(&d, MODULE_A);
    assert_eq!((module.change, module.seed), ("modified", false), "{module:#?}");
    assert_eq!(seed_qnames(&d), [PRICE]);

    let text = String::from_utf8_lossy(&repo.git(&["diff"]).stdout).into_owned();
    let after = generate_one(repo.path()).expect("build the working tree");
    let pasted = diff_impact_from_diff(&after.merged, &text, &opts(Reach::Backward));
    let module = changed(&pasted, MODULE_A);
    assert_eq!((module.change, module.seed), ("diff_hit", false), "{module:#?}");
    assert_eq!(seed_qnames(&pasted), [PRICE]);
    assert_eq!(rows(&pasted), rows(&d));
}

/// A deleted file: its nodes are changed rows located in the BEFORE graph and
/// never seeds; the node they called is seeded through the removed edge.
#[test]
fn removed_nodes_are_reported_not_seeded() {
    let repo = committed_shop();
    repo.remove("shop/b.py");
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &opts(Reach::Forward)).expect("diff impact");
    let checkout = changed(&d, CHECKOUT);
    assert_eq!((checkout.change, checkout.seed), ("removed", false), "{checkout:#?}");
    assert_eq!((checkout.file.as_deref(), checkout.line), (Some("shop/b.py"), Some(4)));
    assert!(d.edges_removed.iter().any(|e| e.category == "CALLS" && e.from_qname == CHECKOUT && e.to_qname == PLACE));
    assert_eq!(seed_qnames(&d), [PLACE]);
    assert_eq!(changed(&d, PLACE).change, "edge_endpoint");
    assert_eq!(rows(&d), [(PRICE.to_string(), 1, PLACE.to_string())]);
}

/// A moved file: its nodes are seeds under their new qname.
#[test]
fn moved_nodes_seed_under_their_new_identity() {
    let repo = committed_shop();
    repo.git_mv("shop/b.py", "shop/c.py");
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &opts(Reach::Forward)).expect("diff impact");
    let moved = changed(&d, "shop::c::checkout");
    assert_eq!((moved.change, moved.seed), ("moved", true), "{moved:#?}");
    assert_eq!(moved.file.as_deref(), Some("shop/c.py"));
    assert!(seed_qnames(&d).contains(&"shop::c::checkout".to_string()), "{:?}", seed_qnames(&d));
    let place = d.impact.results.iter().find(|r| r.qname == PLACE).expect("place row");
    assert_eq!((place.depth, place.seed.as_str()), (1, "shop::c::checkout"));
    assert!(repo.root().join("shop/c.py").is_file());
}

/// Options reach the blast radius: `top_k` cuts, `scope` keeps rows under it,
/// and an unknown rev is an error, never an empty answer.
#[test]
fn options_reach_the_radius_and_bad_rev_is_an_error() {
    let repo = committed_shop();
    repo.write("shop/a.py", A_PY_EDITED);
    let mut o = opts(Reach::Backward);
    o.top_k = Some(1);
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &o).expect("top_k");
    assert_eq!(d.impact.results.len(), 1);
    let mut o = opts(Reach::Backward);
    o.scope = Some("shop/b.py".to_string());
    let d = diff_impact_vs_rev(repo.path(), "HEAD", &o).expect("scope");
    assert_eq!(rows(&d), [(CHECKOUT.to_string(), 2, PRICE.to_string())]);
    let err = match diff_impact_vs_rev(repo.path(), "no-such-rev", &o) {
        Ok(_) => panic!("an unknown rev must be an error"),
        Err(e) => e,
    };
    assert!(err.contains("no-such-rev"), "{err}");
}

/// A plain changed-file list seeds every node of each file it names; a path
/// no node sits in is listed as unresolved.
#[test]
fn changed_file_list_seeds_files_and_lists_unplaced_ones() {
    let repo = committed_shop();
    let g = generate_one(repo.path()).expect("build");
    let d = diff_impact_from_diff(&g.merged, "shop/b.py\ndocs/notes.md\n", &opts(Reach::Forward));
    assert_eq!(d.unresolved_diff_files, ["docs/notes.md"]);
    assert!(d.changed.iter().any(|c| c.qname == CHECKOUT && c.seed), "{:#?}", d.changed);
    assert!(rows(&d).iter().any(|(q, _, s)| q == PLACE && s == CHECKOUT), "{:?}", rows(&d));
}
