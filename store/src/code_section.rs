//! The code domain's section of a container (LC.5b): `CodeSection` - the
//! sorted-`Vec` mirrors of `CodeNav` and `SymbolTable` (`CodeNavStore`,
//! `SymbolTableStore`), the unresolved calls / refs and the `@property`-style
//! accessor set (`properties`, LC.7) - written as the named section `"code"`
//! beside the domain-free core, and the `RepoGraph` codec over core + section
//! (`encode_repo_graph` / `decode_repo_graph`, `write_repo_graph`).
//!
//! Node kinds are not here: they are core state (`Container::node_kinds`), so
//! `CodeNavStore` carries no `kind_by_id` and `CodeNavStore::to_owned` rebuilds
//! `CodeNav::kind_by_id` from the core index.

use std::collections::HashSet;
use std::path::Path;

use repo_graph_code_domain::{
    CallSite, CodeNav, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category, node_kind,
};
use repo_graph_core::{CellTypeId, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use repo_graph_graph::{RepoGraph, SymbolTable};

use crate::container::{
    Container, EncodedSection, Header, MmapContainer, encode_file, encode_section, write_atomic,
};
use crate::error::StoreError;

/// Name of the code domain's section in a `.gmap`.
pub const CODE_SECTION: &str = "code";

/// Everything the code domain persists beyond the core: navigation maps, the
/// symbol table, the references resolution left unbound, and the accessor set.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CodeSection {
    pub nav: CodeNavStore,
    pub symbols: SymbolTableStore,
    pub unresolved_calls: Vec<CallSite>,
    pub unresolved_refs: Vec<UnresolvedRef>,
    /// `RepoGraph.properties` (LC.7): the methods a parser marked as property
    /// accessors (Python `@property`), sorted by id so the bytes are
    /// deterministic. Before LC.7 a loaded graph had none.
    pub properties: Vec<NodeId>,
}

impl CodeSection {
    /// The code section of `g`: its nav maps and symbol table flattened to
    /// sorted `Vec`s, and its unresolved calls / refs.
    pub fn from_repo_graph(g: &RepoGraph) -> Self {
        Self {
            nav: CodeNavStore::from_owned(&g.nav),
            symbols: SymbolTableStore::from_owned(&g.symbols),
            unresolved_calls: g.unresolved_calls.clone(),
            unresolved_refs: g.unresolved_refs.clone(),
            properties: sorted_properties(&g.properties),
        }
    }

    /// True when the section would carry nothing. Such a section is not
    /// written (a nav-less graph's file is its core alone), and a file without
    /// one decodes to exactly this.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// `properties` as a `Vec` sorted by id: a `HashSet` iterates in a
/// per-process random order.
fn sorted_properties(set: &HashSet<NodeId>) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = set.iter().copied().collect();
    out.sort_by_key(|id| id.0);
    out
}

impl ArchivedCodeSection {
    /// Look up a node id's qname via binary search on the sorted nav vec.
    pub fn qname(&self, id: NodeId) -> Option<&str> {
        let pairs = &self.nav.qname_by_id;
        let i = pairs
            .binary_search_by(|entry| entry.0.0.to_native().cmp(&id.0))
            .ok()?;
        Some(pairs[i].1.as_str())
    }
}

// ============================================================================
// CodeNav / SymbolTable — serialised mirrors using sorted Vecs
// ============================================================================

/// Serialised mirror of `CodeNav` (less `kind_by_id`, which is the core's
/// `node_kinds`). Each field is the source HashMap flattened into a Vec of
/// pairs, **sorted by key**. Sorting makes the on-disk bytes
/// deterministic (so two builds of the same graph produce byte-identical files,
/// useful for content-hash-based shard manifests at v0.4.5c) and lets future
/// readers binary-search the archived form for point lookups.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CodeNavStore {
    pub name_by_id: Vec<(NodeId, String)>,
    pub qname_by_id: Vec<(NodeId, String)>,
    pub parent_of: Vec<(NodeId, NodeId)>,
    pub children_of: Vec<(NodeId, Vec<NodeId>)>,
}

/// Serialised mirror of `SymbolTable`: every map flattened into a `Vec`
/// sorted by key at both levels (outer by `NodeId`, inner by name), so the
/// shard bytes are identical across processes. Field order is archive order.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct SymbolTableStore {
    pub module_by_qname: Vec<(String, NodeId)>,
    pub module_symbols: Vec<(NodeId, Vec<(String, NodeId)>)>,
    pub class_methods: Vec<(NodeId, Vec<(String, NodeId)>)>,
    /// `SymbolTable.interface_methods` (A6.6): INTERFACE id -> (method name ->
    /// METHOD id). Persisted since LC.6 (FORMAT_VERSION 2), so a graph loaded
    /// from a `.gmap` carries the same interface table as a fresh build.
    pub interface_methods: Vec<(NodeId, Vec<(String, NodeId)>)>,
    pub module_import_bindings: Vec<(NodeId, Vec<(String, NodeId)>)>,
}

impl CodeNavStore {
    pub fn from_owned(nav: &CodeNav) -> Self {
        let mut name_by_id: Vec<_> = nav
            .name_by_id
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        name_by_id.sort_by_key(|(k, _)| k.0);

        let mut qname_by_id: Vec<_> = nav
            .qname_by_id
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        qname_by_id.sort_by_key(|(k, _)| k.0);

        let mut parent_of: Vec<_> =
            nav.parent_of.iter().map(|(k, v)| (*k, *v)).collect();
        parent_of.sort_by_key(|(k, _)| k.0);

        let mut children_of: Vec<_> = nav
            .children_of
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        children_of.sort_by_key(|(k, _)| k.0);

        Self {
            name_by_id,
            qname_by_id,
            parent_of,
            children_of,
        }
    }
}

impl CodeNavStore {
    /// Inverse of `from_owned` — rehydrate a `CodeNav` from this on-disk shape
    /// and the core's `node_kinds` (which carries `kind_by_id`). Used by the
    /// cache-load path so a `RepoGraph` produced from disk has the same nav
    /// structure a freshly-parsed one does.
    pub fn to_owned(&self, node_kinds: &[(NodeId, NodeKindId)]) -> CodeNav {
        let mut nav = CodeNav::default();
        for (k, v) in &self.name_by_id {
            nav.name_by_id.insert(*k, v.clone());
        }
        for (k, v) in &self.qname_by_id {
            nav.qname_by_id.insert(*k, v.clone());
        }
        for (k, v) in node_kinds {
            nav.kind_by_id.insert(*k, *v);
        }
        for (k, v) in &self.parent_of {
            nav.parent_of.insert(*k, *v);
        }
        for (k, v) in &self.children_of {
            nav.children_of.insert(*k, v.clone());
        }
        nav
    }
}

impl SymbolTableStore {
    pub fn from_owned(sym: &SymbolTable) -> Self {
        let mut module_by_qname: Vec<_> = sym
            .module_by_qname
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        module_by_qname.sort_by(|a, b| a.0.cmp(&b.0));

        let to_pair_vec = |m: &std::collections::HashMap<NodeId, std::collections::HashMap<String, NodeId>>| {
            let mut out: Vec<_> = m
                .iter()
                .map(|(k, inner)| {
                    let mut pairs: Vec<_> =
                        inner.iter().map(|(s, n)| (s.clone(), *n)).collect();
                    pairs.sort_by(|a, b| a.0.cmp(&b.0));
                    (*k, pairs)
                })
                .collect();
            out.sort_by_key(|(k, _)| k.0);
            out
        };

        Self {
            module_by_qname,
            module_symbols: to_pair_vec(&sym.module_symbols),
            class_methods: to_pair_vec(&sym.class_methods),
            interface_methods: to_pair_vec(&sym.interface_methods),
            module_import_bindings: to_pair_vec(&sym.module_import_bindings),
        }
    }
}

impl SymbolTableStore {
    /// Inverse of `from_owned`. `properties` lives on `RepoGraph`, not on
    /// `SymbolTable`: it is its own `CodeSection` field.
    pub fn to_owned_table(&self) -> SymbolTable {
        use std::collections::HashMap;
        let module_by_qname: HashMap<_, _> = self
            .module_by_qname
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        let to_map = |pairs: &Vec<(NodeId, Vec<(String, NodeId)>)>| -> HashMap<NodeId, HashMap<String, NodeId>> {
            pairs
                .iter()
                .map(|(k, inner)| {
                    let m: HashMap<_, _> =
                        inner.iter().map(|(s, n)| (s.clone(), *n)).collect();
                    (*k, m)
                })
                .collect()
        };
        // Exhaustive on purpose: a new `SymbolTable` field fails to compile
        // here until the store persists it (or says why it does not).
        SymbolTable {
            module_by_qname,
            module_symbols: to_map(&self.module_symbols),
            class_methods: to_map(&self.class_methods),
            interface_methods: to_map(&self.interface_methods),
            module_import_bindings: to_map(&self.module_import_bindings),
        }
    }
}

/// The domain-free core of `g`: a code header, its nodes and edges, and
/// `node_kinds` from `g.nav.kind_by_id` (sorted by the writer).
fn code_core(g: &RepoGraph) -> Container {
    Container {
        header: Header::for_code(),
        repo: g.repo,
        nodes: g.nodes.clone(),
        edges: g.edges.clone(),
        node_kinds: g.nav.kind_by_id.iter().map(|(k, v)| (*k, *v)).collect(),
        sections: Vec::new(),
    }
}

impl Container {
    /// Build a `Container` carrying only cross-repo edges. Used by the sharded
    /// layout to write `cross_stack.gmap` — nodes and kinds are empty and no
    /// code section is written, because every cross-edge's endpoints live in
    /// some other shard. The synthetic `repo` comes from
    /// `RepoId::from_canonical("cross_stack")` so the file is self-identifying
    /// without needing a new container variant.
    pub fn for_cross_edges(edges: Vec<Edge>) -> Self {
        Self {
            header: Header::for_code(),
            repo: RepoId::from_canonical("cross_stack"),
            nodes: Vec::new(),
            edges,
            node_kinds: Vec::new(),
            sections: Vec::new(),
        }
    }
}

/// Code-domain `(id, name)` pairs from one of its `ALL` tables.
fn code_pairs<I: Copy>(table: &[(I, &'static str)], id: fn(I) -> u32) -> Vec<(u32, &'static str)> {
    table.iter().map(|(i, name)| (id(*i), *name)).collect()
}

impl Header {
    /// Header for a code-domain container (LC.4): self-describing, its three
    /// registries filled from code-domain's `node_kind::ALL`,
    /// `edge_category::ALL` and `cell_type::ALL`, so every code shard and
    /// `cross_stack.gmap` names its ids without a reader linking code-domain.
    ///
    /// Infallible: `for_domain` only fails on an id repeated within one table,
    /// and the code tables are unique by construction
    /// (`code_registries_have_unique_ids` pins it). Were that ever broken, the
    /// file would still be written, with empty registries (`Header::new`),
    /// rather than panic mid-build.
    pub fn for_code() -> Self {
        Self::for_domain(
            GRAPH_TYPE,
            &code_pairs(node_kind::ALL, |k: NodeKindId| k.0),
            &code_pairs(edge_category::ALL, |c: EdgeCategoryId| c.0),
            &code_pairs(cell_type::ALL, |c: CellTypeId| c.0),
        )
        .unwrap_or_else(|_| Self::new(GRAPH_TYPE))
    }
}

// ============================================================================
// RepoGraph codec — core + "code" section
// ============================================================================

/// Encode `g` as the bytes of one `.gmap`, and say whether a code section was
/// written (the `[gmap] layout` marker counts them).
pub(crate) fn encode_repo_graph_counted(g: &RepoGraph) -> Result<(Vec<u8>, bool), StoreError> {
    let mut core = code_core(g);
    let code = CodeSection::from_repo_graph(g);
    // LC.6 marker, un-gated: one line per encoded shard that carries an
    // interface method table (owners = INTERFACE entries, methods = their sum).
    let iface = &code.symbols.interface_methods;
    if !iface.is_empty() {
        let methods: usize = iface.iter().map(|(_, m)| m.len()).sum();
        eprintln!(
            "[gmap] code symbols: repo={} iface_owners={} iface_methods={methods}",
            g.repo.0,
            iface.len(),
        );
    }
    let sections: Vec<EncodedSection> = if code.is_empty() {
        Vec::new()
    } else {
        vec![encode_section(CODE_SECTION, &code)?]
    };
    let has_code = !sections.is_empty();
    Ok((encode_file(&mut core, &sections)?, has_code))
}

/// Encode a `RepoGraph` as the bytes of one `.gmap`: the domain-free core
/// (code header, nodes, edges, `node_kinds` from `g.nav.kind_by_id`) plus the
/// `"code"` section (nav maps, symbols, unresolved calls / refs, properties).
/// A graph with nothing for the section (no nav, symbols, unresolved refs or
/// properties) is written as its core alone. Deterministic: every map and set
/// is flattened sorted by key.
pub fn encode_repo_graph(g: &RepoGraph) -> Result<Vec<u8>, StoreError> {
    Ok(encode_repo_graph_counted(g)?.0)
}

/// The inverse of `encode_repo_graph`: the core's nodes, edges and kinds plus
/// the `"code"` section, `properties` included (LC.7). A file without a code
/// section (a nav-less graph, or `cross_stack.gmap`) decodes with empty nav /
/// symbols / unresolved refs / properties.
pub fn decode_repo_graph(m: &MmapContainer) -> Result<RepoGraph, StoreError> {
    let core: Container = rkyv::deserialize::<Container, rkyv::rancor::Error>(m.archived()?)?;
    let code: CodeSection = match code_section_of(m)? {
        Some(archived) => rkyv::deserialize::<CodeSection, rkyv::rancor::Error>(archived)?,
        None => CodeSection::default(),
    };
    Ok(RepoGraph {
        repo: core.repo,
        nodes: core.nodes,
        edges: core.edges,
        nav: code.nav.to_owned(&core.node_kinds),
        symbols: code.symbols.to_owned_table(),
        unresolved_calls: code.unresolved_calls,
        unresolved_refs: code.unresolved_refs,
        properties: code.properties.into_iter().collect(),
    })
}

/// The file's code section, validated and borrowed zero-copy; `None` when it
/// has none.
pub fn code_section_of(m: &MmapContainer) -> Result<Option<&ArchivedCodeSection>, StoreError> {
    m.section::<ArchivedCodeSection>(CODE_SECTION)
}

/// A node id's qname, read from the file's code section. `None` when the file
/// has no code section or the section has no qname for `id`. Validates the
/// section on every call: for many lookups, take `code_section_of` once and
/// call `ArchivedCodeSection::qname`.
pub fn qname_of(m: &MmapContainer, id: NodeId) -> Result<Option<String>, StoreError> {
    Ok(code_section_of(m)?.and_then(|s| s.qname(id)).map(str::to_string))
}

/// Serialise a `RepoGraph` to a `.gmap` file (preamble + code section + rkyv
/// core, see `FORMAT_VERSION`). Writes to `<path>.tmp` first, then atomically
/// renames over `<path>` so a crash mid-write never leaves a half-written file
/// in place. Existing readers' mmaps stay valid against the old inode until
/// they re-open.
pub fn write_repo_graph(g: &RepoGraph, path: &Path) -> Result<(), StoreError> {
    let bytes = encode_repo_graph(g)?;
    write_atomic(path, &bytes)
}

// ============================================================================
// Tests
// ============================================================================


#[cfg(test)]
mod tests {
    use super::*;

    /// LC.4: `for_code`'s fallback to empty registries is unreachable - every
    /// code-domain table is unique by id, so `for_domain` accepts all three.
    #[test]
    fn code_registries_have_unique_ids() {
        let full = Header::for_domain(
            GRAPH_TYPE,
            &code_pairs(node_kind::ALL, |k: NodeKindId| k.0),
            &code_pairs(edge_category::ALL, |c: EdgeCategoryId| c.0),
            &code_pairs(cell_type::ALL, |c: CellTypeId| c.0),
        )
        .expect("code-domain ALL tables repeat an id");
        assert_eq!(Header::for_code(), full);
        assert_eq!(full.node_kind_registry.len(), node_kind::ALL.len());
        assert_eq!(full.edge_category_registry.len(), edge_category::ALL.len());
        assert_eq!(full.cell_registry.len(), cell_type::ALL.len());
    }

    #[test]
    fn nav_store_sorts_by_node_id() {
        let mut nav = CodeNav::default();
        nav.record(NodeId(50), "b", "m::b", NodeKindId(1), None);
        nav.record(NodeId(10), "a", "m::a", NodeKindId(1), None);
        nav.record(NodeId(30), "c", "m::c", NodeKindId(1), None);
        let store = CodeNavStore::from_owned(&nav);
        let ids: Vec<u64> = store.name_by_id.iter().map(|(k, _)| k.0).collect();
        assert_eq!(ids, vec![10, 30, 50]);
    }

    /// LC.6: `interface_methods` is flattened sorted at both levels (outer by
    /// id, inner by name) and decodes back into the same table.
    #[test]
    fn symbol_store_persists_interface_methods_sorted() {
        let mut sym = SymbolTable::default();
        for (iface, names) in [(90u64, ["Save", "GetById"]), (20, ["Put", "Delete"])] {
            let inner = names.iter().enumerate().map(|(i, n)| (n.to_string(), NodeId(iface + 1 + i as u64)));
            sym.interface_methods.insert(NodeId(iface), inner.collect());
        }
        let store = SymbolTableStore::from_owned(&sym);
        let flat: Vec<(u64, Vec<&str>)> = store
            .interface_methods
            .iter()
            .map(|(k, m)| (k.0, m.iter().map(|(n, _)| n.as_str()).collect()))
            .collect();
        assert_eq!(flat, vec![(20, vec!["Delete", "Put"]), (90, vec!["GetById", "Save"])]);
        assert_eq!(store.to_owned_table().interface_methods, sym.interface_methods);
    }

    /// LC.7: `properties` rides the code section, sorted, and decodes back into
    /// the set - also for a graph whose only code-section state it is.
    #[test]
    fn properties_round_trip_through_the_code_section() {
        let repo = RepoId::from_canonical("test://lc7-properties");
        let g = RepoGraph {
            repo,
            nodes: vec![],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: [NodeId(30), NodeId(7), NodeId(19)].into_iter().collect(),
        };
        let section = CodeSection::from_repo_graph(&g);
        assert_eq!(section.properties, vec![NodeId(7), NodeId(19), NodeId(30)]);
        assert!(!section.is_empty());

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.gmap");
        write_repo_graph(&g, &path).unwrap();
        let back = decode_repo_graph(&MmapContainer::open(&path).unwrap()).unwrap();
        assert_eq!(back.properties, g.properties);
    }
}
