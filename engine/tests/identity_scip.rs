//! DD.7: SCIP identity. Two repos whose `.glia/scip-snapshot/`s define one
//! package + descriptor (`scip-python python money <version>
//! \`lib.money\`/format_cents().`, the version cut out of the key) bind those
//! definitions into one FACT identity class, rule `scip_symbol`
//! (`scip_symbol/diverged` when the bodies differ). The `identity` Resolve
//! pass reads each repo's snapshot through `CodeBuildCtx::scip_roots`, and a
//! layout merge passes its members' recorded roots, so merge == joint build
//! holds byte for byte.
//!
//! The sources are the substrate-gap fixture `identity-scip-symbols`
//! (`billing/lib/money.py`, and `shop/lib/money.py` with ` EUR` appended).
//! `fixture_snapshots_are_current` pins the committed snapshots to those
//! sources; regenerate them with
//! `cargo test -p glia-engine --test identity_scip -- --ignored write_identity_scip_fixture`
//! (the CE.1d `write_dict_dispatch_fixture` pattern, `scip_ingest.rs`).
//!
//! Baseline before DD.7 (HEAD d263996c): the SCIP stage binds the definition
//! rows and drops the binding, a merge runs the passes with no inputs, and
//! the fixture has no SAME_AS edge (grade.py on the wave-4 wheel: SAME_AS
//! 0/1, BLIND SPOT).
//!
//! The fired_on markers (`[scip] identity repo=<label> ...`, the
//! `[identity] ... scip_symbol=<n>` field) are read from a child run of this
//! test binary ([`child_build_for_stderr`]), the identity_vendored.rs pattern.

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::same_as;
use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipSymbolRecord, read_scip, source_hash, write_scip,
};
use glia_code_domain::{cell_type, edge_category};
use glia_core::{Confidence, NodeId};
use glia_engine::merge::{MergeMember, merge_layouts, persist_merge, read_workspace};
use glia_engine::persist::persist_result;
use glia_engine::why::why_edge;
use glia_engine::{GenerateResult, generate_many};
use glia_graph::MergedGraph;
use glia_store::MANIFEST_NAME;

const FIXTURE: &str = "identity-scip-symbols";
const FILE: &str = "lib/money.py";
const QNAME: &str = "lib::money::format_cents";

const BILLING_PY: &str =
    include_str!("../../bench/substrate-gap/fixtures/identity-scip-symbols/billing/lib/money.py");
const SHOP_PY: &str =
    include_str!("../../bench/substrate-gap/fixtures/identity-scip-symbols/shop/lib/money.py");

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(FIXTURE)
}

/// The one symbol each snapshot defines: package `money` at `version`.
fn symbol(version: &str) -> String {
    format!("scip-python python money {version} `lib.money`/format_cents().")
}

/// The snapshot of `root`'s `lib/money.py` as it is on disk now: `symbol`
/// defined at row 0, the `def` line, named `format_cents`.
fn snapshot(root: &Path, symbol: &str) {
    let bytes = std::fs::read(root.join(FILE)).expect("fixture source");
    let documents = vec![ScipDocumentRecord {
        path: FILE.to_string(),
        language: "python".to_string(),
        source_hash: source_hash(&bytes),
        defs: vec![ScipDefRow {
            s: 0,
            line: 0,
            name: "format_cents".to_string(),
        }],
        refs: Vec::new(),
    }];
    let symbols = vec![ScipSymbolRecord {
        id: 0,
        symbol: symbol.to_string(),
        implements: Vec::new(),
    }];
    let meta = ScipMeta::new(
        "scip-python".to_string(),
        "0.6.0".to_string(),
        String::new(),
        0,
    );
    write_scip(root, meta, &documents, &symbols).expect("write snapshot");
}

/// `<dir>/lib/money.py` holding `source`.
fn tree(dir: &Path, source: &str) {
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join(FILE), source).unwrap();
}

/// `billing` and `shop` under `parent`, each with its source and, when given,
/// a snapshot of that symbol.
fn repos(
    parent: &Path,
    shop_source: &str,
    billing_sym: Option<&str>,
    shop_sym: Option<&str>,
) -> (PathBuf, PathBuf) {
    let (billing, shop) = (parent.join("billing"), parent.join("shop"));
    tree(&billing, BILLING_PY);
    tree(&shop, shop_source);
    if let Some(s) = billing_sym {
        snapshot(&billing, s);
    }
    if let Some(s) = shop_sym {
        snapshot(&shop, s);
    }
    (billing, shop)
}

fn build(roots: &[&Path]) -> GenerateResult {
    let paths: Vec<String> = roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

/// One SAME_AS edge: `(from qname, to qname, EVIDENCE rule, confidence)`,
/// each asserted to carry exactly one EVIDENCE cell from `resolver:identity`.
type Star = (String, String, String, Confidence);

fn stars(m: &MergedGraph) -> Vec<Star> {
    let qname = |id: NodeId| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_else(|| panic!("no qname for {id:?}"))
    };
    let mut out: Vec<Star> = m
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .map(|e| {
            let n = e
                .cells
                .iter()
                .filter(|c| c.kind == cell_type::EVIDENCE)
                .count();
            assert_eq!(n, 1, "one EVIDENCE cell: {e:?}");
            let ev = Evidence::of(e).unwrap_or_else(|| panic!("no evidence: {e:?}"));
            assert_eq!(ev.emitter, "resolver:identity", "{e:?}");
            (
                qname(e.from),
                qname(e.to),
                ev.rule.unwrap_or_default(),
                e.confidence,
            )
        })
        .collect();
    out.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
    out
}

/// The SAME_AS edges whose rule is `scip_symbol`, either spelling.
fn scip_stars(m: &MergedGraph) -> Vec<Star> {
    stars(m)
        .into_iter()
        .filter(|s| same_as::split_evidence_rule(&s.2).0 == same_as::RULE_SCIP_SYMBOL)
        .collect()
}

fn star(rule: &str) -> Star {
    (
        QNAME.to_string(),
        QNAME.to_string(),
        rule.to_string(),
        Confidence::Strong,
    )
}

/// The two `format_cents` FUNCTIONs, billing's first: the SAME_AS edge runs
/// shop -> billing (the center is the first graph's copy of the one qname).
fn defs(m: &MergedGraph) -> [NodeId; 2] {
    let mut ids: Vec<(usize, NodeId)> = m
        .graphs
        .iter()
        .enumerate()
        .flat_map(|(i, g)| {
            g.nav
                .qname_by_id
                .iter()
                .filter(|(_, q)| q.as_str() == QNAME)
                .map(move |(id, _)| (i, *id))
        })
        .collect();
    ids.sort_by_key(|(i, id)| (*i, id.0));
    ids.dedup_by_key(|(_, id)| *id);
    match ids.as_slice() {
        [(_, a), (_, b)] => [*a, *b],
        other => panic!("two {QNAME} nodes expected, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The committed fixture.
// ---------------------------------------------------------------------------

#[test]
fn fixture_snapshots_are_current() {
    let root = fixture();
    for (dir, version) in [("billing", "1.0"), ("shop", "1.1")] {
        let repo = root.join(dir);
        let snap = read_scip(&repo)
            .unwrap_or_else(|| panic!("{dir}: the committed snapshot reads back complete"));
        let bytes = std::fs::read(repo.join(FILE)).expect("fixture source");
        assert_eq!(snap.documents.len(), 1, "{dir}");
        assert_eq!(
            snap.documents[0].source_hash,
            source_hash(&bytes),
            "{dir}/{FILE} changed since the snapshot"
        );
        assert_eq!(
            snap.documents[0].defs,
            [ScipDefRow {
                s: 0,
                line: 0,
                name: "format_cents".to_string()
            }]
        );
        assert_eq!(snap.symbols.len(), 1, "{dir}");
        assert_eq!(snap.symbols[0].symbol, symbol(version), "{dir}");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("billing").join(FILE)).unwrap(),
        BILLING_PY
    );
    assert_eq!(
        std::fs::read_to_string(root.join("shop").join(FILE)).unwrap(),
        SHOP_PY
    );
}

/// Regenerates the committed fixture snapshots (see the module doc).
#[test]
#[ignore]
fn write_identity_scip_fixture() {
    let root = fixture();
    snapshot(&root.join("billing"), &symbol("1.0"));
    snapshot(&root.join("shop"), &symbol("1.1"));
}

#[test]
fn fixture_pairs_the_definitions() {
    let root = fixture();
    let r = build(&[&root.join("billing"), &root.join("shop")]);
    assert_eq!(stars(&r.merged), [star("scip_symbol/diverged")]);
}

// ---------------------------------------------------------------------------
// Builds over temp trees.
// ---------------------------------------------------------------------------

#[test]
fn two_indexed_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let (billing, shop) = repos(
        tmp.path(),
        SHOP_PY,
        Some(&symbol("1.0")),
        Some(&symbol("1.1")),
    );
    let r = build(&[&billing, &shop]);
    // The two files share a path but no identical declaration: the vendored
    // rules confirm no offset, so the SCIP key is the one edge.
    assert_eq!(stars(&r.merged), [star("scip_symbol/diverged")]);
    let [b, s] = defs(&r.merged);
    let e = r
        .merged
        .cross_edges
        .iter()
        .find(|e| e.category == edge_category::SAME_AS)
        .expect("the SAME_AS edge");
    assert_eq!((e.from, e.to), (s, b), "member shop -> center billing");
    let why = why_edge(&r.merged, QNAME, QNAME, Some("SAME_AS")).expect("why");
    assert!(why.found);
    let rows: Vec<(&str, Option<&str>, bool)> = why
        .edges
        .iter()
        .map(|e| (e.tier, e.rule.as_deref(), e.cross_repo))
        .collect();
    assert_eq!(rows, [("fact", Some("scip_symbol/diverged"), true)]);

    // Equal bodies: plain `scip_symbol`. The identical same-path copies are
    // also a vendored `body_hash` candidate (Weak, DD.6), never the FACT rule.
    let eq = tempfile::tempdir().unwrap();
    let (billing, shop) = repos(
        eq.path(),
        BILLING_PY,
        Some(&symbol("1.0")),
        Some(&symbol("1.1")),
    );
    let r = build(&[&billing, &shop]);
    assert_eq!(scip_stars(&r.merged), [star("scip_symbol")]);
    for (from, to, rule, confidence) in stars(&r.merged) {
        let (base, _) = same_as::split_evidence_rule(&rule);
        assert!(
            base == same_as::RULE_SCIP_SYMBOL
                || (base == same_as::RULE_BODY_HASH && confidence == Confidence::Weak),
            "{from} -> {to} {rule} {confidence:?}"
        );
    }
}

#[test]
fn local_and_stale() {
    // A `local` symbol is file-scoped: it keys nothing.
    let tmp = tempfile::tempdir().unwrap();
    let (billing, shop) = repos(tmp.path(), SHOP_PY, Some(&symbol("1.0")), Some("local 0"));
    let got = scip_stars(&build(&[&billing, &shop]).merged);
    assert!(got.is_empty(), "{got:?}");

    // A document whose source changed after the snapshot keys nothing.
    let stale = tempfile::tempdir().unwrap();
    let (billing, shop) = repos(
        stale.path(),
        SHOP_PY,
        Some(&symbol("1.0")),
        Some(&symbol("1.1")),
    );
    std::fs::write(
        shop.join(FILE),
        format!("{SHOP_PY}\n\ndef other():\n    return 1\n"),
    )
    .unwrap();
    let got = scip_stars(&build(&[&billing, &shop]).merged);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn one_repo_indexed() {
    let tmp = tempfile::tempdir().unwrap();
    let (billing, shop) = repos(tmp.path(), SHOP_PY, Some(&symbol("1.0")), None);
    let got = stars(&build(&[&billing, &shop]).merged);
    assert!(got.is_empty(), "{got:?}");
}

// ---------------------------------------------------------------------------
// Layout merge: the recorded roots are the SCIP roots.
// ---------------------------------------------------------------------------

/// One `gmap` member through the workspace reader, as a caller outside the
/// crate builds it (merge_layouts.rs's pattern).
fn gmap(name: &str, dir: &Path) -> MergeMember {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("glia.workspace.json");
    let member = format!(r#"{{"name":"{name}","gmap":"{}"}}"#, dir.display());
    std::fs::write(&path, format!(r#"{{"version":1,"members":[{member}]}}"#)).unwrap();
    read_workspace(&path).unwrap().remove(0)
}

/// Every `.gmap` file directly in `dir`, by name.
fn gmaps(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".gmap"))
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

fn manifest(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).unwrap()).unwrap()
}

/// The merged layout at `m` holds exactly the joint build's bytes at `t`:
/// every `.gmap` byte-identical, the manifest equal once `members` is gone.
fn assert_same_layout(m: &Path, t: &Path) {
    let (gm, gt) = (gmaps(m), gmaps(t));
    let names = |g: &[(String, Vec<u8>)]| g.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&gm), names(&gt), "shard sets differ");
    for ((name, bm), (_, bt)) in gm.iter().zip(&gt) {
        assert!(
            bm == bt,
            "{name}: merged bytes differ from the joint build's"
        );
    }
    let mut mm = manifest(m);
    mm.as_object_mut().unwrap().remove("members");
    assert_eq!(mm, manifest(t));
}

/// The two repos with snapshots under `<tmp>/src`, and each built alone into
/// `<tmp>/out_billing` / `<tmp>/out_shop`.
fn member_layouts(tmp: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let (billing, shop) = repos(
        &tmp.join("src"),
        SHOP_PY,
        Some(&symbol("1.0")),
        Some(&symbol("1.1")),
    );
    let (a, b) = (tmp.join("out_billing"), tmp.join("out_shop"));
    persist_result(&build(&[&billing]), &a, "test").unwrap();
    persist_result(&build(&[&shop]), &b, "test").unwrap();
    (billing, shop, a, b)
}

#[test]
fn merge_parity() {
    let tmp = tempfile::tempdir().unwrap();
    let (billing, shop, a, b) = member_layouts(tmp.path());
    let joint = build(&[&billing, &shop]);
    let t = tmp.path().join("out_joint");
    persist_result(&joint, &t, "test").unwrap();
    assert_eq!(
        stars(&joint.merged),
        [star("scip_symbol/diverged")],
        "control: the joint build pairs them"
    );

    let m = merge_layouts(&[gmap("billing", &a), gmap("shop", &b)]).unwrap();
    assert_eq!(stars(&m.result.merged), [star("scip_symbol/diverged")]);
    let out = tmp.path().join("out_merged");
    persist_merge(&m, &out, "test").unwrap();
    assert_same_layout(&out, &t);
}

#[test]
fn merge_without_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, shop, a, b) = member_layouts(tmp.path());
    std::fs::rename(&shop, tmp.path().join("src").join("shop-moved")).unwrap();
    let m =
        merge_layouts(&[gmap("billing", &a), gmap("shop", &b)]).expect("the merge still succeeds");
    let got = stars(&m.result.merged);
    assert!(
        got.is_empty(),
        "a member whose root is gone keys nothing: {got:?}"
    );
    // Both definitions are still merged (`defs` finds exactly two).
    defs(&m.result.merged);
}

// ---------------------------------------------------------------------------
// Child run: the markers go to stderr, so a parent re-runs this binary.
// ---------------------------------------------------------------------------

const CHILD_ENV: &str = "DD7_IDENTITY_BUILD";

/// Builds the `|`-separated dirs named by [`CHILD_ENV`] (relative to the
/// child's working directory); a no-op in a normal run.
#[test]
fn child_build_for_stderr() {
    let Ok(dirs) = std::env::var(CHILD_ENV) else {
        return;
    };
    let paths: Vec<String> = dirs.split('|').map(String::from).collect();
    generate_many(&paths).unwrap();
}

#[test]
fn marker() {
    let tmp = tempfile::tempdir().unwrap();
    repos(
        tmp.path(),
        SHOP_PY,
        Some(&symbol("1.0")),
        Some(&symbol("1.1")),
    );
    // Relative paths from the tree's parent: each marker's `repo=` is the path
    // as given.
    let out = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "--exact",
            "child_build_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(tmp.path())
        .env(CHILD_ENV, "billing|shop")
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child build failed:\n{stderr}");
    let scip: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[scip] identity "))
        .collect();
    assert_eq!(
        scip,
        [
            "[scip] identity repo=billing defs=1 keyed=1",
            "[scip] identity repo=shop defs=1 keyed=1"
        ],
        "{stderr}"
    );
    let identity: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[identity] "))
        .collect();
    assert_eq!(
        identity,
        [
            "[identity] classes=1 edges=1 diverged=1 package_name=0 manifest_name=0 message_fqname=0 \
             contract_op=0 contract_route=0 scip_symbol=1 body_hash=0 path_suffix=0 offsets=0/1"
        ],
        "{stderr}"
    );
    let rules: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[evidence-rules] resolver=identity "))
        .collect();
    assert_eq!(
        rules,
        [
            "[evidence-rules] resolver=identity package_name=0 manifest_name=0 message_fqname=0 \
             contract_op=0 contract_route=0 scip_symbol=1 body_hash=0 path_suffix=0"
        ]
    );
}
