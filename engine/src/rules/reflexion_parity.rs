//! The reflexion goldens (DB.8a, DB.8b): check's reflexion model, derived through the
//! rules layer (`rules::reflexion`, `reflexion.dl`), must render `CheckReport` JSON
//! equal to the goldens `engine/tests/data/reflexion/<name>.json`, captured from the
//! native derivation while DB.8a held the two byte-identical; DB.8b deleted the
//! native code, so the goldens are the oracle. Seven fixture trees: the five scenarios
//! of engine/tests/check_reflexion.rs re-created inline (closed layers with an allow,
//! an allow the code never realises, an open model, the longest path winning, the
//! unmapped counts), `root_component` (a `.` component, a PROJECT at the root, a
//! component whose path is one file, `webhooks/` beside `web`) and `strict_and_ties`
//! (a strict layer skipping one, two allows on one pair with `categories`, a two-path
//! component, every edge held by two graphs). `GLIA_UPDATE_GOLDEN=1` rewrites them
//! from the rules path: only for a declared change to the model's semantics.
//!
//! Beside the JSON, the rules layer's tier of every `dependent(C1, C2)` must be the
//! strongest tier of the pair's edges, the tier the matrix renders: the lattice's min
//! over derivations. The fixture test ends with `[reflexion] golden fixtures=<n>
//! identical=<k>`.
//!
//! `quokka_parity` (ignored: it needs a local copy) builds `GLIA_PARITY_REPO`, a
//! prepared repository whose `.glia/overlay.toml` is
//! `engine/tests/data/reflexion/quokka.overlay.toml`, and compares its report with
//! `GLIA_PARITY_GOLDEN`, the native report DB.8a's run of the same copy wrote (not
//! committed: it depends on a local repository), or writes it under
//! `GLIA_UPDATE_GOLDEN=1`. It prints `[reflexion] golden repo=<path> components=<c>
//! deps=<d> identical=true` and the derivation's cost.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use glia_core::NodeId;
use glia_graph::MergedGraph;
use glia_rules::value::Tier;

use super::facts::CodeFacts;
use super::reflexion;
use crate::check::{CheckReport, ModelEdb, ReflexionFacts, check_rules};
use crate::external::declared::declared_constraints;

const WEB_APP: &str =
    "from services.api.handlers import get_order\n\n\ndef show(o):\n    return get_order(o)\n";
const HANDLERS: &str = "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n";
const WEB_ADMIN: &str = "from store.db import load\n\n\ndef audit(o):\n    return load(o)\n";
const DB: &str = "def load(o):\n    return o\n";
const TOOL: &str = "from store.db import load\n";
/// A file under `web/admin/` (a directory), for the longest path.
const PANEL: &str = "from store.db import load\n\n\ndef panel(o):\n    return load(o)\n";
/// An unmapped caller of a mapped function.
const RUN: &str = "from web.app import show\n\n\ndef go(o):\n    return show(o)\n";
/// Under `webhooks/`, which `web` does not hold (segment boundary).
const HOOK: &str = "from web.app import show\n\n\ndef hook(o):\n    return show(o)\n";
const ROOT_MANIFEST: (&str, &str) = ("pyproject.toml", "[project]\nname = \"monorepo\"\n");

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

/// ui (strict) over core over data.
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

const ALLOW_WEB_STORE: &str = "
[[constraint]]
id = \"web-uses-store-cache\"
kind = \"allow\"
from = \"web\"
to = \"store\"
";

const ALLOW_ABSENCE: &str = "version = 1

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

const ADMIN: &str = "
[[component]]
name = \"admin\"
paths = [\"web/admin\"]
";

/// store/ and scripts/ belong to no component.
const UNMAPPED: &str = "version = 1

[[component]]
name = \"web\"
paths = [\"web\"]

[[component]]
name = \"api\"
paths = [\"services/api\"]
";

/// `rest` owns the root and everything nothing longer owns; `tool` is one file.
const ROOTED: &str = "version = 1

[[component]]
name = \"rest\"
paths = [\".\"]

[[component]]
name = \"web\"
paths = [\"web\"]

[[component]]
name = \"tool\"
paths = [\"scripts/tool.py\"]

[[constraint]]
id = \"web-uses-rest\"
kind = \"allow\"
from = \"web\"
to = \"rest\"

[[constraint]]
id = \"tool-uses-rest\"
kind = \"allow\"
from = \"tool\"
to = \"rest\"
";

/// A two-path component, a strict top layer, two allows on api -> store (the
/// smaller id names it; one sets `categories`, which narrows the checked set).
const STRICT_AND_TIES: &str = "version = 1

[[component]]
name = \"web\"
paths = [\"web\"]

[[component]]
name = \"api\"
paths = [\"services/api\"]

[[component]]
name = \"store\"
paths = [\"store\", \"scripts\"]

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

[[constraint]]
id = \"z-api-store\"
kind = \"allow\"
from = \"api\"
to = \"store\"
categories = [\"IMPORTS\", \"CALLS\"]

[[constraint]]
id = \"a-api-store\"
kind = \"allow\"
from = \"api\"
to = \"store\"
";

/// One parity fixture: the shared tree plus `extra` files, `overlay` as
/// `.glia/overlay.toml`; `twice` appends a copy of every graph, so every edge is
/// held by two graphs (check's site dedup).
struct Fixture {
    name: &'static str,
    overlay: String,
    extra: Vec<(&'static str, &'static str)>,
    twice: bool,
}

fn fixtures() -> Vec<Fixture> {
    let fx = |name, overlay: String, extra: Vec<(&'static str, &'static str)>, twice| Fixture {
        name,
        overlay,
        extra,
        twice,
    };
    vec![
        fx(
            "closed_layers",
            format!("{COMPONENTS}{LAYERS}{ALLOW_WEB_STORE}"),
            vec![],
            false,
        ),
        fx("allow_absence", ALLOW_ABSENCE.to_string(), vec![], false),
        fx("open_model", COMPONENTS.to_string(), vec![], false),
        fx(
            "longest_path",
            format!("{COMPONENTS}{ADMIN}"),
            vec![("web/admin/panel.py", PANEL)],
            false,
        ),
        fx(
            "unmapped",
            UNMAPPED.to_string(),
            vec![("scripts/run.py", RUN)],
            false,
        ),
        fx(
            "root_component",
            ROOTED.to_string(),
            vec![ROOT_MANIFEST, ("webhooks/hook.py", HOOK)],
            false,
        ),
        fx("strict_and_ties", STRICT_AND_TIES.to_string(), vec![], true),
    ]
}

/// Build `fx`'s tree; the tempdir is returned so it outlives the graph's use.
fn build(fx: &Fixture) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let files = MANIFESTS
        .iter()
        .chain(SOURCES.iter())
        .chain(fx.extra.iter())
        .copied()
        .chain(std::iter::once((".glia/overlay.toml", fx.overlay.as_str())));
    for (rel, src) in files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let mut merged = crate::generate_one(tmp.path().to_str().expect("utf-8 temp path"))
        .expect("generate_one")
        .merged;
    if fx.twice {
        let copies = merged.graphs.clone();
        merged.graphs.extend(copies);
    }
    (tmp, merged)
}

/// The model's EDB as `check_rules_with` declares it: component and layer entries,
/// then every allow admitted (an allow's error is the report's, not the EDB's).
fn model_of(rules: &[(NodeId, ConstraintRule)]) -> ModelEdb {
    let decls: Vec<&ConstraintRule> = rules
        .iter()
        .map(|(_, r)| r)
        .filter(|r| {
            matches!(
                r.kind,
                ConstraintKind::Component { .. } | ConstraintKind::Layer { .. }
            )
        })
        .collect();
    let mut m = ModelEdb::declare(&decls);
    for (_, r) in rules {
        if let ConstraintKind::Allow { from, to } = &r.kind {
            // A refused allow is a report error row; the EDB just leaves it out.
            let _ = m.admit_allow(r, from, to);
        }
    }
    m
}

/// The strongest tier of each pair's edges: the tier the matrix renders.
fn strongest(f: &ReflexionFacts<'_>) -> BTreeMap<(usize, usize), Tier> {
    f.dep
        .iter()
        .map(|(pair, edges)| {
            let t = edges
                .iter()
                .map(|d| Tier::parse(d.tier).unwrap_or(Tier::Derived))
                .min()
                .unwrap_or(Tier::Fact);
            (*pair, t)
        })
        .collect()
}

/// The `dependent` tier lattice of `merged`'s model: every pair whose rules-layer
/// tier is not the strongest tier of its edges, named.
fn tier_lattice(merged: &MergedGraph, rules: &[(NodeId, ConstraintRule)]) -> Vec<String> {
    let facts = CodeFacts::new(merged);
    let edb = model_of(rules);
    if edb.components.is_empty() {
        return vec!["no component is declared".to_string()];
    }
    let checked = edb.checked();
    let (derived, tiers) = match reflexion::derive_tiered(&facts, merged, edb, checked) {
        Ok(x) => x,
        Err(e) => return vec![format!("prelude derive failed: {e}")],
    };
    let name = |c: usize| derived.components.get(c).map_or("?", |d| d.name.as_str());
    let want = strongest(&derived);
    let mut out = Vec::new();
    for (pair, t) in &want {
        if tiers.get(pair) != Some(t) {
            out.push(format!(
                "dependent({}, {}): rules tier {:?}, strongest edge {t:?}",
                name(pair.0),
                name(pair.1),
                tiers.get(pair)
            ));
        }
    }
    for pair in tiers.keys().filter(|p| !want.contains_key(p)) {
        out.push(format!(
            "dependent({}, {}) has no dep edge",
            name(pair.0),
            name(pair.1)
        ));
    }
    out
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/reflexion")
        .join(format!("{name}.json"))
}

fn updating() -> bool {
    std::env::var("GLIA_UPDATE_GOLDEN").is_ok_and(|v| v == "1")
}

/// `report` against the golden at `path` (pretty JSON, one trailing newline), or
/// written there when updating.
fn against_golden(report: &CheckReport, path: &std::path::Path) -> Option<String> {
    let pretty = serde_json::to_string_pretty(report).expect("a report serialises") + "\n";
    if updating() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("mkdir golden dir");
        }
        std::fs::write(path, &pretty).expect("write golden");
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(golden) if golden == pretty => None,
        Ok(_) => Some(format!(
            "{}: the report differs from the golden the native derivation captured\nreport {pretty}",
            path.display()
        )),
        Err(e) => Some(format!("{}: {e}", path.display())),
    }
}

#[test]
fn fixtures_equal_their_goldens() {
    let all = fixtures();
    let mut identical = 0;
    let mut failures: Vec<String> = Vec::new();
    for fx in &all {
        let (_tmp, merged) = build(fx);
        let rules = declared_constraints(&merged);
        let report = check_rules(&merged, &rules);
        let mut bad: Vec<String> = Vec::new();
        if report.reflexion.is_none() {
            bad.push(format!(
                "no reflexion model was evaluated: {:?}",
                report.errors
            ));
        }
        bad.extend(tier_lattice(&merged, &rules));
        bad.extend(against_golden(&report, &golden_path(fx.name)));
        if bad.is_empty() {
            identical += 1;
        } else {
            failures.push(format!("{}:\n{}", fx.name, bad.join("\n")));
        }
    }
    eprintln!(
        "[reflexion] golden fixtures={} identical={identical}",
        all.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert_eq!(identical, 7);
}

/// What each fixture exercises, pinned so a fixture cannot quietly stop covering
/// its edge case (the goldens pin the rest).
#[test]
fn fixtures_cover_their_cases() {
    let all = fixtures();
    let report = |name: &str| {
        let fx = all.iter().find(|f| f.name == name).expect("a fixture");
        let (_tmp, merged) = build(fx);
        check_rules(&merged, &declared_constraints(&merged))
    };
    let cells = |r: &CheckReport| -> Vec<(String, String, &'static str, Option<String>)> {
        r.reflexion
            .as_ref()
            .expect("a model")
            .matrix
            .iter()
            .map(|c| (c.from.clone(), c.to.clone(), c.status, c.allowed_by.clone()))
            .collect()
    };
    let s = |x: &str| x.to_string();

    let r = report("root_component");
    let m = r.reflexion.as_ref().expect("a model");
    assert!(r.errors.is_empty(), "{r:#?}");
    assert_eq!(
        cells(&r),
        [
            (s("rest"), s("web"), "divergence", None),
            (
                s("tool"),
                s("rest"),
                "convergence",
                Some(s("tool-uses-rest"))
            ),
            (s("web"), s("rest"), "convergence", Some(s("web-uses-rest"))),
        ],
        "webhooks/hook.py is rest's, not web's; tool.py is its own component"
    );
    assert_eq!(m.unmapped.files, 0, "`.` owns every file: {m:#?}");
    let tool = m
        .components
        .iter()
        .find(|c| c.name == "tool")
        .expect("tool");
    assert_eq!(tool.nodes, 1, "the one file's module: {tool:#?}");

    let r = report("strict_and_ties");
    assert!(r.errors.is_empty(), "{r:#?}");
    assert_eq!(
        cells(&r),
        [
            (s("api"), s("store"), "convergence", Some(s("a-api-store"))),
            (s("web"), s("api"), "convergence", Some(s("layer:ui>core"))),
            (s("web"), s("store"), "divergence", None),
        ],
        "the smallest allow id names the pair; a strict layer may not skip one"
    );
    let m = r.reflexion.as_ref().expect("a model");
    assert_eq!(
        m.unmapped.files, 0,
        "scripts/ is store's second path: {m:#?}"
    );
    for c in &m.matrix {
        assert_eq!(
            c.edges, 2,
            "one IMPORTS + one CALLS, each held twice: {c:#?}"
        );
    }

    let r = report("unmapped");
    let u = &r.reflexion.as_ref().expect("a model").unmapped;
    assert_eq!(
        u.sample,
        ["scripts/run.py", "scripts/tool.py", "store/db.py"],
        "{u:#?}"
    );
    assert!(u.edges_to_mapped > 0, "both directions cross: {u:#?}");
}

#[test]
#[ignore = "needs GLIA_PARITY_REPO and GLIA_PARITY_GOLDEN, a prepared repository copy and its native report (module doc)"]
fn quokka_parity() {
    let Ok(repo) = std::env::var("GLIA_PARITY_REPO") else {
        panic!(
            "set GLIA_PARITY_REPO to a repository copy whose .glia/overlay.toml is engine/tests/data/reflexion/quokka.overlay.toml"
        );
    };
    let Ok(golden) = std::env::var("GLIA_PARITY_GOLDEN") else {
        panic!(
            "set GLIA_PARITY_GOLDEN to the report DB.8a's native run of the same copy wrote (GLIA_UPDATE_GOLDEN=1 writes it)"
        );
    };
    let merged = crate::generate_one(&repo).expect("generate_one").merged;
    let rules = declared_constraints(&merged);

    let t = Instant::now();
    let report = check_rules(&merged, &rules);
    let check_time = t.elapsed();

    // The derivation alone, over code facts already read.
    let facts = CodeFacts::new(&merged);
    let edb = model_of(&rules);
    let checked = edb.checked();
    let _ = facts.files();
    let _ = facts.projects();
    let t = Instant::now();
    reflexion::derive(&facts, &merged, edb, checked).expect("the prelude evaluates");
    let derive_time = t.elapsed();
    assert!(
        tier_lattice(&merged, &rules).is_empty(),
        "{:?}",
        tier_lattice(&merged, &rules)
    );

    let m = report.reflexion.as_ref().expect("a model is declared");
    if let Some(e) = against_golden(&report, std::path::Path::new(&golden)) {
        panic!("{e}");
    }
    eprintln!(
        "[reflexion] golden repo={repo} components={} deps={} identical=true",
        m.components.len(),
        m.matrix.len()
    );
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    eprintln!(
        "[reflexion] golden cost derive_ms={:.1} check_ms={:.1}",
        ms(derive_time),
        ms(check_time)
    );
}
