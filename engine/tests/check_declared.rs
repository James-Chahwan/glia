//! DT.7b: the declared rules that check nothing. `check_with_roots` re-reads
//! each repo root's `.glia/overlay.toml`, binds its `[[constraint]]` /
//! `[[component]]` / `[[layer]]` stanzas through the build's own binder and
//! lists, in `CheckReport::unanchored`, every one that reached no
//! CONSTRAINT cell (orphaned, rejected or absent from the graph), after the
//! rules whose scope no located node sits in (empty_scope). The rows never
//! block unless `CheckOptions::strict`, which also makes each an errors row.
//! `check(merged)` reads no overlay and keeps an empty scope an errors row.
//!
//! The base tree is DT.7a's substrate-gap fixture `overlay-subdir-constraint`
//! (read only, copied into a temp dir per test): its `stale-anchor` stanza
//! names a qname no node carries, so the build orphans it.

use std::path::{Path, PathBuf};

use glia_engine::check::{
    ABSENT, CheckOptions, CheckReport, EMPTY_SCOPE, ORPHANED, REJECTED, Unanchored, check,
    check_with_roots,
};
use glia_engine::{GenerateResult, generate_one};

const FIXTURE: &str = "../bench/substrate-gap/fixtures/overlay-subdir-constraint";

/// `from` copied into `to`, recursively.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The fixture under `<tmp>/fx`, its overlay replaced by `overlay` when given.
fn fixture(tmp: &Path, overlay: Option<&str>) -> PathBuf {
    let dir = tmp.join("fx");
    copy_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE), &dir);
    if let Some(o) = overlay {
        std::fs::write(dir.join(".glia/overlay.toml"), o).unwrap();
    }
    dir
}

fn build(dir: &Path) -> GenerateResult {
    generate_one(dir.to_str().unwrap()).unwrap()
}

fn roots(r: &GenerateResult) -> Vec<(u64, PathBuf)> {
    r.repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect()
}

fn strict() -> CheckOptions {
    let mut o = CheckOptions::default();
    o.strict = true;
    o
}

/// `(rule_id, section, decl, status, reason)` of each row.
fn rows(r: &CheckReport) -> Vec<(&str, &str, Option<&str>, &str, &str)> {
    r.unanchored
        .iter()
        .map(|u: &Unanchored| {
            (
                u.rule_id.as_str(),
                u.section,
                u.decl.as_deref(),
                u.status,
                u.reason.as_str(),
            )
        })
        .collect()
}

const STALE: &str = "orphaned: anchor=backend::internal::gone::Thing binds no node";

#[test]
fn stale_anchor_is_a_rule_that_checks_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path(), None);
    let r = build(&dir);

    let bare = check(&r.merged);
    assert!(
        bare.unanchored.is_empty() && bare.errors.is_empty(),
        "{bare:#?}"
    );

    let lax = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert_eq!(
        rows(&lax),
        [(
            "stale-anchor",
            "constraint",
            Some(".glia/overlay.toml:20"),
            ORPHANED,
            STALE
        )]
    );
    let u = &lax.unanchored[0];
    assert!(!u.blocking);
    assert_eq!(u.repo.as_deref(), Some(dir.to_str().unwrap()));
    assert!(
        u.fix
            .starts_with("drop `anchor` to place it by its `scope`"),
        "{}",
        u.fix
    );
    assert!(lax.errors.is_empty(), "{:?}", lax.errors);
    // Nothing else moves: the rules the graph holds are checked as before.
    assert_eq!((lax.rules, lax.checked), (bare.rules, bare.checked));
    assert_eq!(
        lax.violations
            .iter()
            .map(|v| v.rule_id.as_str())
            .collect::<Vec<_>>(),
        bare.violations
            .iter()
            .map(|v| v.rule_id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        lax.violations
            .iter()
            .map(|v| v.rule_id.as_str())
            .collect::<Vec<_>>(),
        ["handlers-no-repository"]
    );

    let hard = check_with_roots(&r.merged, &roots(&r), &strict());
    assert!(hard.unanchored[0].blocking);
    assert_eq!(hard.errors.len(), 1, "{:?}", hard.errors);
    let (id, msg) = &hard.errors[0];
    assert_eq!(id, "stale-anchor");
    assert_eq!(
        msg,
        &format!(
            "constraint .glia/overlay.toml:20: {STALE} - not evaluated\nfix: {}",
            u.fix
        )
    );

    // The JSON carries the rows only when there is one.
    let json = serde_json::to_value(&lax).unwrap();
    assert_eq!(json["unanchored"][0]["status"], "orphaned");
    assert!(
        serde_json::to_value(&bare)
            .unwrap()
            .get("unanchored")
            .is_none()
    );
}

/// No roots, or a root without an overlay: the report is `check`'s.
#[test]
fn no_overlay_is_check_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path(), None);
    let r = build(&dir);
    let bare = serde_json::to_string(&check(&r.merged)).unwrap();
    assert_eq!(
        serde_json::to_string(&check_with_roots(&r.merged, &[], &strict())).unwrap(),
        bare
    );
    std::fs::remove_file(dir.join(".glia/overlay.toml")).unwrap();
    assert_eq!(
        serde_json::to_string(&check_with_roots(&r.merged, &roots(&r), &strict())).unwrap(),
        bare
    );
}

/// A component on a path another claimed (a project label and its
/// directory are one path) is rejected; a component whose
/// path holds no code orphans, and so does the layer it heads (no root
/// PROJECT in this tree); a rule whose `to` no located node sits in is
/// empty_scope - an errors row for `check`, a row that checks nothing for
/// `check_with_roots`, both under strict.
#[test]
fn rejected_orphaned_and_empty_scope_rows() {
    let overlay = r#"version = 1

[[component]]
name = "backend"
paths = ["backend"]

[[component]]
name = "again"
paths = ["example.com/backend"]

[[component]]
name = "nowhere"
paths = ["ghost/dir"]

[[layer]]
name = "top"
components = ["nowhere"]

[[constraint]]
id = "no-ghost"
kind = "forbid_edge"
from = "backend/internal/handlers"
to = "backend/internal/ghost"
"#;
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path(), Some(overlay));
    let r = build(&dir);
    let empty = "to scope `backend/internal/ghost` matches no located node (no file sits under it and no PROJECT is at it)";

    let bare = check(&r.merged);
    assert_eq!(bare.errors, [("no-ghost".to_string(), empty.to_string())]);
    assert!(bare.unanchored.is_empty());

    let lax = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert_eq!(
        rows(&lax),
        [
            (
                "no-ghost",
                "constraint",
                Some(".glia/overlay.toml:19"),
                EMPTY_SCOPE,
                empty
            ),
            (
                "component:again",
                "component",
                Some(".glia/overlay.toml:7"),
                REJECTED,
                "rejected: path example.com/backend -> backend is already claimed by component backend"
            ),
            (
                "component:nowhere",
                "component",
                Some(".glia/overlay.toml:11"),
                ORPHANED,
                "orphaned: path=ghost/dir -> ghost/dir (no PROJECT or MODULE)"
            ),
            (
                "layer:top",
                "layer",
                Some(".glia/overlay.toml:15"),
                ORPHANED,
                "orphaned: component=nowhere (no root PROJECT; the component did not anchor)"
            ),
        ]
    );
    assert!(lax.errors.is_empty(), "{:?}", lax.errors);
    assert!(lax.unanchored.iter().all(|u| !u.blocking));
    let fix = |id: &str| {
        lax.unanchored
            .iter()
            .find(|u| u.rule_id == id)
            .map(|u| u.fix.clone())
            .unwrap()
    };
    assert!(
        fix("no-ghost").contains("the nearest path above it that holds code is `backend/internal`"),
        "{}",
        fix("no-ghost")
    );
    assert!(
        fix("layer:top").starts_with("anchor component `nowhere` first"),
        "{}",
        fix("layer:top")
    );
    // An empty-scope row reads across the merged repos, so it names none.
    assert_eq!(lax.unanchored[0].repo, None);

    let hard = check_with_roots(&r.merged, &roots(&r), &strict());
    let ids: Vec<&str> = hard.errors.iter().map(|(id, _)| id.as_str()).collect();
    // The rule's own error at its place, then the misses after the model's.
    assert_eq!(
        ids,
        [
            "no-ghost",
            "component:again",
            "component:nowhere",
            "layer:top"
        ]
    );
    assert!(
        hard.errors[0].1.starts_with(&format!("{empty}\nfix: ")),
        "{}",
        hard.errors[0].1
    );
    assert!(hard.unanchored.iter().all(|u| u.blocking));
}

/// A valid stanza added to the overlay after the build binds, but the graph
/// holds no rule for it: absent (a layout older than its overlay).
#[test]
fn overlay_edited_after_the_build_is_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path(), None);
    let r = build(&dir);
    let path = dir.join(".glia/overlay.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(
        "\n[[constraint]]\nid = \"late\"\nkind = \"no_cycle\"\nscope = \"frontend/src\"\n",
    );
    std::fs::write(&path, text).unwrap();
    let report = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert_eq!(
        rows(&report),
        [
            (
                "stale-anchor",
                "constraint",
                Some(".glia/overlay.toml:20"),
                ORPHANED,
                STALE
            ),
            (
                "late",
                "constraint",
                Some(".glia/overlay.toml:26"),
                ABSENT,
                "absent: declared in the overlay but not in this graph (rebuild; or the build refused to merge it, see its [declared] rejected= line)"
            ),
        ]
    );
    // Rebuilt, the late rule is held and only the stale anchor remains.
    let again = build(&dir);
    let report = check_with_roots(&again.merged, &roots(&again), &CheckOptions::default());
    assert_eq!(
        rows(&report),
        [(
            "stale-anchor",
            "constraint",
            Some(".glia/overlay.toml:20"),
            ORPHANED,
            STALE
        )]
    );
}
