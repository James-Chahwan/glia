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
//! A receiver the parser typed as a local of the caller (DF.3: a parameter,
//! `let s = Store()`, `let t: Store`, `if let o = ..`) binds its member on
//! that type in the caller's own module, so two targets that both declare
//! `Store` never meet.
//!
//! GD.15: a protocol's function requirements are METHODs of its INTERFACE
//! node, so a receiver typed by a protocol binds the requirement (and,
//! through method-level IMPLEMENTS, every conformance's method). A receiver
//! the parser typed `any P` / `some P` (a parameter, a local, a stored
//! property) keeps the keyword, which keeps the repo-wide generic receiver
//! pass off it: the hook binds it in the caller's own module. A call chain
//! (`makeStore().load()`, `let s = Store.shared.make()` then `s.load()`)
//! types through each callee's recorded return type ([`MAX_CHAIN`] hops).
//!
//! fired_on marker, once per Swift graph:
//! `[swift-scope] implicit_self=<n> same_module_static=<n> same_module_type=<n> extension_member=<n> local_receiver=<n> protocol_receivers=<p> chain_receivers=<c> existentials=<e>`
//! — grep `^\[swift-scope\]`. `protocol_receivers` counts the receiver
//! calls the hook bound on a protocol's type group, `chain_receivers` those
//! typed through a return type, `existentials` those whose type was read
//! through an `any` / `some`.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CallQualifier, CallSite, cell_type, node_kind};
use glia_core::{CellPayload, Node, NodeId};

use crate::calls::{enclosing_class_or_struct, enclosing_module, graph_evidence, receiver_type};
use crate::types::RepoGraph;

/// The EVIDENCE emitter of every edge the hook binds.
const EMITTER: &str = "graph:swift_scope";

/// GD.15: the most call / member hops a receiver chain is typed through
/// (a local holding a chain whose head is another such local counts each).
const MAX_CHAIN: usize = 8;

/// GD.15: a receiver type text with its `any ` / `some ` keyword stripped,
/// and whether it had one (`"any Store"` -> `("Store", true)`).
fn unwrap_existential(ty: &str) -> (&str, bool) {
    match ty.strip_prefix("any ").or_else(|| ty.strip_prefix("some ")) {
        Some(t) => (t.trim(), true),
        None => (ty, false),
    }
}

/// GD.15: the type group a receiver was typed to, with how: through an
/// `any` / `some` (`existential`) and / or a callee's return type
/// (`chained`).
#[derive(Clone, Copy)]
struct Reached<'s> {
    group: &'s TypeGroup,
    existential: bool,
    chained: bool,
}

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
    /// GD.15: module dir -> free function name -> the one FUNCTION of the
    /// directory's files with that name (`None`: declared twice), a chain
    /// head `makeStore()`'s callee.
    functions: BTreeMap<String, BTreeMap<String, Option<NodeId>>>,
    protocol_receivers: Cell<usize>,
    chain_receivers: Cell<usize>,
    existentials: Cell<usize>,
    same_module_static: Cell<usize>,
    same_module_type: Cell<usize>,
    extension_member: Cell<usize>,
    local_receiver: Cell<usize>,
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
        let mut functions: BTreeMap<String, BTreeMap<String, Option<NodeId>>> = BTreeMap::new();
        for n in &g.nodes {
            let Some(&kind) = nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if kind == node_kind::FUNCTION
                && let (Some(name), Some(parent)) =
                    (nav.name_by_id.get(&n.id), nav.parent_of.get(&n.id))
                && nav.kind_by_id.get(parent) == Some(&node_kind::MODULE)
                && let Some(module) = nav.qname_by_id.get(parent)
            {
                functions
                    .entry(module_dir(module).to_string())
                    .or_default()
                    .entry(name.clone())
                    .and_modify(|f| {
                        if *f != Some(n.id) {
                            *f = None;
                        }
                    })
                    .or_insert(Some(n.id));
                continue;
            }
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
            functions,
            protocol_receivers: Cell::new(0),
            chain_receivers: Cell::new(0),
            existentials: Cell::new(0),
            same_module_static: Cell::new(0),
            same_module_type: Cell::new(0),
            extension_member: Cell::new(0),
            local_receiver: Cell::new(0),
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
    /// - `Attribute { base: x, name: m }`, `x` no type of the caller's module
    ///   directory but a local of the caller the parser typed `T` (DF.3,
    ///   [`receiver_type`]: a parameter, an annotation, a construction, or a
    ///   `self.<f>` alias read through the stored property's type): the member
    ///   `m` of `T`'s group in the caller's OWN module directory, never a
    ///   same-named type of another target — rule `local_receiver`. A local
    ///   of unknown type, or a type only another module declares, binds
    ///   nothing.
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
                let dir = self.caller_dir(g, site.from)?;
                match self.group(dir, base) {
                    Some(group) => (
                        Self::member(g, group, name)?,
                        "same_module_static",
                        &self.same_module_static,
                    ),
                    None => return self.attribute_receiver(g, site, dir, base, name),
                }
            }
            CallQualifier::ComplexReceiver { receiver, name } => {
                // GD.15: `makeStore().load()`, `self.repo.make().load()`,
                // `self.monitor.request()` on an `any P` property; a plain
                // field chain is the generic pass's.
                let dir = self.caller_dir(g, site.from)?;
                let reached = self
                    .chain(g, site.from, dir, receiver, 0)
                    .filter(|r| r.existential || r.chained)?;
                let rule = if reached.chained {
                    "chain_receiver"
                } else {
                    "field_receiver"
                };
                return self.bind_receiver(g, reached, name, rule);
            }
            CallQualifier::Bare(name) => {
                let group = self.group(self.caller_dir(g, site.from)?, name)?;
                (group.ctor?, "same_module_type", &self.same_module_type)
            }
            CallQualifier::SuperMethod(_) => return None,
        };
        count.set(count.get() + 1);
        Some((to, graph_evidence(EMITTER, rule)))
    }

    /// `x.m()`, `x` no type of the caller's module directory:
    /// - a local of the caller (DF.3) typed `T` / `any T` / `some T`
    ///   ([`receiver_type`], which reads a `self.<f>` alias through the
    ///   stored property): `T`'s member — rule `local_receiver`;
    /// - GD.15: a local holding a call chain (`let s = makeStore()`): the
    ///   chain's type ([`Self::chain`]) — rule `chain_receiver`;
    /// - GD.15: no local, but a stored property of the caller's type typed
    ///   `any P` / `some P` (implicit `self`), which the generic pass cannot
    ///   read — rule `field_receiver`. A plain-typed property stays the
    ///   generic pass's.
    fn attribute_receiver(
        &self,
        g: &RepoGraph,
        site: &CallSite,
        dir: &str,
        base: &str,
        name: &str,
    ) -> Option<(NodeId, Evidence)> {
        let local = g.nav.local_types.get(&site.from).and_then(|l| l.get(base));
        match local {
            Some(chain) if chain.contains("()") => {
                let reached = self.chain(g, site.from, dir, chain, 1)?;
                self.bind_receiver(g, reached, name, "chain_receiver")
            }
            Some(_) => {
                let (ty, existential) = unwrap_existential(receiver_type(g, site)?);
                let reached = Reached {
                    group: self.group(dir, ty)?,
                    existential,
                    chained: false,
                };
                self.bind_receiver(g, reached, name, "local_receiver")
            }
            None => {
                let reached = self
                    .chain(g, site.from, dir, base, 0)
                    .filter(|r| r.existential || r.chained)?;
                self.bind_receiver(g, reached, name, "field_receiver")
            }
        }
    }

    /// GD.15: bind the member `name` of the receiver's type group, counting
    /// what [`Self::marker`] reports.
    fn bind_receiver(
        &self,
        g: &RepoGraph,
        reached: Reached,
        name: &str,
        rule: &'static str,
    ) -> Option<(NodeId, Evidence)> {
        let to = Self::member(g, reached.group, name)?;
        let bump = |c: &Cell<usize>| c.set(c.get() + 1);
        if rule == "local_receiver" {
            bump(&self.local_receiver);
        }
        let protocol = reached
            .group
            .nodes
            .iter()
            .any(|id| g.nav.kind_by_id.get(id) == Some(&node_kind::INTERFACE));
        if protocol {
            bump(&self.protocol_receivers);
        }
        if reached.chained {
            bump(&self.chain_receivers);
        }
        if reached.existential {
            bump(&self.existentials);
        }
        Some((to, graph_evidence(EMITTER, rule)))
    }

    /// GD.15: the type group of the receiver chain `text`, the parser's
    /// normalised form (`self.repo.make().next()`, `makeStore()`, `s`): its
    /// head ([`Self::chain_head`]), then per segment a call `m()` (the
    /// member's recorded return type) or a stored property `p` (its declared
    /// type), each looked up in the caller's module directory. `None` past
    /// [`MAX_CHAIN`] hops or at any segment that types nothing.
    fn chain<'s>(
        &'s self,
        g: &RepoGraph,
        from: NodeId,
        dir: &str,
        text: &str,
        depth: usize,
    ) -> Option<Reached<'s>> {
        let mut segs = text.split('.');
        let mut cur = self.chain_head(g, from, dir, segs.next()?, depth)?;
        for (hop, seg) in segs.enumerate() {
            let ty = match seg.strip_suffix("()") {
                Some(m) => {
                    cur.chained = true;
                    g.nav
                        .return_types
                        .get(&Self::member(g, cur.group, m)?)?
                        .as_str()
                }
                None => Self::field(g, cur.group, seg)?,
            };
            cur = self.type_text(
                g,
                from,
                dir,
                ty,
                depth + hop + 1,
                (cur.existential, cur.chained),
            )?;
        }
        Some(cur)
    }

    /// GD.15: a chain's first segment: `self` / `Self` (the caller's type),
    /// `f()` (the caller type's member `f`, Swift's implicit `self`, else the
    /// one free function `f` of the module directory: its return type), a
    /// local of the caller (its recorded type), a type of the directory (a
    /// static member follows), else a stored property of the caller's type.
    fn chain_head<'s>(
        &'s self,
        g: &RepoGraph,
        from: NodeId,
        dir: &str,
        head: &str,
        depth: usize,
    ) -> Option<Reached<'s>> {
        if depth > MAX_CHAIN {
            return None;
        }
        let plain = |group| Reached {
            group,
            existential: false,
            chained: false,
        };
        let own = || self.group_of_type(enclosing_class_or_struct(&g.nav, from)?);
        if head == "self" || head == "Self" {
            return own().map(plain);
        }
        if let Some(f) = head.strip_suffix("()") {
            let callee = self.extension_member_of(g, from, f).or_else(|| {
                self.functions
                    .get(dir)
                    .and_then(|fs| fs.get(f))
                    .copied()
                    .flatten()
            })?;
            let ret = g.nav.return_types.get(&callee)?;
            return self.type_text(g, from, dir, ret, depth + 1, (false, true));
        }
        match g.nav.local_types.get(&from).and_then(|l| l.get(head)) {
            Some(ty) if ty.is_empty() => None,
            Some(ty) => self.type_text(g, from, dir, ty, depth + 1, (false, false)),
            None => match self.group(dir, head) {
                Some(group) => Some(plain(group)),
                None => {
                    let ty = Self::field(g, own()?, head)?;
                    self.type_text(g, from, dir, ty, depth + 1, (false, false))
                }
            },
        }
    }

    /// GD.15: the group a type text read along a chain names, carrying the
    /// `(existential, chained)` flags read so far: its `any` / `some`
    /// stripped (counted), a chain text (a local's `makeStore()` or
    /// `self.repo`) typed in turn.
    fn type_text<'s>(
        &'s self,
        g: &RepoGraph,
        from: NodeId,
        dir: &str,
        ty: &str,
        depth: usize,
        (existential_before, chained_before): (bool, bool),
    ) -> Option<Reached<'s>> {
        let (name, existential) = unwrap_existential(ty);
        let mut reached = if name.contains(['.', '(']) {
            self.chain(g, from, dir, name, depth + 1)?
        } else {
            Reached {
                group: self.group(dir, name)?,
                existential: false,
                chained: false,
            }
        };
        reached.existential |= existential || existential_before;
        reached.chained |= chained_before;
        Some(reached)
    }

    /// GD.15: the declared type of the stored property `name` over every
    /// node of `group`: one distinct text, else `None`.
    fn field<'g>(g: &'g RepoGraph, group: &TypeGroup, name: &str) -> Option<&'g str> {
        let mut hit: Option<&str> = None;
        for id in &group.nodes {
            match (hit, g.nav.field_types.get(id).and_then(|f| f.get(name))) {
                (Some(h), Some(f)) if h != f => return None,
                (None, Some(f)) => hit = Some(f),
                _ => {}
            }
        }
        hit
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
            "[swift-scope] implicit_self={implicit_self} same_module_static={} same_module_type={} extension_member={} local_receiver={} protocol_receivers={} chain_receivers={} existentials={}",
            self.same_module_static.get(),
            self.same_module_type.get(),
            self.extension_member.get(),
            self.local_receiver.get(),
            self.protocol_receivers.get(),
            self.chain_receivers.get(),
            self.existentials.get()
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

    /// DF.3: `<dir>::Store`, a STRUCT with the METHODs `save` and `load`,
    /// declared in the file `<dir>::Store`.
    fn store_in(dir: &str) -> FileParse {
        let store = format!("{dir}::Store");
        let (save, load) = (format!("{store}::save"), format!("{store}::load"));
        swift_file(
            &store,
            &[
                (node_kind::STRUCT, &store, None, "struct Store {"),
                (
                    node_kind::METHOD,
                    &save,
                    Some(&store),
                    "func save(_ x: Int)",
                ),
                (node_kind::METHOD, &load, Some(&store), "func load() -> Int"),
            ],
            vec![],
        )
    }

    /// DF.3: `<dir>::Service`, a CLASS whose METHOD `run` calls each of
    /// `calls` (`(receiver, member)`) with each local of `locals` recorded,
    /// and a stored property `repo: Store`.
    fn service_in(dir: &str, locals: &[(&str, &str)], calls: &[(&str, &str)]) -> FileParse {
        let service = format!("{dir}::Service");
        let run = format!("{service}::run");
        let sites = calls
            .iter()
            .map(|(base, name)| {
                let q = CallQualifier::Attribute {
                    base: (*base).into(),
                    name: (*name).into(),
                };
                (run.as_str(), q)
            })
            .collect();
        let mut file = swift_file(
            &service,
            &[
                (node_kind::CLASS, &service, None, "final class Service {"),
                (node_kind::METHOD, &run, Some(&service), "func run()"),
            ],
            sites,
        );
        let run_id = gid(node_kind::METHOD, &run);
        for (name, ty) in locals {
            file.nav.record_local_type(run_id, name, ty);
        }
        file.nav
            .record_field_type(gid(node_kind::CLASS, &service), "repo", "Store");
        file
    }

    /// DF.3: two targets declare `Store`, so no name-only lookup can pick
    /// one; a parameter / local of the caller (and a `self.repo` alias)
    /// binds the member on the CALLER's target's `Store`.
    #[test]
    fn local_receiver_binds_in_own_module() {
        let locals = [("store", "Store"), ("r", "self.repo")];
        let calls = [("store", "save"), ("r", "load")];
        let g = build(vec![
            store_in("Sources::App"),
            store_in("Sources::Admin"),
            service_in("Sources::App", &locals, &calls),
            service_in("Sources::Admin", &locals, &calls),
        ]);
        for dir in ["Sources::App", "Sources::Admin"] {
            let mut got = calls_from(&g, node_kind::METHOD, &format!("{dir}::Service::run"));
            got.sort();
            assert_eq!(
                got,
                vec![
                    (format!("{dir}::Store::load"), "local_receiver".to_string()),
                    (format!("{dir}::Store::save"), "local_receiver".to_string()),
                ],
                "{dir}: its own target's Store, never the other's"
            );
        }
    }

    /// DF.3: a local of unknown type (`""`) shadows the same-named stored
    /// property and binds nothing; a name that is no local is not the
    /// hook's (a bare stored-property receiver stays the generic pass's).
    #[test]
    fn local_of_unknown_type_binds_nothing() {
        let g = build(vec![
            store_in("Sources::App"),
            store_in("Sources::Admin"),
            service_in(
                "Sources::App",
                &[("repo", "")],
                &[("repo", "save"), ("other", "save")],
            ),
        ]);
        assert!(calls_from(&g, node_kind::METHOD, "Sources::App::Service::run").is_empty());
    }

    /// DF.3: a local typed by a type only another target declares binds
    /// nothing (an `import`ed module's types stay unresolved, CB.26).
    #[test]
    fn other_module_only_type_binds_nothing() {
        let g = build(vec![
            store_in("Sources::Admin"),
            store_in("Sources::Other"),
            service_in("Sources::App", &[("store", "Store")], &[("store", "save")]),
        ]);
        assert!(calls_from(&g, node_kind::METHOD, "Sources::App::Service::run").is_empty());
    }

    /// DF.3: a repo-unique type is the generic receiver pass's (A6.2a
    /// through the recorded local, rule `receiver_type`); the hook is only
    /// consulted after it missed.
    #[test]
    fn unique_type_keeps_the_generic_receiver_binding() {
        let g = build(vec![
            store_in("Sources::App"),
            service_in("Sources::App", &[("store", "Store")], &[("store", "save")]),
        ]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "Sources::App::Service::run"),
            vec![(
                "Sources::App::Store::save".to_string(),
                "receiver_type".to_string()
            )]
        );
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
        let checkout = gid(node_kind::METHOD, "Sources::App::Cart::checkout");
        let mut cart = pricing(vec![]);
        cart.nav.record_local_type(checkout, "s", "Store");
        let mut files = vec![
            formatter,
            cart,
            store_in("Sources::App"),
            store_in("Sources::Admin"),
        ];
        for f in &mut files {
            all_calls.append(&mut f.calls);
        }
        let g = build(files);
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
            CallSite {
                from: checkout,
                qualifier: CallQualifier::Attribute {
                    base: "s".into(),
                    name: "save".into(),
                },
                line: 3,
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
            "[swift-scope] implicit_self=1 same_module_static=1 same_module_type=1 extension_member=0 local_receiver=1 protocol_receivers=0 chain_receivers=0 existentials=0"
        );
    }

    /// GD.15: `<dir>::Store`, a protocol with the requirement METHOD `load`
    /// (its return type `String`), and `<dir>::Mem`, a class conforming to
    /// it (an IMPLEMENTS heritage ref, as the parser writes one) with its own
    /// `load`, plus the free function `makeStore() -> some Store`, all in
    /// the file `<dir>::Store`.
    fn protocol_in(dir: &str) -> FileParse {
        let file = format!("{dir}::Store");
        let (store, mem) = (format!("{dir}::Store"), format!("{dir}::Mem"));
        let (req, own) = (format!("{store}::load"), format!("{mem}::load"));
        let make = format!("{file}::makeStore");
        let mut f = swift_file(
            &file,
            &[
                (node_kind::INTERFACE, &store, None, "protocol Store {"),
                (
                    node_kind::METHOD,
                    &req,
                    Some(&store),
                    "func load() -> String",
                ),
                (node_kind::CLASS, &mem, None, "final class Mem: Store {"),
                (node_kind::METHOD, &own, Some(&mem), "func load() -> String"),
                (
                    node_kind::FUNCTION,
                    &make,
                    None,
                    "func makeStore() -> some Store",
                ),
            ],
            vec![],
        );
        f.nav
            .record_return_type(gid(node_kind::FUNCTION, &make), "some Store");
        f.nav
            .record_return_type(gid(node_kind::METHOD, &req), "String");
        f.refs.push(glia_code_domain::UnresolvedRef {
            from: gid(node_kind::CLASS, &mem),
            from_module: gid(node_kind::MODULE, &file),
            qualifier: bare("Store"),
            category: edge_category::IMPLEMENTS,
            line: 4,
        });
        f
    }

    /// GD.15: `<dir>::Use`, the FUNCTION `run` with each local of `locals`
    /// and each call of `calls` (a qualifier), and the CLASS `<dir>::Holder`
    /// whose METHOD `tick` reads its stored property `monitor: any Store`.
    fn use_in(dir: &str, locals: &[(&str, &str)], calls: Vec<CallQualifier>) -> FileParse {
        let file = format!("{dir}::Use");
        let run = format!("{file}::run");
        let holder = format!("{dir}::Holder");
        let tick = format!("{holder}::tick");
        let sites = calls.into_iter().map(|q| (run.as_str(), q)).collect();
        let mut f = swift_file(
            &file,
            &[
                (node_kind::FUNCTION, &run, None, "func run()"),
                (node_kind::CLASS, &holder, None, "final class Holder {"),
                (node_kind::METHOD, &tick, Some(&holder), "func tick()"),
            ],
            sites,
        );
        f.calls.push(CallSite {
            from: gid(node_kind::METHOD, &tick),
            qualifier: attr("monitor", "load"),
            line: 9,
        });
        f.nav
            .record_field_type(gid(node_kind::CLASS, &holder), "monitor", "any Store");
        let run_id = gid(node_kind::FUNCTION, &run);
        for (name, ty) in locals {
            f.nav.record_local_type(run_id, name, ty);
        }
        f
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.into(),
            name: name.into(),
        }
    }

    fn sorted(mut v: Vec<(String, String)>) -> Vec<(String, String)> {
        v.sort();
        v
    }

    /// GD.15: a receiver typed `any Store` / `some Store` binds the
    /// protocol's requirement in the caller's own target, never another
    /// target's same-named protocol and never a conforming class's method;
    /// the conformance's method IMPLEMENTS the requirement.
    #[test]
    fn existential_receiver_binds_the_protocol_requirement() {
        let g = build(vec![
            protocol_in("Sources::App"),
            protocol_in("Sources::Admin"),
            use_in(
                "Sources::App",
                &[("s", "any Store"), ("t", "some Store")],
                vec![attr("s", "load"), attr("t", "load")],
            ),
        ]);
        assert_eq!(
            sorted(calls_from(
                &g,
                node_kind::FUNCTION,
                "Sources::App::Use::run"
            )),
            vec![
                (
                    "Sources::App::Store::load".to_string(),
                    "local_receiver".to_string()
                ),
                (
                    "Sources::App::Store::load".to_string(),
                    "local_receiver".to_string()
                ),
            ]
        );
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "Sources::App::Holder::tick"),
            vec![(
                "Sources::App::Store::load".to_string(),
                "field_receiver".to_string()
            )],
            "a stored property typed `any Store`, read through implicit self"
        );
        let implements = |from: &str, to: &str| {
            g.edges.iter().any(|e| {
                e.category == edge_category::IMPLEMENTS
                    && g.nav.qname_by_id.get(&e.from).map(String::as_str) == Some(from)
                    && g.nav.qname_by_id.get(&e.to).map(String::as_str) == Some(to)
            })
        };
        assert!(implements("Sources::App::Mem", "Sources::App::Store"));
        assert!(implements(
            "Sources::App::Mem::load",
            "Sources::App::Store::load"
        ));
    }

    /// GD.15: a call chain types through each callee's recorded return
    /// type: a free function's (`makeStore().load()`), a local holding its
    /// result (`let made = makeStore()`), a static member's
    /// (`Factory.shared.make().load()`).
    #[test]
    fn chain_receivers_type_through_return_types() {
        let factory = swift_file(
            "Sources::App::Factory",
            &[
                (
                    node_kind::STRUCT,
                    "Sources::App::Factory",
                    None,
                    "struct Factory {",
                ),
                (
                    node_kind::METHOD,
                    "Sources::App::Factory::make",
                    Some("Sources::App::Factory"),
                    "func make() -> any Store",
                ),
            ],
            vec![],
        );
        let mut factory = factory;
        factory.nav.record_return_type(
            gid(node_kind::METHOD, "Sources::App::Factory::make"),
            "any Store",
        );
        factory.nav.record_field_type(
            gid(node_kind::STRUCT, "Sources::App::Factory"),
            "shared",
            "Factory",
        );
        let complex = |receiver: &str| CallQualifier::ComplexReceiver {
            receiver: receiver.into(),
            name: "load".into(),
        };
        let g = build(vec![
            protocol_in("Sources::App"),
            protocol_in("Sources::Admin"),
            factory,
            use_in(
                "Sources::App",
                &[("made", "makeStore()"), ("loop", "loop.next()")],
                vec![
                    complex("makeStore()"),
                    attr("made", "load"),
                    complex("Factory.shared.make()"),
                    attr("loop", "load"),
                    complex("unknown()"),
                ],
            ),
        ]);
        assert_eq!(
            calls_from(&g, node_kind::FUNCTION, "Sources::App::Use::run"),
            vec![
                (
                    "Sources::App::Store::load".to_string(),
                    "chain_receiver".to_string()
                ),
                (
                    "Sources::App::Store::load".to_string(),
                    "chain_receiver".to_string()
                ),
                (
                    "Sources::App::Store::load".to_string(),
                    "chain_receiver".to_string()
                ),
            ],
            "a self-referencing local and an unknown callee bind nothing"
        );
    }

    /// GD.15: the marker's three new counts.
    #[test]
    fn marker_counts_protocol_chain_and_existential_receivers() {
        let mut files = vec![
            protocol_in("Sources::App"),
            protocol_in("Sources::Admin"),
            use_in(
                "Sources::App",
                &[("s", "any Store"), ("made", "makeStore()")],
                vec![],
            ),
        ];
        let mut all_calls: Vec<CallSite> = vec![];
        for f in &mut files {
            all_calls.append(&mut f.calls);
        }
        let g = build(files);
        let run = gid(node_kind::FUNCTION, "Sources::App::Use::run");
        let types = SwiftModuleTypes::new(&g);
        for q in [attr("s", "load"), attr("made", "load")] {
            let site = CallSite {
                from: run,
                qualifier: q,
                line: 0,
            };
            assert!(types.resolve(&g, &site).is_some(), "{site:?}");
        }
        assert_eq!(
            types.marker(0),
            "[swift-scope] implicit_self=0 same_module_static=0 same_module_type=0 extension_member=0 local_receiver=1 protocol_receivers=2 chain_receivers=1 existentials=2"
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
