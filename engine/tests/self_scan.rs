//! FD.6: the zero-phantom self-scan gate.
//!
//! glia's own production and test code holds a small, fixed set of real
//! mechanism nodes (the CLI invocations it runs, the env vars it reads, ..).
//! Anything else a build of glia's tree mints is a phantom: a needle read
//! out of a string literal, a comment or a test fixture string. The 0.5.2
//! waves reintroduced such phantoms three times; this test stops a fourth.
//!
//! [`self_scan_has_no_phantoms`] copies the tracked tree (`git ls-files`)
//! into a temp dir, adds a `[walk] skip` for the fixture trees that are MEANT
//! to mint mechanisms ([`SKIP`]), builds the copy and compares every
//! mechanism-kind node ([`MECHANISM_KINDS`]) with the committed allowlist
//! `engine/tests/data/self_scan_allow.json`, exactly: a minted pair not on
//! the list fails, and so does a listed pair no longer minted.
//!
//! A later packet that adds a real mechanism to glia's own non-fixture code
//! (a new env var read, a new `Command::new`) adds an allowlist row in the
//! same commit, its `why` naming the source file. A phantom is never
//! allowlisted: the scanner that minted it is fixed (the code guard, FD.4a).
//!
//! [`self_scan_control_catches_a_phantom`] runs the same checker over a
//! two-file tree with an empty allowlist: it must report the Python Kafka
//! consumer and nothing from the Rust file whose only Kafka text sits in a
//! string literal (the gate sees a real mechanism; the guard hides the
//! literal).
//!
//! [`self_graph_has_build_pipeline`] (GD.25) reads the same build: every
//! function of the engine's build pipeline ([`PIPELINE`]) is a FUNCTION node
//! located under `engine/src/build/`. A source directory named `build` is
//! walked, not collapsed to a build-output REGION, unless it sits directly
//! under the repo root or a project root (`walk_gating::gate_dir_in`).
//!
//! fired_on, on stderr: `[self-scan] mechanism_nodes=<n> allowlisted=<a>
//! phantoms=<p> skipped_roots=<k> ms=<t>` and `[self-scan] pipeline
//! functions=<found>/<expected>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

use glia_code_domain::evidence::{Evidence, locate};
use glia_code_domain::glia_config::PathGlobs;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::{GenerateResult, generate_one};

/// The fixture trees skipped in the self-scan, as `[walk] skip` patterns
/// (gitignore syntax: a trailing-slash name matches that directory at any
/// depth). bench/ holds the substrate-gap fixtures, dev-notes/ the plans,
/// every `tests/` the integration tests and their fixture repos (the root
/// `tests/` holds go_smoke / py_smoke / ts_smoke / http_stack_smoke /
/// arch_* repos), and `tests/data/` / `tests/fixtures/` fixture sources.
const SKIP: [&str; 6] = [
    "bench/",
    "dev-notes/",
    "tests/",
    "**/tests/data/",
    "**/tests/fixtures/",
    "py/tests/",
];

/// The node kinds a mechanism extractor mints.
const MECHANISM_KINDS: [NodeKindId; 23] = [
    node_kind::ROUTE,
    node_kind::ENDPOINT,
    node_kind::QUEUE_PRODUCER,
    node_kind::QUEUE_CONSUMER,
    node_kind::EVENT_EMITTER,
    node_kind::EVENT_HANDLER,
    node_kind::WS_CLIENT,
    node_kind::WS_HANDLER,
    node_kind::GRPC_CLIENT,
    node_kind::GRPC_SERVER,
    node_kind::GRPC_SERVICE,
    node_kind::GRAPHQL_OPERATION,
    node_kind::GRAPHQL_RESOLVER,
    node_kind::RPC_CALL,
    node_kind::RPC_PROCEDURE,
    node_kind::CRON_JOB,
    node_kind::CLI_INVOCATION,
    node_kind::DATA_ENTITY,
    node_kind::DATABASE,
    node_kind::CACHE,
    node_kind::CONFIG_KEY,
    node_kind::INFRA_RESOURCE,
    node_kind::MESSAGE_TYPE,
];

/// One minted mechanism node: its kind name, qname and where it was read:
/// its own POSITION, then its first incoming edge's EVIDENCE
/// (`<file>:<1-based line>`, the least over the incoming edges, a
/// DOCUMENTS edge only when no other one has a file).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Minted {
    kind: String,
    qname: String,
    site: String,
}

/// One allowlist row.
#[derive(Debug, Clone, serde::Deserialize)]
struct AllowRow {
    kind: String,
    qname: String,
    why: String,
}

/// Removes the temp tree when dropped, also on a panic.
struct TempTree(PathBuf);

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl TempTree {
    fn new(tag: &str) -> TempTree {
        let dir = std::env::temp_dir().join(format!(
            "glia-self-scan-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the temp tree");
        TempTree(dir)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("a UTF-8 temp path")
    }
}

/// `<file>:<1-based line>` from a 0-based location.
fn site((file, line): (String, Option<u32>)) -> String {
    match line {
        Some(l) => format!("{file}:{}", l + 1),
        None => file,
    }
}

/// Every mechanism-kind node of a build, sorted by (kind, qname), one row per
/// pair (a pair minted in several graphs or under several ids counts once,
/// its least site kept), and the count of distinct mechanism node ids.
fn mechanisms(r: &GenerateResult) -> (Vec<Minted>, usize) {
    let mechanism: BTreeSet<u32> = MECHANISM_KINDS.iter().map(|k| k.0).collect();
    let mut nodes: Vec<(String, String, NodeId, Option<String>)> = Vec::new();
    let mut ids: BTreeSet<u64> = BTreeSet::new();
    for graph in &r.merged.graphs {
        for n in &graph.nodes {
            let Some(kind) = graph.nav.kind_by_id.get(&n.id).copied() else {
                continue;
            };
            if !mechanism.contains(&kind.0) {
                continue;
            }
            let Some(qname) = graph.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            ids.insert(n.id.0);
            nodes.push((
                node_kind::name(kind).to_string(),
                qname.clone(),
                n.id,
                locate(&n.cells).map(site),
            ));
        }
    }
    // (is a DOCUMENTS edge, site): the least per node, so a doc mention
    // never hides the minting edge and no edge order leaks in.
    let mut edge_site: BTreeMap<u64, (bool, String)> = BTreeMap::new();
    let edges = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(r.merged.cross_edges.iter());
    for e in edges {
        if !ids.contains(&e.to.0) {
            continue;
        }
        let Some(ev) = Evidence::of(e) else {
            continue;
        };
        let Some(file) = ev.file else {
            continue;
        };
        let key = (
            e.category == edge_category::DOCUMENTS,
            site((file, ev.line)),
        );
        edge_site
            .entry(e.to.0)
            .and_modify(|s| {
                if key < *s {
                    *s = key.clone();
                }
            })
            .or_insert(key);
    }
    let mut minted: Vec<Minted> = nodes
        .into_iter()
        .map(|(kind, qname, id, at)| {
            let first_edge = edge_site.get(&id.0).map(|(_, s)| s.as_str());
            let site = match (at, first_edge) {
                (Some(at), Some(e)) => format!("at {at}; first incoming edge {e}"),
                (Some(at), None) => format!("at {at}; no incoming edge evidence"),
                (None, Some(e)) => format!("first incoming edge {e}"),
                (None, None) => "no position, no incoming edge evidence".to_string(),
            };
            Minted { kind, qname, site }
        })
        .collect();
    minted.sort();
    minted.dedup_by(|a, b| a.kind == b.kind && a.qname == b.qname);
    (minted, ids.len())
}

/// The gate's verdict: every minted pair not allowlisted (a phantom), and
/// every allowlisted pair no longer minted, each a sorted failure line.
#[derive(Debug, Default)]
struct Verdict {
    phantoms: Vec<String>,
    stale: Vec<String>,
    allowlisted: usize,
}

fn check(minted: &[Minted], allow: &[AllowRow]) -> Verdict {
    let allowed: BTreeSet<(&str, &str)> = allow
        .iter()
        .map(|a| (a.kind.as_str(), a.qname.as_str()))
        .collect();
    let seen: BTreeSet<(&str, &str)> = minted
        .iter()
        .map(|m| (m.kind.as_str(), m.qname.as_str()))
        .collect();
    let mut v = Verdict::default();
    for m in minted {
        if allowed.contains(&(m.kind.as_str(), m.qname.as_str())) {
            v.allowlisted += 1;
        } else {
            v.phantoms
                .push(format!("{} {}  ({})", m.kind, m.qname, m.site));
        }
    }
    for a in allow {
        if !seen.contains(&(a.kind.as_str(), a.qname.as_str())) {
            v.stale
                .push(format!("{} {}  (allowlisted: {})", a.kind, a.qname, a.why));
        }
    }
    v.phantoms.sort();
    v.stale.sort();
    v
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root")
        .to_path_buf()
}

fn allowlist() -> Vec<AllowRow> {
    let path = repo_root().join("engine/tests/data/self_scan_allow.json");
    let text = std::fs::read_to_string(&path).expect("read self_scan_allow.json");
    let rows: Vec<AllowRow> = serde_json::from_str(&text).expect("parse self_scan_allow.json");
    let keys: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.kind.clone(), r.qname.clone()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        keys, sorted,
        "self_scan_allow.json rows are sorted by (kind, qname), unique"
    );
    for r in &rows {
        // Each `why` starts with the real source file the mechanism sits in.
        let file = r.why.split_whitespace().next().unwrap_or("");
        assert!(
            !file.is_empty() && repo_root().join(file).is_file(),
            "{} {}: its why must start with an existing source file, got {:?}",
            r.kind,
            r.qname,
            r.why
        );
    }
    rows
}

/// The tracked tree (`git ls-files`), copied file by file into `dst`;
/// returns the repo-relative paths copied.
fn copy_tracked(src: &Path, dst: &Path) -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(src)
        .args(["ls-files", "-z"])
        .output()
        .expect("run git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    let mut copied = Vec::new();
    for rel in out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let rel = String::from_utf8_lossy(rel).into_owned();
        let from = src.join(&rel);
        // A tracked file deleted in the working tree, or a submodule.
        if !from.is_file() {
            continue;
        }
        let to = dst.join(&rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).expect("create a copy dir");
        }
        std::fs::copy(&from, &to).expect("copy a tracked file");
        copied.push(rel);
    }
    copied
}

/// The distinct shallowest directories a skip pattern matched among `files`.
fn skipped_roots(files: &[String], globs: &PathGlobs) -> usize {
    let mut roots: BTreeSet<String> = BTreeSet::new();
    for f in files {
        if !globs.matches(f) {
            continue;
        }
        let parts: Vec<&str> = f.split('/').collect();
        let mut root = f.clone();
        for i in 1..parts.len() {
            let dir = format!("{}/", parts[..i].join("/"));
            if globs.matches(&format!("{dir}x")) {
                root = dir;
                break;
            }
        }
        roots.insert(root);
    }
    roots.len()
}

fn report(minted: &[Minted], nodes: usize, v: &Verdict, skipped: usize, start: Instant) {
    eprintln!(
        "[self-scan] mechanism_nodes={} allowlisted={} phantoms={} skipped_roots={} ms={}",
        nodes.max(minted.len()),
        v.allowlisted,
        v.phantoms.len(),
        skipped,
        start.elapsed().as_millis()
    );
}

fn failure(v: &Verdict) -> Option<String> {
    if v.phantoms.is_empty() && v.stale.is_empty() {
        return None;
    }
    let mut msg = String::from(
        "the self-scan is not exact.\n\
         A phantom is fixed at the scanner that minted it (route it through the code guard, \
         FD.4a), never allowlisted. A REAL mechanism added to glia's own code (a new env var \
         read, a new Command::new) gets a row in engine/tests/data/self_scan_allow.json in the \
         same commit, its `why` naming the source file.\n",
    );
    for p in &v.phantoms {
        msg.push_str(&format!("  phantom: {p}\n"));
    }
    for s in &v.stale {
        msg.push_str(&format!("  allowlisted but no longer minted: {s}\n"));
    }
    Some(msg)
}

/// One build of glia's own tracked tree, shared by the tests that read it.
struct SelfBuild {
    result: GenerateResult,
    /// Distinct shallowest directories the [`SKIP`] patterns matched.
    skipped: usize,
    /// When the copy started, for the marker's `ms=`.
    start: Instant,
}

/// Copies the tracked tree (`git ls-files`) into a temp dir with the
/// [`SKIP`] overlay and builds it, once per test binary run.
fn self_build() -> &'static SelfBuild {
    static BUILD: OnceLock<SelfBuild> = OnceLock::new();
    BUILD.get_or_init(|| {
        let start = Instant::now();
        let tree = TempTree::new("tree");
        let root = repo_root();
        let copied = copy_tracked(&root, &tree.0);
        // The scratch overlay: glia's own (copied) plus the fixture skips.
        let overlay = tree.0.join(".glia/overlay.toml");
        let own = std::fs::read_to_string(&overlay).unwrap_or_else(|_| "version = 1\n".to_string());
        assert!(
            !own.lines().any(|l| l.trim() == "[walk]"),
            "glia's .glia/overlay.toml declares [walk]; merge SKIP into it here"
        );
        let skip: Vec<String> = SKIP.iter().map(|s| s.to_string()).collect();
        let skip_line = skip
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::create_dir_all(overlay.parent().expect("overlay dir")).expect("create .glia");
        std::fs::write(&overlay, format!("{own}\n[walk]\nskip = [{skip_line}]\n"))
            .expect("write the scratch overlay");
        let globs = PathGlobs::new(&skip).expect("the skip patterns compile");
        let skipped = skipped_roots(&copied, &globs);
        let result = generate_one(tree.path()).expect("glia's own tree builds");
        SelfBuild {
            result,
            skipped,
            start,
        }
    })
}

#[test]
fn self_scan_has_no_phantoms() {
    let b = self_build();
    let (minted, nodes) = mechanisms(&b.result);
    let v = check(&minted, &allowlist());
    report(&minted, nodes, &v, b.skipped, b.start);
    if let Some(msg) = failure(&v) {
        panic!("{msg}");
    }
}

/// The engine's build pipeline (engine/src/build/mod.rs and assemble.rs):
/// the functions GD.25's probe found missing from glia's self-graph while
/// `engine/src/build/` was collapsed as build output.
const PIPELINE: [&str; 5] = [
    "aux_graphs",
    "assemble_many",
    "build_graphs_for_repo",
    "finish_build",
    "parse_and_graft",
];

#[test]
fn self_graph_has_build_pipeline() {
    let r = &self_build().result;
    let mut found: BTreeSet<&str> = BTreeSet::new();
    for graph in &r.merged.graphs {
        for n in &graph.nodes {
            if graph.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::FUNCTION) {
                continue;
            }
            let Some(qname) = graph.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let name = qname.rsplit("::").next().unwrap_or(qname.as_str());
            let Some(want) = PIPELINE.iter().find(|p| **p == name) else {
                continue;
            };
            let in_build =
                locate(&n.cells).is_some_and(|(file, _)| file.starts_with("engine/src/build/"));
            if in_build {
                found.insert(want);
            }
        }
    }
    eprintln!(
        "[self-scan] pipeline functions={}/{}",
        found.len(),
        PIPELINE.len()
    );
    let missing: Vec<&str> = PIPELINE
        .iter()
        .copied()
        .filter(|p| !found.contains(p))
        .collect();
    assert!(
        missing.is_empty(),
        "glia's self-graph has no FUNCTION node under engine/src/build/ for: {missing:?}"
    );
}

#[test]
fn self_scan_control_catches_a_phantom() {
    let start = Instant::now();
    let tree = TempTree::new("control");
    std::fs::write(
        tree.0.join("consumer.py"),
        "from kafka import KafkaConsumer\n\nconsumer = KafkaConsumer(\"selfscan_probe\", bootstrap_servers=\"localhost:9092\")\n",
    )
    .expect("write the python probe");
    std::fs::write(
        tree.0.join("lit.rs"),
        "pub fn note() -> &'static str {\n    \"from kafka import KafkaConsumer\\nconsumer = KafkaConsumer(\\\"selfscan_literal\\\")\"\n}\n",
    )
    .expect("write the rust literal");
    let r = generate_one(tree.path()).expect("the control tree builds");
    let (minted, nodes) = mechanisms(&r);
    let v = check(&minted, &[]);
    report(&minted, nodes, &v, 0, start);
    let pairs: Vec<String> = minted
        .iter()
        .map(|m| format!("{} {}", m.kind, m.qname))
        .collect();
    assert_eq!(
        pairs,
        ["QUEUE_CONSUMER queue_consumer:selfscan_probe"],
        "the control must see the probe and not the literal; checker phantoms: {:?}",
        v.phantoms
    );
    assert_eq!(v.phantoms.len(), 1, "an empty allowlist reports the probe");
    assert!(
        v.phantoms[0].contains("consumer.py"),
        "located: {:?}",
        v.phantoms
    );
}
