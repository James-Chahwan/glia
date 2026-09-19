//! Output types shared by every builder and resolver: the per-repo graph,
//! its symbol index, and the build error.

use std::collections::{HashMap, HashSet};

use glia_code_domain::{CallSite, CodeNav, UnresolvedRef};
use glia_core::{Edge, Node, NodeId, RepoId};

// ============================================================================
// Output graph
// ============================================================================

#[derive(Debug)]
pub struct RepoGraph {
    pub repo: RepoId,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    pub symbols: SymbolTable,
    /// Call sites left unresolved after cross-file resolution. Kept as a
    /// diagnostic surface (at v0.4.5 they also feed the dense-text `?` sigil).
    pub unresolved_calls: Vec<CallSite>,
    /// `UnresolvedRef`s the resolver couldn't bind. Same diagnostic role as
    /// `unresolved_calls`. v0.4.4 use case: gin route handler refs that point
    /// at packages the parser couldn't link to a known module.
    pub unresolved_refs: Vec<UnresolvedRef>,
    /// v0.4.13b — method ids tagged as property-style (read as `self.x`).
    /// Populated from `FileParse.properties`. Consumed by composition-path
    /// synthesis to filter method→class hops down to syntactically valid
    /// attribute reads.
    pub properties: HashSet<NodeId>,
}

/// Symbol index built during resolution. Everything keyed by node id so
/// consumers never re-parse qnames.
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// Module qname (`"myapp::users"`) → module node id.
    pub module_by_qname: HashMap<String, NodeId>,
    /// Module node id → (top-level def name → def node id).
    /// Used for `from X import Y` resolution and for module-attribute calls.
    pub module_symbols: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Class node id → (method name → method node id).
    pub class_methods: HashMap<NodeId, HashMap<String, NodeId>>,
    /// INTERFACE node id → (method name → METHOD node id). Kept apart from
    /// `class_methods` on purpose: the HANDLED_BY global fallback
    /// (`unique_global_method`) scans `class_methods`, and an interface method
    /// sharing its implementation's name would turn every such lookup
    /// ambiguous and silently drop the route's HANDLED_BY edge (A6.6).
    /// Build-time only: the store does not persist it, so a graph loaded from
    /// a `.gmap` carries an empty table (resolution is finished before write).
    pub interface_methods: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Module node id → (bound name in that module → target node id).
    /// Populated from resolved imports. Powers cross-file call resolution.
    pub module_import_bindings: HashMap<NodeId, HashMap<String, NodeId>>,
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GraphError {
    #[error("module qname collision: {0}")]
    ModuleCollision(String),
}
