//! The synth passes as [`SynthHook`]s over a [`RepoGraph`] (LD.12b).
//!
//! Each hook wraps a pass that used to run only inside its own research bin,
//! so one `activation::plan::ActivationPlan` can run several of them over one
//! graph and one ranked view, each hook seeing the cells the earlier ones
//! emitted:
//!
//! * [`AccessPathSynth`] (ungated): [`synth_paths`] + [`render_cells`], the
//!   A+ access-path cells behind the `synth_composition` bin;
//! * [`CallsiteArgflowSynth`] (`research`):
//!   [`crate::synth_callsite_argflow::run`], the polymorphic call-site cells
//!   behind the `synth_callsite_argflow` bin.
//!
//! A hook maps its pass's output 1:1, in the pass's order, so a bin that runs
//! the hook writes what it wrote when it called the pass directly. The pass
//! functions stay public for callers that do not use a plan.

use repo_graph_activation::plan::{ActivatedView, SynthCell, SynthHook};
use repo_graph_graph::RepoGraph;

use crate::composition::{render_cells, synth_paths};

/// [`SynthHook::name`] of [`AccessPathSynth`]: the `hook` of its cells.
pub const ACCESS_PATH: &str = "access_path";

/// [`SynthHook::name`] of [`CallsiteArgflowSynth`]: the `hook` of its cells.
#[cfg(feature = "research")]
pub const CALLSITE_ARGFLOW: &str = "callsite_argflow";

/// A+ access paths from the view's activated methods to attributes in its
/// neighbourhood ([`synth_paths`]), one cell per path in [`render_cells`]'
/// order: `key` is the cell's `synth::AccessPath::<expression>` qname,
/// `anchor` the class the path starts on (where `self` is typed), `id`,
/// `text` and `score` the rendered cell's.
///
/// The view's ids are the activated set and its scores the per-node scores
/// the path score blends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessPathSynth {
    /// BFS hop cap, including the terminal data-attribute hop.
    pub max_hops: usize,
}

impl SynthHook<RepoGraph> for AccessPathSynth {
    fn name(&self) -> &'static str {
        ACCESS_PATH
    }

    fn synth(&self, graph: &RepoGraph, view: &ActivatedView) -> Vec<SynthCell> {
        let ids = view.ids();
        let paths = synth_paths(&ids, &view.scores, graph, self.max_hops);
        let cells = render_cells(&paths);
        paths
            .iter()
            .zip(cells)
            .map(|(path, cell)| SynthCell {
                hook: ACCESS_PATH,
                id: cell.id,
                key: cell.qname,
                anchor: Some(path.start_class),
                text: cell.summary,
                score: cell.score,
                attrs: Vec::new(),
            })
            .collect()
    }
}

/// Call-site arg-flow for polymorphic method names
/// ([`crate::synth_callsite_argflow::run`] over the view's ids, in view
/// order), one cell per name in `run`'s order: `key` is the peer method the
/// cell is filed under, `id` counts up from `id_start`, no anchor.
#[cfg(feature = "research")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallsiteArgflowSynth {
    /// The first cell's id: a high-water mark above the summary ids the
    /// cells are appended to.
    pub id_start: u64,
}

#[cfg(feature = "research")]
impl SynthHook<RepoGraph> for CallsiteArgflowSynth {
    fn name(&self) -> &'static str {
        CALLSITE_ARGFLOW
    }

    fn synth(&self, graph: &RepoGraph, view: &ActivatedView) -> Vec<SynthCell> {
        crate::synth_callsite_argflow::run(graph, &view.ids(), self.id_start)
            .into_iter()
            .map(|cell| SynthCell {
                hook: CALLSITE_ARGFLOW,
                id: cell.id,
                key: cell.qname,
                anchor: None,
                text: cell.summary,
                score: cell.score,
                attrs: Vec::new(),
            })
            .collect()
    }
}
