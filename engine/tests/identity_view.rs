//! DD.8: the identity view every answer collapses through and the
//! `identity_classes` report, over `generate_many` builds of DD.5's
//! substrate-gap identity fixtures (real builds, not hand-built graphs).
//!
//! Baseline before DD.8 (HEAD e9bbe6fc): `glia_engine::identity` was a
//! doc-only slot, so this file did not compile (no `Collapse`,
//! `IdentityView`, `identity_classes`).
//!
//! The `[identity] report` / `[identity] collapse` markers are asserted from a
//! child run of this test binary ([`child_markers_for_stderr`]), the
//! identity_resolver.rs pattern.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::{self, Evidence};
use glia_core::{Confidence, Edge, NodeId};
use glia_engine::identity::{
    AlsoIn, Collapse, IdentityQuery, IdentityReport, IdentityView, RowIdentity, identity_classes,
};
use glia_engine::{GenerateResult, generate_many};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

fn build_paths(paths: &[PathBuf]) -> GenerateResult {
    let paths: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

fn build(root: &Path, dirs: &[&str]) -> GenerateResult {
    let paths: Vec<PathBuf> = dirs.iter().map(|d| root.join(d)).collect();
    build_paths(&paths)
}

/// The node with `qname` in the graph of the repo labelled `label`.
fn node(r: &GenerateResult, label: &str, qname: &str) -> NodeId {
    r.merged
        .graphs
        .iter()
        .filter(|g| r.repo_labels.get(&g.repo.0).map(String::as_str) == Some(label))
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(|(_, q)| q.as_str() == qname)
                .map(|(id, _)| *id)
                .min_by_key(|id| id.0)
        })
        .unwrap_or_else(|| panic!("no {qname} in {label}: {:?}", r.repo_labels))
}

fn same_as_edges(r: &GenerateResult) -> usize {
    r.merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .count()
}

/// The report with every default, labels from the build.
fn report(
    r: &GenerateResult,
    f: impl FnOnce(&mut IdentityQuery),
) -> Result<IdentityReport, String> {
    let mut q = IdentityQuery::default();
    q.repo_labels = r.repo_labels.clone();
    f(&mut q);
    identity_classes(&r.merged, &q)
}

/// `(repo, qname, file, line)` of an also_in row.
fn also(a: &AlsoIn) -> (String, String, Option<String>, Option<i64>) {
    (a.repo.clone(), a.qname.clone(), a.file.clone(), a.line)
}

const MONEY_DEP: &str = "package:npm:@shop/money";
const MONEY_PROJECT: &str = "project:packages/money";
const ORDER_CREATED: &str = "message:proto:shop.v1.OrderCreated";

#[test]
fn no_same_as_no_view() {
    let r = build(&fixture("identity-npm-package"), &["web"]);
    assert_eq!(same_as_edges(&r), 0, "control: one repo, no class");
    let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    assert!(view.is_empty());
    assert!(!view.has_walk_classes());
    assert!(view.quotient(&r.merged).is_none());
    let react = node(&r, "web", "package:npm:react");
    assert_eq!(view.display(react), react);
    assert_eq!(view.walk_rep(react), react);
    assert!(view.row_identity(react, react).is_none());
    let rows = vec![react, react];
    let folded = view.fold(
        rows.clone(),
        |id| *id,
        |_, _| panic!("no identity outside a class"),
    );
    assert_eq!(folded, rows, "an empty view folds nothing");

    let rep = report(&r, |_| {}).expect("a valid query");
    assert!(rep.classes.is_empty(), "{:?}", rep.classes);
    assert_eq!(rep.counts.classes, 0);
    let a = rep.absence.expect("an empty report says why");
    assert_eq!(a.reason, "no_match");
    assert!(a.note.contains("no SAME_AS edge"), "{}", a.note);

    // Collapse::off() is an empty view even over a build with classes.
    let two = build(&fixture("identity-npm-package"), &["web", "admin"]);
    assert!(same_as_edges(&two) > 0, "control: two repos share react");
    assert!(IdentityView::new(&two.merged, &Collapse::off()).is_empty());
    assert!(!IdentityView::new(&two.merged, &Collapse::default()).is_empty());
}

#[test]
fn tiers_and_walks() {
    // identity-npm-workspace-lib: one FACT class of the PROJECT and both
    // @shop/money deps; manifest_name walks, so it is one walk class.
    let r = build(
        &fixture("identity-npm-workspace-lib"),
        &["monorepo", "admin"],
    );
    let rep = report(&r, |_| {}).unwrap();
    assert_eq!(rep.classes.len(), 1, "{:?}", rep.classes);
    let c = &rep.classes[0];
    assert_eq!(c.tier, "fact");
    assert_eq!(c.rules, ["package_name", "manifest_name"], "table order");
    assert!(c.collapses && c.walks && !c.diverged);
    let mut members: Vec<(&str, &str)> = c
        .members
        .iter()
        .map(|m| (m.repo.as_str(), m.qname.as_str()))
        .collect();
    members.sort();
    assert_eq!(
        members,
        [
            ("admin", MONEY_DEP),
            ("monorepo", MONEY_DEP),
            ("monorepo", MONEY_PROJECT)
        ]
    );
    // The display member is the first graph's: a monorepo node.
    assert_eq!(c.members[0].repo, "monorepo");
    assert_eq!(c.key, c.members[0].qname);

    let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    assert!(!view.is_empty() && view.has_walk_classes());
    let project = node(&r, "monorepo", MONEY_PROJECT);
    let dep_mono = node(&r, "monorepo", MONEY_DEP);
    let dep_admin = node(&r, "admin", MONEY_DEP);
    let rep_id = view.walk_rep(project);
    assert_eq!(view.walk_rep(dep_mono), rep_id);
    assert_eq!(view.walk_rep(dep_admin), rep_id);
    let q = view
        .quotient(&r.merged)
        .expect("a walk class gives a quotient");
    assert_eq!(q.rep(dep_admin), rep_id);
    // Every member, the walk representative included, displays as one node.
    let shown = view.display(rep_id);
    for id in [project, dep_mono, dep_admin] {
        assert_eq!(view.display(id), shown);
    }

    // identity-npm-package: react and lodash collapse, but package_name does
    // not walk, so there is no walk class at all.
    let r = build(&fixture("identity-npm-package"), &["web", "admin"]);
    let rep = report(&r, |_| {}).unwrap();
    let keys: Vec<(&str, &str, bool, bool)> = rep
        .classes
        .iter()
        .map(|c| (c.key.as_str(), c.tier, c.collapses, c.walks))
        .collect();
    assert_eq!(
        keys,
        [
            ("package:npm:lodash", "fact", true, false),
            ("package:npm:react", "fact", true, false)
        ]
    );
    let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    assert!(!view.is_empty() && !view.has_walk_classes());
    assert!(view.quotient(&r.merged).is_none());
    let (web, admin) = (
        node(&r, "web", "package:npm:react"),
        node(&r, "admin", "package:npm:react"),
    );
    assert_eq!(view.walk_rep(admin), admin, "not contracted in walks");
    assert_eq!(view.display(admin), web, "first graph displays");

    // identity-contract-ops: one DERIVED class, contract_op + contract_route.
    let r = build(&fixture("identity-contract-ops"), &["."]);
    let rep = report(&r, |_| {}).unwrap();
    assert_eq!(rep.classes.len(), 1, "{:?}", rep.classes);
    let c = &rep.classes[0];
    assert_eq!((c.tier, c.collapses, c.walks), ("derived", true, true));
    assert_eq!(c.rules, ["contract_op", "contract_route"]);
    assert_eq!(c.members.len(), 3);
    assert_eq!((rep.counts.fact, rep.counts.derived), (0, 1));
}

#[test]
fn diverged_and_variants() {
    let r = build(
        &fixture("identity-proto-diverged"),
        &["orders", "billing", "shipping"],
    );
    let rep = report(&r, |_| {}).unwrap();
    assert_eq!(rep.classes.len(), 1, "{:?}", rep.classes);
    let c = &rep.classes[0];
    assert_eq!((c.key.as_str(), c.tier), (ORDER_CREATED, "fact"));
    assert_eq!(c.rules, ["message_fqname"]);
    assert!(c.diverged && c.collapses && c.walks);
    assert_eq!(c.variants, 2);
    let variants: Vec<(&str, Option<u32>)> = c
        .members
        .iter()
        .map(|m| (m.repo.as_str(), m.variant))
        .collect();
    assert_eq!(
        variants,
        [
            ("orders", Some(0)),
            ("billing", Some(0)),
            ("shipping", Some(1))
        ],
        "shipping's extra field is alone in its variant"
    );
    assert!(c.members.iter().all(|m| m.kind == "MESSAGE_TYPE"));
    assert_eq!(rep.counts.diverged, 1);
    let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    let orders = node(&r, "orders", ORDER_CREATED);
    let id = view.row_identity(orders, orders).expect("a collapse class");
    assert!(id.diverged);
    assert_eq!(
        (id.tier, id.rules.as_slice()),
        ("fact", ["message_fqname"].as_slice())
    );
}

/// Copy `src` to `dst`, leaving out the layout and cache dirs a build may
/// have written into the fixture.
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let name = entry.file_name();
        if name == ".glia" || name == ".ai" {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

#[test]
fn display_is_content_chosen() {
    let dirs = ["orders", "billing", "shipping"];
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("renamed");
    let renamed: Vec<String> = dirs.iter().map(|d| format!("{d}-copy")).collect();
    for (d, n) in dirs.iter().zip(&renamed) {
        copy_tree(&fixture("identity-proto-diverged").join(d), &copy.join(n));
    }
    let a = build(&fixture("identity-proto-diverged"), &dirs);
    let b = build_paths(&renamed.iter().map(|n| copy.join(n)).collect::<Vec<_>>());
    let repos = |r: &GenerateResult| r.merged.graphs.iter().map(|g| g.repo).collect::<Vec<_>>();
    assert_ne!(repos(&a), repos(&b), "control: the RepoIds differ");

    // What a reader sees of one build: the shown member and its also_in, with
    // the `-copy` suffix taken off the labels.
    let seen = |r: &GenerateResult, members: [&str; 3]| {
        let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
        let ids: Vec<NodeId> = members.iter().map(|l| node(r, l, ORDER_CREATED)).collect();
        let shown = view.display(ids[2]);
        assert!(
            ids.iter().all(|id| view.display(*id) == shown),
            "one display member"
        );
        let label = |l: &str| l.trim_end_matches("-copy").to_string();
        let shown_repo = ids
            .iter()
            .zip(members)
            .find(|(id, _)| **id == shown)
            .map(|(_, l)| label(l))
            .expect("the display member is a member");
        let also_in: Vec<_> = view
            .row_identity(shown, shown)
            .expect("a collapse class")
            .also_in
            .iter()
            .map(|x| {
                let (repo, q, f, l) = also(x);
                (label(&repo), q, f, l)
            })
            .collect();
        (shown_repo, also_in)
    };
    let sa = seen(&a, ["orders", "billing", "shipping"]);
    let sb = seen(&b, ["orders-copy", "billing-copy", "shipping-copy"]);
    assert_eq!(sa.0, "orders", "the first graph in argument order");
    assert_eq!(sa, sb, "a RepoId move does not move the shown row");
    assert_eq!(
        sa.1.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
        ["billing", "shipping"]
    );
    assert!(
        sa.1.iter()
            .all(|x| x.2.as_deref() == Some("proto/orders.proto") && x.3.is_some())
    );

    // Shipping first: its copy is the first graph, so it is the one shown.
    let c = build(
        &fixture("identity-proto-diverged"),
        &["shipping", "billing", "orders"],
    );
    assert_eq!(seen(&c, ["orders", "billing", "shipping"]).0, "shipping");
}

#[test]
fn also_in_labels() {
    let r = build(
        &fixture("identity-proto-diverged"),
        &["orders", "billing", "shipping"],
    );
    let orders = node(&r, "orders", ORDER_CREATED);
    let labelled = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    let repos: Vec<String> = labelled
        .row_identity(orders, orders)
        .unwrap()
        .also_in
        .iter()
        .map(|a| a.repo.clone())
        .collect();
    assert_eq!(repos, ["billing", "shipping"]);

    let bare = IdentityView::new(&r.merged, &Collapse::default());
    let by_label: BTreeMap<&str, u64> = r
        .repo_labels
        .iter()
        .map(|(id, l)| (l.as_str(), *id))
        .collect();
    let mut want: Vec<String> = ["billing", "shipping"]
        .iter()
        .map(|l| format!("repo#{}", by_label[l] % 1000))
        .collect();
    want.sort();
    let got: Vec<String> = bare
        .row_identity(orders, orders)
        .unwrap()
        .also_in
        .iter()
        .map(|a| a.repo.clone())
        .collect();
    assert_eq!(got, want, "no label: `repo#<id % 1000>`");

    // The report labels the same way.
    let rep = identity_classes(&r.merged, &IdentityQuery::default()).unwrap();
    assert!(
        rep.classes[0]
            .members
            .iter()
            .all(|m| m.repo.starts_with("repo#")),
        "{:?}",
        rep.classes[0].members
    );
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    id: NodeId,
    identity: Option<RowIdentity>,
}

#[test]
fn fold_keeps_first() {
    let r = build(&fixture("identity-npm-package"), &["web", "admin"]);
    let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    let web = node(&r, "web", "package:npm:react");
    let admin = node(&r, "admin", "package:npm:react");
    let other = node(&r, "web", "package:npm:left-pad");
    let rows: Vec<Row> = [admin, other, web]
        .into_iter()
        .map(|id| Row { id, identity: None })
        .collect();
    let folded = view.fold(rows, |row| row.id, |row, id| row.identity = Some(id));
    assert_eq!(
        folded.iter().map(|row| row.id).collect::<Vec<_>>(),
        [admin, other],
        "the first row of the class is kept, in the answer's order"
    );
    let id = folded[0]
        .identity
        .as_ref()
        .expect("the kept row carries the class");
    assert_eq!(
        (id.tier, id.rules.as_slice(), id.diverged),
        ("fact", ["package_name"].as_slice(), false)
    );
    assert_eq!(id.also_in.len(), 1, "{:?}", id.also_in);
    assert_eq!(
        (id.also_in[0].repo.as_str(), id.also_in[0].qname.as_str()),
        ("web", "package:npm:react"),
        "every OTHER member than the row's"
    );
    assert!(
        folded[1].identity.is_none(),
        "a node in no class is untouched"
    );
    assert!(view.row_identity(other, other).is_none());
}

/// An `[[edge]]`-style SAME_AS: emitter `overlay:edge`, so why::tier_of reads
/// it as heuristic.
fn overlay_same_as(r: &mut GenerateResult, from: NodeId, to: NodeId) {
    let mut e = Edge::new(from, to, edge_category::SAME_AS, Confidence::Weak);
    evidence::attach(&mut e, Evidence::emitter("overlay:edge").rule("edge#0"));
    r.merged.cross_edges.push(e);
}

#[test]
fn heuristic_classes_never_collapse() {
    let mut r = build(&fixture("identity-npm-package"), &["web", "admin"]);
    let left_pad = node(&r, "web", "package:npm:left-pad");
    let typescript = node(&r, "admin", "package:npm:typescript");
    let (react_web, react_admin) = (
        node(&r, "web", "package:npm:react"),
        node(&r, "admin", "package:npm:react"),
    );
    overlay_same_as(&mut r, left_pad, typescript);
    // A heuristic edge inside a FACT class does not weaken it.
    overlay_same_as(&mut r, react_admin, react_web);

    let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    assert!(
        view.row_identity(left_pad, left_pad).is_none(),
        "a candidate never collapses"
    );
    assert_eq!(view.display(typescript), typescript);
    assert_eq!(
        view.row_identity(react_admin, react_admin).unwrap().tier,
        "fact"
    );

    let rep = report(&r, |_| {}).unwrap();
    let rows: Vec<(&str, &str, bool, bool, usize)> = rep
        .classes
        .iter()
        .map(|c| {
            (
                c.key.as_str(),
                c.tier,
                c.collapses,
                c.walks,
                c.members.len(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("package:npm:lodash", "fact", true, false, 2),
            ("package:npm:react", "fact", true, false, 2),
            ("package:npm:left-pad", "heuristic", false, false, 2),
        ]
    );
    assert!(
        rep.classes[2].rules.is_empty(),
        "an overlay edge names no identity rule"
    );
    assert_eq!((rep.counts.fact, rep.counts.heuristic), (2, 1));
}

#[test]
fn report_filters() {
    let r = build(&fixture("identity-npm-package"), &["web", "admin"]);
    let rep = report(&r, |q| q.tier = Some("heuristic".into())).unwrap();
    assert!(rep.classes.is_empty());
    let a = rep.absence.expect("filtered to nothing");
    assert_eq!(a.reason, "no_match");
    assert!(a.note.contains("filters removed 2 classes"), "{}", a.note);

    let err = report(&r, |q| q.rule = Some("nope".into())).unwrap_err();
    for rule in [
        "package_name",
        "manifest_name",
        "message_fqname",
        "contract_op",
        "contract_route",
        "scip_symbol",
        "body_hash",
        "path_suffix",
    ] {
        assert!(err.contains(rule), "{err}");
    }
    assert!(err.contains("nope"), "{err}");
    let err = report(&r, |q| q.tier = Some("certain".into())).unwrap_err();
    assert!(err.contains("fact") && err.contains("heuristic"), "{err}");

    // Five repos: the npm pair and the proto triple, three classes.
    let npm = fixture("identity-npm-package");
    let proto = fixture("identity-proto-diverged");
    let all = build_paths(&[
        npm.join("web"),
        npm.join("admin"),
        proto.join("orders"),
        proto.join("billing"),
        proto.join("shipping"),
    ]);
    let keys = |rep: &IdentityReport| {
        rep.classes
            .iter()
            .map(|c| c.key.clone())
            .collect::<Vec<_>>()
    };
    let full = report(&all, |_| {}).unwrap();
    assert_eq!(
        keys(&full),
        ["package:npm:lodash", "package:npm:react", ORDER_CREATED],
        "by (tier, rules in table order, key)"
    );
    assert_eq!((full.counts.classes, full.counts.members), (3, 7));
    assert_eq!((full.counts.fact, full.counts.diverged), (3, 1));
    assert_eq!(full.counts.by_rule[0], ("package_name", 2));
    assert_eq!(full.counts.by_rule[2], ("message_fqname", 1));
    assert_eq!(
        full.counts.by_rule.len(),
        glia_code_domain::same_as::ALL_RULES.len(),
        "every table rule, in table order"
    );
    assert_eq!(full.omitted, 0);
    assert!(full.absence.is_none());

    let diverged = report(&all, |q| q.diverged_only = true).unwrap();
    assert_eq!(keys(&diverged), [ORDER_CREATED]);
    let pkg = report(&all, |q| q.rule = Some("package_name".into())).unwrap();
    assert_eq!(keys(&pkg), ["package:npm:lodash", "package:npm:react"]);
    let fact = report(&all, |q| q.tier = Some("fact".into())).unwrap();
    assert_eq!(fact.classes.len(), 3);
    let cut = report(&all, |q| q.limit = Some(1)).unwrap();
    assert_eq!(
        (keys(&cut), cut.omitted, cut.counts.classes),
        (vec!["package:npm:lodash".to_string()], 2, 3)
    );
    let every = report(&all, |q| q.limit = Some(0)).unwrap();
    assert_eq!(
        (every.classes.len(), every.omitted),
        (3, 0),
        "limit 0 is every class"
    );
}

// ---------------------------------------------------------------------------
// Child run: the markers go to stderr, so a parent re-runs this binary.
// ---------------------------------------------------------------------------

const CHILD_ENV: &str = "DD8_IDENTITY_MARKERS";

/// Builds the `|`-separated dirs named by [`CHILD_ENV`], runs one report and
/// one collapse over an empty and a full view; a no-op in a normal run.
#[test]
fn child_markers_for_stderr() {
    let Ok(dirs) = std::env::var(CHILD_ENV) else {
        return;
    };
    let paths: Vec<PathBuf> = dirs.split('|').map(PathBuf::from).collect();
    let r = build_paths(&paths);
    identity_classes(&r.merged, &IdentityQuery::default()).unwrap();
    IdentityView::new(&r.merged, &Collapse::off()).collapse_marker("off", 3, 3);
    IdentityView::new(&r.merged, &Collapse::default()).collapse_marker("probe", 5, 3);
}

#[test]
fn markers() {
    let npm = fixture("identity-npm-package");
    let proto = fixture("identity-proto-diverged");
    let dirs = [
        npm.join("web"),
        npm.join("admin"),
        proto.join("orders"),
        proto.join("billing"),
        proto.join("shipping"),
    ]
    .map(|p| p.to_string_lossy().into_owned())
    .join("|");
    let out = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "--exact",
            "child_markers_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, &dirs)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child run failed:\n{stderr}");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[identity] report ") || l.starts_with("[identity] collapse "))
        .collect();
    assert_eq!(
        lines,
        [
            "[identity] report classes=3 fact=3 derived=0 heuristic=0 diverged=1",
            "[identity] collapse surface=probe classes=3 walk_classes=1 folded=2 rows=3",
        ],
        "an empty view prints no collapse line:\n{stderr}"
    );
}
