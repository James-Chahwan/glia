//! GP.4: a layout write runs each shard's per-cell work (the copy into the
//! core, EVIDENCE interning's decisions, the CODE span files' reads and
//! matches) on a `PersistPool`, and writes the same bytes with any pool or
//! none. The graph is synthetic and large enough to cut into many chunks:
//! CODE texts that are spans of their POSITION line, texts that are not
//! (edited, a missing file, no POSITION, a JSON payload, two CODE cells on
//! one node), canonical EVIDENCE, EVIDENCE left as written and cross edges.
//! It is written with no pool and with 3 and 8 workers (jobs run in reverse
//! order on real threads); then its sources are edited, the layout read back
//! (unresolved spans) and written again both ways.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use glia_code_domain::code_span::CodeSpan;
use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{CodeNav, cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable};
use glia_store::{
    LayoutMeta, MANIFEST_NAME, MmapContainer, PersistPool, RepoMeta, decode_repo_graph,
    read_merged_sharded_meta, write_merged_sharded_meta, write_merged_sharded_pooled,
};

const FILES: usize = 24;
const LINES: usize = 400;

/// A pool of `threads` scoped threads that take jobs from the back, so the
/// chunks finish in an order unlike their input order. Counts the jobs run.
struct ReversePool {
    threads: usize,
    jobs: AtomicUsize,
}

impl ReversePool {
    fn new(threads: usize) -> Self {
        Self {
            threads,
            jobs: AtomicUsize::new(0),
        }
    }
}

impl PersistPool for ReversePool {
    fn threads(&self) -> usize {
        self.threads
    }

    fn run(&self, n: usize, job: &(dyn Fn(usize) + Sync)) {
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..self.threads {
                s.spawn(|| {
                    loop {
                        let k = next.fetch_add(1, Ordering::SeqCst);
                        if k >= n {
                            break;
                        }
                        self.jobs.fetch_add(1, Ordering::SeqCst);
                        job(n - 1 - k);
                    }
                });
            }
        });
    }
}

fn repo_id() -> RepoId {
    RepoId::from_canonical("test://gp4/layout_parallel")
}

fn file_name(f: usize) -> String {
    format!("src/mod_{:02}/file_{f}.rs", f % 5)
}

fn line_text(f: usize, l: usize) -> String {
    format!("fn f_{f}_{l}() {{ let x = {}; }}", l * 7 + f)
}

/// `<tmp>/repo`: `FILES` source files of `LINES` lines each.
fn write_sources(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    for f in 0..FILES {
        let path = repo.join(file_name(f));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: String = (0..LINES).map(|l| line_text(f, l) + "\n").collect();
        std::fs::write(path, body).unwrap();
    }
    repo
}

fn position(file: &str, line: usize) -> Cell {
    Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(format!(
            r#"{{"file":"{file}","start_line":{line},"end_line":{}}}"#,
            line + 1
        )),
    }
}

fn code(text: String) -> Cell {
    Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Text(text),
    }
}

/// One node's cells, by `n`: the CODE shapes a real shard holds.
fn node_cells(n: usize) -> Vec<Cell> {
    let f = n % FILES;
    let l = n % (LINES - 2);
    let span = || line_text(f, l) + "\n" + &line_text(f, l + 1);
    let at = || position(&file_name(f), l);
    match n % 9 {
        // No CODE at all.
        0 => vec![at()],
        // No POSITION: inline.
        1 => vec![code(span())],
        // A JSON CODE payload: inline.
        2 => vec![
            at(),
            Cell {
                kind: cell_type::CODE,
                payload: CellPayload::Json(format!(r#"{{"n":{n}}}"#)),
            },
        ],
        // A file the source cannot read: inline.
        3 => vec![position("src/missing.rs", l), code(span())],
        // Not on its line (edited): inline.
        4 => vec![at(), code(format!("fn edited_{n}() {{}}"))],
        // Two CODE cells: one span, one starting mid-line.
        5 => vec![at(), code(span()), code(format!("let x = {};", l * 7 + f))],
        // A span.
        _ => vec![at(), code(span())],
    }
}

fn evidence_cell(i: usize) -> Cell {
    let emitters = ["graph:calls", "graph:imports", "parser:rust", "pass:tests"];
    let rules = [
        "import_binding",
        "module_symbol",
        "receiver_type",
        "intra_file",
    ];
    match i % 23 {
        // JSON in another field order: kept as written.
        0 => Cell {
            kind: cell_type::EVIDENCE,
            payload: CellPayload::Json(format!(r#"{{"rule":"r{i}","emitter":"e"}}"#)),
        },
        // A text payload: kept as written.
        1 => Cell {
            kind: cell_type::EVIDENCE,
            payload: CellPayload::Text(format!("evidence {i}")),
        },
        _ => {
            let mut ev = Evidence::emitter(emitters[i % 4]);
            if !i.is_multiple_of(3) {
                ev = ev.rule(rules[i % 4]);
            }
            if !i.is_multiple_of(5) {
                ev = ev.at(file_name(i % FILES), (i % 3000) as u32);
            }
            ev.basis = [Basis::Site, Basis::FromNode, Basis::File][i % 3];
            ev.to_cell()
        }
    }
}

fn graph(base: usize, nodes: usize, edges: usize) -> RepoGraph {
    let repo = repo_id();
    let ids: Vec<NodeId> = (0..nodes).map(|n| NodeId((base + n) as u64 + 1)).collect();
    let mut nav = CodeNav::default();
    for id in &ids {
        nav.kind_by_id.insert(*id, node_kind::FUNCTION);
    }
    let nodes = ids
        .iter()
        .enumerate()
        .map(|(n, id)| Node {
            id: *id,
            repo,
            confidence: Confidence::Strong,
            cells: node_cells(base + n),
        })
        .collect();
    let edges = (0..edges)
        .map(|i| {
            Edge::new(
                ids[i % ids.len()],
                ids[(i * 7 + 1) % ids.len()],
                edge_category::CALLS,
                Confidence::Strong,
            )
            .with_cell(evidence_cell(base + i))
        })
        .collect();
    RepoGraph {
        repo,
        nodes,
        edges,
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: Default::default(),
    }
}

fn merged() -> MergedGraph {
    let mut m = MergedGraph::new(vec![graph(0, 7_000, 11_000), graph(50_000, 600, 900)]);
    m.cross_edges = (0..5_000)
        .map(|i| {
            Edge::new(
                NodeId(i as u64 + 1),
                NodeId(50_001 + (i % 600) as u64),
                edge_category::HTTP_CALLS,
                Confidence::Medium,
            )
            .with_cell(evidence_cell(i * 3))
        })
        .collect();
    m
}

/// One repo rooted at `../repo` from `<tmp>/<layout>`.
fn meta() -> LayoutMeta {
    LayoutMeta {
        repos: vec![RepoMeta {
            id: repo_id().0,
            label: "gp4".into(),
            root: Some("../repo".into()),
            rev: None,
        }],
        parse_errors: vec![],
        code_spans_unresolved: 0,
    }
}

/// Every file of a layout dir, sorted by name, with its bytes.
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
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

/// The CODE cells of `dir`'s shards stored as spans: decoded with no source,
/// each reads back as its `CodeSpan` JSON.
fn spans_in(dir: &Path) -> usize {
    let mut n = 0;
    for (name, _) in files(dir) {
        if !name.starts_with("repo-") {
            continue;
        }
        let g = decode_repo_graph(&MmapContainer::open(&dir.join(&name)).unwrap()).unwrap();
        n += g
            .nodes
            .iter()
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::CODE)
            .filter(|c| CodeSpan::from_payload(&c.payload).is_some())
            .count();
    }
    n
}

/// Write `m` at `<tmp>/<name>` with no pool and with 3 and 8 workers, and
/// through the poolless public writer; every layout holds the same files.
/// Returns the no-pool layout dir.
fn write_every_way(tmp: &Path, m: &MergedGraph, meta: &LayoutMeta, name: &str) -> PathBuf {
    let serial = tmp.join(format!("{name}-serial"));
    let (manifest, _) = write_merged_sharded_pooled(m, meta, &serial, None).unwrap();
    let public = tmp.join(format!("{name}-public"));
    let public_manifest = write_merged_sharded_meta(m, meta, &public).unwrap();
    assert_eq!(manifest, public_manifest);
    assert_eq!(files(&serial), files(&public), "{name}: public writer");
    for threads in [3usize, 8] {
        let pool = ReversePool::new(threads);
        let dir = tmp.join(format!("{name}-pool{threads}"));
        let (pooled, times) = write_merged_sharded_pooled(m, meta, &dir, Some(&pool)).unwrap();
        assert!(
            pool.jobs.load(Ordering::SeqCst) > threads,
            "{name}: the {threads}-worker pool ran {} jobs; the graph must cut into chunks",
            pool.jobs.load(Ordering::SeqCst)
        );
        assert!(times.spans > std::time::Duration::ZERO, "{name}: {times:?}");
        assert_eq!(pooled, manifest, "{name}: manifest with {threads} workers");
        let (a, b) = (files(&serial), files(&dir));
        assert_eq!(
            a.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            b.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        for ((name_a, bytes_a), (_, bytes_b)) in a.iter().zip(&b) {
            assert!(
                bytes_a == bytes_b,
                "{name}: {name_a} differs with {threads} workers"
            );
        }
    }
    assert!(serial.join(MANIFEST_NAME).is_file());
    serial
}

#[test]
fn pooled_layout_is_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    write_sources(tmp.path());
    let m = merged();
    let serial = write_every_way(tmp.path(), &m, &meta(), "fresh");
    let spanned = spans_in(&serial);
    assert!(spanned > 3_000, "spans written: {spanned}");
    let (back, back_meta) = read_merged_sharded_meta(&serial).unwrap();
    assert_eq!(back_meta.code_spans_unresolved, 0);
    for (b, g) in back.graphs.iter().zip(&m.graphs) {
        assert_eq!(b.nodes, g.nodes, "every span reads back as its text");
        assert_eq!(b.edges, g.edges);
    }
    assert_eq!(back.cross_edges, m.cross_edges);
}

#[test]
fn edited_sources_rewrite_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = write_sources(tmp.path());
    let first = tmp.path().join("first");
    write_merged_sharded_pooled(&merged(), &meta(), &first, None).unwrap();
    // Every third file gains a first line: its spans no longer hash.
    for f in (0..FILES).step_by(3) {
        let path = repo.join(file_name(f));
        let body = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("// edited\n{body}")).unwrap();
    }
    let (back, back_meta) = read_merged_sharded_meta(&first).unwrap();
    assert!(
        back_meta.code_spans_unresolved > 100,
        "unresolved spans: {}",
        back_meta.code_spans_unresolved
    );
    let serial = write_every_way(tmp.path(), &back, &back_meta, "edited");
    // An unresolved span is `CodeSpan` JSON in memory, written as it is: the
    // rewrite reads back with every source in place.
    let (again, again_meta) = read_merged_sharded_meta(&serial).unwrap();
    assert_eq!(again_meta.code_spans_unresolved, 0);
    for (a, b) in again.graphs.iter().zip(&back.graphs) {
        assert_eq!(a.nodes, b.nodes);
        assert_eq!(a.edges, b.edges);
    }
}

/// The raw-payload refusal names the same edge / node with a pool, past the
/// first chunk.
#[test]
fn pooled_refusal_names_the_same_cell() {
    let tmp = tempfile::tempdir().unwrap();
    write_sources(tmp.path());
    let mut m = merged();
    m.graphs[0].edges[9_000].cells = vec![Cell {
        kind: cell_type::EVIDENCE,
        payload: CellPayload::Bytes(vec![0x01, 0, 0, 0, 0, 1]),
    }];
    m.graphs[0].nodes[6_500].cells = vec![Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Bytes(vec![0x02, 1]),
    }];
    let serial = write_merged_sharded_pooled(&m, &meta(), &tmp.path().join("a"), None)
        .unwrap_err()
        .to_string();
    let pool = ReversePool::new(8);
    let pooled = write_merged_sharded_pooled(&m, &meta(), &tmp.path().join("b"), Some(&pool))
        .unwrap_err()
        .to_string();
    assert!(serial.contains("edge 9000 "), "{serial}");
    assert_eq!(serial, pooled);
    m.graphs[0].edges[9_000].cells = vec![evidence_cell(9_000)];
    let serial = write_merged_sharded_pooled(&m, &meta(), &tmp.path().join("c"), None)
        .unwrap_err()
        .to_string();
    let pooled = write_merged_sharded_pooled(&m, &meta(), &tmp.path().join("d"), Some(&pool))
        .unwrap_err()
        .to_string();
    assert!(serial.contains("node 6500 "), "{serial}");
    assert_eq!(serial, pooled);
}
