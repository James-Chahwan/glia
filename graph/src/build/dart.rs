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
//!
//! DT.5i: Riverpod provider receivers. `ref.read(p).m()`,
//! `ref.read(p.notifier).m()` and a local `final svc = ref.read(p);
//! svc.m()` call a method of the type `p`'s declaration names
//! (`final p = Provider<T>(..)`, `StateNotifierProvider<N, S>(..)`), a fact
//! the parser recorded on the provider's STATE_VAR
//! ([`NavFact::RiverpodProvider`]). The receiver text (a ComplexReceiver's
//! receiver, or the type the parser recorded for an Attribute base's local)
//! is parsed by [`provider_read`]; [`provider_type`] binds `p` through the
//! reader's own scope and resolves the recorded type name in the provider's
//! own file ([`scope_type`]: nothing repo-wide), and [`provider_call`] binds
//! only a METHOD of that type: CALLS, EVIDENCE `graph:dart` rule
//! `provider_receiver`. Two classes declaring one method name never cross.
//! The build passes [`provider_call`] as `resolve_calls`' extra hook (run
//! only after every generic lookup missed: the receiver text names no
//! symbol) and [`provider_type`] as [`resolve_member_reads`]' hook, so a
//! getter read on a provider read runs (`auth.isAuthenticated`, CALLS rule
//! `accessor_read`). fired_on, once per Dart graph whose hook bound a call:
//! `[dart-scope] provider receivers bound=<n>`.

use std::cell::Cell;
use std::collections::HashSet;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CallQualifier, CallSite, NavFact, edge_category, node_kind};
use glia_core::{EdgeCategoryId, NodeId};

use crate::calls::{
    enclosing_home_module, enclosing_module, graph_evidence, push_edge, receiver_type,
    resolve_type_name, scope_type, type_method,
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
/// the Dart build passes [`provider_type`] (DT.5i).
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

/// DT.5i: one Riverpod provider read, as [`provider_read`] parses its text.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProviderRead {
    /// The import prefix the provider is read through (`alias.p`).
    pub(crate) alias: Option<String>,
    /// The provider variable's name.
    pub(crate) provider: String,
    /// `p.notifier`: the read returns the provider's notifier.
    pub(crate) notifier: bool,
}

/// DT.5i: a provider read's text, as the Dart parser spells a ComplexReceiver
/// or a provider-read local's type: an identifier head (`ref`, `_ref`, after
/// an optional `this.`), `.read(` / `.watch(`, one argument, `)`; layout
/// whitespace and a trailing comma dropped. The argument is `p`,
/// `p.notifier`, `alias.p`, `p(arg)` (a family's call, any balanced
/// argument text) or `p(arg).notifier`. `.future` / `.stream` (an
/// AsyncValue's) and anything else -> None.
pub(crate) fn provider_read(text: &str) -> Option<ProviderRead> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let t = t.strip_prefix("this.").unwrap_or(&t);
    let head = t.find(|c: char| !is_ident_char(c))?;
    if !is_ident(&t[..head]) {
        return None;
    }
    let rest = &t[head..];
    let open = rest
        .strip_prefix(".read(")
        .or_else(|| rest.strip_prefix(".watch("))
        .map(|r| t.len() - r.len() - 1)?;
    if close_of(t.as_bytes(), open)? != t.len() - 1 {
        return None;
    }
    let mut arg = &t[open + 1..t.len() - 1];
    arg = arg.strip_suffix(',').unwrap_or(arg);
    if arg.ends_with(".future") || arg.ends_with(".stream") {
        return None;
    }
    let notifier = arg.ends_with(".notifier");
    if notifier {
        arg = &arg[..arg.len() - ".notifier".len()];
    }
    if arg.ends_with(')') {
        let open = arg.find('(')?;
        if close_of(arg.as_bytes(), open)? != arg.len() - 1 {
            return None;
        }
        arg = &arg[..open];
    }
    let (alias, provider) = match arg.split_once('.') {
        Some((alias, provider)) => (Some(alias), provider),
        None => (None, arg),
    };
    if !is_ident(provider) || alias.is_some_and(|a| !is_ident(a)) {
        return None;
    }
    Some(ProviderRead {
        alias: alias.map(str::to_string),
        provider: provider.to_string(),
        notifier,
    })
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// A Dart identifier: `[A-Za-z_$][A-Za-z0-9_$]*`.
fn is_ident(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && s.chars().all(is_ident_char)
}

/// The index of the `)` closing the `(` at `open` in `b`, skipping quoted
/// text.
fn close_of(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\'' | b'"' => quote = Some(c),
                b'(' => depth += 1,
                b')' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// DT.5i: the TYPE node a provider-read receiver of `site` has (module doc):
/// the receiver text is a ComplexReceiver's receiver, or the type the
/// parser recorded for an Attribute base's local in `site.from`'s scope
/// (`final svc = ref.read(p)`). `p` binds in the reader's own file (its
/// import bindings, then its own declarations; `alias.p` through the alias's
/// MODULE symbols) to a STATE_VAR carrying a [`NavFact::RiverpodProvider`],
/// and its read (or `.notifier`) type name resolves in the provider's own
/// file ([`scope_type`]). Any miss -> None.
pub(crate) fn provider_type(g: &RepoGraph, site: &CallSite) -> Option<NodeId> {
    let text = match &site.qualifier {
        CallQualifier::ComplexReceiver { receiver, .. } => receiver.as_str(),
        CallQualifier::Attribute { base, .. } => {
            g.nav.local_types.get(&site.from)?.get(base)?.as_str()
        }
        _ => return None,
    };
    let read = provider_read(text)?;
    let module = enclosing_home_module(g, site.from)?;
    let provider = match &read.alias {
        Some(alias) => {
            let lib = *g.symbols.module_import_bindings.get(&module)?.get(alias)?;
            if g.nav.kind_by_id.get(&lib) != Some(&node_kind::MODULE) {
                return None;
            }
            *g.symbols.module_symbols.get(&lib)?.get(&read.provider)?
        }
        None => g
            .symbols
            .module_import_bindings
            .get(&module)
            .and_then(|b| b.get(&read.provider))
            .or_else(|| g.symbols.module_symbols.get(&module)?.get(&read.provider))
            .copied()?,
    };
    if g.nav.kind_by_id.get(&provider) != Some(&node_kind::STATE_VAR) {
        return None;
    }
    let (value, notifier) = g.nav.nav_facts.get(&provider)?.iter().find_map(|f| match f {
        NavFact::RiverpodProvider { read, notifier } => Some((read, notifier)),
        _ => None,
    })?;
    let name = if read.notifier { notifier } else { value };
    if name.is_empty() {
        return None;
    }
    scope_type(g, enclosing_module(&g.nav, provider)?, name)
}

/// DT.5i: `resolve_calls`' extra hook for the Dart build: a call on a
/// provider-read receiver ([`provider_type`]) binds the METHOD of its name
/// on that type (`class_methods`, then `interface_methods`), CALLS, EVIDENCE
/// `graph:dart` rule `provider_receiver`; a field, a STATE_VAR or a missing
/// method binds nothing. `bound` counts the hits for the `[dart-scope]`
/// marker.
pub(crate) fn provider_call(
    g: &RepoGraph,
    site: &CallSite,
    bound: &Cell<usize>,
) -> Option<(NodeId, Evidence)> {
    let name = match &site.qualifier {
        CallQualifier::Attribute { name, .. } | CallQualifier::ComplexReceiver { name, .. } => name,
        _ => return None,
    };
    let method = type_method(g, provider_type(g, site)?, name)?;
    if g.nav.kind_by_id.get(&method) != Some(&node_kind::METHOD) {
        return None;
    }
    bound.set(bound.get() + 1);
    Some((method, graph_evidence("graph:dart", "provider_receiver")))
}

/// DT.5i: the `[dart-scope]` fired_on line, `None` when the hook bound
/// nothing.
pub(crate) fn provider_marker(bound: usize) -> Option<String> {
    (bound > 0).then(|| format!("[dart-scope] provider receivers bound={bound}"))
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
        let (g, reads, _, _) =
            super::super::dart_passes(repo(), parses, |_, spec| Some(spec.to_string()));
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

    /// DT.5i: the provider-read texts the parser writes parse to their
    /// provider, notifier flag and alias; layout whitespace and a trailing
    /// comma drop; `.future` / `.stream`, a named argument, a chain and a
    /// non-read refuse.
    #[test]
    fn provider_read_texts() {
        let pr = |alias: Option<&str>, provider: &str, notifier: bool| {
            Some(ProviderRead {
                alias: alias.map(str::to_string),
                provider: provider.to_string(),
                notifier,
            })
        };
        assert_eq!(provider_read("ref.read(friendServiceProvider)"), pr(None, "friendServiceProvider", false));
        assert_eq!(provider_read("_ref.watch(authProvider.notifier)"), pr(None, "authProvider", true));
        assert_eq!(provider_read("this.ref.read(p)"), pr(None, "p", false));
        assert_eq!(provider_read("ref.read(fs.friendServiceProvider)"), pr(Some("fs"), "friendServiceProvider", false));
        assert_eq!(provider_read("ref.watch(fam(widget.id))"), pr(None, "fam", false));
        assert_eq!(provider_read("ref.watch(fam(a, 'x)'))"), pr(None, "fam", false));
        assert_eq!(provider_read("ref.read(fam(id).notifier)"), pr(None, "fam", true));
        assert_eq!(
            provider_read("ref.read(\n      authProvider.notifier,\n    )"),
            pr(None, "authProvider", true)
        );
        for refused in [
            "ref.read(apiProvider.future)",
            "ref.watch(feed.stream)",
            "storage.read(key: 'k')",
            "ref.read(p).other",
            "ref.listen(p)",
            "ref.read(a.b.c)",
            "ref.read()",
            "read(p)",
            "ref.read(p",
        ] {
            assert_eq!(provider_read(refused), None, "{refused}");
        }
    }

    /// `lib/core/services.dart`: FriendService and GroupService both declare
    /// `listAll` (FriendService also the getter `isReady`), the provider
    /// `friendServiceProvider` names FriendService, `plainVar` is a
    /// STATE_VAR with no provider fact.
    fn services_parse() -> FileParse {
        let mut fp = FileParse::default();
        let module = node(&mut fp, node_kind::MODULE, "lib::core::services", None);
        let friend = node(&mut fp, node_kind::CLASS, "lib::core::services::FriendService", Some(module));
        node(&mut fp, node_kind::METHOD, "lib::core::services::FriendService::listAll", Some(friend));
        let ready = node(&mut fp, node_kind::METHOD, "lib::core::services::FriendService::isReady", Some(friend));
        fp.properties.insert(ready);
        let group = node(&mut fp, node_kind::CLASS, "lib::core::services::GroupService", Some(module));
        node(&mut fp, node_kind::METHOD, "lib::core::services::GroupService::listAll", Some(group));
        let provider = node(&mut fp, node_kind::STATE_VAR, "lib::core::services::friendServiceProvider", Some(module));
        fp.nav.record_fact(
            provider,
            NavFact::RiverpodProvider {
                read: "FriendService".into(),
                notifier: String::new(),
            },
        );
        node(&mut fp, node_kind::STATE_VAR, "lib::core::services::plainVar", Some(module));
        fp
    }

    /// DT.5i: a call on a provider read binds the METHOD of the type the
    /// provider's declaration names (never GroupService's same-named
    /// listAll), directly, through an import alias and through a typed
    /// local's getter read; a STATE_VAR with no provider fact and an empty
    /// notifier type bind nothing.
    #[test]
    fn provider_receivers_bind_strictly() {
        let mut page = FileParse::default();
        let page_mod = node(&mut page, node_kind::MODULE, "lib::page", None);
        let cls = node(&mut page, node_kind::CLASS, "lib::page::Page", Some(page_mod));
        let build = node(&mut page, node_kind::METHOD, "lib::page::Page::build", Some(cls));
        let show = node(&mut page, node_kind::METHOD, "lib::page::Page::show", Some(cls));
        for name in ["friendServiceProvider", "plainVar"] {
            page.imports.push(ImportStmt {
                from_module: "lib::page".into(),
                target: ImportTarget::Symbol {
                    module: "lib::core::services".into(),
                    name: name.into(),
                    alias: None,
                    level: 0,
                },
                line: 0,
            });
        }
        page.imports.push(ImportStmt {
            from_module: "lib::page".into(),
            target: ImportTarget::Module {
                path: "lib::core::services".into(),
                alias: Some("fs".into()),
            },
            line: 1,
        });
        let complex = |from: NodeId, receiver: &str, line: u32| CallSite {
            from,
            qualifier: CallQualifier::ComplexReceiver {
                receiver: receiver.into(),
                name: "listAll".into(),
            },
            line,
        };
        page.calls.push(complex(build, "ref.read(friendServiceProvider)", 5));
        page.calls.push(complex(build, "ref.read(plainVar)", 6));
        page.calls.push(complex(build, "ref.read(friendServiceProvider.notifier)", 7));
        page.calls.push(complex(show, "ref.read(fs.friendServiceProvider)", 9));
        page.nav.record_local_type(build, "svc", "ref.watch(friendServiceProvider)");
        page.nav.record_fact(build, read("svc", "isReady", 8));

        let (g, reads, _, bound) =
            super::super::dart_passes(repo(), vec![services_parse(), page], |_, spec| Some(spec.to_string()));
        let list_all = id(node_kind::METHOD, "lib::core::services::FriendService::listAll");
        let mut provider_edges: Vec<(NodeId, NodeId, Option<u32>)> = g
            .edges
            .iter()
            .filter(|e| {
                e.category == edge_category::CALLS
                    && Evidence::of(e).is_some_and(|ev| {
                        ev.emitter == "graph:dart" && ev.rule.as_deref() == Some("provider_receiver")
                    })
            })
            .map(|e| (e.from, e.to, Evidence::of(e).and_then(|ev| ev.line)))
            .collect();
        provider_edges.sort_by_key(|r| format!("{r:?}"));
        let mut want = vec![(build, list_all, Some(5)), (show, list_all, Some(9))];
        want.sort_by_key(|r| format!("{r:?}"));
        assert_eq!(provider_edges, want);
        let group_list = id(node_kind::METHOD, "lib::core::services::GroupService::listAll");
        assert!(
            !g.edges.iter().any(|e| e.to == group_list && e.category == edge_category::CALLS),
            "GroupService::listAll stays unbound"
        );
        assert_eq!(
            g.unresolved_calls.iter().filter(|c| c.from == build).count(),
            2,
            "the plain STATE_VAR and the empty notifier"
        );
        assert_eq!(bound, 2);
        assert_eq!(provider_marker(bound).as_deref(), Some("[dart-scope] provider receivers bound=2"));
        assert_eq!(provider_marker(0), None);
        assert_eq!(
            read_edges(&g),
            vec![(
                build,
                id(node_kind::METHOD, "lib::core::services::FriendService::isReady"),
                edge_category::CALLS,
                "accessor_read".to_string(),
                Some(8),
            )]
        );
        assert_eq!(reads.accessor_read, 1);
    }
}
