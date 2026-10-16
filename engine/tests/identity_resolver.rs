//! DD.5: the identity resolver's SAME_AS stars on real builds of the
//! substrate-gap identity fixtures (`generate_many` / `generate_one` on their
//! dirs), not hand-built graphs.
//!
//! Baseline before DD.5 (HEAD a08fc669): no SAME_AS edge in any of these
//! builds; identity-npm-package had two SHARES_DEPENDENCY pairs and
//! identity-proto-diverged the three-edge SHARES_SCHEMA clique.
//!
//! The `[identity]` marker is asserted from a child run of this test binary
//! ([`child_build_for_stderr`]), the merge_layouts.rs pattern.

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::{Confidence, NodeId};
use glia_engine::{GenerateResult, generate_many};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

fn build(root: &Path, dirs: &[&str]) -> GenerateResult {
    let paths: Vec<String> = dirs
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{} builds: {e}", root.display()))
}

/// One SAME_AS edge read back by qname.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Star {
    from: String,
    to: String,
    rule: String,
    confidence: String,
}

/// Every SAME_AS edge of `r`, sorted, each asserted to carry exactly one
/// EVIDENCE cell from `resolver:identity`.
fn stars(r: &GenerateResult) -> Vec<Star> {
    let qname = |id: NodeId| {
        r.merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_else(|| panic!("no qname for {id:?}"))
    };
    let mut out: Vec<Star> = r
        .merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SAME_AS)
        .map(|e| {
            let n = e
                .cells
                .iter()
                .filter(|c| c.kind == glia_code_domain::cell_type::EVIDENCE)
                .count();
            assert_eq!(n, 1, "one EVIDENCE cell: {e:?}");
            let ev = Evidence::of(e).unwrap_or_else(|| panic!("no evidence: {e:?}"));
            assert_eq!(ev.emitter, "resolver:identity", "{e:?}");
            Star {
                from: qname(e.from),
                to: qname(e.to),
                rule: ev.rule.unwrap_or_default(),
                confidence: format!("{:?}", e.confidence),
            }
        })
        .collect();
    out.sort();
    out
}

fn star(from: &str, to: &str, rule: &str, confidence: Confidence) -> Star {
    Star {
        from: from.to_string(),
        to: to.to_string(),
        rule: rule.to_string(),
        confidence: format!("{confidence:?}"),
    }
}

/// `(from, to, rule)` only: the qname-level star a RepoId cannot move.
fn triples(s: &[Star]) -> Vec<(String, String, String)> {
    s.iter()
        .map(|s| (s.from.clone(), s.to.clone(), s.rule.clone()))
        .collect()
}

#[test]
fn npm_package() {
    let r = build(&fixture("identity-npm-package"), &["web", "admin"]);
    let got = stars(&r);
    assert_eq!(
        got,
        [
            star(
                "package:npm:lodash",
                "package:npm:lodash",
                "package_name",
                Confidence::Strong
            ),
            star(
                "package:npm:react",
                "package:npm:react",
                "package_name",
                Confidence::Strong
            ),
        ]
    );
    assert!(
        got.iter()
            .all(|s| !s.from.contains("left-pad") && !s.from.contains("typescript")),
        "{got:?}"
    );
}

#[test]
fn workspace_lib() {
    let r = build(
        &fixture("identity-npm-workspace-lib"),
        &["monorepo", "admin"],
    );
    let got = stars(&r);
    let dep = "package:npm:@shop/money";
    assert_eq!(
        got,
        [
            // manifest_name: {PROJECT, the monorepo dep, the admin dep}, into
            // the least qname - the monorepo dep (first graph).
            star(dep, dep, "manifest_name", Confidence::Strong),
            // package_name: the two @shop/money deps.
            star(dep, dep, "package_name", Confidence::Strong),
            star(
                "project:packages/money",
                dep,
                "manifest_name",
                Confidence::Strong
            ),
        ]
    );
    for s in &got {
        for q in ["project:packages/utils", "package:npm:utils"] {
            assert!(
                s.from != q && s.to != q,
                "a dir-derived label is no declared name: {s:?}"
            );
        }
    }
}

#[test]
fn proto_diverged() {
    let root = fixture("identity-proto-diverged");
    let q = "message:proto:shop.v1.OrderCreated";
    let fwd = stars(&build(&root, &["orders", "billing", "shipping"]));
    // The center is the orders copy (first graph): billing matches it,
    // shipping's extra field does not.
    assert_eq!(
        fwd,
        [
            star(q, q, "message_fqname", Confidence::Strong),
            star(q, q, "message_fqname/diverged", Confidence::Strong),
        ]
    );
    // Shipping first: its copy is the center, so both others differ from it.
    let rev = stars(&build(&root, &["shipping", "billing", "orders"]));
    assert_eq!(rev.len(), 2, "{rev:?}");
    assert!(rev.iter().any(|s| s.rule.ends_with("/diverged")), "{rev:?}");
    let pairs = |s: &[Star]| {
        s.iter()
            .map(|s| (s.from.clone(), s.to.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(pairs(&fwd), pairs(&rev), "the same star at qname level");
}

#[test]
fn contract_ops() {
    let r = build(&fixture("identity-contract-ops"), &["."]);
    let swagger = "contract::docs::swagger::GET:/orders";
    let swaggo = "contract::main::GET:/orders";
    let feature = "contract::features::orders::feature::GET:/api/orders";
    let got = stars(&r);
    assert_eq!(
        got,
        [
            // contract_route: all three GET ops reach ROUTE GET /api/orders.
            star(feature, swagger, "contract_route", Confidence::Medium),
            // contract_op: swaggo and swagger both declare /orders under
            // project:. (no basePath in either).
            star(swaggo, swagger, "contract_op", Confidence::Medium),
            star(swaggo, swagger, "contract_route", Confidence::Medium),
        ]
    );
    assert!(
        got.iter()
            .all(|s| !s.from.contains("cancel") && !s.to.contains("cancel")),
        "{got:?}"
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

/// DE.1's invariance with SAME_AS present: the same sources under two
/// directory names (two RepoIds, LB.1) give the same qname-level stars.
#[test]
fn star_edges_are_repo_id_invariant() {
    let tmp = tempfile::tempdir().unwrap();
    for (fixture_name, dirs) in [
        ("identity-contract-ops", vec!["."]),
        ("identity-npm-workspace-lib", vec!["monorepo", "admin"]),
    ] {
        let (a, b) = (
            tmp.path().join("one").join(fixture_name),
            tmp.path().join("two").join("renamed"),
        );
        copy_tree(&fixture(fixture_name), &a);
        copy_tree(&fixture(fixture_name), &b);
        // A one-dir fixture is named by its own directory; a multi-dir one by
        // its member dirs, so rename those in the second copy.
        let (ra, rb) = if dirs == ["."] {
            (build(&a, &["."]), build(&b, &["."]))
        } else {
            let renamed: Vec<String> = dirs.iter().map(|d| format!("{d}-copy")).collect();
            for (d, n) in dirs.iter().zip(&renamed) {
                std::fs::rename(b.join(d), b.join(n)).unwrap();
            }
            let renamed: Vec<&str> = renamed.iter().map(String::as_str).collect();
            (build(&a, &dirs), build(&b, &renamed))
        };
        let repos = |r: &GenerateResult| r.merged.graphs.iter().map(|g| g.repo).collect::<Vec<_>>();
        assert_ne!(
            repos(&ra),
            repos(&rb),
            "{fixture_name}: control, the RepoIds differ"
        );
        let (sa, sb) = (stars(&ra), stars(&rb));
        assert!(!sa.is_empty(), "{fixture_name}");
        assert_eq!(triples(&sa), triples(&sb), "{fixture_name}");
    }
}

// ---------------------------------------------------------------------------
// Child run: the marker goes to stderr, so a parent re-runs this binary.
// ---------------------------------------------------------------------------

const CHILD_ENV: &str = "DD5_IDENTITY_BUILD";

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
    let root = fixture("identity-npm-package");
    let dirs = [root.join("web"), root.join("admin")]
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
    assert_eq!(
        lines,
        [
            // DD.6's vendored fields: the two src/index.js MODULEs share a
            // path but no body, so their one offset is seen, never confirmed.
            "[identity] classes=2 edges=2 diverged=0 package_name=2 manifest_name=0 message_fqname=0 \
             contract_op=0 contract_route=0 body_hash=0 path_suffix=0 offsets=0/1"
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
            "[evidence-rules] resolver=identity package_name=2 manifest_name=0 message_fqname=0 \
             contract_op=0 contract_route=0 body_hash=0 path_suffix=0"
        ]
    );
}
