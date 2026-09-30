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
//!
//! EVIDENCE interning (format 3, CD.7b): every edge's EVIDENCE cell is a JSON
//! object whose three strings (emitter, rule, file) repeat across thousands of
//! edges. The writer stores each canonical one as a compact `Bytes` payload
//! pointing into the file's `"strings"` section (`StringTable`,
//! `intern_evidence`), and every reader that hands out an owned graph
//! (`decode_repo_graph`, the layout's cross-stack read) expands it back to the
//! exact JSON (`expand_evidence`), so the in-memory graph is byte for byte the
//! pre-3 graph. Both a code shard and `cross_stack.gmap` carry the section when
//! they hold any interned payload.
//!
//! CODE spans (format 3, CD.7c): a CODE cell whose text is a verbatim slice of
//! the file its node's first POSITION cell names, starting on the POSITION
//! start line, is written as a `glia_code_domain::code_span::CodeSpan` - the
//! file interned in the same `"strings"` table, the byte range and the xxh64
//! of the slice (tag `0x02`) - when the writer is given a [`CodeSource`]. A
//! reader given one reads the slice back as the text; a file that moved,
//! changed or is absent (or a read with no source) yields the span as JSON
//! (`CodeSpan::to_json`), counted unresolved ([`CodeSpanStats`]).

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use glia_code_domain::code_span::{CODE_SPAN_TAG, CodeSpan};
use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{
    CallSite, CodeNav, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_core::{
    Cell, CellPayload, CellTypeId, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId,
};
use glia_graph::{RepoGraph, SymbolTable};

use crate::container::{
    Container, EncodedSection, Header, MmapContainer, encode_file, encode_section, write_atomic,
};
use crate::error::StoreError;

/// Name of the code domain's section in a `.gmap`.
pub const CODE_SECTION: &str = "code";

/// Name of the per-file string table section (format 3, CD.7b): the strings
/// the file's interned edge-cell payloads index into. Written only when it has
/// entries, after the `"code"` section.
pub const STRINGS_SECTION: &str = "strings";

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
    /// `sym.home_module` (CB.15) is not persisted: it only steers call
    /// resolution, which is over before a graph is written.
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
            // CB.15: build-time only. Call resolution has finished before a
            // graph is written, so `from_owned` skips it and a loaded graph
            // carries none; the store format is unchanged.
            home_module: HashMap::new(),
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
    /// some other shard; the layout writer interns the edges' EVIDENCE and
    /// adds the `"strings"` section they need (format 3). The synthetic `repo` comes from
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
// EVIDENCE interning — the "strings" section (format 3, CD.7b)
// ============================================================================

/// Tag byte that opens an interned EVIDENCE payload. Only VECTOR cells carry
/// `Bytes` otherwise, so an EVIDENCE cell with a `Bytes` payload opening with
/// this tag is unambiguous on disk (a writer refuses one in memory:
/// [`encode_repo_graph`]).
const EVIDENCE_TAG: u8 = 0x01;

/// The per-file string table (`STRINGS_SECTION`): every string an interned
/// payload of the file indexes, in first-seen order. Filled while the edges
/// are walked in their stored (sorted build) order, so an identical graph
/// writes an identical table.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct StringTable {
    pub strings: Vec<String>,
}

impl StringTable {
    /// True when the table holds no string (such a table is not written).
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

/// What [`intern_evidence`] did to a slice of edges: `interned` EVIDENCE
/// cells now carry the compact payload, `kept_json` were left as written
/// (not canonical JSON, see [`intern_evidence`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct InternStats {
    pub interned: usize,
    pub kept_json: usize,
}

impl InternStats {
    fn absorb(&mut self, other: InternStats) {
        self.interned += other.interned;
        self.kept_json += other.kept_json;
    }
}

/// Index of each string already in a table, so interning is one hash lookup.
struct Interner<'t> {
    table: &'t mut StringTable,
    index: HashMap<String, u64>,
}

impl<'t> Interner<'t> {
    fn new(table: &'t mut StringTable) -> Self {
        let mut index = HashMap::with_capacity(table.strings.len());
        for (i, s) in table.strings.iter().enumerate() {
            index.entry(s.clone()).or_insert(i as u64);
        }
        Self { table, index }
    }

    fn ix(&mut self, s: &str) -> u64 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.table.strings.len() as u64;
        self.table.strings.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }
}

/// The evidence `payload` holds when interning it is lossless: a JSON payload
/// that parses as [`Evidence`] AND that `Evidence::to_cell` writes back byte
/// for byte (field order, no extra fields, no whitespace). `None` otherwise.
fn canonical_evidence(payload: &CellPayload) -> Option<Evidence> {
    let CellPayload::Json(s) = payload else {
        return None;
    };
    let ev: Evidence = serde_json::from_str(s).ok()?;
    (ev.to_cell().payload == *payload).then_some(ev)
}

fn basis_byte(b: Basis) -> u8 {
    match b {
        Basis::None => 0,
        Basis::Site => 1,
        Basis::FromNode => 2,
        Basis::ToNode => 3,
        Basis::File => 4,
    }
}

fn basis_of(b: u8) -> Option<Basis> {
    Some(match b {
        0 => Basis::None,
        1 => Basis::Site,
        2 => Basis::FromNode,
        3 => Basis::ToNode,
        4 => Basis::File,
        _ => return None,
    })
}

/// Unsigned LEB128.
fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Read one unsigned LEB128 at `*at`, advancing it. `what` names the field
/// for the error.
fn take_varint(bytes: &[u8], at: &mut usize, what: &str) -> Result<u64, String> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let Some(&b) = bytes.get(*at) else {
            return Err(format!("truncated varint ({what}) at byte {at}"));
        };
        *at += 1;
        let low = u64::from(b & 0x7f);
        if shift == 63 && low > 1 {
            return Err(format!("varint ({what}) overflows u64"));
        }
        v |= low << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(format!("varint ({what}) overflows u64"))
}

/// `[0x01, varint(emitter), varint(rule + 1 | 0), varint(file + 1 | 0),
/// varint(line + 1 | 0), basis]`: string fields are table indices, an absent
/// optional field is 0.
fn encode_evidence(ev: &Evidence, strings: &mut Interner<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.push(EVIDENCE_TAG);
    let emitter = strings.ix(&ev.emitter);
    put_varint(&mut out, emitter);
    let rule = ev.rule.as_deref().map_or(0, |r| strings.ix(r) + 1);
    put_varint(&mut out, rule);
    let file = ev.file.as_deref().map_or(0, |f| strings.ix(f) + 1);
    put_varint(&mut out, file);
    put_varint(&mut out, ev.line.map_or(0, |l| u64::from(l) + 1));
    out.push(basis_byte(ev.basis));
    out
}

/// The inverse of [`encode_evidence`]; `string(i)` is table entry `i`. Any
/// malformed payload (truncated, an index outside the table, a line past
/// `u32`, an unknown basis, trailing bytes) is a reason string.
fn decode_evidence<'s>(
    bytes: &[u8],
    table_len: usize,
    string: &impl Fn(usize) -> Option<&'s str>,
) -> Result<Evidence, String> {
    let mut at = 1usize;
    let lookup = |ix: u64, what: &str| -> Result<String, String> {
        usize::try_from(ix)
            .ok()
            .and_then(string)
            .map(str::to_string)
            .ok_or_else(|| format!("{what} index {ix} outside the {table_len}-entry strings table"))
    };
    let emitter = lookup(take_varint(bytes, &mut at, "emitter")?, "emitter")?;
    let rule = match take_varint(bytes, &mut at, "rule")? {
        0 => None,
        i => Some(lookup(i - 1, "rule")?),
    };
    let file = match take_varint(bytes, &mut at, "file")? {
        0 => None,
        i => Some(lookup(i - 1, "file")?),
    };
    let line = match take_varint(bytes, &mut at, "line")? {
        0 => None,
        l => Some(u32::try_from(l - 1).map_err(|_| format!("line {} past u32", l - 1))?),
    };
    let Some(&b) = bytes.get(at) else {
        return Err(format!("truncated payload: no basis byte at byte {at}"));
    };
    let basis = basis_of(b).ok_or_else(|| format!("unknown basis byte {b}"))?;
    if at + 1 != bytes.len() {
        return Err(format!("{} trailing byte(s)", bytes.len() - at - 1));
    }
    Ok(Evidence { emitter, rule, file, line, basis })
}

/// Intern every canonical EVIDENCE cell of `edges` into `table`, in edge
/// order (emitter, then rule, then file of each), replacing its JSON payload
/// with the compact `Bytes` form. A payload is interned only when it parses as
/// [`Evidence`] AND `Evidence::to_cell` reproduces it byte for byte, so the
/// round trip is lossless by construction; any other EVIDENCE cell (a `Text`
/// payload, JSON in another field order or with extra fields) is left as
/// written and counted `kept_json`. Strings already in `table` are reused.
pub fn intern_evidence(edges: &mut [Edge], table: &mut StringTable) -> InternStats {
    let mut strings = Interner::new(table);
    let mut stats = InternStats::default();
    for e in edges.iter_mut() {
        for c in e.cells.iter_mut().filter(|c| c.kind == cell_type::EVIDENCE) {
            match canonical_evidence(&c.payload) {
                Some(ev) => {
                    c.payload = CellPayload::Bytes(encode_evidence(&ev, &mut strings));
                    stats.interned += 1;
                }
                None => stats.kept_json += 1,
            }
        }
    }
    stats
}

/// Expand every interned EVIDENCE cell of `edges` back to the exact JSON
/// `Evidence::to_cell` wrote before interning, reading its strings from
/// `table` (the file's `"strings"` section). Cells that were never interned
/// are untouched. A payload that does not decode (truncated, an index outside
/// the table) is `StoreError::Corrupt` naming the edge; a caller reading a
/// layout adds the shard's name.
pub fn expand_evidence(edges: &mut [Edge], table: &ArchivedStringTable) -> Result<(), StoreError> {
    let strings = &table.strings;
    expand_with(edges, strings.len(), &|i| strings.get(i).map(|s| s.as_str()))
}

fn expand_with<'s>(
    edges: &mut [Edge],
    table_len: usize,
    string: &impl Fn(usize) -> Option<&'s str>,
) -> Result<(), StoreError> {
    for (i, e) in edges.iter_mut().enumerate() {
        let (from, to) = (e.from.0, e.to.0);
        for c in e.cells.iter_mut().filter(|c| c.kind == cell_type::EVIDENCE) {
            let CellPayload::Bytes(b) = &c.payload else {
                continue;
            };
            if b.first() != Some(&EVIDENCE_TAG) {
                continue;
            }
            let ev = decode_evidence(b, table_len, string).map_err(|why| StoreError::Corrupt {
                detail: format!("interned EVIDENCE of edge {i} ({from} -> {to}): {why}"),
            })?;
            *c = ev.to_cell();
        }
    }
    Ok(())
}

/// Expand the interned EVIDENCE of `edges`, read from the file `m`, with the
/// file's own string table. A file with no `"strings"` section expands
/// against an empty table, so an interned payload in it is `Corrupt`.
pub(crate) fn expand_file_evidence(
    m: &MmapContainer,
    edges: &mut [Edge],
) -> Result<(), StoreError> {
    match m.section::<ArchivedStringTable>(STRINGS_SECTION)? {
        Some(table) => expand_evidence(edges, table),
        None => expand_with(edges, 0, &|_| None),
    }
}

// ============================================================================
// CODE spans — CODE text as a range of its source file (format 3, CD.7c)
// ============================================================================

/// Where the CODE span codec reads a repo's source files: the writer to find
/// each CODE text in its file, the reader to read it back.
pub trait CodeSource {
    /// The bytes of `file` - repo-relative, `/`-separated, as a POSITION cell
    /// names it - in the repo `repo`. `None` when the repo is unknown or the
    /// file cannot be read.
    fn read(&self, repo: RepoId, file: &str) -> Option<Cow<'_, [u8]>>;
}

/// A [`CodeSource`] over repo roots on disk, keyed by `RepoId.0`: the roots a
/// layout's manifest records (`RepoMeta::root`, resolved against the layout
/// directory). Each root is canonicalised once, when built; one that does not
/// exist is dropped, so its repo's spans stay inline on a write and read back
/// unresolved.
///
/// A file resolves to `root.join(file)` only when `file` is relative and made
/// of plain components (no `..`, no root or drive prefix) AND its canonical
/// path still lies under the canonical root, so neither a hostile manifest nor
/// a symlink reads outside the repo. It holds no file: the codec reads each
/// file once per shard and drops them with the shard.
#[derive(Debug, Clone, Default)]
pub struct FsCodeSource {
    roots: BTreeMap<u64, PathBuf>,
}

impl FsCodeSource {
    /// A source over `roots` (`RepoId.0` -> repo root), each canonicalised;
    /// a root that does not resolve is left out.
    pub fn new(roots: impl IntoIterator<Item = (u64, PathBuf)>) -> Self {
        let roots = roots
            .into_iter()
            .filter_map(|(id, root)| Some((id, std::fs::canonicalize(root).ok()?)))
            .collect();
        Self { roots }
    }

    /// True when no root resolved: every span would be unresolved.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

impl CodeSource for FsCodeSource {
    fn read(&self, repo: RepoId, file: &str) -> Option<Cow<'_, [u8]>> {
        let root = self.roots.get(&repo.0)?;
        let rel = Path::new(file);
        let plain = rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if file.is_empty() || !plain {
            return None;
        }
        let path = std::fs::canonicalize(root.join(rel)).ok()?;
        if !path.starts_with(root) {
            return None;
        }
        std::fs::read(path).ok().map(Cow::Owned)
    }
}

/// What the CODE span codec did. Written: `spanned` CODE cells stored as
/// spans, `inline` stored as written (not a slice starting on the POSITION
/// line, no POSITION, a JSON payload, no source), `saved_bytes` = the spanned
/// texts' bytes minus their span payloads' bytes. Read: `rehydrated` spans
/// read back as their text, `unresolved` handed out as `CodeSpan` JSON (the
/// file moved, changed or is absent, or no source was given).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CodeSpanStats {
    pub spanned: usize,
    pub inline: usize,
    pub saved_bytes: i64,
    pub rehydrated: usize,
    pub unresolved: usize,
}

impl CodeSpanStats {
    pub(crate) fn absorb(&mut self, other: CodeSpanStats) {
        self.spanned += other.spanned;
        self.inline += other.inline;
        self.saved_bytes += other.saved_bytes;
        self.rehydrated += other.rehydrated;
        self.unresolved += other.unresolved;
    }

    /// Did the file(s) counted hold any span?
    pub fn has_spans(&self) -> bool {
        self.rehydrated + self.unresolved > 0
    }
}

/// One source file of a shard: its bytes and, built on first use (the writer
/// only), the byte offset each line starts at.
struct SourceFile {
    bytes: Vec<u8>,
    line_starts: OnceCell<Vec<usize>>,
}

impl SourceFile {
    /// Byte offset of the first place on 0-based `line` (any byte of the line,
    /// its `\n` included) at which `text` occurs verbatim.
    fn find_on_line(&self, line: usize, text: &[u8]) -> Option<usize> {
        let starts = self.line_starts.get_or_init(|| {
            let mut v = vec![0usize];
            v.extend(self.bytes.iter().enumerate().filter(|(_, b)| **b == b'\n').map(|(i, _)| i + 1));
            v
        });
        let from = *starts.get(line)?;
        let to = starts.get(line + 1).map_or(self.bytes.len(), |next| next - 1);
        let first = *text.first()?;
        (from..=to).find(|&k| {
            self.bytes.get(k) == Some(&first) && self.bytes.get(k..k + text.len()) == Some(text)
        })
    }
}

/// One shard's view of a [`CodeSource`]: every file read at most once, kept
/// until the shard is encoded / decoded.
struct ShardFiles<'s> {
    source: &'s dyn CodeSource,
    repo: RepoId,
    files: HashMap<String, Option<SourceFile>>,
}

impl<'s> ShardFiles<'s> {
    fn new(source: &'s dyn CodeSource, repo: RepoId) -> Self {
        Self { source, repo, files: HashMap::new() }
    }

    fn get(&mut self, file: &str) -> Option<&SourceFile> {
        if !self.files.contains_key(file) {
            let read = self.source.read(self.repo, file).map(|b| SourceFile {
                bytes: b.into_owned(),
                line_starts: OnceCell::new(),
            });
            self.files.insert(file.to_string(), read);
        }
        self.files.get(file)?.as_ref()
    }
}

/// `(file, 0-based start_line)` of the node's first POSITION cell that has
/// both, JSON or text.
fn first_position(cells: &[Cell]) -> Option<(String, usize)> {
    cells.iter().filter(|c| c.kind == cell_type::POSITION).find_map(|c| {
        let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
            return None;
        };
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let file = v.get("file")?.as_str().filter(|f| !f.is_empty())?.to_string();
        let line = usize::try_from(v.get("start_line")?.as_u64()?).ok()?;
        Some((file, line))
    })
}

/// Store every CODE text of `nodes` found verbatim in its POSITION file,
/// starting on the POSITION start line, as an interned span (the file in
/// `strings`). Every other CODE cell is left as written.
fn span_code_cells(
    nodes: &mut [Node],
    strings: &mut Interner<'_>,
    files: &mut ShardFiles<'_>,
) -> CodeSpanStats {
    let mut stats = CodeSpanStats::default();
    for n in nodes.iter_mut() {
        let pos = first_position(&n.cells);
        for c in n.cells.iter_mut().filter(|c| c.kind == cell_type::CODE) {
            let CellPayload::Text(text) = &c.payload else {
                stats.inline += 1;
                continue;
            };
            let found = pos.as_ref().and_then(|(file, line)| {
                let at = files.get(file)?.find_on_line(*line, text.as_bytes())?;
                Some(CodeSpan::of(file, at as u64, text.as_bytes()))
            });
            let Some(span) = found else {
                stats.inline += 1;
                continue;
            };
            let bytes = span.encode(strings.ix(&span.file));
            stats.spanned += 1;
            stats.saved_bytes += text.len() as i64 - bytes.len() as i64;
            c.payload = CellPayload::Bytes(bytes);
        }
    }
    stats
}

/// Read every CODE span of `nodes` back: the text when `files` holds the file
/// unchanged, else the span as `CodeSpan` JSON (unresolved). A payload that
/// does not decode is `Corrupt` naming the node.
fn expand_code_spans<'s>(
    nodes: &mut [Node],
    table_len: usize,
    string: &impl Fn(usize) -> Option<&'s str>,
    mut files: Option<&mut ShardFiles<'_>>,
) -> Result<CodeSpanStats, StoreError> {
    let mut stats = CodeSpanStats::default();
    for (i, n) in nodes.iter_mut().enumerate() {
        let id = n.id.0;
        for c in n.cells.iter_mut().filter(|c| c.kind == cell_type::CODE) {
            let CellPayload::Bytes(b) = &c.payload else {
                continue;
            };
            if b.first() != Some(&CODE_SPAN_TAG) {
                continue;
            }
            let span = CodeSpan::decode(b, table_len, string).map_err(|why| {
                StoreError::Corrupt { detail: format!("CODE span of node {i} ({id}): {why}") }
            })?;
            let text = files
                .as_deref_mut()
                .and_then(|f| f.get(&span.file))
                .and_then(|f| span.slice(&f.bytes))
                .map(str::to_string);
            c.payload = match text {
                Some(text) => {
                    stats.rehydrated += 1;
                    CellPayload::Text(text)
                }
                None => {
                    stats.unresolved += 1;
                    span.to_payload()
                }
            };
        }
    }
    Ok(stats)
}

/// Read the CODE spans of `nodes`, from the file `m` of repo `repo`, back
/// through `source` with the file's own string table (see
/// [`expand_code_spans`]); with no source every span is unresolved.
fn expand_file_code_spans(
    m: &MmapContainer,
    nodes: &mut [Node],
    repo: RepoId,
    source: Option<&dyn CodeSource>,
) -> Result<CodeSpanStats, StoreError> {
    let mut files = source.map(|s| ShardFiles::new(s, repo));
    match m.section::<ArchivedStringTable>(STRINGS_SECTION)? {
        Some(table) => {
            let strings = &table.strings;
            let get = |i: usize| strings.get(i).map(|s| s.as_str());
            expand_code_spans(nodes, strings.len(), &get, files.as_mut())
        }
        None => expand_code_spans(nodes, 0, &|_| None, files.as_mut()),
    }
}

/// Intern `core`'s edge EVIDENCE and, given a `source`, its nodes' CODE spans
/// in place for writing, and return the `"strings"` section to write beside
/// them (`None` when nothing was interned) with the counts. EVIDENCE is
/// interned first, in edge order, then span files in node order, so a graph
/// with no span writes the table it wrote before CD.7c. `intern` false (a
/// test's baseline) leaves every cell as written. An in-memory EVIDENCE or
/// CODE cell that already holds an interned-looking payload is `Invalid`: its
/// strings are not in this file's table, so it would read back as something
/// else (or as `Corrupt`).
fn intern_for_write(
    core: &mut Container,
    intern: bool,
    source: Option<&dyn CodeSource>,
) -> Result<(Option<EncodedSection>, InternStats, CodeSpanStats), StoreError> {
    let raw = core.edges.iter().position(|e| {
        e.cells.iter().any(|c| {
            c.kind == cell_type::EVIDENCE
                && matches!(&c.payload, CellPayload::Bytes(b) if b.first() == Some(&EVIDENCE_TAG))
        })
    });
    if let Some(i) = raw {
        return Err(StoreError::Invalid(format!(
            "edge {i} carries an EVIDENCE cell in the store's interned Bytes form (tag \
             {EVIDENCE_TAG:#04x}); EVIDENCE is JSON in memory (code-domain evidence.rs), and \
             read_to_owned's raw form is only written back through its own file"
        )));
    }
    let raw_span = core.nodes.iter().position(|n| {
        n.cells.iter().any(|c| {
            c.kind == cell_type::CODE
                && matches!(&c.payload, CellPayload::Bytes(b) if b.first() == Some(&CODE_SPAN_TAG))
        })
    });
    if let Some(i) = raw_span {
        return Err(StoreError::Invalid(format!(
            "node {i} carries a CODE cell in the store's span Bytes form (tag \
             {CODE_SPAN_TAG:#04x}); CODE is text (or code_span JSON) in memory, and \
             read_to_owned's raw form is only written back through its own file"
        )));
    }
    let code_cells = || {
        core.nodes.iter().flat_map(|n| &n.cells).filter(|c| c.kind == cell_type::CODE).count()
    };
    if !intern {
        let kept_json = core
            .edges
            .iter()
            .flat_map(|e| &e.cells)
            .filter(|c| c.kind == cell_type::EVIDENCE)
            .count();
        let spans = CodeSpanStats { inline: code_cells(), ..CodeSpanStats::default() };
        return Ok((None, InternStats { interned: 0, kept_json }, spans));
    }
    let mut table = StringTable::default();
    let stats = intern_evidence(&mut core.edges, &mut table);
    let spans = match source {
        Some(source) => {
            let mut files = ShardFiles::new(source, core.repo);
            span_code_cells(&mut core.nodes, &mut Interner::new(&mut table), &mut files)
        }
        None => CodeSpanStats { inline: code_cells(), ..CodeSpanStats::default() },
    };
    let section = if table.is_empty() {
        None
    } else {
        Some(encode_section(STRINGS_SECTION, &table)?)
    };
    Ok((section, stats, spans))
}

// ============================================================================
// RepoGraph codec — core + "code" section + "strings" section
// ============================================================================

/// One encoded file, and what the `[gmap] layout` / `[gmap] code spans`
/// markers count about it.
pub(crate) struct EncodedFile {
    pub(crate) bytes: Vec<u8>,
    pub(crate) has_code: bool,
    pub(crate) has_strings: bool,
    pub(crate) evidence: InternStats,
    pub(crate) spans: CodeSpanStats,
}

/// A layout write's running totals over its encoded files.
#[derive(Default)]
pub(crate) struct LayoutCounts {
    pub(crate) code_sections: usize,
    pub(crate) strings_sections: usize,
    pub(crate) evidence: InternStats,
    pub(crate) spans: CodeSpanStats,
}

impl EncodedFile {
    /// Fold this file's counts into a layout's running totals.
    pub(crate) fn count_into(&self, counts: &mut LayoutCounts) {
        counts.code_sections += usize::from(self.has_code);
        counts.strings_sections += usize::from(self.has_strings);
        counts.evidence.absorb(self.evidence);
        counts.spans.absorb(self.spans);
    }
}

/// Encode `g` as the bytes of one `.gmap`, CODE spans read through `source`
/// when given, with what the layout markers count (code / strings sections
/// written, EVIDENCE interned / kept, CODE spanned / inline).
pub(crate) fn encode_repo_graph_counted(
    g: &RepoGraph,
    source: Option<&dyn CodeSource>,
) -> Result<EncodedFile, StoreError> {
    encode_shard(g, true, source)
}

/// `cross_stack.gmap`'s bytes: a core of `edges` (no nodes, no code section)
/// plus the `"strings"` section their interned EVIDENCE needs.
pub(crate) fn encode_cross_edges(edges: &[Edge]) -> Result<EncodedFile, StoreError> {
    let mut core = Container::for_cross_edges(edges.to_vec());
    let (strings, evidence, spans) = intern_for_write(&mut core, true, None)?;
    let sections: Vec<EncodedSection> = strings.into_iter().collect();
    Ok(EncodedFile {
        bytes: encode_file(&mut core, &sections)?,
        has_code: false,
        has_strings: !sections.is_empty(),
        evidence,
        spans,
    })
}

fn encode_shard(
    g: &RepoGraph,
    intern: bool,
    source: Option<&dyn CodeSource>,
) -> Result<EncodedFile, StoreError> {
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
    let mut sections: Vec<EncodedSection> = Vec::new();
    if !code.is_empty() {
        sections.push(encode_section(CODE_SECTION, &code)?);
    }
    let has_code = !sections.is_empty();
    let (strings, evidence, spans) = intern_for_write(&mut core, intern, source)?;
    let has_strings = strings.is_some();
    sections.extend(strings);
    Ok(EncodedFile {
        bytes: encode_file(&mut core, &sections)?,
        has_code,
        has_strings,
        evidence,
        spans,
    })
}

/// Encode a `RepoGraph` as the bytes of one `.gmap`: the domain-free core
/// (code header, nodes, edges, `node_kinds` from `g.nav.kind_by_id`) plus the
/// `"code"` section (nav maps, symbols, unresolved calls / refs, properties)
/// and, since format 3, the `"strings"` section of its interned EVIDENCE
/// ([`intern_evidence`]). A graph with nothing for a section (no nav, symbols,
/// unresolved refs or properties; no canonical EVIDENCE) is written without
/// it. Deterministic: every map and set is flattened sorted by key, and the
/// string table is filled in edge order. An edge whose EVIDENCE cell already
/// holds the interned `Bytes` form, or a node whose CODE cell holds the span
/// `Bytes` form (`read_to_owned`'s raw core, taken out of its file), is
/// `StoreError::Invalid`. Every CODE cell is written as it is in memory: see
/// [`encode_repo_graph_with`] for spans.
pub fn encode_repo_graph(g: &RepoGraph) -> Result<Vec<u8>, StoreError> {
    encode_repo_graph_with(g, None)
}

/// [`encode_repo_graph`] storing CODE as spans into the source (CD.7c) when
/// `source` is given: a CODE `Text` payload found verbatim in the file its
/// node's first POSITION cell names, starting at any byte of the POSITION
/// start line, is written as that file (interned in the `"strings"` section,
/// after the EVIDENCE strings, in node order), the byte range and the xxh64
/// of the slice. Any other CODE cell (not found, no POSITION, a JSON payload,
/// a file the source cannot read) is written as it is, so the encode is
/// lossless whatever the source holds. Each file is read at most once per
/// call. Deterministic for a given graph and given source bytes.
pub fn encode_repo_graph_with(
    g: &RepoGraph,
    source: Option<&dyn CodeSource>,
) -> Result<Vec<u8>, StoreError> {
    Ok(encode_repo_graph_counted(g, source)?.bytes)
}

/// The inverse of `encode_repo_graph`: the core's nodes, edges and kinds plus
/// the `"code"` section, `properties` included (LC.7), with every interned
/// EVIDENCE cell expanded back to its JSON ([`expand_evidence`]), so the graph
/// equals the one written. A file without a code section (a nav-less graph, or
/// `cross_stack.gmap`) decodes with empty nav / symbols / unresolved refs /
/// properties. An interned payload that does not decode is `Corrupt`. With no
/// source, every CODE span (CD.7c) decodes as its `CodeSpan` JSON
/// (`{"code_span":{"file":..,"start":..,"end":..,"xxh64":..}}`): see
/// [`decode_repo_graph_with`].
pub fn decode_repo_graph(m: &MmapContainer) -> Result<RepoGraph, StoreError> {
    decode_repo_graph_with(m, None)
}

/// [`decode_repo_graph`] reading every CODE span back through `source`: the
/// file's byte range, when it still hashes to the span's xxh64 and is UTF-8,
/// becomes the `Text` it was written from; a file the source cannot read, too
/// short, changed or not UTF-8 (and every span, with no source) yields the
/// span's `CodeSpan` JSON instead. Each file is read at most once per call.
pub fn decode_repo_graph_with(
    m: &MmapContainer,
    source: Option<&dyn CodeSource>,
) -> Result<RepoGraph, StoreError> {
    Ok(decode_repo_graph_counted(m, source)?.0)
}

/// [`decode_repo_graph_with`] plus how many spans were rehydrated /
/// unresolved.
pub(crate) fn decode_repo_graph_counted(
    m: &MmapContainer,
    source: Option<&dyn CodeSource>,
) -> Result<(RepoGraph, CodeSpanStats), StoreError> {
    let mut core: Container = rkyv::deserialize::<Container, rkyv::rancor::Error>(m.archived()?)?;
    expand_file_evidence(m, &mut core.edges)?;
    let spans = expand_file_code_spans(m, &mut core.nodes, core.repo, source)?;
    let code: CodeSection = match code_section_of(m)? {
        Some(archived) => rkyv::deserialize::<CodeSection, rkyv::rancor::Error>(archived)?,
        None => CodeSection::default(),
    };
    let g = RepoGraph {
        repo: core.repo,
        nodes: core.nodes,
        edges: core.edges,
        nav: code.nav.to_owned(&core.node_kinds),
        symbols: code.symbols.to_owned_table(),
        unresolved_calls: code.unresolved_calls,
        unresolved_refs: code.unresolved_refs,
        properties: code.properties.into_iter().collect(),
    };
    Ok((g, spans))
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

/// Serialise a `RepoGraph` to a `.gmap` file (preamble + code and strings
/// sections + rkyv core, see `FORMAT_VERSION`). Writes to `<path>.tmp` first, then atomically
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

    // ------------------------------------------------------------------------
    // CD.7b: EVIDENCE interning
    // ------------------------------------------------------------------------

    /// The #[cfg(test)] switch: `g` encoded exactly as the interning writer
    /// does, with interning disabled (every EVIDENCE cell stays JSON, no
    /// `"strings"` section) - the pre-format-3 shard of the same graph.
    fn encode_repo_graph_uninterned(g: &RepoGraph) -> Vec<u8> {
        encode_shard(g, false, None).unwrap().bytes
    }

    /// `n` edges over 5 emitters, 7 rules and 50 files, every basis, lines up
    /// to 3 varint bytes, some optional fields absent - the shape a real
    /// shard's EVIDENCE has.
    fn evidence_graph(n: u64) -> RepoGraph {
        use glia_core::{Confidence, Node};
        let repo = RepoId::from_canonical("test://cd7b-shrink");
        let emitters = ["graph:calls", "graph:imports", "parser:rust", "graph:nav", "pass:tests"];
        let rules = [
            "import_binding",
            "module_symbol",
            "receiver_type",
            "self_method",
            "intra_file",
            "global_unique",
            "enum_member",
        ];
        let bases = [Basis::Site, Basis::FromNode, Basis::ToNode, Basis::File, Basis::None];
        let nodes: Vec<Node> = (0..n + 1)
            .map(|i| Node { id: NodeId(i), repo, confidence: Confidence::Strong, cells: vec![] })
            .collect();
        let edges = (0..n)
            .map(|i| {
                let mut ev = Evidence::emitter(emitters[(i % 5) as usize]);
                if i % 4 != 0 {
                    ev = ev.rule(rules[(i % 7) as usize]);
                }
                if i % 9 != 0 {
                    let file = format!("crates/engine/src/module_{:02}/file_{}.rs", i % 50, i % 50);
                    ev = ev.at(file, (i * 37 % 40_000) as u32);
                }
                ev.basis = bases[(i % 5) as usize];
                Edge::new(NodeId(i), NodeId(i + 1), edge_category::CALLS, Confidence::Strong)
                    .with_cell(ev.to_cell())
            })
            .collect();
        RepoGraph {
            repo,
            nodes,
            edges,
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        }
    }

    #[test]
    fn evidence_bytes_shrink() {
        let g = evidence_graph(10_000);
        let plain = encode_repo_graph_uninterned(&g);
        let encoded = encode_repo_graph_counted(&g, None).unwrap();
        assert_eq!(encoded.evidence, InternStats { interned: 10_000, kept_json: 0 });
        assert!(encoded.has_strings && !encoded.has_code);
        let json_bytes: usize = g
            .edges
            .iter()
            .map(|e| match &e.cells[0].payload {
                CellPayload::Json(s) => s.len(),
                other => panic!("fixture EVIDENCE is not JSON: {other:?}"),
            })
            .sum();
        let saved = plain.len() - encoded.bytes.len();
        eprintln!(
            "[cd7b] shrink: edges=10000 json={json_bytes}B uninterned={}B interned={}B \
             saved={saved}B ({} per edge)",
            plain.len(),
            encoded.bytes.len(),
            saved / 10_000
        );
        assert!(json_bytes / 10_000 >= 80, "fixture payloads are not real-sized: {json_bytes}");
        assert!(saved >= 700_000, "interning saved only {saved} bytes over 10,000 edges");

        // The smaller file is the same graph.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("shrink.gmap");
        std::fs::write(&path, &encoded.bytes).unwrap();
        assert_eq!(decode_repo_graph(&MmapContainer::open(&path).unwrap()).unwrap().edges, g.edges);
    }

    #[test]
    fn varints_round_trip_at_every_width() {
        for v in [0u64, 1, 127, 128, 16_383, 16_384, u64::from(u32::MAX), u64::MAX - 1, u64::MAX] {
            let mut out = Vec::new();
            put_varint(&mut out, v);
            let mut at = 0;
            assert_eq!(take_varint(&out, &mut at, "v"), Ok(v), "{v}");
            assert_eq!(at, out.len(), "{v}: every byte consumed");
            let mut at = 0;
            assert!(take_varint(&out[..out.len() - 1], &mut at, "v").is_err(), "{v}: truncated");
        }
        let mut at = 0;
        assert!(take_varint(&[0xff; 11], &mut at, "v").unwrap_err().contains("overflows"));
    }

    /// The table is filled in first-seen edge order and shared by every
    /// payload; an absent rule / file / line is index 0 and costs one byte.
    #[test]
    fn interning_is_first_seen_and_compact() {
        let cell = |ev: Evidence| ev.to_cell();
        let mut edges = vec![
            Edge::new(NodeId(1), NodeId(2), edge_category::CALLS, glia_core::Confidence::Strong)
                .with_cell(cell(
                    Evidence::emitter("graph:calls").rule("module_symbol").at("a.rs", 4),
                )),
            Edge::new(NodeId(2), NodeId(3), edge_category::CALLS, glia_core::Confidence::Strong)
                .with_cell(cell(Evidence::emitter("graph:nav"))),
            Edge::new(NodeId(3), NodeId(4), edge_category::CALLS, glia_core::Confidence::Strong)
                .with_cell(cell(Evidence::emitter("graph:calls").at("a.rs", 0))),
        ];
        let before = edges.clone();
        let mut table = StringTable::default();
        let stats = intern_evidence(&mut edges, &mut table);
        assert_eq!(stats, InternStats { interned: 3, kept_json: 0 });
        assert_eq!(table.strings, vec!["graph:calls", "module_symbol", "a.rs", "graph:nav"]);
        let bytes = |e: &Edge| match &e.cells[0].payload {
            CellPayload::Bytes(b) => b.clone(),
            other => panic!("not interned: {other:?}"),
        };
        assert_eq!(bytes(&edges[0]), vec![0x01, 0, 2, 3, 5, basis_byte(Basis::Site)]);
        assert_eq!(bytes(&edges[1]), vec![0x01, 3, 0, 0, 0, basis_byte(Basis::None)]);
        assert_eq!(bytes(&edges[2]), vec![0x01, 0, 0, 3, 1, basis_byte(Basis::Site)]);

        // Expanding against the same table gives the exact JSON back.
        let archived = rkyv::to_bytes::<rkyv::rancor::Error>(&table).unwrap();
        let t = rkyv::access::<ArchivedStringTable, rkyv::rancor::Error>(&archived).unwrap();
        expand_evidence(&mut edges, t).unwrap();
        assert_eq!(edges, before);
    }

    /// A payload the writer did not produce never decodes to something else:
    /// every malformation is a reason, never a panic or a wrong evidence.
    #[test]
    fn malformed_payloads_are_reasons() {
        let table = ["graph:calls"];
        let get = |i: usize| table.get(i).copied();
        for (bytes, why) in [
            (vec![0x01], "truncated varint (emitter)"),
            (vec![0x01, 1, 0, 0, 0, 1], "emitter index 1 outside the 1-entry strings table"),
            (vec![0x01, 0, 2, 0, 0, 1], "rule index 1 outside"),
            (vec![0x01, 0, 0, 0, 0], "no basis byte"),
            (vec![0x01, 0, 0, 0, 0, 9], "unknown basis byte 9"),
            (vec![0x01, 0, 0, 0, 0, 1, 0], "1 trailing byte(s)"),
            (vec![0x01, 0, 0, 0, 0x81, 0x80, 0x80, 0x80, 0x10, 1], "past u32"),
        ] {
            let got = decode_evidence(&bytes, 1, &get).unwrap_err();
            assert!(got.contains(why), "{bytes:?}: {got}");
        }
        assert_eq!(
            decode_evidence(&[0x01, 0, 0, 0, 0, 1], 1, &get).unwrap().to_cell().payload,
            CellPayload::Json(r#"{"emitter":"graph:calls","basis":"site"}"#.into())
        );
    }

    /// An EVIDENCE cell that already holds the interned form (a raw core taken
    /// out of its file) is refused at write time, never written against the
    /// wrong table; a VECTOR `Bytes` cell with the same first byte is not
    /// EVIDENCE and is written as is.
    #[test]
    fn raw_interned_evidence_is_not_written() {
        let mut g = evidence_graph(2);
        g.edges[1].cells[0].payload = CellPayload::Bytes(vec![0x01, 0, 0, 0, 0, 1]);
        match encode_repo_graph(&g) {
            Err(StoreError::Invalid(why)) => assert!(why.contains("edge 1"), "{why}"),
            other => panic!("expected Invalid, got {:?}", other.map(|b| b.len())),
        }
        let mut g = evidence_graph(2);
        g.edges[1].cells.push(glia_core::Cell {
            kind: cell_type::VECTOR,
            payload: CellPayload::Bytes(vec![0x01, 7, 7]),
        });
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("vector.gmap");
        write_repo_graph(&g, &path).unwrap();
        assert_eq!(decode_repo_graph(&MmapContainer::open(&path).unwrap()).unwrap().edges, g.edges);
    }

    /// CD.7c: the span scan finds a text at any byte of its line, byte
    /// offsets past multi-byte chars, and nothing off the line or past EOF.
    #[test]
    fn find_on_line_is_byte_exact_and_line_bounded() {
        let f = SourceFile {
            bytes: "é = 1\n    fn b() {}\nfn c() {}".as_bytes().to_vec(),
            line_starts: OnceCell::new(),
        };
        let at_b = f.bytes.windows(2).position(|w| w == b"fn").unwrap();
        assert_eq!(f.find_on_line(1, b"fn b() {}"), Some(at_b));
        assert_eq!(f.find_on_line(1, b"fn b() {}\nfn c"), Some(at_b), "a text may run past its line");
        assert_eq!(f.find_on_line(0, b"fn b() {}"), None, "starts on line 1, not 0");
        assert_eq!(f.find_on_line(2, b"fn c() {}"), Some(f.bytes.len() - 9));
        assert_eq!(f.find_on_line(3, b"fn c() {}"), None, "no line 3");
        assert_eq!(f.find_on_line(0, b"= 1"), Some(3), "after the two-byte e-acute");
        assert_eq!(f.find_on_line(0, b""), None);
    }

    /// CD.7c: a CODE cell already in the on-disk span form is refused like raw
    /// EVIDENCE (its file index points into another file's table); other CODE
    /// bytes are written as they are.
    #[test]
    fn raw_code_span_is_not_written() {
        let node = |payload: CellPayload| glia_core::Node {
            id: NodeId(3),
            repo: RepoId(1),
            confidence: glia_core::Confidence::Strong,
            cells: vec![glia_core::Cell { kind: cell_type::CODE, payload }],
        };
        let mut g = evidence_graph(1);
        g.nodes = vec![node(CellPayload::Bytes(CodeSpan::of("a.rs", 0, b"x").encode(0)))];
        match encode_repo_graph(&g) {
            Err(StoreError::Invalid(why)) => assert!(why.contains("node 0"), "{why}"),
            other => panic!("expected Invalid, got {:?}", other.map(|b| b.len())),
        }
        g.nodes = vec![node(CellPayload::Bytes(vec![0x03, 1]))];
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bytes.gmap");
        write_repo_graph(&g, &path).unwrap();
        assert_eq!(decode_repo_graph(&MmapContainer::open(&path).unwrap()).unwrap().nodes, g.nodes);
    }

    /// CD.7c: a span whose file index is outside the table is `Corrupt`
    /// naming the node, like a malformed interned EVIDENCE.
    #[test]
    fn malformed_span_is_corrupt() {
        let mut nodes = vec![glia_core::Node {
            id: NodeId(9),
            repo: RepoId(1),
            confidence: glia_core::Confidence::Strong,
            cells: vec![glia_core::Cell {
                kind: cell_type::CODE,
                payload: CellPayload::Bytes(CodeSpan::of("a.rs", 0, b"x").encode(4)),
            }],
        }];
        match expand_code_spans(&mut nodes, 1, &|i| (i == 0).then_some("a.rs"), None) {
            Err(StoreError::Corrupt { detail }) => {
                assert!(detail.contains("CODE span of node 0 (9)"), "{detail}")
            }
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }
}
