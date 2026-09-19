//! Endpoint fold (A11.2): resolve a client call's base URL through the repo
//! [`ConstTable`], split the authority off, and key the ENDPOINT on the path.
//!
//! `` this.http.get(`${environment.apiUrl}/users`) `` parses to
//! `endpoint:GET:${…}/users`. The HTTP resolver can only pair that through its
//! BaseFold tier: Medium confidence, and blind to which service the base
//! names. With `environment.apiUrl = 'http://users-service:8080'` in the repo
//! table, this pass re-keys the node to `endpoint:GET:/users`, which pairs at
//! the Exact tier. It also records `"host":"users-service:8080"` on the
//! ENDPOINT_HIT cell, the input A11.4's host narrowing reads. When the base is
//! bound differently per deployment (`environment.ts` vs
//! `environment.prod.ts`), it also records every binding's authority as
//! `"hosts":[…]`, first binding first (A11.4, see [`deployment_hosts`]).
//!
//! WHERE IT RUNS. After the parse cache, over every FileParse of the repo,
//! whether it came from the cache or not (`build_graphs_for_repo`). The cache
//! holds the PRE-fold parse (`route.rs` stores a clone before this runs), so it
//! stays a pure per-file parser cache, and a constant changed in another file
//! is re-folded on the next build (the cache rule in `constants.rs`).
//!
//! WHAT IT READS, per ENDPOINT node entry, from its single ENDPOINT_HIT cell:
//! - `template` (TypeScript): the literal with each `${expr}` source kept. It
//!   is folded through the table. Spans that do not resolve come back as `${…}`.
//! - `raw` (A3.3, TypeScript + Dart): the literal before host/query stripping.
//!   Only its authority is new information.
//! - otherwise the qname's own path.
//!
//! [`url_split`] then gives `(host, path)`.
//!
//! OWNERSHIP. A3.1's BaseFold tier and this pass split the `${base}` shape:
//! this pass owns bases the table CAN resolve. A base it cannot resolve leaves
//! the node untouched, so BaseFold still pairs `/{}/users` at Medium. A3.3
//! owns stripping. This pass never strips the path a second time; it only
//! reads the authority back off `raw`. Both go through the one authority
//! splitter in `code_domain::endpoint`.
//!
//! ZERO-CHANGE GUARANTEE. If a node's path does not move and it has no host to
//! record, nothing about it changes: not its id, its edges, its nav entry, or
//! its cell bytes.
//!
//! PRE-SET HOSTS (A11.5). Go, Python, Java, Swift and Dart clients write
//! `"host"` themselves at extraction, from the literal they saw. That host
//! stands: the pass never replaces it, only counts it, so an entry whose path
//! does not move is left byte-identical. Both kinds of host feed the one
//! `[endpoint-host]` line.
//!
//! OVERLAY CONSTANTS (LF.2d). A key pinned from `.glia/overlay.toml`
//! `[constants]` (`ConstTable::pin`) folds like a source binding. When the
//! folded template read one ([`ConstTable::pinned_keys_in`]), the entry is
//! inference, not extraction: its ENDPOINT_HIT gains
//! `"overlay":"const:<KEY>[,<KEY>...]"` and the node takes the pin's
//! confidence ([`pin_confidence`], Weak), so every pairing it makes is at most
//! Weak. An entry no pin reached is untouched.

use std::collections::HashMap;
use std::fmt;

use repo_graph_code_domain::endpoint::{endpoint_qname, url_split};
use repo_graph_code_domain::glia_config::Origin;
use repo_graph_code_domain::{CodeNav, FileParse, GRAPH_TYPE, cell_type, node_kind};
use repo_graph_code_extractors::constants::{ConstTable, fold_interpolations};
use repo_graph_core::{CellPayload, Confidence, Node, NodeId, RepoId};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rekey::rewrite_node_id;

/// What the pass did to one repo, for the `[endpoint-fold]` and
/// `[endpoint-host]` markers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoldStats {
    /// ENDPOINT node entries (TypeScript: call sites) whose path, and so
    /// whose identity, changed.
    pub folded: usize,
    /// ENDPOINT node entries that gained a `host` from this pass.
    pub hosts: usize,
    /// A11.5: ENDPOINT node entries whose ENDPOINT_HIT already carried a
    /// `host` when they reached the pass (written by the parser). Disjoint
    /// from `hosts`: the pass never re-records a pre-set host.
    pub preset: usize,
}

impl FoldStats {
    fn add(&mut self, other: FoldStats) {
        self.folded += other.folded;
        self.hosts += other.hosts;
        self.preset += other.preset;
    }

    /// The fired_on markers, once per repo, each only when non-zero:
    /// A11.2's `[endpoint-fold]` when the pass changed something, and
    /// A11.5's `[endpoint-host]` when any client endpoint carries an
    /// authority, whichever side recorded it.
    pub(crate) fn report(&self, repo_label: &str) {
        if self.folded + self.hosts > 0 {
            eprintln!(
                "[endpoint-fold] folded {} endpoint paths, captured {} hosts repo={repo_label}",
                self.folded, self.hosts
            );
        }
        let carried = self.hosts + self.preset;
        if carried > 0 {
            eprintln!(
                "[endpoint-host] {carried} client endpoints carry an authority \
                 (preset={}, captured={}) repo={repo_label}",
                self.preset, self.hosts
            );
        }
    }
}

/// Fold every FileParse of one repo.
pub(crate) fn fold_repo<'a>(
    parses: impl IntoIterator<Item = &'a mut FileParse>,
    consts: &ConstTable,
    repo: RepoId,
) -> FoldStats {
    let mut stats = FoldStats::default();
    for fp in parses {
        stats.add(fold_endpoint_paths(fp, consts, repo));
    }
    stats
}

/// What one ENDPOINT node entry becomes.
struct Plan {
    /// Unchanged when only a host was captured.
    id: NodeId,
    name: String,
    qname: String,
    payload: String,
    moved: bool,
    host: bool,
    /// LF.2d: the node's new confidence, when an overlay constant folded it.
    confidence: Option<Confidence>,
}

/// LF.2d: the confidence of an entry an overlay constant folded. `[constants]`
/// is a flat `NAME = "literal"` table with no per-key `origin`, so every pin
/// has the default stanza origin (`llm`): Weak.
fn pin_confidence() -> Confidence {
    Origin::default().confidence()
}

/// Fold the ENDPOINT nodes of one file in place.
pub(crate) fn fold_endpoint_paths(
    fp: &mut FileParse,
    consts: &ConstTable,
    repo: RepoId,
) -> FoldStats {
    // Node entries grouped by their current id, in first-seen order. The
    // TypeScript parser pushes one entry per call site, so an id can repeat.
    let mut order: Vec<NodeId> = Vec::new();
    let mut groups: HashMap<NodeId, Vec<(usize, Option<Plan>)>> = HashMap::new();
    let mut stats = FoldStats::default();
    for (idx, node) in fp.nodes.iter().enumerate() {
        if fp.nav.kind_by_id.get(&node.id) != Some(&node_kind::ENDPOINT) {
            continue;
        }
        stats.preset += usize::from(arrived_with_host(node));
        let plan = plan_entry(node, &fp.nav, consts, repo);
        groups
            .entry(node.id)
            .or_insert_with(|| {
                order.push(node.id);
                Vec::new()
            })
            .push((idx, plan));
    }

    for old in order {
        let Some(entries) = groups.remove(&old) else {
            continue;
        };
        if entries.iter().all(|(_, p)| p.is_none()) {
            continue;
        }
        let targets: Vec<NodeId> = entries
            .iter()
            .map(|(_, p)| p.as_ref().map_or(old, |p| p.id))
            .collect();
        if !retarget_edges(fp, old, &targets) {
            continue;
        }
        update_nav(&mut fp.nav, old, &targets, &entries);
        for (idx, plan) in entries {
            let Some(plan) = plan else { continue };
            let Some(node) = fp.nodes.get_mut(idx) else {
                continue;
            };
            node.id = plan.id;
            if let Some(c) = plan.confidence {
                node.confidence = c;
            }
            for cell in node
                .cells
                .iter_mut()
                .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
            {
                cell.payload = CellPayload::Json(plan.payload.clone());
            }
            stats.folded += usize::from(plan.moved);
            stats.hosts += usize::from(plan.host);
        }
    }
    stats
}

/// Decide what one node entry becomes, or None to leave it alone.
///
/// An entry is only planned when it carries exactly one JSON ENDPOINT_HIT
/// cell, which is what every client emitter writes. Anything else is left
/// alone rather than guessed at.
fn plan_entry(node: &Node, nav: &CodeNav, consts: &ConstTable, repo: RepoId) -> Option<Plan> {
    let qname = nav.qname_by_id.get(&node.id)?;
    let (method, qpath) = qname.strip_prefix("endpoint:")?.split_once(':')?;
    let mut hits = node
        .cells
        .iter()
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT);
    let (Some(cell), None) = (hits.next(), hits.next()) else {
        return None;
    };
    let CellPayload::Json(json) = &cell.payload else {
        return None;
    };
    let mut fields: Fields = serde_json::from_str(json).ok()?;

    let folded = fields
        .str("template")
        .and_then(|t| fold_interpolations(t, consts));
    // LF.2d: the overlay constants that fold read, if it folded at all.
    let pins: Vec<String> = match (&folded, fields.str("template")) {
        (Some(_), Some(t)) => consts.pinned_keys_in(t).into_iter().map(String::from).collect(),
        _ => Vec::new(),
    };
    let input = folded
        .as_deref()
        .or_else(|| fields.str("raw"))
        .unwrap_or(qpath);
    let (host, path) = url_split(input);
    let path = path?;
    // An interpolated authority (`https://${…}/x`) names no service, and a
    // host the parser already wrote (A11.5) is not re-recorded.
    let host = host.filter(|h| !h.contains("${") && fields.str("host").is_none());
    let moved = path != qpath;
    if !moved && host.is_none() {
        return None;
    }
    // Only a folded base can have deployment alternatives.
    let hosts = match (&folded, fields.str("template")) {
        (Some(_), Some(t)) => deployment_hosts(t, consts, host.as_deref()),
        _ => Vec::new(),
    };

    if moved {
        fields.set("path", Value::from(path.as_str()));
        fields.set("folded_from", Value::from(qpath));
    }
    if let Some(h) = &host {
        fields.set("host", Value::from(h.as_str()));
    }
    if !hosts.is_empty() {
        fields.set("hosts", Value::from(hosts));
    }
    if !pins.is_empty() {
        fields.set("overlay", Value::from(format!("const:{}", pins.join(","))));
    }
    let payload = serde_json::to_string(&fields).ok()?;
    // `url_split` only ever returns a path starting with `/`, so this is
    // byte-identical to the literal shape; it keeps ONE qname builder (LB.5).
    let new_qname = endpoint_qname(method, &path);
    let id = if moved {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENDPOINT, &new_qname)
    } else {
        node.id
    };
    Some(Plan {
        id,
        name: format!("{method} {path}"),
        qname: new_qname,
        payload,
        moved,
        host: host.is_some(),
        confidence: (!pins.is_empty()).then(pin_confidence),
    })
}

/// A11.5: the entry's ENDPOINT_HIT already names a `host`, written at
/// extraction by a parser that saw a literal authority.
fn arrived_with_host(node: &Node) -> bool {
    node.cells
        .iter()
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .any(|c| match &c.payload {
            CellPayload::Json(json) => {
                serde_json::from_str::<Fields>(json).is_ok_and(|f| f.str("host").is_some())
            }
            _ => false,
        })
}

/// A11.4: the authority of EVERY binding of the template's leading base, for
/// the `"hosts"` field. `environment.ts` and `environment.prod.ts` routinely
/// bind `environment.apiUrl` to different hosts, and the fold above used only
/// the first, so HTTP host narrowing has to see the whole set.
///
/// A binding with no authority (a relative `/api` base: same origin, service
/// unknown) contributes `""`, which the resolver reads as "do not narrow".
///
/// Empty, so no field is written, unless the bindings disagree. That keeps
/// every single-binding payload byte-identical. Also empty when the set would
/// not start with the `host` the fold recorded (the lenient last-segment
/// lookup resolved a different key), so `hosts[0]` is always `host`.
fn deployment_hosts(template: &str, consts: &ConstTable, host: Option<&str>) -> Vec<String> {
    let Some(expr) = template
        .strip_prefix("${")
        .and_then(|inner| inner.split_once('}'))
        .map(|(expr, _)| expr)
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for value in consts.candidates(expr) {
        let h = url_split(value)
            .0
            .filter(|h| !h.contains("${"))
            .unwrap_or_default();
        if !out.contains(&h) {
            out.push(h);
        }
    }
    let consistent = out.first().map(String::as_str) == Some(host.unwrap_or(""));
    if out.len() < 2 || !consistent {
        return Vec::new();
    }
    out
}

/// Point this file's edges at the new ids. Returns false, changing nothing,
/// when the entries of `old` split across several ids and the CALLS edges
/// cannot be paired with them one-to-one.
///
/// All entries go to one id: every edge touching `old` follows it. They
/// split, which only happens when two TypeScript call sites share a
/// placeholder path and only one of their bases resolves: the parser pushed
/// each call site's node and its CALLS edge in the same order, so the k-th
/// edge into `old` belongs to the k-th entry.
fn retarget_edges(fp: &mut FileParse, old: NodeId, targets: &[NodeId]) -> bool {
    let Some(&first) = targets.first() else {
        return false;
    };
    if targets.iter().all(|t| *t == first) {
        if first != old {
            for e in fp.edges.iter_mut() {
                if e.to == old {
                    e.to = first;
                }
                if e.from == old {
                    e.from = first;
                }
            }
        }
        return true;
    }
    if fp.edges.iter().any(|e| e.from == old) {
        return false;
    }
    let into: Vec<usize> = fp
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.to == old)
        .map(|(i, _)| i)
        .collect();
    if into.len() != targets.len() {
        return false;
    }
    for (i, t) in into.into_iter().zip(targets) {
        if let Some(e) = fp.edges.get_mut(i) {
            e.to = *t;
        }
    }
    true
}

/// Give every new id a nav entry, and retire `old` when no entry is left on it.
fn update_nav(
    nav: &mut CodeNav,
    old: NodeId,
    targets: &[NodeId],
    entries: &[(usize, Option<Plan>)],
) {
    let parent = nav.parent_of.get(&old).copied();
    let moved: Vec<&Plan> = entries
        .iter()
        .filter_map(|(_, p)| p.as_ref())
        .filter(|p| p.moved)
        .collect();
    let Some(first) = moved.first() else {
        return;
    };
    if !targets.contains(&old) {
        rewrite_node_id(nav, old, first.id);
    }
    for plan in moved {
        nav.name_by_id.insert(plan.id, plan.name.clone());
        nav.qname_by_id.insert(plan.id, plan.qname.clone());
        nav.kind_by_id.insert(plan.id, node_kind::ENDPOINT);
        if let Some(p) = parent
            && !nav.parent_of.contains_key(&plan.id)
        {
            nav.parent_of.insert(plan.id, p);
            nav.children_of.entry(p).or_default().push(plan.id);
        }
    }
}

/// A JSON object that keeps its key order through a rewrite. The workspace
/// builds serde_json without `preserve_order`, so a `serde_json::Value`
/// round trip would sort the keys of every folded payload. Unknown fields
/// survive either way.
struct Fields(Vec<(String, Value)>);

impl Fields {
    fn str(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_str())
    }

    /// Replace `key` in place, or append it.
    fn set(&mut self, key: &str, value: Value) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((key.to_string(), value)),
        }
    }
}

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Fields;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = map.next_entry::<String, Value>()? {
                    out.push(entry);
                }
                Ok(Fields(out))
            }
        }
        d.deserialize_map(ObjectVisitor)
    }
}

impl Serialize for Fields {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(k, v)| (k, v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::edge_category;
    use repo_graph_core::{Cell, Confidence, Edge};

    fn repo() -> RepoId {
        RepoId(7)
    }

    fn ep_id(method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            &format!("endpoint:{method}:{path}"),
        )
    }

    fn table() -> ConstTable {
        ConstTable::scan_file(
            "export const environment = {\n  apiUrl: 'http://users-service:8080',\n};\n",
            "typescript",
        )
    }

    fn func() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "src::svc::Svc::list")
    }

    /// One call site the way the TypeScript parser emits it: an ENDPOINT
    /// entry with one ENDPOINT_HIT cell, then a CALLS edge into it. The nav
    /// entry is recorded once per id, under `func()`, so the parent/children
    /// bookkeeping is exercised too.
    fn push_call(fp: &mut FileParse, path: &str, payload: &str) -> NodeId {
        let id = ep_id("GET", path);
        fp.nodes.push(Node {
            id,
            repo: repo(),
            confidence: Confidence::Medium,
            cells: vec![Cell {
                kind: cell_type::ENDPOINT_HIT,
                payload: CellPayload::Json(payload.to_string()),
            }],
        });
        fp.edges.push(Edge {
            from: func(),
            to: id,
            category: edge_category::CALLS,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        if !fp.nav.kind_by_id.contains_key(&id) {
            fp.nav.record(
                id,
                &format!("GET {path}"),
                &format!("endpoint:GET:{path}"),
                node_kind::ENDPOINT,
                Some(func()),
            );
        }
        id
    }

    fn file() -> FileParse {
        let mut fp = FileParse::default();
        fp.nodes.push(Node {
            id: func(),
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![],
        });
        fp.nav.record(
            func(),
            "list",
            "src::svc::Svc::list",
            node_kind::METHOD,
            None,
        );
        fp
    }

    fn payload(fp: &FileParse, idx: usize) -> &str {
        match &fp.nodes[idx].cells[0].payload {
            CellPayload::Json(s) => s,
            other => panic!("not json: {other:?}"),
        }
    }

    fn same(a: &FileParse, b: &FileParse) -> bool {
        a.nodes == b.nodes
            && a.edges == b.edges
            && a.nav.name_by_id == b.nav.name_by_id
            && a.nav.qname_by_id == b.nav.qname_by_id
            && a.nav.kind_by_id == b.nav.kind_by_id
            && a.nav.parent_of == b.nav.parent_of
            && a.nav.children_of == b.nav.children_of
    }

    /// The zero-change guarantee: a relative path, an unresolvable base and a
    /// relative hint are all left exactly as parsed.
    #[test]
    fn unfoldable_endpoints_are_left_byte_identical() {
        let mut fp = file();
        push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong"}"#,
        );
        push_call(
            &mut fp,
            "/users/${…}",
            r#"{"method":"GET","path":"/users/${…}","file":"a.ts","line":2,"col":1,"confidence":"medium","template":"/users/${id}"}"#,
        );
        push_call(
            &mut fp,
            "${…}/orders",
            r#"{"method":"GET","path":"${…}/orders","file":"a.ts","line":3,"col":1,"confidence":"medium","template":"${this.base}/orders"}"#,
        );
        push_call(
            &mut fp,
            "auth/login",
            r#"{"method":"GET","path":"auth/login","file":"a.ts","line":4,"col":1,"confidence":"weak"}"#,
        );
        // The query A3.3 already stripped is not a reason to touch the node.
        push_call(
            &mut fp,
            "/search",
            r#"{"method":"GET","path":"/search","file":"a.ts","line":5,"col":1,"confidence":"strong","raw":"/search?q=1"}"#,
        );
        let before = fp.clone();
        let stats = fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!(stats, FoldStats::default());
        assert!(same(&fp, &before), "unfoldable endpoints must not change");
    }

    #[test]
    fn resolvable_base_is_folded_and_its_host_recorded() {
        let mut fp = file();
        let old = push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        let stats = fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                hosts: 1,
                preset: 0
            }
        );

        let new = ep_id("GET", "/users");
        assert_eq!(fp.nodes[1].id, new);
        assert_eq!(
            payload(&fp, 1),
            r#"{"method":"GET","path":"/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${environment.apiUrl}/users","folded_from":"${…}/users","host":"users-service:8080"}"#,
            "key order kept, new fields appended"
        );
        assert_eq!(fp.edges[0].to, new);
        assert!(fp.edges.iter().all(|e| e.to != old));

        assert_eq!(
            fp.nav.qname_by_id.get(&new).map(String::as_str),
            Some("endpoint:GET:/users")
        );
        assert_eq!(
            fp.nav.name_by_id.get(&new).map(String::as_str),
            Some("GET /users")
        );
        assert_eq!(fp.nav.kind_by_id.get(&new), Some(&node_kind::ENDPOINT));
        assert_eq!(fp.nav.parent_of.get(&new), Some(&func()));
        assert_eq!(fp.nav.children_of.get(&func()), Some(&vec![new]));
        for gone in [
            fp.nav.name_by_id.contains_key(&old),
            fp.nav.qname_by_id.contains_key(&old),
            fp.nav.kind_by_id.contains_key(&old),
            fp.nav.parent_of.contains_key(&old),
        ] {
            assert!(!gone, "old id must leave the nav");
        }
    }

    /// A3.3 already took the host out of the path; the fold only reads it back
    /// off `raw`, so the id is untouched and the cell gains `host`.
    #[test]
    fn absolute_literal_keeps_its_id_and_gains_a_host() {
        let mut fp = file();
        let id = push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong","raw":"https://u:p@api.example.com/users?x=1"}"#,
        );
        // An interpolated authority is not a host.
        push_call(
            &mut fp,
            "/orders",
            r#"{"method":"GET","path":"/orders","file":"a.ts","line":2,"col":1,"confidence":"medium","raw":"https://${…}/orders","template":"https://${host}/orders"}"#,
        );
        let edges = fp.edges.clone();
        let stats = fold_endpoint_paths(&mut fp, &ConstTable::default(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 0,
                hosts: 1,
                preset: 0
            }
        );
        assert_eq!(fp.nodes[1].id, id);
        assert!(payload(&fp, 1).ends_with(
            r#""raw":"https://u:p@api.example.com/users?x=1","host":"api.example.com"}"#
        ));
        assert!(!payload(&fp, 1).contains("folded_from"));
        assert!(
            !payload(&fp, 2).contains(r#""host":"#),
            "{}",
            payload(&fp, 2)
        );
        assert_eq!(fp.edges, edges);
    }

    /// A11.5: a host the parser already wrote is counted as `preset` and left
    /// alone. A Go-shaped entry (no `raw`) and a Dart-shaped one (`raw` whose
    /// authority the pass would otherwise capture) both come out
    /// byte-identical, and neither is counted as captured.
    #[test]
    fn preset_hosts_are_counted_and_left_byte_identical() {
        let mut fp = file();
        push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"client.go","line":9,"col":15,"confidence":"strong","host":"api.example.com"}"#,
        );
        push_call(
            &mut fp,
            "/orders",
            r#"{"method":"POST","path":"/orders","file":"lib/api.dart","line":3,"col":5,"confidence":"strong","raw":"http://svc:8080/orders?x=1","host":"svc:8080"}"#,
        );
        // No host anywhere: neither counter moves.
        push_call(
            &mut fp,
            "/health",
            r#"{"method":"GET","path":"/health","file":"client.go","line":12,"col":3,"confidence":"strong"}"#,
        );
        let before = fp.clone();
        let stats = fold_endpoint_paths(&mut fp, &ConstTable::default(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 0,
                hosts: 0,
                preset: 2
            }
        );
        assert!(same(&fp, &before), "a pre-set host must not be re-recorded");
    }

    /// A11.4: a base bound differently per deployment records every binding's
    /// authority, first binding first and `""` for a relative one. A base
    /// bound once, or twice to the same host, records no `hosts` at all.
    #[test]
    fn per_deployment_bindings_record_the_host_set() {
        let mut consts = ConstTable::scan_file(
            "export const environment = { apiUrl: 'http://users-svc.prod.svc.cluster.local' };\n",
            "typescript",
        );
        for src in [
            "export const environment = { apiUrl: 'http://users-service:8080' };\n",
            "export const environment = { apiUrl: 'http://users-service:8080/v1' };\n",
            "export const environment = { apiUrl: '/api' };\n",
        ] {
            consts.merge_from(&ConstTable::scan_file(src, "typescript"));
        }
        let mut fp = file();
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","template":"${environment.apiUrl}/users"}"#,
        );
        let stats = fold_endpoint_paths(&mut fp, &consts, repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                hosts: 1,
                preset: 0
            }
        );
        assert_eq!(
            payload(&fp, 1),
            r#"{"method":"GET","path":"/users","template":"${environment.apiUrl}/users","folded_from":"${…}/users","host":"users-svc.prod.svc.cluster.local","hosts":["users-svc.prod.svc.cluster.local","users-service:8080",""]}"#
        );

        // One distinct host across two bindings: nothing new is written.
        let mut same = ConstTable::scan_file(
            "export const environment = { apiUrl: 'http://users-service:8080' };\n",
            "typescript",
        );
        same.merge_from(&ConstTable::scan_file(
            "export const environment = { apiUrl: 'http://users-service:8080/v2' };\n",
            "typescript",
        ));
        let mut fp = file();
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","template":"${environment.apiUrl}/users"}"#,
        );
        fold_endpoint_paths(&mut fp, &same, repo());
        assert!(!payload(&fp, 1).contains("hosts"), "{}", payload(&fp, 1));

        // A literal authority has no alternatives.
        assert!(deployment_hosts("https://a/x", &consts, Some("a")).is_empty());
        // The set must start with the host the fold recorded.
        assert!(deployment_hosts("${environment.apiUrl}/x", &consts, Some("other")).is_empty());
    }

    /// A11.4 end to end on the `xstack-host-pairing` fixture, through
    /// `generate_many`: two Go services serve the same paths, the client's
    /// base names `users-service`, and only the users repo declares it.
    #[test]
    fn xstack_host_pairing_fixture_pairs_only_with_the_named_service() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bench/substrate-gap/fixtures/xstack-host-pairing"
        );
        let r = crate::generate_many(&[
            format!("{root}/web"),
            format!("{root}/users"),
            format!("{root}/orders"),
        ])
        .expect("fixture builds");
        let m = &r.merged;
        let alias = m
            .node_id_by_qname("infra:service:users-service")
            .expect("compose alias");
        let users_repo = m
            .graphs
            .iter()
            .find(|g| g.nodes.iter().any(|n| n.id == alias))
            .map(|g| g.repo)
            .expect("alias has a repo");
        let route_repo: HashMap<NodeId, RepoId> = m
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter().map(move |n| (n.id, g.repo)))
            .collect();
        let calls: Vec<_> = m
            .cross_edges
            .iter()
            .filter(|e| e.category == repo_graph_code_domain::edge_category::HTTP_CALLS)
            .collect();
        // Before A11.4: 4 (each endpoint paired with both services).
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls.iter().all(|e| route_repo.get(&e.to) == Some(&users_repo)),
            "every HTTP_CALLS target must be a users-service route"
        );
        for q in ["endpoint:GET:/users", "endpoint:GET:/users/${…}"] {
            let ep = m.node_id_by_qname(q).expect(q);
            assert!(calls.iter().any(|e| e.from == ep), "{q} lost its pairing");
        }
    }

    /// Two call sites on one placeholder path, only one of whose bases
    /// resolves: each keeps its own CALLS edge.
    #[test]
    fn split_call_sites_keep_their_own_edges() {
        let mut fp = file();
        let old = push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":1,"col":1,"confidence":"medium","template":"${other.base}/users"}"#,
        );
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":2,"col":1,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        let stats = fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!(
            stats,
            FoldStats {
                folded: 1,
                hosts: 1,
                preset: 0
            }
        );
        let new = ep_id("GET", "/users");
        assert_eq!((fp.nodes[1].id, fp.nodes[2].id), (old, new));
        assert_eq!((fp.edges[0].to, fp.edges[1].to), (old, new));
        // Both ids are navigable, both under the same parent.
        assert!(fp.nav.qname_by_id.contains_key(&old));
        assert_eq!(fp.nav.parent_of.get(&new), Some(&func()));
        assert_eq!(fp.nav.children_of.get(&func()), Some(&vec![old, new]));
    }

    /// Two call sites already on `/users` and on the placeholder: the fold
    /// lands the second on the first's id without duplicating its nav entry.
    #[test]
    fn fold_onto_an_existing_id_merges_cleanly() {
        let mut fp = file();
        let existing = push_call(
            &mut fp,
            "/users",
            r#"{"method":"GET","path":"/users","file":"a.ts","line":1,"col":1,"confidence":"strong"}"#,
        );
        let old = push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":2,"col":1,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        fold_endpoint_paths(&mut fp, &table(), repo());
        assert_eq!((fp.nodes[1].id, fp.nodes[2].id), (existing, existing));
        assert!(fp.edges.iter().all(|e| e.to == existing));
        assert_eq!(fp.nav.children_of.get(&func()), Some(&vec![existing]));
        assert!(!fp.nav.qname_by_id.contains_key(&old));
    }

    /// End to end on the `angular-base-url` fixture, through `generate_many`,
    /// the path grade.py takes. The base URL is bound in ANOTHER file, and the
    /// folded endpoint pairs with the Go route across the repo boundary.
    #[test]
    fn angular_base_url_fixture_folds_and_pairs_across_repos() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bench/substrate-gap/fixtures/angular-base-url"
        );
        let r = crate::generate_many(&[format!("{root}/client"), format!("{root}/server")])
            .expect("fixture builds");
        let m = &r.merged;
        let ep = m
            .node_id_by_qname("endpoint:GET:/users")
            .expect("folded endpoint");
        assert!(m.node_id_by_qname("endpoint:GET:${…}/users").is_none());
        // LB.11a: the Go route is one node per (method, path).
        let route = m.node_id_by_qname("GET /users").expect("go route");
        assert!(
            m.cross_edges.iter().any(|e| e.from == ep
                && e.to == route
                && e.category == repo_graph_code_domain::edge_category::HTTP_CALLS),
            "HTTP_CALLS endpoint:GET:/users -> GET /users"
        );
        let payloads: Vec<&str> = m
            .graphs
            .iter()
            .flat_map(|g| &g.nodes)
            .filter(|n| n.id == ep)
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(payloads.len(), 1, "{payloads:?}");
        let p = payloads[0];
        for want in [
            r#""path":"/users""#,
            r#""template":"${environment.apiUrl}/users""#,
            r#""folded_from":"${…}/users""#,
            r#""host":"users-service:8080""#,
        ] {
            assert!(p.contains(want), "{want} missing from {p}");
        }
    }

    /// A parse served from the cache is folded too, and the cache keeps the
    /// PRE-fold parse, so an edit to the constant's file re-folds on the next
    /// build instead of replaying a fold made against the old value.
    #[test]
    fn cached_parses_are_folded_and_the_cache_stays_pre_fold() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bench/substrate-gap/fixtures/angular-base-url/client"
        );
        let mut cache = crate::ParseCache::new();
        let cold = crate::generate_one_with_cache(root, &mut cache).expect("cold build");
        let warm = crate::generate_one_with_cache(root, &mut cache).expect("warm build");
        assert_eq!(cache.stats.reparsed, 0, "{:?}", cache.stats);
        assert!(cache.stats.reused > 0, "{:?}", cache.stats);
        for r in [&cold, &warm] {
            assert!(r.merged.node_id_by_qname("endpoint:GET:/users").is_some());
            assert!(
                r.merged
                    .node_id_by_qname("endpoint:GET:${…}/users")
                    .is_none()
            );
        }

        let src = std::fs::read_to_string(format!("{root}/users.service.ts")).expect("source");
        let cached = cache
            .get(
                "users.service.ts",
                crate::cache::content_hash(&src),
                "typescript",
            )
            .expect("users.service.ts is cached");
        let qnames: Vec<&String> = cached.nav.qname_by_id.values().collect();
        assert!(
            qnames.iter().any(|q| *q == "endpoint:GET:${…}/users"),
            "cache must hold the parser's own identity: {qnames:?}"
        );
        assert!(!qnames.iter().any(|q| *q == "endpoint:GET:/users"));
    }

    /// LF.2d: a base the source cannot bind (`process.env`), pinned by an
    /// overlay constant, folds; the entry records which pin it read and
    /// becomes Weak. The same template against the source-only table is left
    /// alone, and a source-folded entry is not marked.
    #[test]
    fn pinned_base_folds_marks_overlay_and_weakens() {
        let json = r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${GATEWAY}/users"}"#;
        let mut consts = ConstTable::scan_file("const GATEWAY = process.env.GATEWAY_URL;\n", "typescript");
        assert!(consts.get("GATEWAY").is_none(), "an env read never binds");

        let mut fp = file();
        push_call(&mut fp, "${…}/users", json);
        let before = fp.clone();
        assert_eq!(fold_endpoint_paths(&mut fp, &consts, repo()), FoldStats::default());
        assert!(same(&fp, &before));

        assert!(consts.pin("GATEWAY", "/orders-svc"));
        let stats = fold_endpoint_paths(&mut fp, &consts, repo());
        assert_eq!(stats.folded, 1);
        let new = ep_id("GET", "/orders-svc/users");
        assert_eq!(fp.nodes[1].id, new);
        assert_eq!(fp.nodes[1].confidence, Confidence::Weak);
        assert_eq!(fp.nav.qname_by_id.get(&new).map(String::as_str), Some("endpoint:GET:/orders-svc/users"));
        let v: Value = serde_json::from_str(payload(&fp, 1)).unwrap();
        assert_eq!(v["overlay"], "const:GATEWAY");
        assert_eq!(v["path"], "/orders-svc/users");
        assert_eq!(v["folded_from"], "${…}/users");
        assert!(fp.edges.iter().any(|e| e.to == new), "the CALLS edge follows the node");

        // A source binding folds without the overlay mark or a confidence change.
        let mut fp = file();
        push_call(
            &mut fp,
            "${…}/users",
            r#"{"method":"GET","path":"${…}/users","file":"a.ts","line":9,"col":5,"confidence":"medium","template":"${environment.apiUrl}/users"}"#,
        );
        assert_eq!(fold_endpoint_paths(&mut fp, &consts_with_source(), repo()).folded, 1);
        assert_eq!(fp.nodes[1].confidence, Confidence::Medium);
        assert!(!payload(&fp, 1).contains("overlay"));
    }

    /// The source table plus an unrelated pin: a pin the template never reads
    /// marks nothing.
    fn consts_with_source() -> ConstTable {
        let mut t = table();
        assert!(t.pin("GATEWAY", "/orders-svc"));
        t
    }
}
