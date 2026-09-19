//! The code-domain section of a container: the sorted-`Vec` mirrors of
//! `CodeNav` and `SymbolTable` (`CodeNavStore`, `SymbolTableStore`), the
//! code-domain constructors on `Container` / `Header`, and `write_repo_graph`.

use std::path::Path;

use repo_graph_code_domain::CodeNav;
use repo_graph_core::{Edge, NodeId, NodeKindId, RepoId};
use repo_graph_graph::{RepoGraph, SymbolTable};

use crate::container::{Container, FORMAT_VERSION, Header, MAGIC, encode_file, write_atomic};
use crate::error::StoreError;

// ============================================================================
// CodeNav / SymbolTable — serialised mirrors using sorted Vecs
// ============================================================================

/// Serialised mirror of `CodeNav`. Each field is the source HashMap flattened
/// into a Vec of pairs, **sorted by key**. Sorting makes the on-disk bytes
/// deterministic (so two builds of the same graph produce byte-identical files,
/// useful for content-hash-based shard manifests at v0.4.5c) and lets future
/// readers binary-search the archived form for point lookups.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CodeNavStore {
    pub name_by_id: Vec<(NodeId, String)>,
    pub qname_by_id: Vec<(NodeId, String)>,
    pub kind_by_id: Vec<(NodeId, NodeKindId)>,
    pub parent_of: Vec<(NodeId, NodeId)>,
    pub children_of: Vec<(NodeId, Vec<NodeId>)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct SymbolTableStore {
    pub module_by_qname: Vec<(String, NodeId)>,
    pub module_symbols: Vec<(NodeId, Vec<(String, NodeId)>)>,
    pub class_methods: Vec<(NodeId, Vec<(String, NodeId)>)>,
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

        let mut kind_by_id: Vec<_> =
            nav.kind_by_id.iter().map(|(k, v)| (*k, *v)).collect();
        kind_by_id.sort_by_key(|(k, _)| k.0);

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
            kind_by_id,
            parent_of,
            children_of,
        }
    }
}

impl CodeNavStore {
    /// Inverse of `from_owned` — rehydrate a `CodeNav` from this on-disk shape.
    /// Used by the cache-load path so a `RepoGraph` produced from disk has the
    /// same nav structure a freshly-parsed one does.
    pub fn to_owned(&self) -> CodeNav {
        let mut nav = CodeNav::default();
        for (k, v) in &self.name_by_id {
            nav.name_by_id.insert(*k, v.clone());
        }
        for (k, v) in &self.qname_by_id {
            nav.qname_by_id.insert(*k, v.clone());
        }
        for (k, v) in &self.kind_by_id {
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
            module_import_bindings: to_pair_vec(&sym.module_import_bindings),
        }
    }
}

impl SymbolTableStore {
    /// Inverse of `from_owned`. Note: `properties` lives on `RepoGraph`, not on
    /// `SymbolTable`, so cache-loaded graphs have `properties = HashSet::new()`
    /// (it's only populated at parse time and isn't currently persisted).
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
        // Starts from Default for the build-time-only tables this store does
        // not persist (`interface_methods`, A6.6): a loaded graph has already
        // been resolved, so they stay empty and the archived layout is unchanged.
        SymbolTable {
            module_by_qname,
            module_symbols: to_map(&self.module_symbols),
            class_methods: to_map(&self.class_methods),
            module_import_bindings: to_map(&self.module_import_bindings),
            ..Default::default()
        }
    }
}

impl Container {
    /// Build a `Container` from an owned `RepoGraph`. Cheap conversion: clones
    /// nodes/edges and flattens the maps. Caller drops the source graph after
    /// writing.
    pub fn from_repo_graph(g: &RepoGraph) -> Self {
        Self {
            header: Header::for_code(),
            repo: g.repo,
            nodes: g.nodes.clone(),
            edges: g.edges.clone(),
            code_nav: CodeNavStore::from_owned(&g.nav),
            symbols: SymbolTableStore::from_owned(&g.symbols),
            unresolved_calls: g.unresolved_calls.clone(),
            unresolved_refs: g.unresolved_refs.clone(),
        }
    }

    /// Inverse of `from_repo_graph` — rebuild a `RepoGraph` from this container.
    /// `properties` is set to empty: it's parse-time state on `RepoGraph` and
    /// isn't currently part of the on-disk schema. Cache-load consumers don't
    /// need it (only parse-time composition does), so this is a deliberate
    /// information loss bounded by the format version.
    pub fn to_repo_graph(&self) -> RepoGraph {
        use std::collections::HashSet;
        RepoGraph {
            repo: self.repo,
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            nav: self.code_nav.to_owned(),
            symbols: self.symbols.to_owned_table(),
            unresolved_calls: self.unresolved_calls.clone(),
            unresolved_refs: self.unresolved_refs.clone(),
            properties: HashSet::new(),
        }
    }

    /// Build a `Container` carrying only cross-repo edges. Used by the sharded
    /// layout to write `cross_stack.gmap` — nodes/nav/symbols are empty because
    /// every cross-edge's endpoints live in some other shard. The synthetic
    /// `repo` comes from `RepoId::from_canonical("cross_stack")` so the file
    /// is self-identifying without needing a new container variant.
    pub fn for_cross_edges(edges: Vec<Edge>) -> Self {
        Self {
            header: Header::for_code(),
            repo: RepoId::from_canonical("cross_stack"),
            nodes: Vec::new(),
            edges,
            code_nav: CodeNavStore::default(),
            symbols: SymbolTableStore::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
        }
    }
}

impl Header {
    /// Header for a code-domain container. v0.4.5a leaves the registries
    /// empty — they're diagnostic surfaces and the per-domain crates haven't
    /// exposed a registration API yet (lands at v0.4.10).
    pub fn for_code() -> Self {
        Self {
            magic: MAGIC,
            version: FORMAT_VERSION,
            graph_type: "code".to_string(),
            cell_registry: Vec::new(),
            edge_category_registry: Vec::new(),
            node_kind_registry: Vec::new(),
        }
    }
}

// ============================================================================
// Write — atomic via .tmp + rename
// ============================================================================

/// Serialise a `RepoGraph` to a `.gmap` file (preamble + rkyv core, see
/// `FORMAT_VERSION`). Writes to `<path>.tmp` first, then atomically renames
/// over `<path>` so a crash mid-write never leaves a half-written file in
/// place. Existing readers' mmaps stay valid against the old inode until they
/// re-open.
pub fn write_repo_graph(g: &RepoGraph, path: &Path) -> Result<(), StoreError> {
    let container = Container::from_repo_graph(g);
    let bytes = encode_file(&container)?;
    write_atomic(path, &bytes)
}

// ============================================================================
// Tests
// ============================================================================


#[cfg(test)]
mod tests {
    use super::*;

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
}
