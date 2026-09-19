//! The synth passes as `SynthHook`s (LD.12b): a hook run through
//! `ActivationPlan::synthesize` emits exactly what its pass emits when called
//! directly, in the pass's order, and the same cells on every run.
//!
//! Fixture: `tests/fixtures/synth_plan/` (shared with LD.12d / LD.12e):
//! `src/m.py`, `seeds.json` (`activated: [[qname, score], ...]`, every
//! METHOD / ATTRIBUTE qname of m.py, scores strictly descending), an
//! empty `summaries.json` and `issue.txt` (LD.12d: a dotted `Outer.use` and
//! issue-quoted identifiers, so key_symbols' tail and body boosts fire). The
//! research bins run over the same files; their output before and after the
//! hook refactor is byte-identical.

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
    use std::collections::HashMap;

    use repo_graph_core::Node;
    use repo_graph_projection_text::driver_utils::{
        build_repo_graph, extract_code_cell, extract_position_cell,
    };
    use repo_graph_projection_text::hooks::{CALLSITE_ARGFLOW, CallsiteArgflowSynth};
    use repo_graph_projection_text::research::key_symbols::{
        KEY_SYMBOLS, KeySymbolsSynth, SUMMARY, TestPatchFacts, cell_attr,
    };
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

    // ------------------------------------------------------------------
    // key_symbols (LD.12d)
    // ------------------------------------------------------------------

    /// The hook as `synth_key_symbols` fills it from the fixture's seeds.json
    /// (`activated` verbatim, no class seeds / test-patch facts / anchors),
    /// issue.txt, no test patch or chain, and its default flags.
    fn fixture_key_symbols() -> KeySymbolsSynth {
        let raw = std::fs::read(fixture_dir().join("seeds.json")).expect("read seeds.json");
        let seeds: serde_json::Value = serde_json::from_slice(&raw).expect("parse seeds.json");
        let activated = seeds["activated"]
            .as_array()
            .expect("activated array")
            .iter()
            .map(|row| {
                let qname = row[0].as_str().expect("seed qname").to_string();
                (qname, row[1].as_f64().expect("seed score"))
            })
            .collect();
        KeySymbolsSynth {
            activated,
            class_seeds: Vec::new(),
            test_patch_facts: TestPatchFacts::default(),
            issue_anchored_qnames: Vec::new(),
            issue: std::fs::read_to_string(fixture_dir().join("issue.txt")).expect("read issue.txt"),
            test_patch: String::new(),
            chain_depths: HashMap::new(),
            top_k: 5,
            aplus_scan: 5,
            chain_grounding_cap: 5,
            max_chars: 3000,
        }
    }

    /// access_path, callsite_argflow and key_symbols in one plan over the
    /// seeds' ranked view: key_symbols reads the access-path cells the first
    /// hook emitted in the same pass.
    fn key_symbols_view(g: &RepoGraph, key_symbols: &KeySymbolsSynth) -> ActivatedView {
        let access = AccessPathSynth { max_hops: 3 };
        let callsite = CallsiteArgflowSynth { id_start: ID_START };
        let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default())
            .synth(&access)
            .synth(&callsite)
            .synth(key_symbols);
        let mut view = ActivatedView::from_ranked(ranked_seeds(g));
        plan.synthesize(g, &mut view);
        view
    }

    /// `qname`'s node as the bins resolve it: a qname two nodes share (a
    /// `@property` is a METHOD and an ATTRIBUTE) goes to the later one in
    /// `graph.nodes`.
    fn node_of<'g>(g: &'g RepoGraph, qname: &str) -> &'g Node {
        g.nodes
            .iter()
            .rev()
            .find(|n| g.nav.qname_by_id.get(&n.id).map(String::as_str) == Some(qname))
            .unwrap_or_else(|| panic!("no node {qname}"))
    }

    #[test]
    fn key_symbols_hook_ranks_the_fixture() {
        let g = bin_graph();
        let view = key_symbols_view(&g, &fixture_key_symbols());
        assert_eq!(view.applied, vec![ACCESS_PATH, CALLSITE_ARGFLOW, KEY_SYMBOLS]);

        let cells: Vec<&SynthCell> = view.cells_of(KEY_SYMBOLS).collect();
        let rows: Vec<(u64, &str, &str)> = cells
            .iter()
            .map(|c| (c.id, c.key.as_str(), cell_attr(c, "reason").expect("reason")))
            .collect();
        assert_eq!(
            rows,
            vec![
                (1, "m::Outer::use", "activated"),
                (2, "m::Source::use", "activated"),
                (3, "m::Outer::__init__", "activated"),
                (4, "m::Target::__init__", "activated"),
                (5, "m::Source::__init__", "activated"),
                (6, "m::Outer::get_inner", "attr-presence"),
                (7, "m::Inner::value", "attr-presence"),
                (8, "m::Source::resolve", "attr-presence"),
                (9, "m::Target::opts", "attr-presence"),
            ],
            "the dotted `.use` lifts both `use` methods over the seed order; \
             the access-path backticks ground the attributes"
        );

        // Outer.use: seed 0.7 + tail `.use` 10.0 + body match `get_inner` 3.0,
        // summed in the ranker's order.
        assert_eq!(cells[0].score, 0.7 + 10.0 + 3.0 + 0.0 + 0.0 + 0.0 + 0.0 + 0.0);
        // Source.use: tail only (`resolve` / `opts` are no issue anchors).
        assert_eq!(cells[1].score, 0.4 + 10.0 + 0.0 + 0.0 + 0.0 + 0.0 + 0.0 + 0.0);
        assert!(cells[5..].iter().all(|c| c.score == 0.0), "no ranked score");

        for c in &cells {
            assert_eq!(c.anchor, Some(node_of(&g, &c.key).id), "{} anchored on its node", c.key);
            let names: Vec<&str> = c.attrs.iter().map(|(k, _)| *k).collect();
            assert_eq!(names, vec!["file", "rank", "reason", "start_line", "end_line"]);
            assert_eq!(cell_attr(c, "file"), Some("m.py"));
            assert_eq!(cell_attr(c, "rank"), Some(c.id.to_string().as_str()));
        }
        let outer_use = node_of(&g, "m::Outer::use");
        assert_eq!(Some(cells[0].text.as_str()), extract_code_cell(outer_use));
        let pos: serde_json::Value =
            serde_json::from_str(extract_position_cell(outer_use).expect("POSITION")).expect("json");
        assert_eq!(cell_attr(cells[0], "start_line"), Some(pos["start_line"].to_string().as_str()));
        assert_eq!(cell_attr(cells[0], "end_line"), Some(pos["end_line"].to_string().as_str()));
        assert!(
            cells[6]
                .text
                .starts_with("# Attribute `.value` presence (derived from HAS_ATTRIBUTE edges)\n")
        );
    }

    /// The bin's route: the same A+ cells loaded onto the view as `summary`
    /// cells before a key_symbols-only plan give the same key_symbols cells.
    #[test]
    fn key_symbols_reads_summary_cells_like_hook_cells() {
        let g = bin_graph();
        let hook = fixture_key_symbols();
        let in_pass = key_symbols_view(&g, &hook);

        let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default()).synth(&hook);
        let mut view = ActivatedView::from_ranked(Vec::new());
        view.synth.extend(
            in_pass
                .synth
                .iter()
                .filter(|c| c.hook != KEY_SYMBOLS)
                .map(|c| SynthCell { hook: SUMMARY, anchor: None, ..c.clone() }),
        );
        let preloaded = view.synth.len();
        assert_eq!(preloaded, 3, "two access paths and one call-site cell");
        plan.synthesize(&g, &mut view);

        assert_eq!(view.applied, vec![KEY_SYMBOLS]);
        let from_summaries: Vec<&SynthCell> = view.synth[preloaded..].iter().collect();
        let from_hooks: Vec<&SynthCell> = in_pass.cells_of(KEY_SYMBOLS).collect();
        assert_eq!(from_summaries, from_hooks);
    }

    #[test]
    fn key_symbols_hook_is_deterministic() {
        let hook = fixture_key_symbols();
        let first = key_symbols_view(&bin_graph(), &hook).synth;
        assert!(first.iter().any(|c| c.hook == KEY_SYMBOLS));
        for run in 1..RUNS {
            assert_eq!(
                key_symbols_view(&bin_graph(), &hook).synth,
                first,
                "run {run} differs from run 0"
            );
        }
    }

    /// A body with `é` (2 bytes) where `max_chars` cuts inside it: HEAD's bin
    /// byte-sliced and panicked ("byte index N is not a char boundary"); the
    /// hook clips at the boundary below.
    #[test]
    fn key_symbols_clip_is_char_safe() {
        let repo = RepoId::from_canonical("synth-plan");
        let src = "def greet(name):\n    return \"h\u{e9}llo \" + name\n";
        let parse = parse_file(src, "u.py", "u", repo).expect("parse u.py");
        let g = build_python(repo, vec![parse]).expect("build_python");
        let code = extract_code_cell(node_of(&g, "u::greet")).expect("CODE cell");
        let e_acute = code.find('\u{e9}').expect("é in the body");

        let clip = |max_chars: usize| {
            let hook = KeySymbolsSynth {
                activated: vec![("u::greet".to_string(), 1.0)],
                top_k: 5,
                max_chars,
                ..KeySymbolsSynth::default()
            };
            let plan = ActivationPlan::<RepoGraph>::new(ActivationConfig::default()).synth(&hook);
            let mut view = ActivatedView::from_ranked(Vec::new());
            plan.synthesize(&g, &mut view);
            assert_eq!(view.synth.len(), 1);
            view.synth[0].text.clone()
        };

        assert!(!code.is_char_boundary(e_acute + 1), "the cut falls inside `é`");
        assert_eq!(clip(e_acute + 1), format!("{}\n# ... (truncated)", &code[..e_acute]));
        // On a boundary the cut is exact, as HEAD's byte slice was.
        assert_eq!(clip(e_acute + 2), format!("{}\n# ... (truncated)", &code[..e_acute + 2]));
        assert_eq!(clip(e_acute), format!("{}\n# ... (truncated)", &code[..e_acute]));
        // 0 = no cap; a cap at or above the length leaves the source whole.
        assert_eq!(clip(0), code);
        assert_eq!(clip(code.len()), code);
    }
}
