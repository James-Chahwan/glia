//! CC.5b — `check` v2 evaluates the reflexion model CC.5a stores as
//! CONSTRAINT entries (`[[component]]`, `[[layer]]`, `kind = "allow"`): the
//! component dependency matrix, convergences, divergences (VIOLATIONs with
//! located, tiered evidence), absences (FACT + coverage caveats) and the
//! files no component owns.
//!
//! One python tree with three manifests (web/, services/api/, store/):
//! web/app.py imports and calls services.api.handlers.get_order;
//! services/api/handlers.py imports and calls store.db.load; web/admin.py
//! imports store.db and calls load (the layering violation); store/db.py
//! defines load; scripts/tool.py, which no component owns, imports store.db.

use glia_code_domain::cell_type;
use glia_core::{Cell, CellPayload};
use glia_engine::check::{CheckReport, Reflexion, check};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const WEB_APP: &str =
    "from services.api.handlers import get_order\n\n\ndef show(o):\n    return get_order(o)\n";
const HANDLERS: &str = "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n";
const WEB_ADMIN: &str = "from store.db import load\n\n\ndef audit(o):\n    return load(o)\n";
const DB: &str = "def load(o):\n    return o\n";
const TOOL: &str = "from store.db import load\n";
/// A file under `web/admin/` (a directory), for the longest-path test.
const PANEL: &str = "from store.db import load\n\n\ndef panel(o):\n    return load(o)\n";

const MANIFESTS: [(&str, &str); 3] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("store/pyproject.toml", "[project]\nname = \"store\"\n"),
];

const SOURCES: [(&str, &str); 5] = [
    ("web/app.py", WEB_APP),
    ("services/api/handlers.py", HANDLERS),
    ("web/admin.py", WEB_ADMIN),
    ("store/db.py", DB),
    ("scripts/tool.py", TOOL),
];

/// Three components: web (line 3), api (line 7), store (line 11).
const COMPONENTS: &str = "version = 1

[[component]]
name = \"web\"
paths = [\"web\"]

[[component]]
name = \"api\"
paths = [\"services/api\"]

[[component]]
name = \"store\"
paths = [\"store\"]
";

/// ui (strict) over core over data: web may use api only, api may use store.
const LAYERS: &str = "
[[layer]]
name = \"ui\"
components = [\"web\"]
strict = true

[[layer]]
name = \"core\"
components = [\"api\"]

[[layer]]
name = \"data\"
components = [\"store\"]
";

/// The explicit exception that lets web reach the store.
const ALLOW_WEB_STORE: &str = "
[[constraint]]
id = \"web-uses-store-cache\"
kind = \"allow\"
from = \"web\"
to = \"store\"
";

/// Build the fixture tree plus `.glia/overlay.toml` = `overlay` and any
/// `extra` files; the tempdir is returned so it outlives the graph's use.
fn build(overlay: &str, extra: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let files = MANIFESTS
        .iter()
        .chain(SOURCES.iter())
        .chain(extra)
        .copied()
        .chain(std::iter::once((".glia/overlay.toml", overlay)));
    for (rel, src) in files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged)
}

fn model(r: &CheckReport) -> &Reflexion {
    r.reflexion.as_ref().expect("a model is declared")
}

/// `(from, to, status, allowed_by)` of every matrix cell, in report order.
fn matrix(r: &Reflexion) -> Vec<(&str, &str, &str, Option<&str>)> {
    r.matrix
        .iter()
        .map(|c| {
            (
                c.from.as_str(),
                c.to.as_str(),
                c.status,
                c.allowed_by.as_deref(),
            )
        })
        .collect()
}

#[test]
fn closed_layers_report_convergence_divergence_absence() {
    let overlay = format!("{COMPONENTS}{LAYERS}{ALLOW_WEB_STORE}");
    let (_tmp, merged) = build(&overlay, &[]);
    let report = check(&merged);
    // Components and layers are model declarations, not rules; the allow is a
    // rule, checked because the model is evaluated.
    assert_eq!((report.rules, report.checked), (1, 1), "{report:#?}");
    assert!(report.unchecked.is_empty(), "{report:#?}");
    assert!(report.errors.is_empty(), "{report:#?}");
    assert!(report.violations.is_empty(), "{report:#?}");
    let m = model(&report);
    assert!(m.closed, "{m:#?}");
    assert_eq!(
        matrix(m),
        [
            ("api", "store", "convergence", Some("layer:core>data")),
            ("web", "api", "convergence", Some("layer:ui>core")),
            ("web", "store", "convergence", Some("web-uses-store-cache")),
        ],
        "{m:#?}"
    );
    assert_eq!((m.convergences, m.divergences), (3, 0), "{m:#?}");
    assert!(m.absences.is_empty(), "{m:#?}");
    for c in &m.matrix {
        assert_eq!(c.edges, 2, "one IMPORTS + one CALLS per pair: {c:#?}");
        assert_eq!(c.tier, "fact", "{c:#?}");
    }
    let names: Vec<(&str, Option<&str>, Option<&str>)> = m
        .components
        .iter()
        .map(|c| (c.name.as_str(), c.layer.as_deref(), c.decl.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("api", Some("core"), Some(".glia/overlay.toml:7")),
            ("store", Some("data"), Some(".glia/overlay.toml:11")),
            ("web", Some("ui"), Some(".glia/overlay.toml:3")),
        ]
    );

    // Drop the allow: web -> store skips a layer below a strict one.
    let overlay = format!("{COMPONENTS}{LAYERS}");
    let (_tmp, merged) = build(&overlay, &[]);
    let report = check(&merged);
    assert_eq!((report.rules, report.checked), (0, 0), "{report:#?}");
    assert!(report.errors.is_empty(), "{report:#?}");
    let m = model(&report);
    assert!(m.closed);
    assert_eq!(
        matrix(m),
        [
            ("api", "store", "convergence", Some("layer:core>data")),
            ("web", "api", "convergence", Some("layer:ui>core")),
            ("web", "store", "divergence", None),
        ],
        "{m:#?}"
    );
    assert_eq!((m.convergences, m.divergences), (2, 1), "{m:#?}");
    assert_eq!(report.violations.len(), 1, "{report:#?}");
    let v = &report.violations[0];
    assert_eq!(v.rule_id, "reflexion:web->store");
    assert_eq!(v.rule_kind, "divergence");
    assert_eq!(v.severity, "VIOLATION");
    assert_eq!(v.tier, "fact");
    assert_eq!(
        v.decl.as_deref(),
        Some(".glia/overlay.toml:3"),
        "web's component decl"
    );
    assert_eq!(v.count, 2, "{v:#?}");
    let sites: Vec<(&str, Option<&str>, Option<i64>, &str)> = v
        .evidence
        .iter()
        .map(|e| (e.category, e.file.as_deref(), e.line, e.tier))
        .collect();
    assert_eq!(
        sites,
        [
            ("IMPORTS", Some("web/admin.py"), Some(1), "fact"),
            ("CALLS", Some("web/admin.py"), Some(5), "fact"),
        ],
        "{v:#?}"
    );
    assert_eq!(v.evidence[1].from_qname, "web::admin::audit");
    assert_eq!(v.evidence[1].to_qname, "store::db::load");
}

#[test]
fn allow_without_dependency_is_an_absence() {
    let overlay = "version = 1

[[component]]
name = \"web\"
paths = [\"web\"]

[[component]]
name = \"api\"
paths = [\"services/api\"]

[[constraint]]
id = \"web-uses-api\"
kind = \"allow\"
from = \"web\"
to = \"api\"

[[constraint]]
id = \"api-uses-web\"
kind = \"allow\"
from = \"api\"
to = \"web\"
";
    let (_tmp, merged) = build(overlay, &[]);
    let report = check(&merged);
    assert_eq!((report.rules, report.checked), (2, 2), "{report:#?}");
    assert!(report.errors.is_empty(), "{report:#?}");
    assert!(
        report.violations.is_empty(),
        "an absence is never a violation: {report:#?}"
    );
    let m = model(&report);
    assert!(m.closed);
    assert_eq!(
        matrix(m),
        [("web", "api", "convergence", Some("web-uses-api"))],
        "{m:#?}"
    );
    let absences: Vec<(&str, &str, &str, &str)> = m
        .absences
        .iter()
        .map(|a| (a.from.as_str(), a.to.as_str(), a.rule_id.as_str(), a.tier))
        .collect();
    assert_eq!(absences, [("api", "web", "api-uses-web", "fact")], "{m:#?}");
    let a = &m.absences[0];
    assert_eq!(a.decl.as_deref(), Some(".glia/overlay.toml:17"), "{a:#?}");
    assert!(
        !a.caveats.is_empty(),
        "a blind extraction looks like an absence: {a:#?}"
    );
    assert!(
        a.caveats.iter().any(|c| c.edge_category == "CALLS"),
        "the default checked set holds CALLS: {:#?}",
        a.caveats
    );
}

#[test]
fn open_model_only_observes() {
    let (_tmp, merged) = build(COMPONENTS, &[]);
    let report = check(&merged);
    assert_eq!((report.rules, report.checked), (0, 0), "{report:#?}");
    assert!(
        report.unchecked.is_empty() && report.errors.is_empty(),
        "{report:#?}"
    );
    assert!(
        report.violations.is_empty(),
        "an open model never fails CI: {report:#?}"
    );
    let m = model(&report);
    assert!(!m.closed);
    assert_eq!(
        matrix(m),
        [
            ("api", "store", "observed", None),
            ("web", "api", "observed", None),
            ("web", "store", "observed", None),
        ],
        "{m:#?}"
    );
    assert_eq!((m.convergences, m.divergences), (0, 0));
    assert!(m.absences.is_empty());
}

#[test]
fn longest_path_wins() {
    let overlay = format!(
        "{COMPONENTS}
[[component]]
name = \"admin\"
paths = [\"web/admin\"]
"
    );
    let (_tmp, merged) = build(&overlay, &[("web/admin/panel.py", PANEL)]);
    let report = check(&merged);
    assert!(report.errors.is_empty(), "{report:#?}");
    let m = model(&report);
    // web/admin/panel.py sits under both `web` and `web/admin`: the longer
    // path owns it. web/admin.py is NOT under `web/admin` (path-segment
    // boundary), so it stays web's.
    assert_eq!(
        matrix(m),
        [
            ("admin", "store", "observed", None),
            ("api", "store", "observed", None),
            ("web", "api", "observed", None),
            ("web", "store", "observed", None),
        ],
        "{m:#?}"
    );
    let cell = |from: &str, to: &str| {
        m.matrix
            .iter()
            .find(|c| c.from == from && c.to == to)
            .map(|c| c.edges)
    };
    assert_eq!(
        cell("admin", "store"),
        Some(2),
        "panel.py's import and call"
    );
    assert_eq!(cell("web", "store"), Some(2), "admin.py's import and call");
    let admin = m
        .components
        .iter()
        .find(|c| c.name == "admin")
        .expect("admin");
    assert_eq!(admin.paths, ["web/admin"]);
    assert_eq!(
        admin.nodes, 2,
        "the panel module and its function: {admin:#?}"
    );
}

#[test]
fn unmapped_counts_files() {
    let (_tmp, merged) = build(COMPONENTS, &[]);
    let report = check(&merged);
    let u = &model(&report).unmapped;
    assert_eq!(u.files, 1, "{u:#?}");
    assert_eq!(u.nodes, 1, "the scripts::tool module: {u:#?}");
    assert_eq!(u.edges_to_mapped, 1, "tool.py's import of store.db: {u:#?}");
    assert_eq!(u.sample, ["scripts/tool.py"]);
}

#[test]
fn no_model_is_none() {
    // The LE.8 shape: a forbid_edge rule, no component.
    let rules = "version = 1

[[constraint]]
id = \"web-no-store\"
kind = \"forbid_edge\"
from = \"web\"
to = \"store\"
categories = [\"IMPORTS\", \"CALLS\"]
";
    let (_tmp, merged) = build(rules, &[]);
    let report = check(&merged);
    assert!(report.reflexion.is_none(), "{report:#?}");
    assert_eq!((report.rules, report.checked), (1, 1), "{report:#?}");
    assert_eq!(report.violations.len(), 1, "{report:#?}");
    assert_eq!(report.violations[0].rule_kind, "forbid_edge");
    assert_eq!(report.violations[0].count, 2);
    let json = serde_json::to_value(&report).expect("serialises");
    assert!(json["reflexion"].is_null(), "{json}");
    let keys: Vec<&str> = json
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert!(keys.contains(&"reflexion"), "{keys:?}");
}

#[test]
fn deterministic() {
    let overlay = format!("{COMPONENTS}{LAYERS}");
    let (_a, first) = build(&overlay, &[]);
    let (_b, second) = build(&overlay, &[]);
    let one = serde_json::to_string(&check(&first)).expect("serialises");
    let two = serde_json::to_string(&check(&second)).expect("serialises");
    assert_eq!(one, two);
    assert!(one.contains("\"reflexion\":{\"closed\":true"), "{one}");
}

/// Add a cell-API CONSTRAINT entry to the `project:web` node, as `glia cell
/// set` would: appended to the entry array the checker reads (the first
/// copy of the node, in graph order, that carries a CONSTRAINT cell), else
/// as a new cell on the node's first copy.
fn add_api_entry(merged: &mut MergedGraph, entry: &str) {
    let copies: Vec<(usize, usize)> = merged
        .graphs
        .iter()
        .enumerate()
        .flat_map(|(gi, g)| {
            g.nodes
                .iter()
                .enumerate()
                .filter(move |(_, n)| {
                    g.nav.qname_by_id.get(&n.id).map(String::as_str) == Some("project:web")
                })
                .map(move |(ni, _)| (gi, ni))
        })
        .collect();
    let has_rules = |&(gi, ni): &(usize, usize)| {
        merged.graphs[gi].nodes[ni]
            .cells
            .iter()
            .any(|c| c.kind == cell_type::CONSTRAINT)
    };
    let (gi, ni) = copies
        .iter()
        .copied()
        .find(has_rules)
        .or_else(|| copies.first().copied())
        .expect("a project:web node");
    let n = &mut merged.graphs[gi].nodes[ni];
    match n.cells.iter_mut().find(|c| c.kind == cell_type::CONSTRAINT) {
        Some(cell) => {
            let CellPayload::Json(s) = &cell.payload else {
                panic!("CONSTRAINT payload is JSON");
            };
            let mut arr: Vec<serde_json::Value> = serde_json::from_str(s).expect("an array");
            arr.push(serde_json::from_str(entry).expect("an entry"));
            cell.payload = CellPayload::Json(serde_json::to_string(&arr).expect("serialises"));
        }
        None => n.cells.push(Cell {
            kind: cell_type::CONSTRAINT,
            payload: CellPayload::Json(format!("[{entry}]")),
        }),
    }
}

#[test]
fn allow_naming_an_undeclared_component_is_a_rule_error() {
    // The loader refuses this in an overlay; an API write (or a component
    // rejected at build) can still leave the name dangling.
    let (_tmp, mut merged) = build(&format!("{COMPONENTS}{LAYERS}"), &[]);
    add_api_entry(
        &mut merged,
        r#"{"source":"api","id":"web-uses-ghost","kind":"allow","from":"web","to":"ghost"}"#,
    );
    let report = check(&merged);
    assert_eq!((report.rules, report.checked), (1, 0), "{report:#?}");
    assert_eq!(report.errors.len(), 1, "{report:#?}");
    assert_eq!(report.errors[0].0, "web-uses-ghost");
    assert!(report.errors[0].1.contains("`ghost`"), "{report:#?}");
    // The rest of the model is still evaluated.
    assert_eq!(model(&report).divergences, 1, "{report:#?}");
}

#[test]
fn api_component_on_a_claimed_path_loses_by_name() {
    // `zweb` claims web's path through the API: the smaller name keeps it and
    // the loser is reported, never a silent coin flip.
    let (_tmp, mut merged) = build(COMPONENTS, &[]);
    add_api_entry(
        &mut merged,
        r#"{"source":"api","id":"component:zweb","kind":"component","name":"zweb","paths":["web"]}"#,
    );
    let report = check(&merged);
    let m = model(&report);
    let web = m.components.iter().find(|c| c.name == "web").expect("web");
    let zweb = m
        .components
        .iter()
        .find(|c| c.name == "zweb")
        .expect("zweb");
    assert!(web.nodes > 0 && zweb.nodes == 0, "{m:#?}");
    assert_eq!(report.errors.len(), 1, "{report:#?}");
    assert_eq!(report.errors[0].0, "component:zweb");
    assert!(report.errors[0].1.contains("`web`"), "{report:#?}");
}
