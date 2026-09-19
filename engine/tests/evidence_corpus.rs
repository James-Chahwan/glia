//! LC.3a corpus gate: every edge of every substrate-gap fixture carries
//! exactly one EVIDENCE cell whose emitter names a known stage. A new edge
//! emitter (LE.4a-c, LF.2b, LF.5b, ...) that no stage attributes fails here,
//! even when no fixture of its own asserts evidence.
//!
//! Each fixture is built the way `grade.py` builds it (`generate_one` for one
//! dir, `generate_many` over its `dirs` otherwise) and, for a multi-dir
//! fixture, also cold over its root as `glia analyze <fixture>` does. Both are
//! cold builds that write nothing into the fixture.

use std::path::{Path, PathBuf};

use glia_code_domain::cell_type;
use glia_code_domain::evidence::{Evidence, STAGES};
use glia_engine::{generate_many, generate_one};
use glia_graph::MergedGraph;

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures")
}

/// The fixture's `dirs` (default `["."]`), resolved against it.
fn fixture_dirs(fixture: &Path) -> Vec<String> {
    let key = std::fs::read_to_string(fixture.join("key.json")).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&key).unwrap_or(serde_json::Value::Null);
    let dirs: Vec<String> = v
        .get("dirs")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| d.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_else(|| vec![".".to_string()]);
    dirs.iter()
        .map(|d| fixture.join(d).to_string_lossy().into_owned())
        .collect()
}

/// One line per defect in `m`: an edge with no (or several) EVIDENCE cells,
/// or an emitter outside the stage vocabulary. Capped per build.
fn defects(m: &MergedGraph, label: &str) -> Vec<String> {
    let qname = |id| {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_else(|| format!("{id:?}"))
    };
    let mut out = Vec::new();
    for e in m.all_edges() {
        let n = e
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::EVIDENCE)
            .count();
        let problem = match Evidence::of(e) {
            None => Some(format!("{n} EVIDENCE cell(s), none readable")),
            Some(_) if n != 1 => Some(format!("{n} EVIDENCE cells")),
            Some(ev) => {
                let stage = ev.emitter.split(':').next().unwrap_or("");
                (!STAGES.contains(&stage)).then(|| format!("unknown stage in {}", ev.emitter))
            }
        };
        if let Some(p) = problem {
            out.push(format!(
                "{label}: {} -[{}]-> {}: {p}",
                qname(e.from),
                glia_code_domain::edge_category::name(e.category),
                qname(e.to)
            ));
        }
    }
    out.truncate(5);
    out
}

#[test]
fn every_fixture_edge_has_evidence() {
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(fixtures_root())
        .expect("bench/substrate-gap/fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("key.json").is_file())
        .collect();
    fixtures.sort();
    assert!(!fixtures.is_empty(), "no fixtures found");

    let mut failures = Vec::new();
    let mut builds = 0usize;
    for fixture in &fixtures {
        let name = fixture
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dirs = fixture_dirs(fixture);
        let graded = if dirs.len() == 1 {
            generate_one(&dirs[0])
        } else {
            generate_many(&dirs)
        };
        match graded {
            Ok(r) => failures.extend(defects(&r.merged, &name)),
            Err(e) => failures.push(format!("{name}: build failed: {e}")),
        }
        builds += 1;
        if dirs.len() > 1 {
            match generate_one(&fixture.to_string_lossy()) {
                Ok(r) => failures.extend(defects(&r.merged, &format!("{name} (root)"))),
                Err(e) => failures.push(format!("{name} (root): build failed: {e}")),
            }
            builds += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "{} defect(s) over {builds} builds of {} fixtures:\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n")
    );
}
