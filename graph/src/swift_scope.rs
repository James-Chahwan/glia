//! Swift call scope (CB.18): implicit `self` and same-module static calls. A
//! bare call inside a type binds the type's own member first, across its
//! extensions in other files, and a static call or construction on a type
//! declared elsewhere in the same module binds that type. Crate-private:
//! [`crate::build::build_swift`] runs [`implicit_self`] before
//! `resolve_calls` and consults [`SwiftModuleTypes::resolve`] from its
//! extra-hook.
//!
//! A Swift module (target) is one directory to the parser (LB.7c): a
//! top-level type is scoped to it (`Sources::App::Cart`), so `class Cart` and
//! every `extension Cart` of the directory are one CLASS node. An extension of
//! a STRUCT / ENUM / protocol declared in ANOTHER file is a second node (the
//! parser only sees its own file's declarations, so it stays CLASS) with the
//! same qname; the two form one type group here, and a member is looked up
//! across the group. A member METHOD's id is its qname, so the same member
//! reached through either node is one id.
//!
//! fired_on marker, once per Swift graph:
//! `[swift-scope] implicit_self=<n> same_module_static=<n> same_module_type=<n> extension_member=<n>`
//! — grep `^\[swift-scope\]`.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CallQualifier, CallSite, cell_type, node_kind};
use glia_core::{CellPayload, Node, NodeId};

use crate::calls::{enclosing_class_or_struct, enclosing_module, graph_evidence};
use crate::types::RepoGraph;

/// The EVIDENCE emitter of every edge the hook binds.
const EMITTER: &str = "graph:swift_scope";

/// The module directory of a Swift qname: the qname minus its last segment,
/// the parser's `type_scope` (`Sources::App::Pricing` -> `Sources::App`, a
/// repo-root `Widget` -> `""`).
fn module_dir(qname: &str) -> &str {
    qname.rsplit_once("::").map_or("", |(dir, _)| dir)
}

/// Every top-level node named one type in one module directory.
#[derive(Default)]
struct TypeGroup {
    /// The group's nodes in `g.nodes` order: the declaration and / or the
    /// CLASS node(s) of its extensions in other files.
    nodes: Vec<NodeId>,
    /// The node a construction `T(...)` binds: the one CLASS / STRUCT / ENUM
    /// of the group that DECLARES the type ([`declares_type`]). `None` when
    /// the group only extends a type from outside the repo (`extension
    /// String`) or declares it twice (not valid Swift).
    ctor: Option<NodeId>,
}

/// The top-level types of a Swift graph by module directory and name, and
/// the counts of what [`SwiftModuleTypes::resolve`] bound.
pub(crate) struct SwiftModuleTypes {
    /// module dir -> type name -> group. BTreeMaps: iteration never decides
    /// a binding, and lookups take `&str`.
    by_dir: BTreeMap<String, BTreeMap<String, TypeGroup>>,
    /// Type node -> its `(dir, name)`, for a member lookup from inside the
    /// type. Lookup only.
    group_of: HashMap<NodeId, (String, String)>,
    same_module_static: Cell<usize>,
    same_module_type: Cell<usize>,
    extension_member: Cell<usize>,
}

impl SwiftModuleTypes {
    /// Group every top-level CLASS / STRUCT / ENUM / INTERFACE of `g`: a node
    /// whose nav parent is a MODULE of the directory its qname is scoped to.
    /// A nested type (parent a type) and a private / fileprivate one (its
    /// qname keeps the file segment, so its "directory" is its file) are in
    /// no group: they are not visible module-wide.
    pub(crate) fn new(g: &RepoGraph) -> Self {
        let nav = &g.nav;
        let mut by_dir: BTreeMap<String, BTreeMap<String, TypeGroup>> = BTreeMap::new();
        let mut group_of: HashMap<NodeId, (String, String)> = HashMap::new();
        let mut declared: HashSet<NodeId> = HashSet::new();
        for n in &g.nodes {
            let Some(&kind) = nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if !is_type_kind(kind) || group_of.contains_key(&n.id) {
                continue;
            }
            let (Some(qname), Some(name), Some(parent)) = (
                nav.qname_by_id.get(&n.id),
                nav.name_by_id.get(&n.id),
                nav.parent_of.get(&n.id),
            ) else {
                continue;
            };
            let Some(parent_qname) = nav
                .qname_by_id
                .get(parent)
                .filter(|_| nav.kind_by_id.get(parent) == Some(&node_kind::MODULE))
            else {
                continue;
            };
            let dir = module_dir(qname);
            if dir != module_dir(parent_qname) {
                continue;
            }
            if kind != node_kind::INTERFACE && (kind != node_kind::CLASS || declares_type(n)) {
                declared.insert(n.id);
            }
            by_dir
                .entry(dir.to_string())
                .or_default()
                .entry(name.clone())
                .or_default()
                .nodes
                .push(n.id);
            group_of.insert(n.id, (dir.to_string(), name.clone()));
        }
        for group in by_dir.values_mut().flat_map(BTreeMap::values_mut) {
            let mut ctors = group.nodes.iter().filter(|id| declared.contains(id));
            group.ctor = match (ctors.next(), ctors.next()) {
                (Some(&only), None) => Some(only),
                _ => None,
            };
        }
        SwiftModuleTypes {
            by_dir,
            group_of,
            same_module_static: Cell::new(0),
            same_module_type: Cell::new(0),
            extension_member: Cell::new(0),
        }
    }

    fn group(&self, dir: &str, name: &str) -> Option<&TypeGroup> {
        self.by_dir.get(dir)?.get(name)
    }

    /// The group of the type node `owner`.
    fn group_of_type(&self, owner: NodeId) -> Option<&TypeGroup> {
        let (dir, name) = self.group_of.get(&owner)?;
        self.group(dir, name)
    }

    /// The member `name` of a group, over every node of it (`class_methods`,
    /// or `interface_methods` for a protocol): one distinct METHOD, else
    /// `None`, never a first match.
    fn member(g: &RepoGraph, group: &TypeGroup, name: &str) -> Option<NodeId> {
        let mut hit: Option<NodeId> = None;
        for id in &group.nodes {
            let found = g
                .symbols
                .class_methods
                .get(id)
                .or_else(|| g.symbols.interface_methods.get(id))
                .and_then(|m| m.get(name).copied());
            match (hit, found) {
                (Some(h), Some(f)) if h != f => return None,
                (None, Some(f)) => hit = Some(f),
                _ => {}
            }
        }
        hit
    }

    /// The member `name` of the type `from` sits in, found through a node of
    /// its group other than the one `resolve_calls` reads (an extension in
    /// another file of a type declared as a STRUCT / ENUM, or the reverse).
    fn extension_member_of(&self, g: &RepoGraph, from: NodeId, name: &str) -> Option<NodeId> {
        let owner = enclosing_class_or_struct(&g.nav, from)?;
        Self::member(g, self.group_of_type(owner)?, name)
    }

    /// `resolve_calls`' extra-hook, consulted after every generic lookup
    /// missed. The caller's module directory is its file MODULE's.
    /// - `SelfMethod(m)` (`self.m()`, or a bare call [`implicit_self`]
    ///   rewrote): the member `m` of the caller's type group — rule
    ///   `extension_member`.
    /// - `Attribute { base: T, name: m }`, `T` a type of the caller's module
    ///   directory: its member `m` (a static member is a METHOD too) — rule
    ///   `same_module_static`.
    /// - `Bare(T)`, `T` a type of the caller's module directory that the
    ///   module declares: the type (a construction) — rule `same_module_type`.
    pub(crate) fn resolve(&self, g: &RepoGraph, site: &CallSite) -> Option<(NodeId, Evidence)> {
        let (to, rule, count) = match &site.qualifier {
            CallQualifier::SelfMethod(name) => (
                self.extension_member_of(g, site.from, name)?,
                "extension_member",
                &self.extension_member,
            ),
            CallQualifier::Attribute { base, name } => {
                let group = self.group(self.caller_dir(g, site.from)?, base)?;
                (
                    Self::member(g, group, name)?,
                    "same_module_static",
                    &self.same_module_static,
                )
            }
            CallQualifier::Bare(name) => {
                let group = self.group(self.caller_dir(g, site.from)?, name)?;
                (group.ctor?, "same_module_type", &self.same_module_type)
            }
            CallQualifier::SuperMethod(_) | CallQualifier::ComplexReceiver { .. } => return None,
        };
        count.set(count.get() + 1);
        Some((to, graph_evidence(EMITTER, rule)))
    }

    /// The module directory of the file `from` is in.
    fn caller_dir<'g>(&self, g: &'g RepoGraph, from: NodeId) -> Option<&'g str> {
        let module = enclosing_module(&g.nav, from)?;
        g.nav.qname_by_id.get(&module).map(|q| module_dir(q))
    }

    /// The `[swift-scope]` fired_on line, `implicit_self` being
    /// [`implicit_self`]'s count.
    pub(crate) fn marker(&self, implicit_self: usize) -> String {
        format!(
            "[swift-scope] implicit_self={implicit_self} same_module_static={} same_module_type={} extension_member={}",
            self.same_module_static.get(),
            self.same_module_type.get(),
            self.extension_member.get()
        )
    }
}

/// The kinds a Swift type declaration or extension is minted as.
fn is_type_kind(kind: glia_core::NodeKindId) -> bool {
    kind == node_kind::CLASS
        || kind == node_kind::STRUCT
        || kind == node_kind::ENUM
        || kind == node_kind::INTERFACE
}

/// True when one of `n`'s CODE cells declares the type rather than extends
/// it: its first declaration keyword outside parentheses and string
/// literals is `class` / `struct` / `enum` / `actor` / `protocol`, not
/// `extension`. Only a CLASS needs it: the parser mints an extension of a
/// type declared in another file (or outside the repo) as CLASS, and folds a
/// same-file extension onto the declaration, whose CODE comes first. No CODE
/// cell: not known to declare.
fn declares_type(n: &Node) -> bool {
    n.cells.iter().any(|c| match &c.payload {
        CellPayload::Text(code) if c.kind == cell_type::CODE => {
            matches!(decl_keyword(code), Some(k) if k != "extension")
        }
        _ => false,
    })
}

/// The first of `class` / `struct` / `enum` / `actor` / `protocol` /
/// `extension` written as a whole word at parenthesis depth 0 and outside a
/// string literal (an attribute's arguments, `@available(*, message: "...")`,
/// are skipped).
fn decl_keyword(code: &str) -> Option<&str> {
    const KEYWORDS: &[&str] = &["class", "struct", "enum", "actor", "protocol", "extension"];
    let (mut depth, mut in_string) = (0usize, false);
    let mut word_start: Option<usize> = None;
    for (i, c) in code
        .char_indices()
        .chain(std::iter::once((code.len(), ' ')))
    {
        let word_char = c.is_alphanumeric() || c == '_';
        if word_char && !in_string && depth == 0 {
            word_start.get_or_insert(i);
            continue;
        }
        if let Some(start) = word_start.take()
            && let Some(k) = KEYWORDS.iter().find(|k| **k == &code[start..i])
        {
            return Some(*k);
        }
        match c {
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth = depth.saturating_sub(1),
            '{' if !in_string && depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// Rewrite every `Bare(name)` call whose caller sits in a CLASS / STRUCT /
/// ENUM owning a member `name` ([`enclosing_class_or_struct`]; its own
/// `class_methods`, or another node of its type group) into
/// `SelfMethod(name)`, in place: inside a type an unqualified name is the
/// type's member before any free function (Swift's implicit `self`), where
/// the generic pass would bind the caller file's same-named free function.
/// `resolve_calls` then binds an own member through its SelfMethod branch
/// (rule `self_method`), and a member of another node of the group through
/// [`SwiftModuleTypes::resolve`] (rule `extension_member`). Returns the
/// number rewritten. Order-preserving; nothing else moves.
pub(crate) fn implicit_self(
    g: &RepoGraph,
    types: &SwiftModuleTypes,
    calls: &mut [CallSite],
) -> usize {
    let mut rewritten = 0usize;
    for site in calls.iter_mut() {
        let CallQualifier::Bare(name) = &site.qualifier else {
            continue;
        };
        let Some(owner) = enclosing_class_or_struct(&g.nav, site.from) else {
            continue;
        };
        let own = g
            .symbols
            .class_methods
            .get(&owner)
            .is_some_and(|m| m.contains_key(name));
        if own || types.extension_member_of(g, site.from, name).is_some() {
            site.qualifier = CallQualifier::SelfMethod(name.clone());
            rewritten += 1;
        }
    }
    rewritten
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use glia_code_domain::{CodeNav, FileParse, GRAPH_TYPE, edge_category};
    use glia_core::{Cell, Confidence, Edge, NodeKindId};

    use super::*;
    use crate::build::build_swift;
    use crate::test_support::repo;

    fn gid(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    /// One item of a synthetic Swift file: its kind, qname, the qname of its
    /// parent in this file (`None`: the file MODULE) and its CODE text.
    type Item<'a> = (NodeKindId, &'a str, Option<&'a str>, &'a str);

    /// A Swift file shaped the way the parser emits it: the MODULE `module`
    /// (`Sources::App::Pricing`), each item with its DEFINES edge and CODE
    /// cell, and `calls` as `(caller qname, qualifier)`.
    fn swift_file(module: &str, items: &[Item], calls: Vec<(&str, CallQualifier)>) -> FileParse {
        let r = repo();
        let m = gid(node_kind::MODULE, module);
        let mut nav = CodeNav::default();
        nav.record(
            m,
            module.rsplit("::").next().unwrap_or(module),
            module,
            node_kind::MODULE,
            None,
        );
        let mut ids: HashMap<&str, NodeId> = HashMap::new();
        let mut nodes = vec![Node {
            id: m,
            repo: r,
            confidence: Confidence::Strong,
            cells: vec![],
        }];
        let mut edges = vec![];
        for &(kind, qname, parent, code) in items {
            let id = gid(kind, qname);
            let parent_id = parent.map_or(m, |p| ids[p]);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            nav.record(id, name, qname, kind, Some(parent_id));
            let cells = vec![Cell {
                kind: cell_type::CODE,
                payload: CellPayload::Text(code.into()),
            }];
            nodes.push(Node {
                id,
                repo: r,
                confidence: Confidence::Strong,
                cells,
            });
            edges.push(Edge {
                from: parent_id,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            ids.insert(qname, id);
        }
        let calls = calls
            .into_iter()
            .map(|(from, qualifier)| CallSite {
                from: ids[from],
                qualifier,
                line: 3,
            })
            .collect();
        FileParse {
            nodes,
            edges,
            imports: vec![],
            calls,
            refs: vec![],
            nav,
            properties: HashSet::new(),
        }
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.into())
    }

    fn build(files: Vec<FileParse>) -> RepoGraph {
        build_swift(repo(), files, |_, _| None).expect("builds")
    }

    /// The CALLS edges' targets out of `from` (a qname of `kind`), with the
    /// rule of each edge's EVIDENCE.
    fn calls_from(g: &RepoGraph, kind: NodeKindId, from: &str) -> Vec<(String, String)> {
        let from = gid(kind, from);
        g.edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::CALLS)
            .map(|e| {
                let to = g.nav.qname_by_id.get(&e.to).cloned().unwrap_or_default();
                let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
                (to, rule)
            })
            .collect()
    }

    /// The swift-implicit-self fixture's Pricing.swift: a free `helper()`
    /// beside `class Cart` whose `helper()` member and `checkout()` share it.
    fn pricing(calls: Vec<(&str, CallQualifier)>) -> FileParse {
        swift_file(
            "Sources::App::Pricing",
            &[
                (
                    node_kind::FUNCTION,
                    "Sources::App::Pricing::helper",
                    None,
                    "func helper() -> Int",
                ),
                (
                    node_kind::CLASS,
                    "Sources::App::Cart",
                    None,
                    "final class Cart {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Cart::helper",
                    Some("Sources::App::Cart"),
                    "func helper()",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Cart::checkout",
                    Some("Sources::App::Cart"),
                    "func checkout()",
                ),
            ],
            calls,
        )
    }

    #[test]
    fn member_beats_free_function() {
        let g = build(vec![pricing(vec![(
            "Sources::App::Cart::checkout",
            bare("helper"),
        )])]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "Sources::App::Cart::checkout"),
            vec![(
                "Sources::App::Cart::helper".to_string(),
                "self_method".to_string()
            )]
        );
    }

    #[test]
    fn free_function_still_binds_from_a_free_function() {
        let mut file = pricing(vec![]);
        let main = gid(node_kind::FUNCTION, "Sources::App::Pricing::helper");
        file.calls.push(CallSite {
            from: main,
            qualifier: bare("helper"),
            line: 0,
        });
        let g = build(vec![file]);
        assert_eq!(
            calls_from(&g, node_kind::FUNCTION, "Sources::App::Pricing::helper"),
            vec![(
                "Sources::App::Pricing::helper".to_string(),
                "module_symbol".to_string()
            )]
        );
    }

    #[test]
    fn extension_member_in_another_file() {
        let ext = swift_file(
            "Sources::App::Cart+Tax",
            &[
                (
                    node_kind::CLASS,
                    "Sources::App::Cart",
                    None,
                    "extension Cart {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Cart::withTax",
                    Some("Sources::App::Cart"),
                    "func withTax()",
                ),
            ],
            vec![("Sources::App::Cart::withTax", bare("helper"))],
        );
        let g = build(vec![ext, pricing(vec![])]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "Sources::App::Cart::withTax"),
            vec![(
                "Sources::App::Cart::helper".to_string(),
                "self_method".to_string()
            )]
        );
    }

    /// A STRUCT's extension in another file is a second (CLASS) node of the
    /// same qname: its bare call and its `self.` call reach the struct's
    /// member through the type group.
    #[test]
    fn struct_extension_in_another_file_reaches_the_structs_member() {
        let part = swift_file(
            "Sources::App::Part",
            &[
                (
                    node_kind::STRUCT,
                    "Sources::App::Part",
                    None,
                    "struct Part {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Part::base",
                    Some("Sources::App::Part"),
                    "func base()",
                ),
            ],
            vec![],
        );
        let ext = swift_file(
            "Sources::App::Part+Weight",
            &[
                (
                    node_kind::FUNCTION,
                    "Sources::App::Part+Weight::base",
                    None,
                    "func base() -> Int",
                ),
                (
                    node_kind::CLASS,
                    "Sources::App::Part",
                    None,
                    "extension Part {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Part::weight",
                    Some("Sources::App::Part"),
                    "func weight()",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Part::heavy",
                    Some("Sources::App::Part"),
                    "func heavy()",
                ),
            ],
            vec![
                ("Sources::App::Part::weight", bare("base")),
                (
                    "Sources::App::Part::heavy",
                    CallQualifier::SelfMethod("base".into()),
                ),
            ],
        );
        let g = build(vec![part, ext]);
        for from in ["Sources::App::Part::weight", "Sources::App::Part::heavy"] {
            assert_eq!(
                calls_from(&g, node_kind::METHOD, from),
                vec![(
                    "Sources::App::Part::base".to_string(),
                    "extension_member".to_string()
                )],
                "{from}: the struct's member, never the caller file's free `base`"
            );
        }
    }

    #[test]
    fn static_call_on_same_module_type() {
        let formatter = swift_file(
            "Sources::App::Formatter",
            &[
                (
                    node_kind::STRUCT,
                    "Sources::App::Formatter",
                    None,
                    "struct Formatter {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Formatter::money",
                    Some("Sources::App::Formatter"),
                    "static func money(_ x: Int)",
                ),
            ],
            vec![],
        );
        let money = CallQualifier::Attribute {
            base: "Formatter".into(),
            name: "money".into(),
        };
        let g = build(vec![
            formatter,
            pricing(vec![("Sources::App::Cart::checkout", money)]),
        ]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "Sources::App::Cart::checkout"),
            vec![(
                "Sources::App::Formatter::money".to_string(),
                "same_module_static".to_string()
            )]
        );
    }

    #[test]
    fn construction_on_same_module_type() {
        let formatter = swift_file(
            "Sources::App::Formatter",
            &[(
                node_kind::STRUCT,
                "Sources::App::Formatter",
                None,
                "struct Formatter {",
            )],
            vec![],
        );
        let g = build(vec![
            formatter,
            pricing(vec![("Sources::App::Cart::checkout", bare("Formatter"))]),
        ]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "Sources::App::Cart::checkout"),
            vec![(
                "Sources::App::Formatter".to_string(),
                "same_module_type".to_string()
            )]
        );
    }

    /// `extension String` names a type the module does not declare: a
    /// `String(...)` call in another file constructs the standard library's
    /// String, never the extension node. A static member the extension adds
    /// still binds.
    #[test]
    fn extension_of_an_outside_type_is_not_a_construction() {
        let ext = swift_file(
            "Sources::App::String+Money",
            &[
                (
                    node_kind::CLASS,
                    "Sources::App::String",
                    None,
                    "public extension String {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::String::money",
                    Some("Sources::App::String"),
                    "static func money()",
                ),
            ],
            vec![],
        );
        let money = CallQualifier::Attribute {
            base: "String".into(),
            name: "money".into(),
        };
        let g = build(vec![
            ext,
            pricing(vec![
                ("Sources::App::Cart::checkout", bare("String")),
                ("Sources::App::Cart::checkout", money),
            ]),
        ]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "Sources::App::Cart::checkout"),
            vec![(
                "Sources::App::String::money".to_string(),
                "same_module_static".to_string()
            )]
        );
    }

    #[test]
    fn other_module_type_not_bound() {
        let other = swift_file(
            "Sources::Other::Formatter",
            &[
                (
                    node_kind::STRUCT,
                    "Sources::Other::Formatter",
                    None,
                    "struct Formatter {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::Other::Formatter::money",
                    Some("Sources::Other::Formatter"),
                    "static func money(_ x: Int)",
                ),
            ],
            vec![],
        );
        let money = CallQualifier::Attribute {
            base: "Formatter".into(),
            name: "money".into(),
        };
        let g = build(vec![
            other,
            pricing(vec![
                ("Sources::App::Cart::checkout", money),
                ("Sources::App::Cart::checkout", bare("Formatter")),
            ]),
        ]);
        assert!(calls_from(&g, node_kind::METHOD, "Sources::App::Cart::checkout").is_empty());
    }

    /// A private type keeps its file segment (LB.7c) and a nested type hangs
    /// off its outer type: neither is visible module-wide.
    #[test]
    fn private_and_nested_types_are_not_module_types() {
        let boxes = swift_file(
            "Sources::App::Boxes",
            &[
                (
                    node_kind::STRUCT,
                    "Sources::App::Boxes::Box",
                    None,
                    "private struct Box {",
                ),
                (
                    node_kind::STRUCT,
                    "Sources::App::Crate",
                    None,
                    "struct Crate {",
                ),
                (
                    node_kind::STRUCT,
                    "Sources::App::Crate::Lid",
                    Some("Sources::App::Crate"),
                    "struct Lid {",
                ),
            ],
            vec![],
        );
        let g = build(vec![
            boxes,
            pricing(vec![
                ("Sources::App::Cart::checkout", bare("Box")),
                ("Sources::App::Cart::checkout", bare("Lid")),
            ]),
        ]);
        assert!(calls_from(&g, node_kind::METHOD, "Sources::App::Cart::checkout").is_empty());
    }

    #[test]
    fn marker_counts_each_path() {
        let formatter = swift_file(
            "Sources::App::Formatter",
            &[
                (
                    node_kind::STRUCT,
                    "Sources::App::Formatter",
                    None,
                    "struct Formatter {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Formatter::money",
                    Some("Sources::App::Formatter"),
                    "static func money(_ x: Int)",
                ),
            ],
            vec![],
        );
        let money = CallQualifier::Attribute {
            base: "Formatter".into(),
            name: "money".into(),
        };
        let mut all_calls = vec![];
        let mut files = vec![formatter, pricing(vec![])];
        for f in &mut files {
            all_calls.append(&mut f.calls);
        }
        let g = build(files);
        let checkout = gid(node_kind::METHOD, "Sources::App::Cart::checkout");
        all_calls.extend([
            CallSite {
                from: checkout,
                qualifier: bare("helper"),
                line: 0,
            },
            CallSite {
                from: checkout,
                qualifier: money,
                line: 1,
            },
            CallSite {
                from: checkout,
                qualifier: bare("Formatter"),
                line: 2,
            },
        ]);
        let types = SwiftModuleTypes::new(&g);
        let n = implicit_self(&g, &types, &mut all_calls);
        assert_eq!(
            all_calls[0].qualifier,
            CallQualifier::SelfMethod("helper".into())
        );
        for site in &all_calls[1..] {
            assert!(types.resolve(&g, site).is_some(), "{site:?}");
        }
        assert_eq!(
            types.marker(n),
            "[swift-scope] implicit_self=1 same_module_static=1 same_module_type=1 extension_member=0"
        );
    }

    #[test]
    fn decl_keyword_reads_past_attributes_and_modifiers() {
        assert_eq!(decl_keyword("final class Cart {"), Some("class"));
        assert_eq!(
            decl_keyword("@MainActor public struct Box<T> {"),
            Some("struct")
        );
        assert_eq!(decl_keyword("public extension String {"), Some("extension"));
        assert_eq!(
            decl_keyword(
                "@available(*, deprecated, message: \"use the new class\") extension Cart {"
            ),
            Some("extension")
        );
        assert_eq!(decl_keyword("{ class }"), None);
    }
}
