//! The `.gmap` container: the rkyv-archived top-level `Container` / `Header`
//! shape, the atomic write primitive, the zero-copy `MmapContainer` reader,
//! single-file cell mutation, and the accessors on `ArchivedContainer`.
//!
//! This is the domain seam: LC.5b makes this file domain-free. Until then the
//! `Container` fields still name the code-domain sections (`CodeNavStore`,
//! `SymbolTableStore` from `code_section`, `CallSite` / `UnresolvedRef` from
//! code-domain), and the code-domain constructors (`Header::for_code`,
//! `Container::from_repo_graph` / `to_repo_graph` / `for_cross_edges`) live in
//! `code_section` as inherent impls.

use std::{
    fs::{File, OpenOptions, rename},
    io::Write,
    path::{Path, PathBuf},
};

use memmap2::{Mmap, MmapOptions};
use repo_graph_code_domain::{CallSite, UnresolvedRef};
use repo_graph_core::{Cell, CellPayload, CellTypeId, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId};

use crate::code_section::{CodeNavStore, SymbolTableStore};
use crate::error::StoreError;

// ============================================================================
// Container shape
// ============================================================================

/// Magic bytes carried in the container header — `b"GMAP"`.
pub const MAGIC: [u8; 4] = *b"GMAP";

/// Format version. Bump on any layout change. Loader rejects mismatches.
pub const FORMAT_VERSION: u32 = 1;

/// Top-level on-disk shape. Owned form = what the writer builds; Archived form
/// = `&ArchivedContainer`, what mmap returns.
#[derive(Debug, Clone, PartialEq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Container {
    pub header: Header,
    pub repo: RepoId,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub code_nav: CodeNavStore,
    pub symbols: SymbolTableStore,
    pub unresolved_calls: Vec<CallSite>,
    pub unresolved_refs: Vec<UnresolvedRef>,
}

/// File header. Magic + version are checked on load. Registries are
/// diagnostic-only at v0.4.5a — not load-bearing — but they let a future
/// `gmap inspect` command name the u32 ids.
#[derive(Debug, Clone, PartialEq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Header {
    pub magic: [u8; 4],
    pub version: u32,
    pub graph_type: String,
    pub cell_registry: Vec<RegistryEntry>,
    pub edge_category_registry: Vec<RegistryEntry>,
    pub node_kind_registry: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, PartialEq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct RegistryEntry {
    pub id: u32,
    pub name: String,
}

// ============================================================================
// Write — atomic via .tmp + rename
// ============================================================================

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let tmp_path: PathBuf = with_tmp_suffix(path);
    {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    rename(&tmp_path, path)?;
    Ok(())
}

fn with_tmp_suffix(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

/// xxhash64 of `bytes`, hex-encoded (16 lowercase hex chars) — the shard
/// content hash the sharded manifest records and verifies.
pub(crate) fn hex_xxhash64(bytes: &[u8]) -> String {
    use core::hash::Hasher;
    use twox_hash::XxHash64;
    let mut h = XxHash64::with_seed(0);
    h.write(bytes);
    format!("{:016x}", h.finish())
}

// ============================================================================
// Read — mmap zero-copy
// ============================================================================

/// Owns the mmap'd bytes and exposes a borrowed `&ArchivedContainer` view.
///
/// The archived view is reborrowed out of `bytes` via `rkyv::access`. We hold
/// the `Mmap` alive for the lifetime of `MmapContainer` so the borrow stays
/// valid; the `archived()` accessor returns a borrow tied to `&self`, which is
/// exactly what the caller wants — they can't outlive the mmap.
pub struct MmapContainer {
    bytes: Mmap,
}

impl MmapContainer {
    /// Open a `.gmap` file zero-copy. Validates magic + version on first
    /// access; the file stays mmap'd for the life of the returned struct.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let f = File::open(path)?;
        // Safety: the file is opened read-only and Mmap is read-only. Other
        // processes mutating this exact path while we hold the mmap could
        // surprise us, but the write path uses tmp+rename so concurrent
        // writers replace the inode rather than mutate it in place — our
        // mmap stays pinned to the old inode until we drop.
        let bytes = unsafe { MmapOptions::new().map(&f)? };
        let s = Self { bytes };
        s.validate_header()?;
        Ok(s)
    }

    fn validate_header(&self) -> Result<(), StoreError> {
        let archived = self.archived()?;
        let magic_bytes: [u8; 4] = [
            archived.header.magic[0],
            archived.header.magic[1],
            archived.header.magic[2],
            archived.header.magic[3],
        ];
        if magic_bytes != MAGIC {
            return Err(StoreError::BadMagic {
                expected: MAGIC,
                got: magic_bytes,
            });
        }
        let v = archived.header.version.to_native();
        if v != FORMAT_VERSION {
            return Err(StoreError::UnsupportedVersion(v, FORMAT_VERSION));
        }
        Ok(())
    }

    /// Borrow the archived container for the life of `self`. Cheap: a single
    /// `rkyv::access` call. Cache the result if you call it in a hot loop.
    pub fn archived(&self) -> Result<&ArchivedContainer, StoreError> {
        Ok(rkyv::access::<ArchivedContainer, rkyv::rancor::Error>(
            &self.bytes,
        )?)
    }

    /// Raw mmap byte length — useful for diagnostics and size-on-disk asserts.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

// ============================================================================
// Mutation — read-modify-write cycle for cell operations
// ============================================================================

/// Deserialize an archived `.gmap` back to an owned `Container`. Used by
/// mutation operations that need to modify and re-write.
pub fn read_to_owned(path: &Path) -> Result<Container, StoreError> {
    let mmap = MmapContainer::open(path)?;
    let archived = mmap.archived()?;
    let owned: Container =
        rkyv::deserialize::<Container, rkyv::rancor::Error>(archived)?;
    Ok(owned)
}

/// Insert or replace a cell on a node. Full read-modify-write cycle:
/// opens the `.gmap`, deserializes to owned form, mutates, re-serializes
/// atomically. If a cell of the same type already exists on the node, its
/// payload is replaced; otherwise a new cell is appended.
pub fn upsert_cell(
    path: &Path,
    node_id: NodeId,
    cell_type: CellTypeId,
    payload: CellPayload,
) -> Result<(), StoreError> {
    let mut container = read_to_owned(path)?;
    let node = container
        .nodes
        .iter_mut()
        .find(|n| n.id == node_id)
        .ok_or(StoreError::NodeNotFound(node_id))?;

    if let Some(cell) = node.cells.iter_mut().find(|c| c.kind == cell_type) {
        cell.payload = payload;
    } else {
        node.cells.push(Cell {
            kind: cell_type,
            payload,
        });
    }

    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&container)?;
    write_atomic(path, &bytes)
}

/// Remove a cell of a given type from a node. Returns `Ok(true)` if a cell
/// was removed, `Ok(false)` if the node existed but had no cell of that type.
/// Returns `NodeNotFound` if the node id isn't in the container.
pub fn remove_cell(
    path: &Path,
    node_id: NodeId,
    cell_type: CellTypeId,
) -> Result<bool, StoreError> {
    let mut container = read_to_owned(path)?;
    let node = container
        .nodes
        .iter_mut()
        .find(|n| n.id == node_id)
        .ok_or(StoreError::NodeNotFound(node_id))?;

    let before = node.cells.len();
    node.cells.retain(|c| c.kind != cell_type);
    let removed = node.cells.len() < before;

    if removed {
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&container)?;
        write_atomic(path, &bytes)?;
    }
    Ok(removed)
}

// ============================================================================
// Convenience accessors on the archived form
// ============================================================================

impl ArchivedContainer {
    /// Walk the archived edges, yielding `(from, to, category)`. Useful for
    /// cheap edge counts and traversal without rehydrating into owned types.
    pub fn edges_iter(&self) -> impl Iterator<Item = (NodeId, NodeId, EdgeCategoryId)> + '_ {
        self.edges.iter().map(|e| {
            (
                NodeId(e.from.0.to_native()),
                NodeId(e.to.0.to_native()),
                EdgeCategoryId(e.category.0.to_native()),
            )
        })
    }

    /// Look up a node id's qname via binary search on the sorted nav vec.
    pub fn qname(&self, id: NodeId) -> Option<&str> {
        let pairs = &self.code_nav.qname_by_id;
        let i = pairs
            .binary_search_by(|entry| entry.0.0.to_native().cmp(&id.0))
            .ok()?;
        Some(pairs[i].1.as_str())
    }

    /// Look up a node id's kind via binary search.
    pub fn kind(&self, id: NodeId) -> Option<NodeKindId> {
        let pairs = &self.code_nav.kind_by_id;
        let i = pairs
            .binary_search_by(|entry| entry.0.0.to_native().cmp(&id.0))
            .ok()?;
        Some(NodeKindId(pairs[i].1.0.to_native()))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips_via_rkyv() {
        let h = Header::for_code();
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&h).unwrap();
        let archived =
            rkyv::access::<ArchivedHeader, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(archived.magic, MAGIC);
        assert_eq!(archived.version.to_native(), FORMAT_VERSION);
        assert_eq!(archived.graph_type.as_str(), "code");
    }

    #[test]
    fn empty_container_round_trips() {
        let c = Container {
            header: Header::for_code(),
            repo: RepoId::from_canonical("test://empty"),
            nodes: Vec::new(),
            edges: Vec::new(),
            code_nav: CodeNavStore::default(),
            symbols: SymbolTableStore::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&c).unwrap();
        let archived =
            rkyv::access::<ArchivedContainer, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(archived.nodes.len(), 0);
        assert_eq!(archived.edges.len(), 0);
    }

    fn make_test_container() -> Container {
        Container {
            header: Header::for_code(),
            repo: RepoId::from_canonical("test://cells"),
            nodes: vec![
                Node {
                    id: NodeId(100),
                    repo: RepoId::from_canonical("test://cells"),
                    confidence: repo_graph_core::Confidence::Strong,
                    cells: vec![Cell {
                        kind: CellTypeId(1),
                        payload: CellPayload::Text("fn main() {}".into()),
                    }],
                },
                Node {
                    id: NodeId(200),
                    repo: RepoId::from_canonical("test://cells"),
                    confidence: repo_graph_core::Confidence::Strong,
                    cells: vec![],
                },
            ],
            edges: Vec::new(),
            code_nav: CodeNavStore::default(),
            symbols: SymbolTableStore::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
        }
    }

    fn write_test_gmap(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("test.gmap");
        let container = make_test_container();
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&container).unwrap();
        write_atomic(&path, &bytes).unwrap();
        path
    }

    #[test]
    fn read_to_owned_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_gmap(dir.path());
        let owned = read_to_owned(&path).unwrap();
        assert_eq!(owned.nodes.len(), 2);
        assert_eq!(owned.nodes[0].id, NodeId(100));
        assert_eq!(owned.nodes[0].cells.len(), 1);
    }

    #[test]
    fn upsert_cell_adds_new_cell() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_gmap(dir.path());

        upsert_cell(
            &path,
            NodeId(200),
            CellTypeId(13),
            CellPayload::Text("conversation here".into()),
        )
        .unwrap();

        let owned = read_to_owned(&path).unwrap();
        let node = owned.nodes.iter().find(|n| n.id == NodeId(200)).unwrap();
        assert_eq!(node.cells.len(), 1);
        assert_eq!(node.cells[0].kind, CellTypeId(13));
    }

    #[test]
    fn upsert_cell_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_gmap(dir.path());

        upsert_cell(
            &path,
            NodeId(100),
            CellTypeId(1),
            CellPayload::Text("fn main() { updated }".into()),
        )
        .unwrap();

        let owned = read_to_owned(&path).unwrap();
        let node = owned.nodes.iter().find(|n| n.id == NodeId(100)).unwrap();
        assert_eq!(node.cells.len(), 1);
        assert!(matches!(&node.cells[0].payload, CellPayload::Text(s) if s.contains("updated")));
    }

    #[test]
    fn upsert_cell_node_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_gmap(dir.path());

        let err = upsert_cell(
            &path,
            NodeId(999),
            CellTypeId(1),
            CellPayload::Text("nope".into()),
        );
        assert!(matches!(err, Err(StoreError::NodeNotFound(NodeId(999)))));
    }

    #[test]
    fn remove_cell_removes_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_gmap(dir.path());

        let removed = remove_cell(&path, NodeId(100), CellTypeId(1)).unwrap();
        assert!(removed);

        let owned = read_to_owned(&path).unwrap();
        let node = owned.nodes.iter().find(|n| n.id == NodeId(100)).unwrap();
        assert!(node.cells.is_empty());
    }

    #[test]
    fn remove_cell_returns_false_for_missing_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_gmap(dir.path());

        let removed = remove_cell(&path, NodeId(100), CellTypeId(99)).unwrap();
        assert!(!removed);
    }
}
