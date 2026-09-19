//! LF.4a: `.glia/overlay.toml` `[[constraint]]`, `[[decision]]` and
//! `[[note]]` stanzas land as CONSTRAINT / DECISION / CONV entries on the node
//! each anchors on - an `anchor` qname, else the PROJECT node its scope names
//! - and a build with them is as byte-reproducible as one without.
//!
//! The layout and overlay file are the substrate-gap fixture
//! `overlay-declared-cells`, copied into a temp dir per test so a test can
//! edit them.

use std::path::Path;

use glia_code_domain::cell_type;
use glia_code_domain::external_inputs::{ConstraintKind, parse_constraints};
use glia_core::{CellPayload, CellTypeId};
use glia_engine::{
    BuildOptions, GenerateResult, ParseCache, generate_one, generate_one_opts, generate_one_with_cache,
};
use glia_store::write_merged_sharded;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/overlay-declared-cells";
const OVERLAY: &str = include_str!("../../bench/substrate-gap/fixtures/overlay-declared-cells/.glia/overlay.toml");
const FILES: &[&str] = &["services/api/pyproject.toml", "services/api/app.py", "web/package.json", "web/src/ui.ts"];

/// The fixture tree under `<tmp>/repo`, with `overlay` as its overlay file
/// (none when `None`) and `cells` as its `.glia/cells.jsonl`.
fn repo(tmp: &Path, overlay: Option<&str>, cells: Option<&str>) -> String {
    let dir = tmp.join("repo");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    for f in FILES {
        std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    std::fs::create_dir_all(dir.join(".glia")).unwrap();
    if let Some(o) = overlay {
        std::fs::write(dir.join(".glia/overlay.toml"), o).unwrap();
    }
    if let Some(c) = cells {
        std::fs::write(dir.join(".glia/cells.jsonl"), c).unwrap();
    }
    dir.to_str().unwrap().to_string()
}

/// The `t` payloads of the one node whose qname is `qname`, as strings.
fn cell(r: &GenerateResult, qname: &str, t: CellTypeId) -> Vec<String> {
    let nodes: Vec<_> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)))
        .collect();
    assert_eq!(nodes.len(), 1, "exactly one node is {qname}");
    nodes[0]
        .cells
        .iter()
        .filter(|c| c.kind == t)
        .map(|c| match &c.payload {
            CellPayload::Json(s) => s.clone(),
            other => panic!("{qname} cell {t:?} is JSON, got {other:?}"),
        })
        .collect()
}

/// Every payload of type `t` anywhere in the graph.
fn all_of(r: &GenerateResult, t: CellTypeId) -> Vec<CellPayload> {
    let cells = r.merged.graphs.iter().flat_map(|g| &g.nodes).flat_map(|n| &n.cells);
    cells.filter(|c| c.kind == t).map(|c| c.payload.clone()).collect()
}

#[test]
fn constraint_anchors_on_project() {
    let tmp = tempfile::tempdir().unwrap();
    let r = generate_one(&repo(tmp.path(), Some(OVERLAY), None)).unwrap();
    assert_eq!(
        cell(&r, "project:web", cell_type::CONSTRAINT),
        [r#"[{"categories":["CALLS"],"decl":".glia/overlay.toml:3","from":"web","from_raw":"web","id":"web-no-api-internals","kind":"forbid_edge","origin":"human","source":"overlay","text":"web goes through HTTP","to":"services/api","to_raw":"services/api"}]"#]
    );
    assert!(cell(&r, "project:services/api", cell_type::CONSTRAINT).is_empty(), "a forbid_edge hangs on its `from`");
    assert_eq!(all_of(&r, cell_type::CONSTRAINT).len(), 1);
}

#[test]
fn decision_and_note_anchor_on_qname() {
    let tmp = tempfile::tempdir().unwrap();
    let r = generate_one(&repo(tmp.path(), Some(OVERLAY), None)).unwrap();
    let charge = "services::api::app::charge";
    assert_eq!(
        cell(&r, charge, cell_type::DECISION),
        [r#"[{"decl":".glia/overlay.toml:12","id":"charge-idempotent","source":"overlay","status":"accepted","title":"charge() is idempotent on order id"}]"#]
    );
    assert_eq!(
        cell(&r, charge, cell_type::CONV),
        [r#"[{"decl":".glia/overlay.toml:18","id":"note#1","source":"overlay","text":"retries are safe"}]"#]
    );
}

/// `from` given as the web package's LABEL binds the same PROJECT and is
/// stored as its path, beside the raw label.
#[test]
fn label_scope_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = repo(tmp.path(), None, None);
    std::fs::write(Path::new(&dir).join("web/package.json"), "{\"name\": \"@shop/web\"}\n").unwrap();
    let overlay = "version = 1\n\n[[constraint]]\nid = \"c\"\nkind = \"forbid_edge\"\nfrom = \"@shop/web\"\nto = \"api\"\n";
    std::fs::write(Path::new(&dir).join(".glia/overlay.toml"), overlay).unwrap();
    let r = generate_one(&dir).unwrap();
    assert_eq!(
        cell(&r, "project:web", cell_type::CONSTRAINT),
        [r#"[{"decl":".glia/overlay.toml:3","from":"web","from_raw":"@shop/web","id":"c","kind":"forbid_edge","origin":"llm","source":"overlay","to":"services/api","to_raw":"api"}]"#]
    );
}

/// A scope no PROJECT sits at, an anchor no node carries, and an unscoped
/// rule in a repo without a root manifest are orphaned: no cell anywhere.
#[test]
fn orphan_scope_is_counted() {
    let tmp = tempfile::tempdir().unwrap();
    let overlay = "version = 1

[[constraint]]
id = \"nope-acyclic\"
kind = \"no_cycle\"
scope = \"nope\"

[[constraint]]
id = \"unscoped\"
kind = \"invariant\"
text = \"charges are idempotent\"

[[decision]]
id = \"d\"
scope = \"web/src\"
title = \"a path that is not a project\"

[[note]]
anchor = \"services::api::app::gone\"
text = \"stale anchor\"
";
    let r = generate_one(&repo(tmp.path(), Some(overlay), None)).unwrap();
    for t in [cell_type::CONSTRAINT, cell_type::DECISION, cell_type::CONV] {
        assert!(all_of(&r, t).is_empty(), "cell {t:?} was written: {:?}", all_of(&r, t));
    }
}

/// An unscoped rule anchors on the repo root's PROJECT when one exists.
#[test]
fn unscoped_rule_anchors_on_root_project() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = repo(tmp.path(), None, None);
    std::fs::write(Path::new(&dir).join("pyproject.toml"), "[project]\nname = \"shop\"\n").unwrap();
    let overlay = "version = 1\n\n[[constraint]]\nid = \"acyclic\"\nkind = \"no_cycle\"\n";
    std::fs::write(Path::new(&dir).join(".glia/overlay.toml"), overlay).unwrap();
    let r = generate_one(&dir).unwrap();
    assert_eq!(
        cell(&r, "project:.", cell_type::CONSTRAINT),
        [r#"[{"decl":".glia/overlay.toml:3","id":"acyclic","kind":"no_cycle","origin":"llm","source":"overlay"}]"#]
    );
}

/// What the stage stores is what the rule reader returns: one ForbidEdge with
/// the stanza's line as its `decl`.
#[test]
fn declared_constraints_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let r = generate_one(&repo(tmp.path(), Some(OVERLAY), None)).unwrap();
    let payloads = all_of(&r, cell_type::CONSTRAINT);
    assert_eq!(payloads.len(), 1);
    let rules = parse_constraints(&payloads[0]);
    assert_eq!(rules.len(), 1, "{rules:?}");
    let rule = &rules[0];
    assert_eq!(rule.id, "web-no-api-internals");
    assert_eq!(rule.kind, ConstraintKind::ForbidEdge { from: "web".into(), to: "services/api".into() });
    assert_eq!(rule.categories, ["CALLS"]);
    assert_eq!(rule.source, "overlay");
    assert_eq!(rule.decl.as_deref(), Some(".glia/overlay.toml:3"));
}

/// A sidecar DECISION row and an overlay decision on one node share one
/// array, sorted by `(source, id)`; neither overwrites the other.
#[test]
fn api_and_overlay_entries_coexist() {
    let tmp = tempfile::tempdir().unwrap();
    let row = r#"{"qname":"services::api::app::charge","cell":"DECISION","entry":{"source":"api","id":"zz","title":"from the API"}}"#;
    let r = generate_one(&repo(tmp.path(), Some(OVERLAY), Some(row))).unwrap();
    assert_eq!(
        cell(&r, "services::api::app::charge", cell_type::DECISION),
        [r#"[{"id":"zz","source":"api","title":"from the API"},{"decl":".glia/overlay.toml:12","id":"charge-idempotent","source":"overlay","status":"accepted","title":"charge() is idempotent on order id"}]"#]
    );
}

/// Declared knowledge is a rule, not an inferred edge: `--no-overlay` keeps it.
#[test]
fn no_overlay_keeps_declared_knowledge() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = repo(tmp.path(), Some(OVERLAY), None);
    let on = generate_one(&dir).unwrap();
    let off = generate_one_opts(&dir, false, &BuildOptions::default().with_overlay(false)).unwrap();
    for t in [cell_type::CONSTRAINT, cell_type::DECISION, cell_type::CONV] {
        assert_eq!(all_of(&off, t).len(), 1, "cell {t:?} survives --no-overlay");
        assert_eq!(all_of(&off, t), all_of(&on, t));
    }
}

/// The stage writes node cells and nothing else: every edge and every other
/// node is what the same tree without the overlay file gives.
#[test]
fn declared_changes_only_its_node_cells() {
    let (with_tmp, bare_tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let with = generate_one(&repo(with_tmp.path(), Some(OVERLAY), None)).unwrap();
    let bare = generate_one(&repo(bare_tmp.path(), None, None)).unwrap();
    assert_eq!(with.merged.cross_edges, bare.merged.cross_edges);
    assert_eq!(with.merged.graphs.len(), bare.merged.graphs.len());
    let mut changed = 0;
    for (a, b) in with.merged.graphs.iter().zip(&bare.merged.graphs) {
        assert_eq!(a.edges, b.edges);
        assert_eq!(a.nodes.len(), b.nodes.len());
        for (na, nb) in a.nodes.iter().zip(&b.nodes) {
            assert_eq!(na.id, nb.id);
            if na.cells != nb.cells {
                changed += 1;
                assert_eq!(na.cells[..nb.cells.len()], nb.cells[..], "declared cells are appended");
            }
        }
    }
    assert_eq!(changed, 2, "project:web and services::api::app::charge");
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
fn declared_builds_are_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let row = r#"{"qname":"services::api::app::charge","cell":"DECISION","entry":{"source":"api","id":"zz","title":"from the API"}}"#;
    let repo_s = repo(tmp.path(), Some(OVERLAY), Some(row));

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
    assert_eq!(cell(&warm, "services::api::app::charge", cell_type::DECISION).len(), 1);
}
