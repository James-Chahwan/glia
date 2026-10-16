//! DD.6: vendored copies as HEURISTIC same-as candidates, on a real build of
//! the substrate-gap fixture identity-vendored-subtree (`generate_many` on
//! its `money` and `shop` dirs), and through a layout merge with and without
//! the sources on disk.
//!
//! The fixture: `shop` copies `money` under `third_party/money/`. `format.ts`
//! is byte-identical (rule `body_hash`), `parse.ts` diverged (`Math.floor`
//! for `Math.round`, rule `path_suffix/diverged`), and both repos hold a
//! `src/index.ts` with different bodies: the offset (``, ``) is never
//! confirmed, so the two `main`s and the two index MODULEs never pair.
//!
//! Baseline before DD.6 (HEAD 00aa1726, a `git archive` build): `glia merge
//! money shop` writes no SAME_AS edge and no `[identity]` line (grade.py on
//! the wave-3 wheel: SAME_AS 0/4, BLIND SPOT).
//!
//! The `[identity]` marker is asserted from a child run of this test binary
//! ([`child_build_for_stderr`]), the identity_resolver.rs pattern.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::code_span::is_span_payload;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category};
use glia_core::NodeId;
use glia_engine::merge::{MergeMember, merge_layouts, persist_merge, read_workspace};
use glia_engine::persist::persist_result;
use glia_engine::why::why_edge;
use glia_engine::{GenerateResult, generate_many};
use glia_graph::MergedGraph;
use glia_store::MANIFEST_NAME;

const FIXTURE: &str = "identity-vendored-subtree";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(FIXTURE)
}

fn build(roots: &[&Path]) -> GenerateResult {
    let paths: Vec<String> = roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

fn fixture_build() -> GenerateResult {
    let root = fixture();
    build(&[&root.join("money"), &root.join("shop")])
}

/// `(from qname, to qname, EVIDENCE rule)` of every SAME_AS edge, sorted,
/// each asserted to carry exactly one EVIDENCE cell from `resolver:identity`
/// and Weak confidence (the HEURISTIC tier).
fn stars(m: &MergedGraph) -> Vec<(String, String, String)> {
    let qname = |id: NodeId| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_else(|| panic!("no qname for {id:?}"))
    };
    let mut out: Vec<(String, String, String)> = m
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
            assert_eq!(e.confidence, glia_core::Confidence::Weak, "{e:?}");
            (qname(e.from), qname(e.to), ev.rule.unwrap_or_default())
        })
        .collect();
    out.sort();
    out
}

fn triple(from: &str, to: &str, rule: &str) -> (String, String, String) {
    (from.to_string(), to.to_string(), rule.to_string())
}

/// The four candidates: the copy (shop, longer qname) points at the library
/// (money, the least qname).
fn expected() -> Vec<(String, String, String)> {
    vec![
        triple(
            "third_party::money::src::format",
            "src::format",
            "body_hash",
        ),
        triple(
            "third_party::money::src::format::formatCents",
            "src::format::formatCents",
            "body_hash",
        ),
        triple(
            "third_party::money::src::parse",
            "src::parse",
            "path_suffix/diverged",
        ),
        triple(
            "third_party::money::src::parse::parseCents",
            "src::parse::parseCents",
            "path_suffix/diverged",
        ),
    ]
}

#[test]
fn vendored_subtree() {
    let r = fixture_build();
    let got = stars(&r.merged);
    assert_eq!(got, expected());
    // The boilerplate trap: same path, different body, unconfirmed offset.
    for (from, to, _) in &got {
        for q in ["src::index", "src::index::main"] {
            assert!(from != q && to != q, "{q} paired: {got:?}");
        }
    }
    // Candidates, never collapsed: every declaration is still its own node.
    let count = |q: &str| {
        r.merged
            .graphs
            .iter()
            .flat_map(|g| g.nav.qname_by_id.values())
            .filter(|v| v.as_str() == q)
            .count()
    };
    assert_eq!(count("src::index::main"), 2);
    assert_eq!(count("src::format::formatCents"), 1);
    assert_eq!(count("third_party::money::src::format::formatCents"), 1);
}

#[test]
fn vendored_edges_tier_heuristic() {
    let r = fixture_build();
    for (from, to, rule) in expected() {
        let why = why_edge(&r.merged, &from, &to, Some("SAME_AS"))
            .unwrap_or_else(|e| panic!("why {from} -> {to}: {e}"));
        assert!(why.found, "{from} -> {to}");
        let rows: Vec<(&str, Option<&str>)> = why
            .edges
            .iter()
            .filter(|e| e.from_qname == from && e.to_qname == to)
            .map(|e| (e.tier, e.rule.as_deref()))
            .collect();
        assert_eq!(rows, [("heuristic", Some(rule.as_str()))], "{from} -> {to}");
        assert!(
            why.edges
                .iter()
                .all(|e| e.tier == "heuristic" && e.cross_repo)
        );
    }
}

// ---------------------------------------------------------------------------
// Layout merge: merge == joint build, and the digests survive missing sources.
// ---------------------------------------------------------------------------

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

/// CODE cells of `m` that came back as unresolved spans (`{"code_span":..}`).
fn unresolved_spans(m: &MergedGraph) -> usize {
    m.graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .flat_map(|n| &n.cells)
        .filter(|c| c.kind == cell_type::CODE && is_span_payload(&c.payload))
        .count()
}

#[test]
fn sources_free_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    copy_tree(&fixture(), &src);
    let (money, shop) = (src.join("money"), src.join("shop"));
    let (a, b, t) = (
        tmp.path().join("out_money"),
        tmp.path().join("out_shop"),
        tmp.path().join("out_joint"),
    );
    persist_result(&build(&[&money]), &a, "test").unwrap();
    persist_result(&build(&[&shop]), &b, "test").unwrap();
    let joint = build(&[&money, &shop]);
    persist_result(&joint, &t, "test").unwrap();
    let want = stars(&joint.merged);
    assert_eq!(
        want,
        expected(),
        "control: the joint build pairs the copies"
    );

    // Sources in place: the merge is the joint build, byte for byte.
    let m = merge_layouts(&[gmap("money", &a), gmap("shop", &b)]).unwrap();
    assert_eq!(
        unresolved_spans(&m.result.merged),
        0,
        "control: sources on disk"
    );
    assert_eq!(stars(&m.result.merged), want);
    let out_m = tmp.path().join("out_merged");
    persist_merge(&m, &out_m, "test").unwrap();
    assert_same_layout(&out_m, &t);

    // The shop sources renamed away: its CODE reads back as spans, whose
    // xxh64 is the digest of the same bytes, so the candidates are the same.
    std::fs::rename(&shop, src.join("shop-moved")).unwrap();
    let gone = merge_layouts(&[gmap("money", &a), gmap("shop", &b)]).unwrap();
    assert!(
        unresolved_spans(&gone.result.merged) > 0,
        "control: the shop CODE cells are unresolved spans"
    );
    assert_eq!(stars(&gone.result.merged), want);
    let sets = |s: &[(String, String, String)]| s.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(sets(&stars(&gone.result.merged)).len(), 4);
}

// ---------------------------------------------------------------------------
// Child run: the marker goes to stderr, so a parent re-runs this binary.
// ---------------------------------------------------------------------------

const CHILD_ENV: &str = "DD6_IDENTITY_BUILD";

/// Builds the `|`-separated dirs named by [`CHILD_ENV`]; a no-op in a normal
/// run.
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
    let root = fixture();
    let dirs = [root.join("money"), root.join("shop")]
        .map(|p| p.to_string_lossy().into_owned())
        .join("|");
    let out = Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            "--exact",
            "child_build_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, &dirs)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child build failed:\n{stderr}");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[identity] "))
        .collect();
    // Two offsets seen: (``, `third_party/money`), confirmed by format.ts,
    // and (``, ``) of the two src/index.ts, never confirmed.
    assert_eq!(
        lines,
        [
            "[identity] classes=4 edges=4 diverged=2 package_name=0 manifest_name=0 message_fqname=0 \
             contract_op=0 contract_route=0 body_hash=2 path_suffix=2 offsets=1/2"
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
             contract_op=0 contract_route=0 body_hash=2 path_suffix=2"
        ]
    );
}
