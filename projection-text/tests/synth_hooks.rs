//! The synth passes as `SynthHook`s (LD.12b): a hook run through
//! `ActivationPlan::synthesize` emits exactly what its pass emits when called
//! directly, in the pass's order, and the same cells on every run.
//!
//! Fixture: `tests/fixtures/synth_plan/` (shared with LD.12d / LD.12e):
//! `src/m.py`, `seeds.json` (`activated: [[qname, score], ...]`, every
//! METHOD / ATTRIBUTE qname of m.py, scores strictly descending) and an
//! empty `summaries.json`. The research bins run over the same files; their
//! output before and after the hook refactor is byte-identical.

use std::path::PathBuf;

use repo_graph_activation::ActivationConfig;
use repo_graph_activation::plan::{ActivatedView, ActivationPlan, SynthCell};
use repo_graph_core::{NodeId, RepoId};
use repo_graph_graph::{RepoGraph, build_python};
use repo_graph_parser_python::parse_file;
use repo_graph_projection_text::composition::{render_cells, synth_paths};
use repo_graph_projection_text::hooks::{ACCESS_PATH, AccessPathSynth};

/// The runs a determinism test compares: each one builds its own graph, so
/// every `HashMap` in it and in the pass gets a fresh `RandomState`.
const RUNS: usize = 20;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synth_plan")
}

/// The graph `synth_composition` builds from `--src F/src`: m.py parsed as
/// module `m`, one `build_python`.
fn fixture_graph() -> RepoGraph {
    let src = std::fs::read_to_string(fixture_dir().join("src/m.py")).expect("read m.py");
    let repo = RepoId::from_canonical("synth-plan");
    let parse = parse_file(&src, "m.py", "m", repo).expect("parse m.py");
    build_python(repo, vec![parse]).expect("build_python")
}

/// seeds.json resolved the way both bins resolve it: in seeds order, a qname
/// two nodes share going to the later one in `graph.nodes`.
fn ranked_seeds(g: &RepoGraph) -> Vec<(NodeId, f64)> {
    let raw = std::fs::read(fixture_dir().join("seeds.json")).expect("read seeds.json");
    let seeds: serde_json::Value = serde_json::from_slice(&raw).expect("parse seeds.json");
    let activated = seeds["activated"].as_array().expect("activated array");
    let mut by_qname = std::collections::HashMap::new();
    for n in &g.nodes {
        if let Some(q) = g.nav.qname_by_id.get(&n.id) {
            by_qname.insert(q.as_str(), n.id);
        }
    }
    let ranked: Vec<(NodeId, f64)> = activated
        .iter()
        .filter_map(|row| {
            let qname = row[0].as_str()?;
            let score = row[1].as_f64()?;
            by_qname.get(qname).map(|&id| (id, score))
        })
        .collect();
    assert_eq!(ranked.len(), activated.len(), "every fixture seed resolves");
    ranked
}

fn access_path_view(g: &RepoGraph, max_hops: usize) -> ActivatedView {
    let hook = AccessPathSynth { max_hops };
    let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default()).synth(&hook);
    let mut view = ActivatedView::from_ranked(ranked_seeds(g));
    plan.synthesize(g, &mut view);
    view
}

#[test]
fn access_path_hook_equals_direct_synth() {
    let g = fixture_graph();
    let view = access_path_view(&g, 3);

    let ranked = ranked_seeds(&g);
    let ids: Vec<NodeId> = ranked.iter().map(|(id, _)| *id).collect();
    let paths = synth_paths(&ids, &ranked, &g, 3);
    let direct: Vec<SynthCell> = paths
        .iter()
        .zip(render_cells(&paths))
        .map(|(p, c)| SynthCell {
            hook: ACCESS_PATH,
            id: c.id,
            key: c.qname,
            anchor: Some(p.start_class),
            text: c.summary,
            score: c.score,
            attrs: Vec::new(),
        })
        .collect();

    assert_eq!(view.synth, direct);
    assert_eq!(view.applied, vec![ACCESS_PATH]);
    assert_eq!(view.scores, ranked, "synthesize never reranks");
    let keys: Vec<&str> = view.synth.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "synth::AccessPath::self.get_inner.value",
            "synth::AccessPath::self.resolve.opts"
        ],
        "one typed-return path, one docstring-hint path"
    );
    let outer = g
        .nav
        .qname_by_id
        .iter()
        .find(|(_, q)| q.as_str() == "m::Outer")
        .map(|(id, _)| *id);
    assert_eq!(
        view.synth[0].anchor, outer,
        "anchored on the class `self` is typed as"
    );
}

#[test]
fn access_path_hook_is_deterministic() {
    let first = access_path_view(&fixture_graph(), 3).synth;
    assert!(!first.is_empty());
    for run in 1..RUNS {
        assert_eq!(
            access_path_view(&fixture_graph(), 3).synth,
            first,
            "run {run} differs from run 0"
        );
    }
}

#[cfg(feature = "research")]
mod research {
    use super::*;
    use repo_graph_projection_text::driver_utils::build_repo_graph;
    use repo_graph_projection_text::hooks::{CALLSITE_ARGFLOW, CallsiteArgflowSynth};
    use repo_graph_projection_text::synth_callsite_argflow::run;

    const ID_START: u64 = 20_000_000;

    /// The graph `synth_callsite_argflow` builds: `driver_utils::build_repo_graph`.
    fn bin_graph() -> RepoGraph {
        build_repo_graph(&fixture_dir().join("src"), "synth-plan").expect("build fixture graph")
    }

    fn callsite_view(g: &RepoGraph) -> ActivatedView {
        let hook = CallsiteArgflowSynth { id_start: ID_START };
        let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default()).synth(&hook);
        let mut view = ActivatedView::from_ranked(ranked_seeds(g));
        plan.synthesize(g, &mut view);
        view
    }

    #[test]
    fn callsite_hook_equals_run() {
        let g = bin_graph();
        let view = callsite_view(&g);

        let ids: Vec<NodeId> = ranked_seeds(&g).iter().map(|(id, _)| *id).collect();
        let direct: Vec<SynthCell> = run(&g, &ids, ID_START)
            .into_iter()
            .map(|c| SynthCell {
                hook: CALLSITE_ARGFLOW,
                id: c.id,
                key: c.qname,
                anchor: None,
                text: c.summary,
                score: c.score,
                attrs: Vec::new(),
            })
            .collect();

        assert_eq!(view.synth, direct);
        assert_eq!(view.applied, vec![CALLSITE_ARGFLOW]);
        assert_eq!(
            view.synth.len(),
            1,
            "one polymorphic name (`bind`) with two caller classes"
        );
        assert_eq!(view.synth[0].key, "m::ListField::bind");
        assert_eq!(view.synth[0].id, ID_START);
        assert!(
            view.synth[0]
                .text
                .contains("# In `m::Schema::attach`: `field.bind(self (=Schema))`")
        );
        assert!(
            view.synth[0]
                .text
                .contains("# In `m::Nested::attach`: `inner.bind(self (=Nested))`")
        );
    }

    #[test]
    fn callsite_hook_is_deterministic() {
        let first = callsite_view(&bin_graph()).synth;
        assert!(!first.is_empty());
        for run in 1..RUNS {
            assert_eq!(
                callsite_view(&bin_graph()).synth,
                first,
                "run {run} differs from run 0"
            );
        }
    }

    /// Both hooks in one plan: access-path cells first, then call-site cells,
    /// each hook's the same as in its own plan.
    #[test]
    fn both_hooks_append_in_registration_order() {
        let g = bin_graph();
        let access = AccessPathSynth { max_hops: 3 };
        let callsite = CallsiteArgflowSynth { id_start: ID_START };
        let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default())
            .synth(&access)
            .synth(&callsite);
        let mut view = ActivatedView::from_ranked(ranked_seeds(&g));
        plan.synthesize(&g, &mut view);

        let mut chained = access_path_view(&g, 3).synth;
        chained.extend(callsite_view(&g).synth);
        assert_eq!(view.synth, chained);
        assert_eq!(view.applied, vec![ACCESS_PATH, CALLSITE_ARGFLOW]);
    }
}
