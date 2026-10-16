//! LF.4a: `.glia/overlay.toml` `[[constraint]]`, `[[decision]]` and
//! `[[note]]` stanzas land as CONSTRAINT / DECISION / CONV entries on the node
//! each anchors on - an `anchor` qname, else the PROJECT node its scope names
//! (for a constraint, DT.7a: else the deepest PROJECT above its scope, else
//! the smallest-id MODULE under it) - and a build with them is as
//! byte-reproducible as one without.
//!
//! The layout and overlay file are the substrate-gap fixture
//! `overlay-declared-cells`, copied into a temp dir per test so a test can
//! edit them.

use std::path::Path;
use std::process::Command;

use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule, parse_constraints};
use glia_code_domain::{cell_type, glia_config, node_kind};
use glia_core::{CellPayload, CellTypeId, NodeId};
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

const CHILD_ENV: &str = "GLIA_DT7A_CHILD_BUILD";

/// The child half of [`declared_lines`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds that repo so its stderr (with
/// `--nocapture`) carries the `[declared]` markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        generate_one(&dir).expect("build");
    }
}

/// The `[declared]` stderr lines of a build of `dir`, from a child run of
/// this test binary.
fn declared_lines(dir: &str) -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_build_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, dir)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "child build failed: {stderr}");
    stderr.lines().filter(|l| l.starts_with("[declared]")).map(String::from).collect()
}

/// The `file` of a node's first POSITION cell.
fn position_file(cells: &[glia_core::Cell]) -> Option<String> {
    cells.iter().filter(|c| c.kind == cell_type::POSITION).find_map(|c| match &c.payload {
        CellPayload::Json(s) | CellPayload::Text(s) => {
            serde_json::from_str::<serde_json::Value>(s).ok()?.get("file")?.as_str().map(String::from)
        }
        CellPayload::Bytes(_) => None,
    })
}

/// The smallest-id MODULE of the build whose POSITION file lies under `dir`
/// (`.` = any), as the CC.5a component fallback picks it, and its qname.
fn smallest_module_under(r: &GenerateResult, dir: &str) -> (NodeId, String) {
    let under = |f: &str| dir == "." || f == dir || f.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'));
    r.merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE))
                .filter(|n| position_file(&n.cells).is_some_and(|f| under(&f)))
                .map(|n| (n.id, g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default()))
        })
        .min_by_key(|(id, _)| id.0)
        .expect("a MODULE under the path")
}

/// DT.7a: a constraint whose scope lies inside a project (here a file of
/// it) hangs on the deepest PROJECT above it. Its entry is the one a
/// project-scoped stanza stores but for `from` / `from_raw`, `glia check`
/// evaluates it, and the marker counts it as `anchor_enclosing`; the
/// project-scoped stanza's marker keeps HEAD's shape. Pre-fix baseline (HEAD
/// a584c98a): no CONSTRAINT cell, `orphaned=1`, `scope=services/api/app.py
/// -> services/api/app.py (no PROJECT)`.
#[test]
fn subdir_scope_anchors_on_the_enclosing_project() {
    let stanza = |from: &str| {
        format!("version = 1\n\n[[constraint]]\nid = \"api-no-web\"\nkind = \"forbid_edge\"\nfrom = \"{from}\"\nto = \"web\"\ncategories = [\"CALLS\"]\n")
    };
    let (sub_tmp, proj_tmp) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let sub_dir = repo(sub_tmp.path(), Some(&stanza("services/api/app.py")), None);
    let proj_dir = repo(proj_tmp.path(), Some(&stanza("services/api")), None);
    let sub = generate_one(&sub_dir).unwrap();
    let proj = generate_one(&proj_dir).unwrap();
    let at_project = cell(&proj, "project:services/api", cell_type::CONSTRAINT);
    assert_eq!(
        at_project,
        [r#"[{"categories":["CALLS"],"decl":".glia/overlay.toml:3","from":"services/api","from_raw":"services/api","id":"api-no-web","kind":"forbid_edge","origin":"llm","source":"overlay","to":"web","to_raw":"web"}]"#]
    );
    assert_eq!(
        cell(&sub, "project:services/api", cell_type::CONSTRAINT),
        [at_project[0].replace(
            r#""from":"services/api","from_raw":"services/api""#,
            r#""from":"services/api/app.py","from_raw":"services/api/app.py""#
        )]
    );
    assert_eq!(all_of(&sub, cell_type::CONSTRAINT).len(), 1);
    let report = glia_engine::check::check(&sub.merged);
    assert_eq!((report.rules, report.checked), (1, 1), "{report:#?}");
    assert!(report.errors.is_empty() && report.unchecked.is_empty(), "{report:#?}");

    assert_eq!(
        declared_lines(&sub_dir),
        [
            format!("[declared] repo={sub_dir} constraint=1 decision=0 note=0 anchored=1 orphaned=0 (anchor_qname=0 anchor_project=0 anchor_enclosing=1)"),
            "[declared] rules=1 (forbid_edge=1 no_cycle=0 invariant=0)".to_string(),
        ]
    );
    assert_eq!(
        declared_lines(&proj_dir),
        [
            format!("[declared] repo={proj_dir} constraint=1 decision=0 note=0 anchored=1 orphaned=0 (anchor_qname=0 anchor_project=1)"),
            "[declared] rules=1 (forbid_edge=1 no_cycle=0 invariant=0)".to_string(),
        ]
    );
}

/// DT.7a: an unscoped rule in a repo with no root manifest (no PROJECT at
/// or above `.`) hangs on the smallest-id MODULE of the repo, as a
/// component does (CC.5a), and `glia check` reads it. Pre-fix baseline (HEAD
/// a584c98a): no CONSTRAINT cell, `scope= -> . (no PROJECT)`.
#[test]
fn unscoped_rule_without_root_manifest_anchors_on_a_module() {
    let tmp = tempfile::tempdir().unwrap();
    let overlay = "version = 1\n\n[[constraint]]\nid = \"acyclic\"\nkind = \"no_cycle\"\n";
    let dir = repo(tmp.path(), Some(overlay), None);
    let r = generate_one(&dir).unwrap();
    let (module, qname) = smallest_module_under(&r, ".");
    assert_eq!(
        cell(&r, &qname, cell_type::CONSTRAINT),
        [r#"[{"decl":".glia/overlay.toml:3","id":"acyclic","kind":"no_cycle","origin":"llm","source":"overlay"}]"#]
    );
    let holders: Vec<NodeId> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| n.cells.iter().any(|c| c.kind == cell_type::CONSTRAINT))
        .map(|n| n.id)
        .collect();
    assert!(!holders.is_empty() && holders.iter().all(|id| *id == module), "{holders:?} vs {module:?}");
    let report = glia_engine::check::check(&r.merged);
    assert_eq!((report.rules, report.checked), (1, 1), "{report:#?}");
    assert_eq!(
        declared_lines(&dir),
        [
            format!("[declared] repo={dir} constraint=1 decision=0 note=0 anchored=1 orphaned=0 (anchor_qname=0 anchor_project=0 anchor_module=1)"),
            "[declared] rules=1 (forbid_edge=0 no_cycle=1 invariant=0)".to_string(),
        ]
    );
}

/// A constraint scope with no PROJECT at or above it and no MODULE under it,
/// a decision scope no PROJECT sits at, and an anchor no node carries are
/// orphaned; the unscoped rule in this repo without a root manifest is not
/// (DT.7a: it hangs on the smallest-id MODULE, and is the one cell written).
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
    let dir = repo(tmp.path(), Some(overlay), None);
    let r = generate_one(&dir).unwrap();
    for t in [cell_type::DECISION, cell_type::CONV] {
        assert!(all_of(&r, t).is_empty(), "cell {t:?} was written: {:?}", all_of(&r, t));
    }
    let (_, module) = smallest_module_under(&r, ".");
    assert_eq!(
        cell(&r, &module, cell_type::CONSTRAINT),
        [r#"[{"decl":".glia/overlay.toml:8","id":"unscoped","kind":"invariant","origin":"llm","source":"overlay","text":"charges are idempotent"}]"#]
    );
    assert_eq!(all_of(&r, cell_type::CONSTRAINT).len(), 1, "nope-acyclic stays orphaned");
    assert_eq!(
        declared_lines(&dir),
        [
            format!("[declared] repo={dir} constraint=2 decision=1 note=1 anchored=1 orphaned=3 (anchor_qname=0 anchor_project=0 anchor_module=1)"),
            "[declared] orphaned constraint id=nope-acyclic .glia/overlay.toml:3 scope=nope -> nope (no PROJECT at or above it, no MODULE under it)".to_string(),
            "[declared] orphaned decision id=d .glia/overlay.toml:13 scope=web/src -> web/src (no PROJECT)".to_string(),
            "[declared] orphaned note id=note#1 .glia/overlay.toml:18 anchor=services::api::app::gone".to_string(),
            "[declared] rules=1 (forbid_edge=0 no_cycle=0 invariant=1)".to_string(),
        ]
    );
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
    assert_eq!(
        rule.kind,
        ConstraintKind::ForbidEdge { from: "web".into(), to: "services/api".into(), transitive: false, via: vec![] }
    );
    assert_eq!(rule.categories, ["CALLS"]);
    assert_eq!(rule.source, "overlay");
    assert_eq!(rule.decl.as_deref(), Some(".glia/overlay.toml:3"));
    assert_eq!(rule.min_tier, None);
}

/// DB.1: a version-2 overlay's rule fields are stored on the entry - only the
/// keys a stanza sets, the facades resolved beside their raw text - and read
/// back into the new variant fields. The same file at version 1 drops both
/// stanzas and writes no CONSTRAINT cell.
#[test]
fn v2_constraint_entries_round_trip() {
    let overlay = r#"version = 2

[[constraint]]
id = "no-api"
kind = "forbid_edge"
from = "web"
to = "services/api"
transitive = true
via = ["services/api/public"]

[[constraint]]
id = "no-routes"
kind = "invariant"
scope = "services/api"
text = "the api declares no route"
rule = "violation(x) :- node(x, \"ROUTE\")."
min_tier = "fact"
origin = "human"
"#;
    let loaded = glia_config::parse_str(overlay);
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    let tmp = tempfile::tempdir().unwrap();
    let r = generate_one(&repo(tmp.path(), Some(overlay), None)).unwrap();
    assert_eq!(
        cell(&r, "project:web", cell_type::CONSTRAINT),
        [r#"[{"decl":".glia/overlay.toml:3","from":"web","from_raw":"web","id":"no-api","kind":"forbid_edge","origin":"llm","source":"overlay","to":"services/api","to_raw":"services/api","transitive":true,"via":["services/api/public"],"via_raw":["services/api/public"]}]"#]
    );
    assert_eq!(
        cell(&r, "project:services/api", cell_type::CONSTRAINT),
        [r#"[{"decl":".glia/overlay.toml:11","id":"no-routes","kind":"invariant","min_tier":"fact","origin":"human","rule":"violation(x) :- node(x, \"ROUTE\").","scope":"services/api","scope_raw":"services/api","source":"overlay","text":"the api declares no route"}]"#]
    );
    let rules: Vec<ConstraintRule> = all_of(&r, cell_type::CONSTRAINT).iter().flat_map(parse_constraints).collect();
    let by_id = |id: &str| rules.iter().find(|x| x.id == id).cloned().unwrap();
    let forbid = by_id("no-api");
    assert_eq!(
        forbid.kind,
        ConstraintKind::ForbidEdge {
            from: "web".into(),
            to: "services/api".into(),
            transitive: true,
            via: vec!["services/api/public".into()],
        }
    );
    assert_eq!(forbid.min_tier, None);
    let inv = by_id("no-routes");
    assert_eq!(
        inv.kind,
        ConstraintKind::Invariant {
            text: "the api declares no route".into(),
            rule: Some("violation(x) :- node(x, \"ROUTE\").".into()),
        }
    );
    assert_eq!(inv.min_tier.as_deref(), Some("fact"));

    // The same stanzas in a version-1 file: each is dropped, nothing stored.
    let v1 = overlay.replacen("version = 2", "version = 1", 1);
    let loaded = glia_config::parse_str(&v1);
    assert_eq!(loaded.errors.len(), 2, "{:?}", loaded.errors);
    assert!(loaded.errors.iter().all(|e| e.contains("need `version = 2`")), "{:?}", loaded.errors);
    let tmp = tempfile::tempdir().unwrap();
    let r = generate_one(&repo(tmp.path(), Some(&v1), None)).unwrap();
    assert!(all_of(&r, cell_type::CONSTRAINT).is_empty());
}

/// Q9: a `.glia/cells.jsonl` CONSTRAINT row carrying `rule` is rejected by
/// the build's sidecar stage whatever its `source` says (HEAD stored it).
#[test]
fn sidecar_rule_is_rejected() {
    for source in ["api", "overlay"] {
        let row = format!(
            r#"{{"qname":"project:web","cell":"CONSTRAINT","entry":{{"source":"{source}","id":"r","kind":"invariant","text":"t","rule":"x"}}}}"#
        );
        let tmp = tempfile::tempdir().unwrap();
        let r = generate_one(&repo(tmp.path(), None, Some(&row))).unwrap();
        assert!(all_of(&r, cell_type::CONSTRAINT).is_empty(), "source {source}: the rule row is not applied");
    }
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

/// CC.5a: the reflexion model's stanzas are stored as CONSTRAINT entries.
/// With no root manifest, each component hangs on the PROJECT at its first
/// path, and each layer / allow on its first component's anchor.
#[test]
fn reflexion_model_is_stored_and_anchored() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("tier");
    let overlay = "version = 1

[[component]]
name = \"web\"
paths = [\"web\"]
text = \"the browser app\"

[[component]]
name = \"api\"
paths = [\"services/api\"]
origin = \"human\"

[[layer]]
name = \"ui\"
components = [\"web\"]

[[layer]]
name = \"core\"
components = [\"api\"]

[[constraint]]
id = \"web-uses-api\"
kind = \"allow\"
from = \"web\"
to = \"api\"
";
    for (path, body) in [
        ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
        ("web/ui.py", "def render():\n    return 1\n"),
        ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
        ("services/api/app.py", "def charge(order_id):\n    return order_id\n"),
        (".glia/overlay.toml", overlay),
    ] {
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), body).unwrap();
    }
    let loaded = glia_config::parse_str(overlay);
    assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
    let r = generate_one(dir.to_str().unwrap()).unwrap();

    // What `declared_constraints` reads: every CONSTRAINT node once, in
    // NodeId order, a node's rules in stored `(source, id)` order.
    let mut nodes: Vec<(NodeId, String, CellPayload)> = Vec::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            if let Some(c) = n.cells.iter().find(|c| c.kind == cell_type::CONSTRAINT)
                && !nodes.iter().any(|(id, _, _)| *id == n.id)
            {
                let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                nodes.push((n.id, qname, c.payload.clone()));
            }
        }
    }
    nodes.sort_by_key(|(id, _, _)| id.0);
    let rules: Vec<(String, ConstraintRule)> = nodes
        .iter()
        .flat_map(|(_, q, p)| parse_constraints(p).into_iter().map(move |rule| (q.clone(), rule)))
        .collect();
    assert_eq!(rules.len(), 5, "{rules:#?}");
    let mut ids: Vec<&str> = rules.iter().map(|(_, x)| x.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["component:api", "component:web", "layer:core", "layer:ui", "web-uses-api"]);
    let mut kinds: Vec<&str> = rules.iter().map(|(_, x)| x.kind.name()).collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["allow", "component", "component", "layer", "layer"]);
    let anchor = |id: &str| rules.iter().find(|(_, x)| x.id == id).map(|(q, _)| q.as_str()).unwrap();
    assert_eq!(anchor("component:web"), "project:web");
    assert_eq!(anchor("layer:ui"), "project:web", "a layer follows its first component");
    assert_eq!(anchor("web-uses-api"), "project:web", "an allow follows its `from`");
    assert_eq!(anchor("component:api"), "project:services/api");
    assert_eq!(anchor("layer:core"), "project:services/api");
    // One node's rules keep their stored (source, id) order.
    let on_web: Vec<&str> =
        rules.iter().filter(|(q, _)| q == "project:web").map(|(_, x)| x.id.as_str()).collect();
    assert_eq!(on_web, ["component:web", "layer:ui", "web-uses-api"]);
    let kind = |id: &str| rules.iter().find(|(_, x)| x.id == id).map(|(_, x)| x.kind.clone()).unwrap();
    assert_eq!(kind("component:api"), ConstraintKind::Component { name: "api".into(), paths: vec!["services/api".into()] });
    assert_eq!(
        kind("layer:core"),
        ConstraintKind::Layer { name: "core".into(), rank: 1, components: vec!["api".into()], strict: false }
    );
    assert_eq!(kind("web-uses-api"), ConstraintKind::Allow { from: "web".into(), to: "api".into() });

    assert_eq!(
        cell(&r, "project:web", cell_type::CONSTRAINT),
        [concat!(
            r#"[{"decl":".glia/overlay.toml:3","id":"component:web","kind":"component","name":"web","origin":"llm","paths":["web"],"paths_raw":["web"],"source":"overlay","text":"the browser app"},"#,
            r#"{"components":["web"],"decl":".glia/overlay.toml:13","id":"layer:ui","kind":"layer","name":"ui","rank":0,"source":"overlay","strict":false},"#,
            r#"{"decl":".glia/overlay.toml:21","from":"web","id":"web-uses-api","kind":"allow","origin":"llm","source":"overlay","to":"api"}]"#,
        )]
    );
    assert_eq!(
        cell(&r, "project:services/api", cell_type::CONSTRAINT),
        [concat!(
            r#"[{"decl":".glia/overlay.toml:8","id":"component:api","kind":"component","name":"api","origin":"human","paths":["services/api"],"paths_raw":["services/api"],"source":"overlay"},"#,
            r#"{"components":["api"],"decl":".glia/overlay.toml:17","id":"layer:core","kind":"layer","name":"core","rank":1,"source":"overlay","strict":false}]"#,
        )]
    );

    // A component is declared only through [[component]]: the same component
    // written as a [[constraint]] is a load error on its own line.
    let as_constraint = glia_config::parse_str(
        "version = 1\n\n[[component]]\nname = \"web\"\npaths = [\"web\"]\n\n[[constraint]]\nid = \"web\"\nkind = \"component\"\n",
    );
    assert_eq!(as_constraint.errors.len(), 1, "{:?}", as_constraint.errors);
    assert!(
        as_constraint.errors[0].starts_with(".glia/overlay.toml:7: [[constraint]]")
            && as_constraint.errors[0].contains("declared through [[component]]"),
        "{}",
        as_constraint.errors[0]
    );
    assert_eq!(as_constraint.config.component.len(), 1);
    assert!(as_constraint.config.constraint.is_empty());
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
