//! LF.3a — user config for the walk: `.glia/overlay.toml`'s `[walk] skip`
//! extends the walk's gates and `[[project]]` declares a sub-project that has
//! no manifest. Both are honoured identically by the build's walk and the
//! store's freshness scan.
//!
//! Before LF.3a (probe r1): `[walk] skip = ["legacy"]` left MODULE
//! `legacy::old` and FUNCTION `legacy::old::legacy_thing` in the graph, and a
//! manifest-less `tools/migrator` had no PROJECT node. Run with
//! `-- --nocapture` and grep `^\[walk\] collapsed` (the `config=` count) and
//! `^\[roots\] declared=` for the fired_on markers.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use glia_code_domain::{cell_type, node_kind};
use glia_core::{CellPayload, NodeId, NodeKindId};
use glia_engine::persist::{default_layout_dir, persist_result};
use glia_engine::{GenerateResult, generate_one};
use glia_store::{MANIFEST_NAME, is_gmap_stale};

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

/// The probe-r1 layout (bench fixture `walk-user-config`), with `overlay` as
/// the `.glia/overlay.toml` body.
fn probe_repo(root: &Path, overlay: &str) {
    write(&root.join("app.py"), "def main():\n    return 1\n");
    write(&root.join("legacy/old.py"), "def legacy_thing():\n    return 1\n");
    write(&root.join("tools/migrator/run.py"), "def migrate():\n    return 2\n");
    write(&root.join(".glia/overlay.toml"), overlay);
}

const PROBE_OVERLAY: &str = concat!(
    "version = 1\n\n",
    "[walk]\n",
    "skip = [\"legacy\"]\n\n",
    "[[project]]\n",
    "path = \"tools/migrator\"\n",
    "label = \"migrator\"\n",
);

/// One node of the build: its kind, nav name and ORIGIN payload (if any).
struct Seen {
    kind: NodeKindId,
    name: String,
    origin: Option<String>,
}

/// Every node keyed by qname.
fn nodes(r: &GenerateResult) -> HashMap<String, Seen> {
    let mut out = HashMap::new();
    for g in &r.merged.graphs {
        let origin: HashMap<NodeId, String> = g
            .nodes
            .iter()
            .filter_map(|n| {
                n.cells.iter().find(|c| c.kind == cell_type::ORIGIN).and_then(|c| match &c.payload {
                    CellPayload::Json(j) => Some((n.id, j.clone())),
                    _ => None,
                })
            })
            .collect();
        for (id, q) in &g.nav.qname_by_id {
            let Some(kind) = g.nav.kind_by_id.get(id) else { continue };
            out.insert(
                q.clone(),
                Seen {
                    kind: *kind,
                    name: g.nav.name_by_id.get(id).cloned().unwrap_or_default(),
                    origin: origin.get(id).cloned(),
                },
            );
        }
    }
    out
}

fn origin_json(seen: &Seen) -> serde_json::Value {
    serde_json::from_str(seen.origin.as_deref().expect("an ORIGIN cell")).expect("ORIGIN is JSON")
}

fn build(root: &Path) -> HashMap<String, Seen> {
    nodes(&generate_one(root.to_str().unwrap()).unwrap())
}

#[test]
fn skip_dir_becomes_excluded_region() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    probe_repo(&root, PROBE_OVERLAY);
    let all = build(&root);

    assert!(!all.contains_key("legacy::old"), "the skipped tree must not be parsed");
    assert!(!all.contains_key("legacy::old::legacy_thing"));
    assert!(all.keys().all(|q| !q.starts_with("legacy::")), "nothing from legacy/");
    let region = all.get("region:legacy").expect("region:legacy");
    assert_eq!(region.kind, node_kind::REGION);
    assert_eq!(region.name, "legacy");
    assert_eq!(
        origin_json(region),
        serde_json::json!({"provenance": "excluded", "region": "legacy"})
    );
    // The control: the rest of the tree is still parsed.
    assert_eq!(all.get("app::main").map(|s| s.kind), Some(node_kind::FUNCTION));
    assert_eq!(
        all.get("tools::migrator::run::migrate").map(|s| s.kind),
        Some(node_kind::FUNCTION)
    );
}

/// A skipped FILE is simply not read, and the patterns are gitignore syntax
/// from the repo root: a slash-less pattern matches at any depth, an anchored
/// one only at its path.
#[test]
fn skip_files_and_anchoring() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    probe_repo(&root, "version = 1\n[walk]\nskip = [\"*_gen.py\", \"/tools/old\"]\n");
    write(&root.join("api_gen.py"), "def generated():\n    return 1\n");
    write(&root.join("pkg/models_gen.py"), "def generated_too():\n    return 1\n");
    write(&root.join("tools/old/x.py"), "def anchored():\n    return 1\n");
    write(&root.join("pkg/tools/old/y.py"), "def not_anchored():\n    return 1\n");
    let all = build(&root);

    assert!(all.keys().all(|q| !q.contains("_gen")), "skipped files are not read");
    assert!(all.contains_key("region:tools/old"), "the anchored dir is a region");
    assert!(!all.contains_key("region:pkg/tools/old"), "the anchor does not match below the root");
    assert!(all.contains_key("pkg::tools::old::y::not_anchored"));
    assert!(all.contains_key("legacy::old::legacy_thing"), "no skip for legacy in this config");
}

/// Config only extends the defaults: a `!` pattern never un-collapses a
/// region nor re-includes a gitignored path, and a user skip on a name rule's
/// directory records the more specific `excluded`.
#[test]
fn skip_never_unskips() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    probe_repo(&root, "version = 1\n[walk]\nskip = [\"!node_modules\", \"!gen\", \"vendor\"]\n");
    write(&root.join(".gitignore"), "/gen\n");
    write(&root.join("node_modules/left-pad/index.js"), "module.exports = function pad() {};\n");
    write(&root.join("gen/out.py"), "def generated():\n    return 1\n");
    write(&root.join("vendor/lib.py"), "def vendored():\n    return 1\n");
    let all = build(&root);

    let provenance = |q: &str| origin_json(all.get(q).unwrap_or_else(|| panic!("no {q}")))["provenance"].clone();
    assert_eq!(provenance("region:node_modules"), "vendored");
    assert_eq!(provenance("region:gen"), "build_output");
    assert_eq!(provenance("region:vendor"), "excluded");
    assert!(all.keys().all(|q| !q.contains("generated") && !q.contains("vendored") && !q.contains("pad")));
    assert!(all.contains_key("legacy::old::legacy_thing"), "legacy is not skipped by this config");
}

#[test]
fn declared_root_emits_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    probe_repo(&root, PROBE_OVERLAY);
    let all = build(&root);

    let project = all.get("project:tools/migrator").expect("project:tools/migrator");
    assert_eq!(project.kind, node_kind::PROJECT);
    assert_eq!(project.name, "migrator");
    assert_eq!(
        origin_json(project),
        serde_json::json!({
            "provenance": "project_root",
            "ecosystem": "declared",
            "manifest": ".glia/overlay.toml",
            "label": "migrator",
            "path": "tools/migrator",
        })
    );
    // The repo has no manifest anywhere else: it is the only PROJECT.
    let projects: Vec<&String> = all.iter().filter(|(_, s)| s.kind == node_kind::PROJECT).map(|(q, _)| q).collect();
    assert_eq!(projects, ["project:tools/migrator"]);
}

/// Config extends, never replaces: a manifest at the declared path keeps its
/// own ecosystem and label.
#[test]
fn declared_root_shadowed_by_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    probe_repo(&root, PROBE_OVERLAY);
    write(&root.join("tools/migrator/pyproject.toml"), "[project]\nname = \"db-migrator\"\n");
    let all = build(&root);

    let project = all.get("project:tools/migrator").expect("project:tools/migrator");
    assert_eq!(project.name, "db-migrator");
    let origin = origin_json(project);
    assert_eq!(origin["ecosystem"], "python");
    assert_eq!(origin["manifest"], "tools/migrator/pyproject.toml");
    let projects = all.values().filter(|s| s.kind == node_kind::PROJECT).count();
    assert_eq!(projects, 1, "one PROJECT for the path, not two");
}

/// A declared root inside a collapsed region, or one that does not exist, is
/// reported and never becomes a PROJECT.
#[test]
fn declared_root_never_inside_a_region() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    probe_repo(
        &root,
        concat!(
            "version = 1\n[walk]\nskip = [\"legacy\"]\n",
            "[[project]]\npath = \"legacy\"\n",
            "[[project]]\npath = \"node_modules/pkg\"\n",
            "[[project]]\npath = \"missing/dir\"\n",
        ),
    );
    write(&root.join("node_modules/pkg/index.js"), "module.exports = 1;\n");
    let all = build(&root);
    assert!(all.values().all(|s| s.kind != node_kind::PROJECT), "no PROJECT at all");
    assert!(all.contains_key("region:legacy") && all.contains_key("region:node_modules"));
}

/// Push `path`'s mtime past the layout's manifest without touching its
/// directory's mtime (the file already exists): an unambiguous "edited after
/// the build" on any filesystem clock.
fn touch_after(path: &Path, manifest: &Path) {
    let built = std::fs::metadata(manifest).unwrap().modified().unwrap();
    let later = built.max(SystemTime::now()) + Duration::from_secs(5);
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(later).unwrap();
}

/// The walk and the store's scan build the same matcher (`IgnoreStack::
/// for_repo` is `with_config` over the same file), so churn in a skipped tree
/// is neither parsed nor freshness churn. Control: without the config the same
/// edit marks stale.
#[test]
fn staleness_agrees_with_walk() {
    for (overlay, legacy_is_source) in [(PROBE_OVERLAY, false), ("version = 1\n", true)] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        probe_repo(&root, overlay);
        let layout = default_layout_dir(&root);
        let r = generate_one(root.to_str().unwrap()).unwrap();
        let parsed_legacy = nodes(&r).contains_key("legacy::old::legacy_thing");
        assert_eq!(parsed_legacy, legacy_is_source, "walk verdict for legacy/ ({overlay:?})");
        persist_result(&r, &layout, "test").unwrap();
        let manifest = layout.join(MANIFEST_NAME);
        assert!(!is_gmap_stale(&layout, &root), "fresh right after the persist");

        touch_after(&root.join("legacy/old.py"), &manifest);
        assert_eq!(
            is_gmap_stale(&layout, &root),
            legacy_is_source,
            "the scan must agree with the walk about legacy/old.py ({overlay:?})"
        );
        touch_after(&root.join("app.py"), &manifest);
        assert!(is_gmap_stale(&layout, &root), "an un-skipped file marks stale");
    }
}
