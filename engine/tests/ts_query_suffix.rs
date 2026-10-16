//! DT.4 — a client URL whose trailing interpolation is a query string folds
//! to its path, over a REAL two-repo build.
//!
//! `bench/substrate-gap/fixtures/ts-query-suffix-endpoints` (Kina and
//! examples-glia reduced): an Angular `web` whose services end a URL with
//! `${q}` / `${query}` for `const q = s ? `?status=${s}` : ''`, or an inline
//! `${term ? '?q=' + term : ''}`, through every base shape the test beds use
//! (a config-aliased readonly member, a literal path, a member built by a
//! call), plus two controls (`${this.base}/${id}`, a `'/archived'` suffix
//! const) and a gin `server` with `/api/*` and `/admin/*` groups. Before DT.4
//! the query substitution stayed glued to the last segment
//! (`endpoint:GET:/api/orders${…}`), so none of the three query sites paired.
//! `bench/` is outside the cargo workspace's crates, so the fixture is copied
//! into a tempdir and built there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, NodeId, NodeKindId};
use glia_engine::generate_many;
use glia_graph::MergedGraph;

fn fixture() -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/ts-query-suffix-endpoints",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy the fixture's sources (never its key.json or a stray cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn build_in(root: &Path) -> MergedGraph {
    let repos: Vec<String> = ["web", "server"]
        .iter()
        .map(|d| root.join(d).to_string_lossy().into_owned())
        .collect();
    generate_many(&repos)
        .expect("ts-query-suffix-endpoints builds")
        .merged
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    let m = build_in(td.path());
    (td, m)
}

/// A node's qname without its ` @<project>` owner segment.
fn qname(m: &MergedGraph, id: NodeId) -> Option<String> {
    m.graphs.iter().find_map(|g| {
        g.nav
            .qname_by_id
            .get(&id)
            .map(|q| q.split(" @").next().unwrap_or(q).to_string())
    })
}

fn qnames_of(m: &MergedGraph, kind: NodeKindId) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .filter_map(move |n| g.nav.qname_by_id.get(&n.id))
                .map(|q| q.split(" @").next().unwrap_or(q).to_string())
        })
        .collect()
}

/// Every HTTP_CALLS edge as `(endpoint qname, route qname)`.
fn http_calls(m: &MergedGraph) -> BTreeSet<(String, String)> {
    m.all_edges()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
        .filter_map(|e| Some((qname(m, e.from)?, qname(m, e.to)?)))
        .collect()
}

/// The ENDPOINT_HIT payloads of the ENDPOINT `endpoint:<method>:<path>`.
fn hits(m: &MergedGraph, endpoint: &str) -> Vec<serde_json::Value> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter(move |n| {
                g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT)
                    && g.nav
                        .qname_by_id
                        .get(&n.id)
                        .is_some_and(|q| q.split(" @").next() == Some(endpoint))
            })
        })
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(s) => serde_json::from_str(s).ok(),
            _ => None,
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn query_suffix_sites_fold_to_their_path_and_pair() {
    let (_td, m) = build();
    assert_eq!(
        qnames_of(&m, node_kind::ENDPOINT),
        set(&[
            "endpoint:GET:/api/orders",
            "endpoint:GET:/api/orders/${…}",
            "endpoint:GET:/api/products${…}",
            "endpoint:GET:/admin/smr",
            "endpoint:GET:${…}/smr",
            "endpoint:POST:${…}/smr",
        ]),
        "the three query sites lose their `${{…}}` suffix; the controls keep theirs"
    );
    let pairs: BTreeSet<(String, String)> = [
        ("endpoint:GET:/api/orders", "GET /api/orders"),
        ("endpoint:GET:/admin/smr", "GET /admin/smr"),
        ("endpoint:GET:${…}/smr", "GET /admin/smr"),
        ("endpoint:GET:/api/orders/${…}", "GET /api/orders/:id"),
        ("endpoint:POST:${…}/smr", "POST /admin/smr"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(
        http_calls(&m),
        pairs,
        "each query site pairs its own route; the path-suffix control pairs nothing"
    );
}

#[test]
fn hits_name_the_dropped_query() {
    let (_td, m) = build();
    // (endpoint, file, query, template): the folded examples-glia shape keeps
    // DF.20's alias and template minus the query.
    let order = hits(&m, "endpoint:GET:/api/orders");
    assert_eq!(order.len(), 1, "{order:?}");
    assert_eq!(order[0]["file"], "src/app/order.service.ts");
    assert_eq!(order[0]["query"], "query");
    assert_eq!(order[0]["alias"], "this.base");
    assert_eq!(order[0]["template"], "${environment.apiUrl}/orders");

    // The two smr.service.ts sites are one node: a const `q` and an inline
    // ternary, both literal paths now (no template).
    let smr = hits(&m, "endpoint:GET:/admin/smr");
    let mut got: Vec<(String, String)> = smr
        .iter()
        .map(|h| {
            assert!(h.get("template").is_none(), "{h}");
            assert_eq!(h["confidence"], "strong", "{h}");
            (
                h["file"].as_str().unwrap_or_default().to_string(),
                h["query"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("src/app/smr.service.ts".to_string(), "inline".to_string()),
            ("src/app/smr.service.ts".to_string(), "q".to_string()),
        ]
    );

    // Kina's call-built base stays `${this.base}`; the query still goes.
    let admin = hits(&m, "endpoint:GET:${…}/smr");
    assert_eq!(admin.len(), 1, "{admin:?}");
    assert_eq!(admin[0]["file"], "src/app/admin/smr-admin.service.ts");
    assert_eq!(admin[0]["query"], "q");
    assert_eq!(admin[0]["template"], "${this.base}/smr");

    // Controls: no `query` key.
    for ep in [
        "endpoint:GET:/api/orders/${…}",
        "endpoint:GET:/api/products${…}",
        "endpoint:POST:${…}/smr",
    ] {
        let h = hits(&m, ep);
        assert_eq!(h.len(), 1, "{ep}: {h:?}");
        assert!(h[0].get("query").is_none(), "{ep}: {}", h[0]);
    }
    let products = hits(&m, "endpoint:GET:/api/products${…}");
    assert_eq!(
        products[0]["template"],
        "${environment.apiUrl}/products${suffix}"
    );
}

const CHILD_ENV: &str = "GLIA_DT4_CHILD_BUILD";

/// The child half of [`build_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds that tree so its stderr (with
/// `--nocapture`) carries the build's markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build_in(Path::new(&dir));
    }
}

/// The stderr lines of one fresh build of the fixture.
fn build_stderr() -> Vec<String> {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_build_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, td.path())
        .output()
        .expect("re-run the test binary");
    assert!(
        out.status.success(),
        "child build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn markers_count_each_query_site() {
    let lines = build_stderr();
    let query: BTreeSet<String> = lines
        .iter()
        .filter(|l| l.starts_with("[ts-endpoints] query suffixes="))
        .cloned()
        .collect();
    assert_eq!(
        query,
        set(&[
            "[ts-endpoints] query suffixes=1 file=src/app/order.service.ts",
            "[ts-endpoints] query suffixes=2 file=src/app/smr.service.ts",
            "[ts-endpoints] query suffixes=1 file=src/app/admin/smr-admin.service.ts",
        ])
    );
    let http: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("[http] routes="))
        .collect();
    assert_eq!(
        http,
        vec![
            "[http] routes=5 endpoints=6 paired=5 (exact=3 any=0 eprefix=0 rprefix=0 base=2 suffix=0)"
        ],
        "{lines:?}"
    );
}
