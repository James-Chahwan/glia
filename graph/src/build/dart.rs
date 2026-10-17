//! DT.5h: the Dart build binds the member reads the Dart parser recorded.
//!
//! A Dart member named with no call after it is still a use of that member:
//! a getter runs (`Env.apiBaseUrl`, `_svc.ready`), a method or factory is
//! torn off (`.map(Friend.fromJson)`), a static field is read off its class
//! (`Env.googleServerClientId`). A bare or `this.` name is same-file and
//! lexically scoped, so the parser binds it itself (rules `method_ref` /
//! `accessor_read` / `accessor_write`); a `Type.member` / `x.member` read can
//! name a class of another file or a typed field, so the parser records it
//! as a [`NavFact::MemberRead`] on the reading scope, never an edge, and
//! [`resolve_member_reads`] binds it here through the reader's own scope:
//!
//! - A plain receiver naming a type in the reader's scope
//!   ([`scope_type`]: the reader's file's import bindings, then its own
//!   declarations; a type kind only, nothing repo-wide) binds that type's
//!   METHOD `name` ([`type_method`]) or, on a CLASS with no such METHOD (a
//!   static field read, which has no node), the CLASS itself, USES rule
//!   `static_member`. An ENUM with no such METHOD binds nothing (an enum
//!   constant is no dead_symbol kind).
//! - Any other receiver (an owner field `this.<f>`, a local the parser
//!   typed) is typed exactly as a call's receiver is ([`receiver_type`] +
//!   [`resolve_type_name`]), else by the builder's `hook`, and binds that
//!   type's METHOD `name` only: a field, a STATE_VAR or an unknown type binds
//!   nothing and is counted `unbound`.
//! - A bound METHOD in `RepoGraph::properties` (the Dart parser's getters and
//!   setters, DT.5h) runs: CALLS rule `accessor_read`. Any other METHOD is
//!   torn off: USES rule `tear_off`.
//!
//! Never a member matched by name across the repo. One edge per (from, to,
//! category), at its earliest recorded row, EVIDENCE emitter `graph:reads`;
//! no self edge.
//!
//! Deterministic: scopes are visited in `g.nodes` order and facts in record
//! order; every map is only probed, and every target is collected before an
//! edge is pushed.
//!
//! fired_on, once per Dart graph build: `[reads] member reads
//! accessor_read=<a> tear_off=<t> static_member=<s> unbound=<u>`: the edges
//! drawn by rule, and the read facts that bound nothing.
//!
//! Dart is this pass's only consumer, so it lives beside the Dart builder
//! rather than in `calls.rs` (one concrete instance before abstracting); a
//! second language reading member accesses lifts it there.

use std::collections::HashSet;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CallQualifier, CallSite, NavFact, edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId};

use crate::calls::{
    enclosing_home_module, push_edge, receiver_type, resolve_type_name, scope_type, type_method,
};
use crate::types::RepoGraph;

/// The emitter of every edge this pass draws.
const EMITTER: &str = "graph:reads";

/// What [`resolve_member_reads`] did to one Dart graph, for the `[reads]`
/// marker: the bound counts are the edges drawn (one per (from, to,
/// category)), `unbound` the read facts that bound nothing.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MemberReads {
    /// Getter / setter METHOD reads drawn: CALLS rule `accessor_read`.
    pub(crate) accessor_read: usize,
    /// Other METHODs read without a call, drawn: USES rule `tear_off`.
    pub(crate) tear_off: usize,
    /// Static field reads on a CLASS of the reader's scope, drawn: USES of
    /// the CLASS, rule `static_member`.
    pub(crate) static_member: usize,
    /// Reads that bound nothing: a field or STATE_VAR, an ENUM constant, an
    /// untyped or external receiver.
    pub(crate) unbound: usize,
}

impl MemberReads {
    pub(crate) fn marker(&self) -> String {
        format!(
            "[reads] member reads accessor_read={} tear_off={} static_member={} unbound={}",
            self.accessor_read, self.tear_off, self.static_member, self.unbound
        )
    }
}

/// How one read binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadKind {
    AccessorRead,
    TearOff,
    StaticMember,
}

impl ReadKind {
    fn category(self) -> EdgeCategoryId {
        match self {
            ReadKind::AccessorRead => edge_category::CALLS,
            ReadKind::TearOff | ReadKind::StaticMember => edge_category::USES,
        }
    }

    fn rule(self) -> &'static str {
        match self {
            ReadKind::AccessorRead => "accessor_read",
            ReadKind::TearOff => "tear_off",
            ReadKind::StaticMember => "static_member",
        }
    }
}

/// Bind every [`NavFact::MemberRead`] of `g` (module doc). `hook` types a
/// receiver the generic receiver typing missed: it returns the receiver's
/// TYPE node (a CLASS / ENUM / INTERFACE), whose METHOD `name` then binds;
/// the Dart build passes `|_, _| None`.
pub(crate) fn resolve_member_reads<H>(g: &mut RepoGraph, hook: H) -> MemberReads
where
    H: Fn(&RepoGraph, &CallSite) -> Option<NodeId>,
{
    let mut stats = MemberReads::default();
    let mut edges: Vec<(NodeId, NodeId, ReadKind, u32)> = Vec::new();
    {
        let g: &RepoGraph = g;
        let mut seen: HashSet<(NodeId, NodeId, EdgeCategoryId)> = HashSet::new();
        let mut scopes: HashSet<NodeId> = HashSet::new();
        for node in &g.nodes {
            if !scopes.insert(node.id) {
                continue;
            }
            let Some(facts) = g.nav.nav_facts.get(&node.id) else {
                continue;
            };
            let mut reads: Vec<(&str, &str, u32)> = facts
                .iter()
                .filter_map(|f| match f {
                    NavFact::MemberRead {
                        receiver,
                        name,
                        line,
                    } => Some((receiver.as_str(), name.as_str(), *line)),
                    _ => None,
                })
                .collect();
            // The earliest row of each (from, to, category) places its edge.
            reads.sort_by_key(|&(_, _, line)| line);
            for (receiver, name, line) in reads {
                let Some((to, kind)) = member_read_target(g, node.id, receiver, name, line, &hook)
                else {
                    stats.unbound += 1;
                    continue;
                };
                if to == node.id || !seen.insert((node.id, to, kind.category())) {
                    continue;
                }
                match kind {
                    ReadKind::AccessorRead => stats.accessor_read += 1,
                    ReadKind::TearOff => stats.tear_off += 1,
                    ReadKind::StaticMember => stats.static_member += 1,
                }
                edges.push((node.id, to, kind, line));
            }
        }
    }
    for (from, to, kind, line) in edges {
        let ev = Evidence::emitter(EMITTER).rule(kind.rule()).line(line);
        push_edge(g, from, to, kind.category(), ev);
    }
    stats
}

/// The node one read of `receiver.name` from `from` binds, and how
/// (module doc).
fn member_read_target<H>(
    g: &RepoGraph,
    from: NodeId,
    receiver: &str,
    name: &str,
    line: u32,
    hook: &H,
) -> Option<(NodeId, ReadKind)>
where
    H: Fn(&RepoGraph, &CallSite) -> Option<NodeId>,
{
    if receiver.is_empty() || name.is_empty() {
        return None;
    }
    let module = enclosing_home_module(g, from);
    let plain = !receiver.contains('.');
    if plain
        && let Some(ty) = module.and_then(|m| scope_type(g, m, receiver))
    {
        if let Some(m) = type_method(g, ty, name) {
            return read_of(g, m);
        }
        let is_class = g.nav.kind_by_id.get(&ty) == Some(&node_kind::CLASS);
        return is_class.then_some((ty, ReadKind::StaticMember));
    }
    let qualifier = if plain {
        CallQualifier::Attribute {
            base: receiver.to_string(),
            name: name.to_string(),
        }
    } else {
        CallQualifier::ComplexReceiver {
            receiver: receiver.to_string(),
            name: name.to_string(),
        }
    };
    let site = CallSite {
        from,
        qualifier,
        line,
    };
    let ty = receiver_type(g, &site)
        .and_then(|t| module.and_then(|m| resolve_type_name(g, m, t)))
        .or_else(|| hook(g, &site))?;
    read_of(g, type_method(g, ty, name)?)
}

/// A bound METHOD's read: a getter / setter (in `g.properties`) runs, any
/// other METHOD is torn off; a non-METHOD binds nothing.
fn read_of(g: &RepoGraph, m: NodeId) -> Option<(NodeId, ReadKind)> {
    if g.nav.kind_by_id.get(&m) != Some(&node_kind::METHOD) {
        return None;
    }
    let kind = if g.properties.contains(&m) {
        ReadKind::AccessorRead
    } else {
        ReadKind::TearOff
    };
    Some((m, kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::{FileParse, GRAPH_TYPE, ImportStmt, ImportTarget};
    use glia_core::{Confidence, Edge, Node, RepoId};

    fn repo() -> RepoId {
        RepoId::from_canonical("test://dart-reads")
    }

    fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn node(fp: &mut FileParse, kind: glia_core::NodeKindId, qname: &str, parent: Option<NodeId>) -> NodeId {
        let nid = id(kind, qname);
        fp.nodes.push(Node {
            id: nid,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        if let Some(p) = parent {
            fp.edges.push(Edge::new(p, nid, edge_category::DEFINES, Confidence::Strong));
        }
        let simple = qname.rsplit("::").next().unwrap_or(qname);
        fp.nav.record(nid, simple, qname, kind, parent);
        nid
    }

    fn read(receiver: &str, name: &str, line: u32) -> NavFact {
        NavFact::MemberRead {
            receiver: receiver.into(),
            name: name.into(),
            line,
        }
    }

    /// `lib/env.dart` declares Env (a getter, a factory, no node for its
    /// static field), the ENUM Mode and the STATE_VAR config.
    fn env_parse() -> FileParse {
        let mut env = FileParse::default();
        let env_mod = node(&mut env, node_kind::MODULE, "lib::env", None);
        let env_cls = node(&mut env, node_kind::CLASS, "lib::env::Env", Some(env_mod));
        let base_url = node(&mut env, node_kind::METHOD, "lib::env::Env::baseUrl", Some(env_cls));
        node(&mut env, node_kind::METHOD, "lib::env::Env::fromJson", Some(env_cls));
        let mode = node(&mut env, node_kind::ENUM, "lib::env::Mode", Some(env_mod));
        node(&mut env, node_kind::ATTRIBUTE, "lib::env::Mode::dark", Some(mode));
        node(&mut env, node_kind::STATE_VAR, "lib::env::config", Some(env_mod));
        env.properties.insert(base_url);
        env
    }

    /// `lib/page.dart`: CLASS Page (field `env` declared Env) with METHODs
    /// `build` and `show`, importing Env, Mode and config from lib/env.dart.
    fn page_parse() -> FileParse {
        let mut page = FileParse::default();
        let page_mod = node(&mut page, node_kind::MODULE, "lib::page", None);
        let page_cls = node(&mut page, node_kind::CLASS, "lib::page::Page", Some(page_mod));
        node(&mut page, node_kind::METHOD, "lib::page::Page::build", Some(page_cls));
        node(&mut page, node_kind::METHOD, "lib::page::Page::show", Some(page_cls));
        page.nav.record_field_type(page_cls, "env", "Env");
        for name in ["Env", "Mode", "config"] {
            page.imports.push(ImportStmt {
                from_module: "lib::page".into(),
                target: ImportTarget::Symbol {
                    module: "lib::env".into(),
                    name: name.into(),
                    alias: None,
                    level: 0,
                },
                line: 0,
            });
        }
        page
    }

    /// The `graph:reads` edges of `g`: (from, to, category, rule, line),
    /// sorted.
    fn read_edges(g: &RepoGraph) -> Vec<(NodeId, NodeId, EdgeCategoryId, String, Option<u32>)> {
        let mut out: Vec<_> = g
            .edges
            .iter()
            .filter_map(|e| {
                let ev = Evidence::of(e)?;
                (ev.emitter == EMITTER)
                    .then(|| (e.from, e.to, e.category, ev.rule.unwrap_or_default(), ev.line))
            })
            .collect();
        out.sort_by_key(|r| format!("{r:?}"));
        out
    }

    fn passes(parses: Vec<FileParse>) -> (RepoGraph, MemberReads) {
        let (g, reads, _) = super::super::dart_passes(repo(), parses, |_, spec| Some(spec.to_string()));
        (g, reads)
    }

    /// A getter read runs (CALLS accessor_read), a factory read is torn off
    /// (USES tear_off), a static field read uses its CLASS (static_member),
    /// and an ENUM constant, a STATE_VAR's member and an unknown receiver
    /// bind nothing; `this.env.baseUrl` is typed through the field `env`.
    #[test]
    fn member_reads_bind_by_what_they_name() {
        let mut page = page_parse();
        let build = id(node_kind::METHOD, "lib::page::Page::build");
        let show = id(node_kind::METHOD, "lib::page::Page::show");
        for f in [
            read("Env", "baseUrl", 4),
            read("Env", "fromJson", 5),
            read("Env", "clientId", 6),
            read("Mode", "dark", 7),
            read("config", "host", 8),
            read("nobody", "x", 9),
        ] {
            page.nav.record_fact(build, f);
        }
        page.nav.record_fact(show, read("this.env", "baseUrl", 12));
        let (g, stats) = passes(vec![env_parse(), page]);
        let base_url = id(node_kind::METHOD, "lib::env::Env::baseUrl");
        let mut want = vec![
            (build, base_url, edge_category::CALLS, "accessor_read".to_string(), Some(4)),
            (
                build,
                id(node_kind::METHOD, "lib::env::Env::fromJson"),
                edge_category::USES,
                "tear_off".to_string(),
                Some(5),
            ),
            (
                build,
                id(node_kind::CLASS, "lib::env::Env"),
                edge_category::USES,
                "static_member".to_string(),
                Some(6),
            ),
            (show, base_url, edge_category::CALLS, "accessor_read".to_string(), Some(12)),
        ];
        want.sort_by_key(|r| format!("{r:?}"));
        assert_eq!(read_edges(&g), want);
        assert_eq!(
            stats,
            MemberReads {
                accessor_read: 2,
                tear_off: 1,
                static_member: 1,
                unbound: 3,
            }
        );
        assert_eq!(
            stats.marker(),
            "[reads] member reads accessor_read=2 tear_off=1 static_member=1 unbound=3"
        );
    }

    /// Two reads of one member from one scope are one edge, placed at the
    /// earliest row whatever the record order, and counted once.
    #[test]
    fn one_edge_per_target_at_its_earliest_row() {
        let mut page = page_parse();
        let build = id(node_kind::METHOD, "lib::page::Page::build");
        for f in [read("Env", "baseUrl", 9), read("Env", "baseUrl", 3)] {
            page.nav.record_fact(build, f);
        }
        let (g, stats) = passes(vec![env_parse(), page]);
        let edges = read_edges(&g);
        assert_eq!(edges.len(), 1, "{edges:?}");
        assert_eq!(edges[0].4, Some(3));
        assert_eq!(stats.accessor_read, 1);
    }
}
