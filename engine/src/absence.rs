//! Absence answers (LD.8a): the [`Answer`] envelope whose empty result carries
//! a FACT-tier not-found ([`Absence`]) plus the coverage caveats of the
//! mechanisms it depended on.
//!
//! An empty answer used to say nothing about WHY it was empty: "no DOCUMENTS
//! edge reaches `x`" and "glia cannot see the edge into `x`" both came back as
//! `[]`, and an unknown symbol was an error. Now every list primitive that can
//! come back empty returns `Answer { results, absence }`, and `absence` is
//! `Some` exactly when `results` is empty (`Answer::from_results` enforces
//! it). The absence is a FACT about the graph as built — `tier` is always
//! `"FACT"` — and carries the coverage caveat rows (`coverage_report`'s table)
//! of the edge categories the answer depended on, so a consumer never reads "no edge"
//! as "no code" without the row that says where the graph may be blind.
//!
//! Wrapped here: `resolve_signal_located`, `governing_docs`,
//! `find::find_nodes`. The primitives that restructure later (LD.4a trace,
//! LD.5 blast radius, LD.7c implementors, LD.8b serves, LE.*) build their own
//! `Option<Absence>` with the crate-private builders here (`unknown_symbol`,
//! `empty`, `scope_emptied`, and `unserved_channel` for LD.8b's channel
//! lookup) and the `mechanisms_for_kind` table.
//!
//! Module slot declared by L0.2 so its owner edits only this file. Its API is
//! reached as `glia_engine::absence::<item>`, never flattened into the
//! crate root.
//!
//! fired_on marker, one line per absence built:
//! `[absence] primitive=<p> reason=<r> mechanisms=<a,b|-> caveats=<n> suggestions=<n>`
//! — grep `^\[absence\] primitive=`.

use std::collections::HashSet;

use glia_code_domain::node_kind;
use glia_core::NodeKindId;
use glia_graph::MergedGraph;

use crate::coverage::{CoverageNote, caveats_for, ext_to_language};
use crate::find::FoundNode;

/// How many nearest qnames an [`unknown_symbol`] absence suggests.
pub(crate) const SUGGESTIONS: usize = 3;

/// Why an answer is empty, stated as a fact about the graph as built, with the
/// caveats that say where that fact may not match the code.
///
/// Built only inside the engine (`#[non_exhaustive]`); a surface reads it and
/// may set [`Absence::unparsed_files`], which the graph does not carry.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct Absence {
    /// Always `"FACT"`: a statement about the graph as built, not about the code.
    pub tier: &'static str,
    /// `unknown_symbol` | `no_edges` | `no_match` | `no_signal_match` |
    /// `unserved_channel`.
    pub reason: &'static str,
    /// The query the answer was asked, as given.
    pub query: String,
    /// One sentence, e.g. "no DOCUMENTS edge reaches `app::helper` in this graph".
    pub note: String,
    /// Edge categories the answer depended on (empty when it depended on none).
    pub mechanisms: Vec<&'static str>,
    /// Coverage rows for those mechanisms, for the languages that matter.
    pub caveats: Vec<CoverageNote>,
    /// Nearest qnames, best first (from `find`), for `unknown_symbol`.
    pub suggestions: Vec<String>,
    /// Distinct nodes in the graph the answer searched.
    pub nodes_searched: usize,
    /// Files that failed to parse. Filled by the surface from the build's
    /// `parse_errors` (the `MergedGraph` does not carry them); 0 otherwise.
    pub unparsed_files: usize,
}

/// A list answer: the rows, or — exactly when there are none — why not.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct Answer<T> {
    pub results: Vec<T>,
    /// `Some` iff `results` is empty.
    pub absence: Option<Absence>,
}

impl<T> Answer<T> {
    /// The only constructor, so the invariant holds by construction: `absence`
    /// is built (and its marker printed) only when `results` is empty.
    pub(crate) fn from_results(results: Vec<T>, absence: impl FnOnce() -> Absence) -> Self {
        let absence = results.is_empty().then(absence);
        Answer { results, absence }
    }
}

/// The query names no node: no qname or name matches it exactly. `near` is the
/// `find` answer for the same query (the caller resolved through it); its
/// first [`SUGGESTIONS`] qnames become the suggestions. Caveats cover every
/// language in the graph — the symbol has no file to narrow them by.
pub(crate) fn unknown_symbol(
    merged: &MergedGraph,
    primitive: &'static str,
    query: &str,
    mechanisms: &[&'static str],
    near: &[FoundNode],
) -> Absence {
    let note = format!(
        "no node has the qname or name `{}` in this graph",
        query.trim()
    );
    let mut a = build(merged, query, "unknown_symbol", note, mechanisms, None);
    a.suggestions = near
        .iter()
        .take(SUGGESTIONS)
        .map(|r| r.qname.clone())
        .collect();
    log(&a, primitive);
    a
}

/// An empty answer for a query that did resolve (or needed no symbol).
/// Caveats are narrowed to the language of `seed_file` when it has one, else
/// every language present in the graph.
pub(crate) fn empty(
    merged: &MergedGraph,
    primitive: &'static str,
    query: &str,
    reason: &'static str,
    note: String,
    mechanisms: &[&'static str],
    seed_file: Option<&str>,
) -> Absence {
    let a = build(merged, query, reason, note, mechanisms, seed_file);
    log(&a, primitive);
    a
}

/// Nothing serves the channel `query` names (LD.8b `serves`): reason
/// `unserved_channel`, caveats for every language in the graph (a channel has
/// no file to narrow them by). Unlike [`empty`], the near misses are part of
/// the build, so the marker's `suggestions=` count is the answer's.
pub(crate) fn unserved_channel(
    merged: &MergedGraph,
    primitive: &'static str,
    query: &str,
    note: String,
    mechanisms: &[&'static str],
    suggestions: Vec<String>,
) -> Absence {
    let mut a = build(merged, query, "unserved_channel", note, mechanisms, None);
    a.suggestions = suggestions;
    log(&a, primitive);
    a
}

/// Scope filtered every one of `dropped` results away — never a silent empty.
/// `scope` is the caller's string (a path or a project label); the path it
/// resolved to is named too when it differs.
pub(crate) fn scope_emptied(
    merged: &MergedGraph,
    primitive: &'static str,
    query: &str,
    dropped: usize,
    scope: &str,
) -> Absence {
    let resolved = crate::answers::resolve_scope(merged, scope);
    let named = if resolved == scope {
        format!("`{scope}`")
    } else {
        format!("`{scope}` (path `{resolved}`)")
    };
    let note = format!(
        "{dropped} {} outside scope {named}",
        plural(dropped, "result", "results")
    );
    empty(merged, primitive, query, "no_match", note, &[], None)
}

/// The absence, without suggestions and without its marker.
fn build(
    merged: &MergedGraph,
    query: &str,
    reason: &'static str,
    note: String,
    mechanisms: &[&'static str],
    seed_file: Option<&str>,
) -> Absence {
    let seed_lang = seed_file.and_then(ext_to_language);
    let langs = seed_lang.as_ref().map(std::slice::from_ref);
    Absence {
        tier: "FACT",
        reason,
        query: query.to_string(),
        note,
        mechanisms: mechanisms.to_vec(),
        caveats: caveats_for(merged, mechanisms, langs),
        suggestions: Vec::new(),
        nodes_searched: distinct_nodes(merged),
        unparsed_files: 0,
    }
}

/// The LD.8a fired_on marker, once per absence built.
fn log(a: &Absence, primitive: &str) {
    let mechs = if a.mechanisms.is_empty() {
        "-".to_string()
    } else {
        a.mechanisms.join(",")
    };
    eprintln!(
        "[absence] primitive={primitive} reason={} mechanisms={mechs} caveats={} suggestions={}",
        a.reason,
        a.caveats.len(),
        a.suggestions.len()
    );
}

/// Distinct node ids across every graph of the merge (a node two per-language
/// graphs both hold counts once). O(V), on the empty path only.
fn distinct_nodes(merged: &MergedGraph) -> usize {
    let mut seen = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            seen.insert(n.id);
        }
    }
    seen.len()
}

/// `one` for 1, `many` otherwise.
pub(crate) fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// The edge categories (by `edge_category::name` spelling) an answer about a
/// node of `kind` depends on: the mechanism whose absence it reports and whose
/// caveat rows it carries. Code knowledge, so it lives in the code engine.
///
/// | kind | mechanisms |
/// |---|---|
/// | FUNCTION, METHOD | CALLS |
/// | ROUTE, ENDPOINT | HTTP_CALLS, HANDLED_BY |
/// | QUEUE_PRODUCER, QUEUE_CONSUMER | QUEUE_FLOWS |
/// | GRPC_SERVICE, GRPC_CLIENT, GRPC_SERVER | GRPC_CALLS |
/// | RPC_PROCEDURE, RPC_CALL | RPC_CALLS |
/// | GRAPHQL_RESOLVER, GRAPHQL_OPERATION | GRAPHQL_CALLS |
/// | WS_HANDLER, WS_CLIENT | WS_CONNECTS |
/// | EVENT_HANDLER, EVENT_EMITTER | EVENT_FLOWS |
/// | CLI_COMMAND, CLI_INVOCATION | CLI_INVOKES |
/// | CLASS, INTERFACE, STRUCT | IMPLEMENTS, INHERITS_FROM, INJECTS |
/// | DATA_ENTITY, DATABASE, CACHE | ACCESSES_DATA |
/// | CONFIG_KEY | READS_CONFIG |
/// | DOC_SECTION | DOCUMENTS |
/// | anything else | CALLS |
pub(crate) fn mechanisms_for_kind(kind: NodeKindId) -> &'static [&'static str] {
    const CALLS: &[&str] = &["CALLS"];
    match kind {
        node_kind::FUNCTION | node_kind::METHOD => CALLS,
        node_kind::ROUTE | node_kind::ENDPOINT => &["HTTP_CALLS", "HANDLED_BY"],
        node_kind::QUEUE_PRODUCER | node_kind::QUEUE_CONSUMER => &["QUEUE_FLOWS"],
        node_kind::GRPC_SERVICE | node_kind::GRPC_CLIENT | node_kind::GRPC_SERVER => {
            &["GRPC_CALLS"]
        }
        node_kind::RPC_PROCEDURE | node_kind::RPC_CALL => &["RPC_CALLS"],
        node_kind::GRAPHQL_RESOLVER | node_kind::GRAPHQL_OPERATION => &["GRAPHQL_CALLS"],
        node_kind::WS_HANDLER | node_kind::WS_CLIENT => &["WS_CONNECTS"],
        node_kind::EVENT_HANDLER | node_kind::EVENT_EMITTER => &["EVENT_FLOWS"],
        node_kind::CLI_COMMAND | node_kind::CLI_INVOCATION => &["CLI_INVOKES"],
        node_kind::CLASS | node_kind::INTERFACE | node_kind::STRUCT => {
            &["IMPLEMENTS", "INHERITS_FROM", "INJECTS"]
        }
        node_kind::DATA_ENTITY | node_kind::DATABASE | node_kind::CACHE => &["ACCESSES_DATA"],
        node_kind::CONFIG_KEY => &["READS_CONFIG"],
        node_kind::DOC_SECTION => &["DOCUMENTS"],
        _ => CALLS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::edge_category;

    #[test]
    fn every_mechanism_is_an_edge_category_spelling() {
        // `caveats_for` and `edges_found` key on these strings, so a typo
        // would silently drop every caveat row of that mechanism.
        for (kind, _) in node_kind::ALL {
            for m in mechanisms_for_kind(*kind) {
                assert!(
                    edge_category::ALL.iter().any(|(_, name)| name == m),
                    "{m} (for kind {}) is not an edge category",
                    node_kind::name(*kind)
                );
            }
        }
        assert_eq!(mechanisms_for_kind(node_kind::DOC_SECTION), ["DOCUMENTS"]);
        assert_eq!(
            mechanisms_for_kind(node_kind::ROUTE),
            ["HTTP_CALLS", "HANDLED_BY"]
        );
        assert_eq!(mechanisms_for_kind(node_kind::MODULE), ["CALLS"]);
    }

    #[test]
    fn from_results_builds_absence_only_when_empty() {
        let merged = MergedGraph::new(Vec::new());
        let found = Answer::from_results(vec![1u8], || panic!("absence built for a found answer"));
        assert!(found.absence.is_none());
        let none: Answer<u8> = Answer::from_results(Vec::new(), || {
            empty(
                &merged,
                "test",
                "q",
                "no_match",
                "n".into(),
                &["CALLS"],
                Some("a.py"),
            )
        });
        let a = none.absence.expect("empty answer carries an absence");
        assert_eq!(
            (a.tier, a.reason, a.nodes_searched),
            ("FACT", "no_match", 0)
        );
        // A `.py` seed keeps `*` and python rows only: the universal CALLS row.
        assert_eq!(a.caveats.len(), 1, "{:?}", a.caveats);
        assert_eq!(
            (a.caveats[0].language, a.caveats[0].edge_category),
            ("*", "CALLS")
        );
    }

    #[test]
    fn scope_note_names_count_and_scope() {
        let merged = MergedGraph::new(Vec::new());
        let a = scope_emptied(&merged, "test", "q", 1, "nowhere");
        assert_eq!(a.note, "1 result outside scope `nowhere`");
        assert!(a.mechanisms.is_empty() && a.caveats.is_empty());
    }
}
