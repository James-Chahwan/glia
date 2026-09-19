//! LF.1a: `.glia/cells.jsonl` and `.glia/vectors.jsonl` are applied to their
//! nodes at build, through `repo_graph_graph::cells`, and a build with a
//! sidecar is as byte-reproducible as one without.

use std::path::Path;

use repo_graph_code_domain::cell_type;
use repo_graph_core::{Cell, CellPayload, CellTypeId};
use repo_graph_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};
use repo_graph_store::write_merged_sharded;

const SOURCE: &str = "def charge(order_id):\n    return order_id\n";
const CONV_ROW: &str = r#"{"qname":"a::charge","cell":"CONV","entry":{"source":"api","id":"000001","text":"retries are safe"}}"#;

/// A repo dir holding `a.py` and the given sidecar files.
fn repo(tmp: &Path, cells: Option<&str>, vectors: Option<&str>) -> String {
    let dir = tmp.join("repo");
    std::fs::create_dir_all(dir.join(".glia")).unwrap();
    std::fs::write(dir.join("a.py"), SOURCE).unwrap();
    if let Some(c) = cells {
        std::fs::write(dir.join(".glia/cells.jsonl"), c).unwrap();
    }
    if let Some(v) = vectors {
        std::fs::write(dir.join(".glia/vectors.jsonl"), v).unwrap();
    }
    dir.to_str().unwrap().to_string()
}

/// The cells of the node whose qname is `qname`.
fn cells_of(r: &GenerateResult, qname: &str) -> Vec<Cell> {
    let hits: Vec<&Vec<Cell>> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)))
        .map(|n| &n.cells)
        .collect();
    assert_eq!(hits.len(), 1, "exactly one node is {qname}");
    hits[0].clone()
}

fn of_type(cells: &[Cell], t: CellTypeId) -> Vec<&CellPayload> {
    cells.iter().filter(|c| c.kind == t).map(|c| &c.payload).collect()
}

#[test]
fn sidecar_conv_row_lands_on_its_node() {
    let tmp = tempfile::tempdir().unwrap();
    let r = generate_one(&repo(tmp.path(), Some(&format!("{CONV_ROW}\n")), None)).unwrap();
    let cells = cells_of(&r, "a::charge");
    assert_eq!(
        of_type(&cells, cell_type::CONV),
        [&CellPayload::Json(r#"[{"id":"000001","source":"api","text":"retries are safe"}]"#.into())]
    );
}

#[test]
fn orphaned_and_rejected_rows_change_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let bare_tmp = tempfile::tempdir().unwrap();
    let bare = generate_one(&repo(bare_tmp.path(), None, None)).unwrap();
    let rows = [
        r#"{"qname":"a::gone","cell":"CONV","entry":{"source":"api","id":"1","text":"t"}}"#,
        r#"{"qname":"a::charge","cell":"CODE","entry":{"source":"api","id":"1","text":"t"}}"#,
        r#"{"qname":"a::charge","cell":"CONV","entry":{"source":"web","id":"1","text":"t"}}"#,
        r#"{"qname":"a::charge","cell":"NOPE","entry":{}}"#,
        "not json at all",
    ];
    let r = generate_one(&repo(tmp.path(), Some(&rows.join("\n")), None)).unwrap();
    let node_cells = |g: &GenerateResult| -> Vec<Vec<Cell>> {
        g.merged.graphs.iter().flat_map(|x| x.nodes.iter().map(|n| n.cells.clone())).collect()
    };
    assert_eq!(node_cells(&r), node_cells(&bare), "no node gains or loses a cell");
    let (after, before) = (cells_of(&r, "a::charge"), cells_of(&bare, "a::charge"));
    let code = of_type(&after, cell_type::CODE);
    assert_eq!(code, of_type(&before, cell_type::CODE), "a::charge's CODE bytes are unchanged");
    assert_eq!(code.len(), 1);
}

#[test]
fn vector_row_decodes_to_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let ok = r#"{"qname":"a::charge","dims":2,"b64":"AACAPwAAAEA="}"#;
    let r = generate_one(&repo(tmp.path(), None, Some(ok))).unwrap();
    assert_eq!(
        of_type(&cells_of(&r, "a::charge"), cell_type::VECTOR),
        [&CellPayload::Bytes(vec![0, 0, 128, 63, 0, 0, 0, 64])]
    );

    let tmp = tempfile::tempdir().unwrap();
    let bad = r#"{"qname":"a::charge","dims":3,"b64":"AACAPwAAAEA="}"#;
    let r = generate_one(&repo(tmp.path(), None, Some(bad))).unwrap();
    assert!(of_type(&cells_of(&r, "a::charge"), cell_type::VECTOR).is_empty(), "dims=3 needs 12 bytes");
}

/// A sidecar writes node cells and nothing else: the edges and every other
/// node are what a build without it gives.
#[test]
fn sidecar_changes_only_its_node_cells() {
    let (with_tmp, bare_tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let vec_row = r#"{"qname":"a::charge","dims":2,"b64":"AACAPwAAAEA="}"#;
    let with = generate_one(&repo(with_tmp.path(), Some(CONV_ROW), Some(vec_row))).unwrap();
    let bare = generate_one(&repo(bare_tmp.path(), None, None)).unwrap();
    assert_eq!(with.merged.cross_edges, bare.merged.cross_edges);
    for (a, b) in with.merged.graphs.iter().zip(&bare.merged.graphs) {
        assert_eq!(a.edges, b.edges);
        for (na, nb) in a.nodes.iter().zip(&b.nodes) {
            assert_eq!(na.id, nb.id);
            if a.nav.qname_by_id.get(&na.id).is_some_and(|q| q == "a::charge") {
                assert_eq!(na.cells.len(), nb.cells.len() + 2, "CONV + VECTOR appended");
                assert_eq!(na.cells[..nb.cells.len()], nb.cells[..]);
            } else {
                assert_eq!(na.cells, nb.cells);
            }
        }
    }
}

/// Map of file name -> bytes for every file in a sharded output dir
/// (byte_identical.rs's helper).
fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn sidecar_builds_are_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let cells = format!(
        "{CONV_ROW}\n{}\n",
        r#"{"qname":"a::charge","cell":"DECISION","entry":{"source":"api","id":"d1","title":"charge is idempotent","status":"Accepted"}}"#
    );
    let vec_row = r#"{"qname":"a::charge","dims":2,"b64":"AACAPwAAAEA="}"#;
    let repo_s = repo(tmp.path(), Some(&cells), Some(vec_row));

    let out = |name: &str| tmp.path().join(name);
    write_merged_sharded(&generate_one(&repo_s).unwrap().merged, &out("clean1")).unwrap();
    write_merged_sharded(&generate_one(&repo_s).unwrap().merged, &out("clean2")).unwrap();
    let mut cache = ParseCache::new();
    generate_one_with_cache(&repo_s, &mut cache).unwrap();
    let warm = generate_one_with_cache(&repo_s, &mut cache).unwrap();
    assert!(cache.stats.reused > 0, "the cached build must reuse its parse");
    write_merged_sharded(&warm.merged, &out("cached")).unwrap();

    let clean1 = dir_bytes(&out("clean1"));
    assert!(!clean1.is_empty());
    assert_eq!(clean1, dir_bytes(&out("clean2")), "clean vs clean");
    assert_eq!(clean1, dir_bytes(&out("cached")), "clean vs cached");
    let decision = of_type(&cells_of(&warm, "a::charge"), cell_type::DECISION)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        decision,
        [CellPayload::Json(r#"[{"id":"d1","source":"api","status":"accepted","title":"charge is idempotent"}]"#.into())]
    );
}

/// A row written before its file moved still binds: its move-stable hint
/// (LB.6) re-binds it to the moved node, reported as `Rekeyed`.
#[test]
fn moved_node_rebinds_through_its_hint() {
    use repo_graph_graph::cells::{CellTarget, QnameIndex};
    use repo_graph_graph::identity::{MoveTier, identity_of};

    let tmp = tempfile::tempdir().unwrap();
    // A body long enough to carry a hash (LB.6 hashes bodies of 40+ chars).
    let body = "def charge(order_id):\n    total = order_id * 2\n    return total + order_id\n";
    let dir = tmp.path().join("repo");
    std::fs::create_dir_all(dir.join("billing")).unwrap();
    std::fs::write(dir.join("a.py"), body).unwrap();
    let before = generate_one(dir.to_str().unwrap()).unwrap();
    let id = before
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == "a::charge")))
        .map(|n| n.id)
        .next()
        .expect("a::charge");
    let hint = identity_of(&before.merged, id).expect("a::charge has an identity").hint();

    // Move a.py under billing/: the qname becomes billing::a::charge.
    std::fs::rename(dir.join("a.py"), dir.join("billing/a.py")).unwrap();
    let row = format!(
        r#"{{"qname":"a::charge","kind":"FUNCTION","hint":{},"cell":"CONV","entry":{{"source":"api","id":"1","text":"moved"}}}}"#,
        serde_json::to_string(&hint).unwrap()
    );
    std::fs::create_dir_all(dir.join(".glia")).unwrap();
    std::fs::write(dir.join(".glia/cells.jsonl"), row).unwrap();
    let after = generate_one(dir.to_str().unwrap()).unwrap();
    let conv = of_type(&cells_of(&after, "billing::a::charge"), cell_type::CONV)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(conv, [CellPayload::Json(r#"[{"id":"1","source":"api","text":"moved"}]"#.into())]);

    // Without the hint the same row is an orphan.
    let idx = QnameIndex::build(&after.merged, None);
    assert_eq!(idx.resolve("a::charge", Some("FUNCTION"), None), CellTarget::Orphaned);
    assert!(matches!(
        idx.resolve("a::charge", Some("FUNCTION"), Some(&hint)),
        CellTarget::Rekeyed { tier: MoveTier::SameName, .. }
    ));
}
