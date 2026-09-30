//! **hotspots** (CC.10b): pyo3 surface for `glia_engine::hotspots` (CC.10a) —
//! `PyGraph.hotspots`, the modules and symbols that change often AND sit where
//! much depends on them, each row with both its churn rank and its centrality
//! rank, as a native dict (LD.2).

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use glia_engine::hotspots::{HotspotArgs, Hotspots, LEVELS, hotspots, parse_level};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

// `hotspots`' `top=20, min_churn=2` below are literals so `__text_signature__`
// shows them; these keep them the engine's defaults.
const _: () = assert!(glia_engine::hotspots::DEFAULT_TOP == 20);
const _: () = assert!(glia_engine::hotspots::DEFAULT_MIN_CHURN == 2);

/// The whole body of [`PyGraph::hotspots`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). An unknown `level`
/// is an error naming the three levels; an absence counts the build's
/// unparsed files. Returns the engine answer itself, not a
/// `serde_json::Value`: `to_py` decodes its JSON text so the dict keeps the
/// struct's field order (a `Value` map would sort it; `convert.rs`).
fn hotspot_answer(
    merged: &MergedGraph,
    level: &str,
    top: usize,
    min_churn: u32,
    include_tests: bool,
    scope: Option<String>,
    unparsed_files: usize,
) -> Result<Hotspots, String> {
    let level = parse_level(level)
        .ok_or_else(|| format!("unknown level '{level}'; valid levels: {}", LEVELS.join(", ")))?;
    let mut args = HotspotArgs::default();
    args.level = level;
    args.top = top;
    args.min_churn = min_churn;
    args.include_tests = include_tests;
    args.scope = scope;
    let mut answer = hotspots(merged, &args);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    Ok(answer)
}

#[pymethods]
impl PyGraph {
    /// **hotspots** (CC.10a / CC.10b): the code that changes often AND sits
    /// where much depends on it, ranked two ways at once with no composite
    /// score. `level` is `"module"` (MODULE nodes; churn = commits that
    /// touched the file), `"symbol"` (FUNCTION / METHOD / CLASS; churn =
    /// distinct blame times in the span, needs `history_sync(..., blame=True)`)
    /// or `"both"`; any other value raises ValueError. `top` rows per level
    /// (`0` = every ranked row), `min_churn` the smallest churn that ranks,
    /// `include_tests` keeps test / generated code, `scope` (a path or project
    /// label) narrows the population the ranks range over.
    ///
    /// Returns a dict `{modules, symbols, history_head, absence}`. Each row is
    /// `{level, qname, kind, file, line, churn, lines_changed, last_change,
    /// churn_rank, centrality_rank, ranked, tier}`: `line` 1-based,
    /// `last_change` and `history_head` unix seconds, the two ranks
    /// competition ranks (1 = highest) over the `ranked` members of the
    /// level's population, `tier` always `"heuristic"`. Rows order by the two
    /// ranks together. `absence` is `None` when any row came back, else a
    /// dict whose `reason` is `no_history` (no history snapshot: run
    /// `history_sync`) or `no_match` (nothing past the filters).
    #[pyo3(signature = (level="both", top=20, min_churn=2, include_tests=false, scope=None))]
    fn hotspots(
        &self,
        py: Python<'_>,
        level: &str,
        top: usize,
        min_churn: u32,
        include_tests: bool,
        scope: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let answer = hotspot_answer(
            &self.merged,
            level,
            top,
            min_churn,
            include_tests,
            scope,
            self.parse_errors.len(),
        )
        .map_err(PyValueError::new_err)?;
        to_py(py, serde_json::to_string(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};

    /// CC.10b: pyo3 `hotspots` is the engine's `hotspots` under the parsed
    /// level, an unknown level an error, the absence counting unparsed files.
    /// The ranking itself is covered by `engine/tests/hotspots.rs`.
    #[test]
    fn hotspots_is_the_engine_hotspots() {
        let root = std::env::temp_dir().join(format!("glia-cc10b-hotspots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in [
            ("core/util.py", "def helper(x):\n    return x + 1\n"),
            ("api/a.py", "from core.util import helper\n\n\ndef get_a(x):\n    return helper(x)\n"),
            ("scripts/once.py", "def run_once():\n    return 0\n"),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
            std::fs::write(path, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path")).expect("bare build");
        let bare = hotspot_answer(&built.merged, "both", 20, 2, false, None, 3).expect("answer");
        let absence = bare.absence.expect("no history is an absence");
        assert_eq!((absence.reason, absence.unparsed_files), ("no_history", 3));

        let file = |p: &str| HistoryFile { p: p.to_string(), a: Some(1), d: Some(0), from: None };
        let commit = |i: i64, paths: &[&str]| HistoryCommit {
            c: format!("{i:02}{}", "0".repeat(38)),
            t: 1_767_225_600 + i * 86_400,
            files: paths.iter().map(|p| file(p)).collect(),
        };
        let commits = [
            commit(3, &["scripts/once.py", "core/util.py"]),
            commit(2, &["scripts/once.py", "core/util.py", "api/a.py"]),
            commit(1, &["scripts/once.py", "core/util.py", "api/a.py"]),
        ];
        write_history(&root, HistoryMeta::new(commits[0].c.clone(), 2000, None, String::new()), &commits, &[])
            .expect("write snapshot");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let answer = hotspot_answer(&built.merged, "module", 0, 2, false, None, 0).expect("answer");
        assert!(answer.absence.is_none() && answer.symbols.is_empty(), "{answer:#?}");
        assert_eq!(answer.modules[0].qname, "core::util", "{answer:#?}");
        assert_eq!(answer.modules.len(), 3);
        assert_eq!(answer.history_head, Some(1_767_225_600 + 3 * 86_400));
        let json = serde_json::to_string(&answer).expect("serialises");
        assert!(json.starts_with("{\"modules\":[{\"level\":\"module\",\"qname\":\"core::util\""), "{json}");

        let cut = hotspot_answer(&built.merged, "both", 1, 2, false, None, 0).expect("answer");
        assert_eq!((cut.modules.len(), cut.modules[0].ranked), (1, 3));

        let err = hotspot_answer(&built.merged, "modules", 20, 2, false, None, 0).expect_err("unknown level");
        assert!(err.contains("modules") && err.contains("module, symbol, both"), "{err}");
    }
}
