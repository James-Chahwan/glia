//! The external-input stage: everything that enters the graph from outside
//! the source (cell sidecars, overlay edges and wrappers, declared knowledge,
//! entrypoints, git history, test reports). Filled by LF.1a; LF.2b, LF.2e,
//! LF.3b, LF.4a, LF.5b and LF.6b add their stage files (overlay.rs,
//! declared.rs, entrypoints.rs, history.rs, test_reports.rs, wrappers.rs) and
//! declare them here as they need them.
//!
//! Directory-module slot declared by L0.2 so its owners edit only this
//! directory. Crate-private: cross-module items are `pub(crate)`.
//!
//! Per repo, a build makes one [`RepoInputs`] right after the walk (before
//! any graph is built, so the build stages can read the overlay) and, once
//! the code passes ran, hands every repo's inputs to [`apply_external_cells`].
//! `.glia/overlay.toml` is loaded ONCE here, through LF.2a's loader; the walk
//! and the store's stale scan read only its `[walk]` / `[[project]]`
//! sections, through `code_domain::walk_gating` (LF.3a).
//!
//! Markers, once per repo that has the file:
//! - `[overlay] loaded .glia/overlay.toml repo=<label> version=<v> (walk=<n> project=<n> ... note=<n>) errors=<e>`
//!   plus one `[overlay] error: <loader error>` line per error;
//! - `[cells] sidecar repo=<label> rows=<r> bound=<b> rekeyed=<k> ambiguous=<a> orphaned=<o> rejected=<x> (CONSTRAINT=<n> DECISION=<n> CONV=<n> VECTOR=<n>)`
//!   (see [`cells`]).

mod cells;

use std::path::PathBuf;

use repo_graph_code_domain::glia_config::{self, LoadedConfig, OVERLAY_FILE};
use repo_graph_core::RepoId;
use repo_graph_graph::MergedGraph;

use crate::passes;

/// One repo's external inputs, made once per build right after its walk.
pub(crate) struct RepoInputs {
    pub(crate) repo: RepoId,
    /// The repo root, as the build was given it.
    pub(crate) root: PathBuf,
    /// The path as given: the `repo=` of every marker.
    pub(crate) label: String,
    /// `.glia/overlay.toml`, loaded once; `None` when the file is absent.
    pub(crate) config: Option<LoadedConfig>,
}

impl RepoInputs {
    /// The `[overlay] loaded` marker and one `[overlay] error:` line per
    /// loader error, when the repo has an overlay file.
    fn report_overlay(&self) {
        let Some(cfg) = &self.config else {
            return;
        };
        let counts: Vec<String> =
            cfg.section_counts().iter().map(|(section, n)| format!("{section}={n}")).collect();
        eprintln!(
            "[overlay] loaded {OVERLAY_FILE} repo={} version={} ({}) errors={}",
            self.label,
            cfg.config.version,
            counts.join(" "),
            cfg.errors.len()
        );
        for e in &cfg.errors {
            eprintln!("[overlay] error: {e}");
        }
    }
}

/// Load `root`'s external inputs and print the overlay marker.
pub(crate) fn repo_inputs(repo: RepoId, root: PathBuf, label: String) -> RepoInputs {
    let config = glia_config::load(&root);
    let inputs = RepoInputs { repo, root, label, config };
    inputs.report_overlay();
    inputs
}

/// Apply every repo's external node cells, per input in argument order and,
/// within a repo, the stages in a FIXED order: the cell sidecars (LF.1a),
/// then the stages LF.4a, LF.3b, LF.5b and LF.6b append, in the order their
/// packets document.
///
/// Runs after the code passes. When any stage changed the graph, the evidence
/// fill and the cross-edge sort run again (LC.3a: fill-then-sort is the last
/// step of every build), so a stage that adds an edge still leaves it located
/// and in canonical order. The sidecar stage writes only CONSTRAINT /
/// DECISION / CONV / VECTOR node cells, which neither the fill (it reads
/// POSITION) nor the sort (edges only) reads. A repo with no external inputs
/// takes no branch that writes anything.
pub(crate) fn apply_external_cells(merged: &mut MergedGraph, inputs: &[RepoInputs]) {
    let mut changed = false;
    for input in inputs {
        changed |= cells::apply_sidecar(merged, input);
    }
    if changed {
        // The first fill's `[evidence]` marker already reported the build.
        passes::fill_evidence_sites(merged);
        merged.sort_cross_edges();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tag: &str, overlay: Option<&str>) -> PathBuf {
        let d = std::env::temp_dir().join(format!("glia_repo_inputs_{}_{tag}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(d.join(".glia")).unwrap();
        if let Some(text) = overlay {
            std::fs::write(d.join(OVERLAY_FILE), text).unwrap();
        }
        d
    }

    #[test]
    fn repo_inputs_loads_config() {
        let repo = RepoId::from_canonical("test://inputs");

        let ok = root("ok", Some("version = 1\n\n[walk]\nskip = [\"gen/\"]\n"));
        let inputs = repo_inputs(repo, ok.clone(), "ok".into());
        let cfg = inputs.config.as_ref().expect("an overlay file loads");
        assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
        assert_eq!(cfg.config.walk.skip, ["gen/"]);

        let none = root("none", None);
        assert!(repo_inputs(repo, none.clone(), "none".into()).config.is_none());

        let bad = root("bad", Some("version = 1\n[[edge]\n"));
        let inputs = repo_inputs(repo, bad.clone(), "bad".into());
        let cfg = inputs.config.as_ref().expect("a malformed overlay is Some, with its error");
        assert_eq!(cfg.errors.len(), 1, "{:?}", cfg.errors);

        for d in [ok, none, bad] {
            std::fs::remove_dir_all(d).ok();
        }
    }
}
