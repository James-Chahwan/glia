//! DT.7b: the declared rules that check nothing. `check_with_roots` re-reads
//! each repo root's `.glia/overlay.toml`, binds its `[[constraint]]` /
//! `[[component]]` / `[[layer]]` stanzas through the build's own binder and
//! lists, in `CheckReport::unanchored`, every one that reached no
//! CONSTRAINT cell (orphaned, rejected or absent from the graph), after the
//! rules whose scope no located node sits in (empty_scope). The rows never
//! block unless `CheckOptions::strict`, which also makes each an errors row.
//! `check(merged)` reads no overlay and keeps an empty scope an errors row.
//! DT.7c: the rule stanzas the overlay loader itself drops (and a whole file
//! it ignores while it holds one) are REJECTED rows too, before the binder's.
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

/// DT.7c's fx-dup + a [[layer]] and an allow naming the dropped component +
/// a version-1 transitive stanza (`fx-all`): the overlay loader drops four
/// stanzas (:18, :22, :26, :32), which reach the build only as `[overlay]
/// error:` lines.
const FX_ALL: &str = r#"version = 1

[[constraint]]
id = "handlers-no-repository"
kind = "forbid_edge"
from = "backend/internal/handlers"
to = "backend/internal/repository"
categories = ["CALLS"]

[[component]]
name = "api"
paths = ["backend"]

[[component]]
name = "web"
paths = ["frontend"]

[[component]]
name = "api-copy"
paths = ["backend"]

[[layer]]
name = "top"
components = ["api-copy"]

[[constraint]]
id = "web-uses-copy"
kind = "allow"
from = "web"
to = "api-copy"

[[constraint]]
id = "frontend-no-backend"
kind = "forbid_edge"
from = "frontend"
to = "backend"
transitive = true
"#;

/// The stanza the fixture orphans, appended to FX_ALL at line 39.
const STALE_STANZA: &str = "
[[constraint]]
id = \"stale-anchor\"
kind = \"invariant\"
anchor = \"backend::internal::gone::Thing\"
text = \"an anchor no node carries\"
";

/// DT.7c: every rule stanza the overlay loader drops is a REJECTED row at
/// its line, with the loader's message and a fix, before the binder's rows
/// (HEAD: none of them was listed; `check` exited 1 with `errors=0` and the
/// stanzas on stderr only). Non-blocking unless strict.
#[test]
fn loader_dropped_stanzas_are_rejected_rows() {
    let overlay = format!("{FX_ALL}{STALE_STANZA}");
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(tmp.path(), Some(&overlay));
    let r = build(&dir);

    let bare = check(&r.merged);
    assert!(
        bare.unanchored.is_empty() && bare.errors.is_empty(),
        "{bare:#?}"
    );

    let lax = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert_eq!(
        rows(&lax),
        [
            (
                "component:api-copy",
                "component",
                Some(".glia/overlay.toml:18"),
                REJECTED,
                "rejected: path \"backend\" is already claimed by component \"api\" (stanza dropped)"
            ),
            (
                "layer:top",
                "layer",
                Some(".glia/overlay.toml:22"),
                REJECTED,
                "rejected: component \"api-copy\" is not a declared [[component]] (stanza dropped)"
            ),
            (
                "web-uses-copy",
                "constraint",
                Some(".glia/overlay.toml:26"),
                REJECTED,
                "rejected: kind allow names \"api-copy\", which is not a declared [[component]] (stanza dropped)"
            ),
            (
                "frontend-no-backend",
                "constraint",
                Some(".glia/overlay.toml:32"),
                REJECTED,
                "rejected: `transitive` need `version = 2` (this file says version = 1) (stanza dropped)"
            ),
            (
                "stale-anchor",
                "constraint",
                Some(".glia/overlay.toml:39"),
                ORPHANED,
                STALE
            ),
        ]
    );
    assert!(lax.unanchored.iter().all(|u| !u.blocking));
    assert!(
        lax.unanchored
            .iter()
            .all(|u| u.repo.as_deref() == Some(dir.to_str().unwrap()))
    );
    assert!(lax.errors.is_empty(), "{:?}", lax.errors);
    let fix = |id: &str| {
        lax.unanchored
            .iter()
            .find(|u| u.rule_id == id)
            .map(|u| u.fix.clone())
            .unwrap()
    };
    let undeclared = "declare that [[component]] or correct the name (a [[component]] the loader dropped is listed here too: fix it first)";
    assert_eq!(fix("layer:top"), undeclared);
    assert_eq!(fix("web-uses-copy"), undeclared);
    assert!(
        fix("frontend-no-backend").starts_with("set `version = 2` at the top of .glia/overlay.toml"),
        "{}",
        fix("frontend-no-backend")
    );
    // The rules the graph holds are checked as before.
    assert_eq!((lax.rules, lax.checked), (bare.rules, bare.checked));

    let hard = check_with_roots(&r.merged, &roots(&r), &strict());
    let ids: Vec<&str> = hard.errors.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "component:api-copy",
            "layer:top",
            "web-uses-copy",
            "frontend-no-backend",
            "stale-anchor"
        ]
    );
    assert!(
        hard.errors[0].1.starts_with(
            "component .glia/overlay.toml:18: rejected: path \"backend\" is already claimed by component \"api\" (stanza dropped) - not evaluated\nfix: give the path to one component"
        ),
        "{}",
        hard.errors[0].1
    );
    assert!(hard.unanchored.iter().all(|u| u.blocking));
}

/// The handlers-no-repository stanza under `head`, its last line `last`
/// (line 8 when `head` is one line).
fn handlers_overlay(head: &str, last: &str) -> String {
    format!(
        "{head}\n[[constraint]]\nid = \"handlers-no-repository\"\nkind = \"forbid_edge\"\nfrom = \"backend/internal/handlers\"\nto = \"backend/internal/repository\"\n{last}\n"
    )
}

/// DT.7c: a whole file the loader ignores while it holds a rule stanza is
/// one REJECTED row, section `overlay` (HEAD: `glia check` said "no rules
/// declared" and exited 0); a file with no rule stanza gives none.
#[test]
fn whole_file_rejection_is_one_row() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = fixture(
        tmp.path(),
        Some(&handlers_overlay("version = 2\n", "categorys = [\"CALLS\"]")),
    );
    let r = build(&dir);
    let report = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert_eq!(
        rows(&report),
        [(
            ".glia/overlay.toml",
            "overlay",
            Some(".glia/overlay.toml:8"),
            REJECTED,
            "rejected: unknown field `categorys`, expected one of `id`, `kind`, `from`, `to`, `categories`, `scope`, `anchor`, `text`, `origin`, `rule`, `transitive`, `via`, `min_tier`; the whole file is ignored (rule stanzas not read: constraint=1 component=0 layer=0)"
        )]
    );
    assert_eq!(report.rules, 0);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!report.unanchored[0].blocking);
    assert!(
        report.unanchored[0]
            .fix
            .starts_with("correct or remove the key the message names"),
        "{}",
        report.unanchored[0].fix
    );
    let hard = check_with_roots(&r.merged, &roots(&r), &strict());
    assert_eq!(hard.errors.len(), 1, "{:?}", hard.errors);
    assert_eq!(hard.errors[0].0, ".glia/overlay.toml");

    // An unsupported version: the row is at its line, every rule header counted.
    let path = dir.join(".glia/overlay.toml");
    let v1 = format!(
        "{}\n[[constraint]]\nid = \"frontend-no-backend\"\nkind = \"forbid_edge\"\nfrom = \"frontend\"\nto = \"backend\"\ntransitive = true\n",
        handlers_overlay("version = 3\n", "categories = [\"CALLS\"]")
    );
    std::fs::write(&path, &v1).unwrap();
    let ver = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert_eq!(
        rows(&ver),
        [(
            ".glia/overlay.toml",
            "overlay",
            Some(".glia/overlay.toml:1"),
            REJECTED,
            "rejected: version = 3 is not supported (this glia reads versions 1 and 2); the whole file is ignored (rule stanzas not read: constraint=2 component=0 layer=0)"
        )]
    );
    assert_eq!(
        ver.unanchored[0].fix,
        "set `version = 2` (this glia reads versions 1 and 2)"
    );

    // A missing version names no line.
    std::fs::write(&path, handlers_overlay("", "categories = [\"CALLS\"]")).unwrap();
    let nover = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert_eq!(nover.unanchored.len(), 1, "{:?}", nover.unanchored);
    assert_eq!(nover.unanchored[0].decl.as_deref(), Some(".glia/overlay.toml"));
    assert_eq!(
        nover.unanchored[0].fix,
        "add `version = 1` as the first line (`version = 2` to use rule / transitive / via / min_tier)"
    );

    // A syntax error in a file with no rule stanza is the build's and gaps'
    // concern, not check's.
    std::fs::write(
        &path,
        "version = 1\n\n[[edge]]\nfrom = \"a::b\nto = \"c::d\"\ncategory = \"CALLS\"\n",
    )
    .unwrap();
    let edge_only = check_with_roots(&r.merged, &roots(&r), &CheckOptions::default());
    assert!(edge_only.unanchored.is_empty(), "{:?}", edge_only.unanchored);
    assert_eq!(
        serde_json::to_string(&edge_only).unwrap(),
        serde_json::to_string(&check(&r.merged)).unwrap()
    );
}
