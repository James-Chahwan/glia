//! The overlay edge stage (LF.2b): every `[[edge]]` stanza of a repo's
//! `.glia/overlay.toml` becomes one cross edge, the escape hatch for what no
//! extractor can see (a fetch whose URL is built in a helper, reflection,
//! config-driven dispatch).
//!
//! Per stanza, in file order:
//! - `from` / `to` bind by EXACT qname: in the stanza's own repo first, else in
//!   any other repo of the build. Several hits bind the smallest NodeId (both
//!   indexes are `graph::cells::QnameIndex`, which keeps hits smallest-first and
//!   never iterates a HashMap), so the choice is deterministic. No hit on
//!   either side leaves the stanza `orphaned`.
//! - A stanza whose `(from, to, category)` is already an edge of the graph (an
//!   extracted one, or an earlier stanza) is `redundant`: the extractor caught
//!   up, and nothing is added (LF.2c reports it).
//! - Otherwise one edge is pushed onto `cross_edges`: the stanza's category,
//!   confidence `Origin::confidence()` (`llm` Weak, `human` Medium, never
//!   Strong), an ORIGIN edge cell
//!   `{"provenance":"overlay:<llm|human>","rule":"edge#<n>","note":<note>}`
//!   (`note` omitted when the stanza has none, redacted by A13.7's
//!   `redact_untrusted` like any checked-in free text) and its EVIDENCE:
//!   emitter [`EMITTER`], rule `edge#<n>`, file `.glia/overlay.toml`, 0-based
//!   line of the stanza's `[[edge]]` header (basis `site`).
//!
//! `<n>` is the stanza's 1-based position among the file's `[[edge]]`
//! stanzas, counting the ones the loader dropped (it records their line in
//! its errors), so the number a reader counts in the file is the number the
//! graph shows. Structural and history-owned categories (DEFINES / CONTAINS /
//! CO_CHANGES) never reach this stage: the loader drops them, and they are
//! counted here as `rejected`.
//!
//! The stage runs as the first `Post` pass of `profile::CODE_PASSES`, after
//! the cross-graph resolvers and before every post-pass, so the HTTP demotion
//! sees an overlay HTTP_CALLS edge as a pairing, and the Finalize sort fixes
//! the edge order (the written bytes do not depend on push order).
//!
//! Marker, once per repo whose file declares any `[[edge]]` (the fired_on line):
//!   `[overlay] edges repo=<label> declared=<d> applied=<a> redundant=<r> orphaned=<o> rejected=<x> (llm=<l> human=<h>)`
//! where `declared` counts every stanza (rejected ones included) and
//! `llm` / `human` split the applied ones by origin. Then one detail line per
//! stanza that did not apply, at most [`MAX_DETAIL_LINES`] per repo:
//!   `[overlay] orphaned|redundant edge#<n> .glia/overlay.toml:<line> ...`.

use std::collections::BTreeSet;

use repo_graph_code_domain::evidence::Evidence;
use repo_graph_code_domain::glia_config::{LoadedConfig, OVERLAY_FILE, Origin};
use repo_graph_code_domain::snapshots::redact_untrusted;
use repo_graph_code_domain::{cell_type, edge_category};
use repo_graph_core::{Cell, CellPayload, Edge, NodeId};
use repo_graph_graph::MergedGraph;
use repo_graph_graph::cells::{CellTarget, QnameIndex};

use super::RepoInputs;

/// The EVIDENCE emitter of every overlay edge: stage `overlay`, component the
/// `[[edge]]` section.
pub(crate) const EMITTER: &str = "overlay:edge";

/// Detail lines printed per repo before the rest are summarised.
const MAX_DETAIL_LINES: usize = 32;

/// What the edge stage keeps across the repos of one build: the key of every
/// edge already in the graph (to spot a redundant stanza) and the
/// all-repo qname index (the cross-repo fallback). Both are built on first
/// use, so a build without `[[edge]]` stanzas builds neither. Adding an edge
/// changes no node, so the index stays valid while edges are pushed.
#[derive(Default)]
pub(super) struct EdgeStage {
    existing: Option<BTreeSet<(u64, u64, u32)>>,
    all_repos: Option<QnameIndex>,
}

/// Outcome counts of one repo's `[[edge]]` stanzas.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct EdgeTally {
    pub(super) declared: usize,
    pub(super) applied: usize,
    pub(super) redundant: usize,
    pub(super) orphaned: usize,
    pub(super) rejected: usize,
    pub(super) llm: usize,
    pub(super) human: usize,
    details: Vec<String>,
}

/// The ORIGIN edge-cell payload, in this field order.
#[derive(serde::Serialize)]
struct OriginJson<'a> {
    provenance: &'a str,
    rule: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a str>,
}

/// One stanza, bound: what it asserts and where it came from.
struct Bound {
    from: NodeId,
    to: NodeId,
    category: repo_graph_core::EdgeCategoryId,
    origin: Origin,
    ordinal: usize,
    line: u32,
    note: Option<String>,
}

/// Apply `cfg`'s `[[edge]]` stanzas (the config of `input`) to `merged`, and
/// print the marker. `None` when the file declares no `[[edge]]` at all.
pub(super) fn apply_overlay_edges(
    merged: &mut MergedGraph,
    input: &RepoInputs,
    cfg: &LoadedConfig,
    stage: &mut EdgeStage,
) -> Option<EdgeTally> {
    let rejected_lines = rejected_edge_lines(cfg);
    let stanzas = &cfg.config.edge;
    if stanzas.is_empty() && rejected_lines.is_empty() {
        return None;
    }
    let mut t = EdgeTally {
        declared: stanzas.len() + rejected_lines.len(),
        rejected: rejected_lines.len(),
        ..EdgeTally::default()
    };

    // Bind every stanza before the graph is touched: the indexes borrow it.
    let mut bound: Vec<Bound> = Vec::with_capacity(stanzas.len());
    if !stanzas.is_empty() {
        let own = QnameIndex::build(merged, Some(input.repo));
        for (kept, stanza) in stanzas.iter().enumerate() {
            let decl = stanza.get_ref();
            let line = cfg.line_of(stanza.span());
            let ordinal = 1 + kept + rejected_lines.iter().filter(|l| **l < line).count();
            let from = bind(merged, &own, &mut stage.all_repos, &decl.from);
            let to = bind(merged, &own, &mut stage.all_repos, &decl.to);
            let Some(category) = decl.category_id() else {
                // The loader drops an unknown category; counted here too so a
                // loader change cannot slip an unnamed edge through.
                t.rejected += 1;
                continue;
            };
            match (from, to) {
                (Some(from), Some(to)) => bound.push(Bound {
                    from,
                    to,
                    category,
                    origin: decl.origin,
                    ordinal,
                    line,
                    note: decl.note.clone(),
                }),
                (from, to) => {
                    t.orphaned += 1;
                    let side = match (from, to) {
                        (None, None) => "from,to",
                        (None, Some(_)) => "from",
                        _ => "to",
                    };
                    t.details.push(format!(
                        "orphaned edge#{ordinal} {OVERLAY_FILE}:{line} from={} to={} category={} (no node: {side})",
                        decl.from, decl.to, decl.category
                    ));
                }
            }
        }
    }

    let existing = stage.existing.get_or_insert_with(|| {
        merged.all_edges().map(|e| (e.from.0, e.to.0, e.category.0)).collect()
    });
    for b in bound {
        let rule = format!("edge#{}", b.ordinal);
        if !existing.insert((b.from.0, b.to.0, b.category.0)) {
            t.redundant += 1;
            t.details.push(format!(
                "redundant {rule} {OVERLAY_FILE}:{} {} -> {} category={} (already an edge)",
                b.line,
                qname_of(merged, b.from),
                qname_of(merged, b.to),
                edge_category::name(b.category)
            ));
            continue;
        }
        let note = b.note.as_deref().map(|n| {
            let (clean, spans) = redact_untrusted(n);
            if spans > 0 {
                t.details.push(format!("redacted {rule} {OVERLAY_FILE}:{} note spans={spans}", b.line));
            }
            clean
        });
        let origin = OriginJson { provenance: b.origin.provenance(), rule: &rule, note: note.as_deref() };
        // Plain strings only: serialising cannot fail; `{}` would read as no
        // provenance rather than lie.
        let origin = serde_json::to_string(&origin).unwrap_or_else(|_| String::from("{}"));
        let evidence = Evidence::emitter(EMITTER).rule(rule).at(OVERLAY_FILE, b.line.saturating_sub(1));
        merged.cross_edges.push(
            Edge::new(b.from, b.to, b.category, b.origin.confidence())
                .with_cell(Cell { kind: cell_type::ORIGIN, payload: CellPayload::Json(origin) })
                .with_cell(evidence.to_cell()),
        );
        t.applied += 1;
        match b.origin {
            Origin::Llm => t.llm += 1,
            Origin::Human => t.human += 1,
        }
    }
    report(&input.label, &t);
    Some(t)
}

/// Bind `qname`: the stanza's own repo first, then every repo of the build.
/// Several hits bind the smallest NodeId.
fn bind(merged: &MergedGraph, own: &QnameIndex, all: &mut Option<QnameIndex>, qname: &str) -> Option<NodeId> {
    let pick = |t: CellTarget| match t {
        CellTarget::Bound(id) | CellTarget::Ambiguous(id) => Some(id),
        _ => None,
    };
    pick(own.resolve(qname, None, None))
        .or_else(|| pick(all.get_or_insert_with(|| QnameIndex::build(merged, None)).resolve(qname, None, None)))
}

/// The 1-based lines of the `[[edge]]` stanzas the loader dropped, read from
/// its errors (`.glia/overlay.toml:<line>: [[edge]] ... (stanza dropped)`).
fn rejected_edge_lines(cfg: &LoadedConfig) -> Vec<u32> {
    let prefix = format!("{OVERLAY_FILE}:");
    cfg.errors
        .iter()
        .filter_map(|e| {
            let (line, msg) = e.strip_prefix(prefix.as_str())?.split_once(": ")?;
            msg.starts_with("[[edge]] ").then(|| line.parse().ok()).flatten()
        })
        .collect()
}

fn report(label: &str, t: &EdgeTally) {
    eprintln!(
        "[overlay] edges repo={label} declared={} applied={} redundant={} orphaned={} rejected={} (llm={} human={})",
        t.declared, t.applied, t.redundant, t.orphaned, t.rejected, t.llm, t.human
    );
    for line in t.details.iter().take(MAX_DETAIL_LINES) {
        eprintln!("[overlay] {line}");
    }
    if t.details.len() > MAX_DETAIL_LINES {
        eprintln!("[overlay] ... {} more detail lines", t.details.len() - MAX_DETAIL_LINES);
    }
}

/// The qname of `id`, for a detail line; its numeric id when no nav names it.
fn qname_of(merged: &MergedGraph, id: NodeId) -> String {
    merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id))
        .cloned()
        .unwrap_or_else(|| id.0.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use repo_graph_code_domain::glia_config::parse_str;
    use repo_graph_graph::MergedGraph;

    use super::*;

    const FIXTURE: &str = "bench/substrate-gap/fixtures/xcut-overlay-edges";

    fn fixture_dir(sub: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(FIXTURE).join(sub)
    }

    /// The fixture's web + api graphs, assembled (no pass run), and the web
    /// repo's inputs as the build loaded them.
    fn assembled() -> (MergedGraph, Vec<RepoInputs>) {
        let dirs = [fixture_dir("web"), fixture_dir("api")].map(|d| d.to_string_lossy().into_owned());
        let a = crate::build::assemble_many(&dirs, false).expect("fixture assembles");
        (a.merged, a.inputs)
    }

    /// The web repo's inputs with `text` as its overlay file.
    fn with_config(input: &RepoInputs, text: &str) -> RepoInputs {
        RepoInputs {
            repo: input.repo,
            root: input.root.clone(),
            label: input.label.clone(),
            config: Some(parse_str(text)),
        }
    }

    fn apply(merged: &mut MergedGraph, input: &RepoInputs) -> EdgeTally {
        let cfg = input.config.as_ref().expect("a config");
        apply_overlay_edges(merged, input, cfg, &mut EdgeStage::default()).expect("[[edge]] declared")
    }

    fn counts(t: &EdgeTally) -> [usize; 7] {
        [t.declared, t.applied, t.redundant, t.orphaned, t.rejected, t.llm, t.human]
    }

    /// The fixture's own file: one CALLS stanza applies, the DEFINES one was
    /// dropped by the loader and counts as rejected (the fired_on counts).
    #[test]
    fn fixture_tally_is_the_marker_counts() {
        let (mut merged, inputs) = assembled();
        let before = merged.cross_edges.len();
        let t = apply(&mut merged, &inputs[0]);
        assert_eq!(counts(&t), [2, 1, 0, 0, 1, 1, 0], "{t:?}");
        assert_eq!(merged.cross_edges.len(), before + 1);
    }

    /// Unknown qnames are orphaned (per side), a stanza equal to an extracted
    /// edge (or to an earlier stanza) is redundant, `human` is Medium, and the
    /// rule number counts the stanzas the loader dropped.
    #[test]
    fn orphaned_redundant_and_ordinals() {
        let (mut merged, inputs) = assembled();
        let text = r#"version = 1

[[edge]]
from = "src::report::loadReport"
to = "report::helper"
category = "CONTAINS"

[[edge]]
from = "src::report::loadReport"
to = "report::nope"
category = "CALLS"

[[edge]]
from = "report::build_report"
to = "report::helper"
category = "CALLS"

[[edge]]
from = "src::report::loadReport"
to = "report::helper"
category = "USES"
origin = "human"

[[edge]]
from = "src::report::loadReport"
to = "report::helper"
category = "USES"
"#;
        let input = with_config(&inputs[0], text);
        let before = merged.cross_edges.len();
        let t = apply(&mut merged, &input);
        // declared 5: 1 rejected (CONTAINS), 1 orphaned (report::nope), 1
        // redundant with the extracted build_report -> helper CALLS, 1 applied
        // (human), 1 redundant with the stanza before it.
        assert_eq!(counts(&t), [5, 1, 2, 1, 1, 0, 1], "{t:?}");
        assert_eq!(merged.cross_edges.len(), before + 1);
        let e = merged.cross_edges.last().expect("the applied edge");
        assert_eq!(e.confidence, repo_graph_core::Confidence::Medium);
        let ev = Evidence::of(e).expect("evidence");
        assert_eq!(ev.rule.as_deref(), Some("edge#4"), "the dropped CONTAINS stanza is edge#1");
        assert_eq!(ev.line, Some(17), "0-based line of the 4th [[edge]] header");
        assert!(t.details.iter().any(|d| d.starts_with("orphaned edge#2 ") && d.ends_with("(no node: to)")), "{:?}", t.details);
        assert!(t.details.iter().any(|d| d.starts_with("redundant edge#3 ")), "{:?}", t.details);
        assert!(t.details.iter().any(|d| d.starts_with("redundant edge#5 ")), "{:?}", t.details);
    }

    /// Own repo first, then any repo of the build; a file with no `[[edge]]`
    /// (or a malformed one) reports nothing.
    #[test]
    fn binding_order_and_silence() {
        let (mut merged, inputs) = assembled();
        // `report` is the api repo's MODULE only; `src::report` the web one's.
        let own = QnameIndex::build(&merged, Some(inputs[0].repo));
        let mut all = None;
        let web_mod = bind(&merged, &own, &mut all, "src::report").expect("own repo");
        let api_mod = bind(&merged, &own, &mut all, "report").expect("other repo");
        let repo_of = |id: NodeId| merged.graphs.iter().find(|g| g.nodes.iter().any(|n| n.id == id)).map(|g| g.repo);
        assert_eq!(repo_of(web_mod), Some(inputs[0].repo));
        assert_eq!(repo_of(api_mod), Some(inputs[1].repo));
        assert!(bind(&merged, &own, &mut all, "nope::nothing").is_none());

        for text in ["version = 1\n[walk]\nskip = [\"gen/\"]\n", "version = 1\n[[edge]\n"] {
            let input = with_config(&inputs[0], text);
            let cfg = input.config.as_ref().expect("a config");
            assert!(apply_overlay_edges(&mut merged, &input, cfg, &mut EdgeStage::default()).is_none(), "{text}");
        }
    }

    #[test]
    fn rejected_lines_are_read_from_loader_errors() {
        let cfg = parse_str("version = 1\n\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"DEFINES\"\n\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"NOPE\"\n");
        assert_eq!(rejected_edge_lines(&cfg), [3, 8], "{:?}", cfg.errors);
        assert!(rejected_edge_lines(&parse_str("[[edge]]\n")).is_empty(), "a whole-file error is not a stanza");
    }
}
