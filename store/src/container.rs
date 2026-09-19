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
    ops::Range,
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
///
/// It lives in two places in every file: the fixed preamble (read before rkyv
/// touches anything, so an old or foreign file reports `OldFormat` /
/// `FutureFormat` instead of an rkyv validation error) and the archived
/// `Header.version` (checked again after validation). Version 2 (0.5.0) added
/// the preamble; every 0.4.x file has none. The file layout:
///
/// ```text
/// [0..8)   PREAMBLE_MAGIC = b"GLIAGMAP"
/// [8..12)  format_version: u32 LE   (= FORMAT_VERSION)
/// [12..16) flags: u32 LE            (0; reserved, ignored on read)
/// [16..24) core_offset: u64 LE      (multiple of 16; 32 = right after the preamble)
/// [24..32) core_len: u64 LE
/// [core_offset .. core_offset + core_len)   the rkyv archive of `Container`
/// ```
///
/// Bytes 0..12 are frozen across every future version: a reader must always be
/// able to name the version of a file it cannot read. This number is glia's
/// store format and is unrelated to engram-core's `GMAP_FORMAT_VERSION`.
pub const FORMAT_VERSION: u32 = 2;

/// First 8 bytes of every `.gmap` since format 2.
pub(crate) const PREAMBLE_MAGIC: [u8; 8] = *b"GLIAGMAP";
/// Preamble length. A multiple of 16, so a core placed right after it stays
/// 16-aligned in a page-aligned mmap (rkyv's `AlignedVec<16>` requirement).
pub(crate) const PREAMBLE_LEN: usize = 32;
/// Alignment the core offset must honour for `rkyv::access` to validate.
const CORE_ALIGN: usize = 16;
/// `Corrupt` detail for a same-version archive that does not validate.
const ARCHIVE_INVALID: &str = "archive does not validate (same format version, different \
layout: a dev build from another commit, or a damaged file)";

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
// File encoding — fixed preamble + rkyv core (see FORMAT_VERSION)
// ============================================================================

/// Serialise `core` into the bytes of one `.gmap` file: the 32-byte preamble,
/// then the rkyv archive. Every writer goes through here, so no file can be
/// written without the preamble. Deterministic: the same `Container` always
/// encodes to the same bytes (the shard `content_hash` covers the preamble).
pub(crate) fn encode_file(core: &Container) -> Result<Vec<u8>, StoreError> {
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(core)?;
    let mut out = Vec::with_capacity(PREAMBLE_LEN + archive.len());
    out.extend_from_slice(&PREAMBLE_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(PREAMBLE_LEN as u64).to_le_bytes());
    out.extend_from_slice(&(archive.len() as u64).to_le_bytes());
    out.extend_from_slice(&archive);
    Ok(out)
}

/// Read the preamble of a whole `.gmap` file and return the byte range of its
/// rkyv core. Runs before rkyv sees a byte, so an old or foreign file never
/// reaches bytecheck:
/// - no `GLIAGMAP` magic -> `OldFormat { found: None }` (every 0.4.x file: rkyv
///   puts its root at the END, so those files open with out-of-line data);
/// - an older / newer version -> `OldFormat { found: Some(v) }` / `FutureFormat`;
/// - a truncated preamble, or a core range outside the file, overlapping the
///   preamble or not 16-aligned -> `Corrupt`.
pub(crate) fn split_file(bytes: &[u8]) -> Result<Range<usize>, StoreError> {
    if bytes.len() < PREAMBLE_MAGIC.len() || bytes[..PREAMBLE_MAGIC.len()] != PREAMBLE_MAGIC {
        return Err(StoreError::OldFormat { found: None });
    }
    if bytes.len() < PREAMBLE_LEN {
        return Err(StoreError::Corrupt {
            detail: format!("truncated preamble ({} of {PREAMBLE_LEN} bytes)", bytes.len()),
        });
    }
    let version = le_u32(bytes, 8);
    if version < FORMAT_VERSION {
        return Err(StoreError::OldFormat { found: Some(version) });
    }
    if version > FORMAT_VERSION {
        return Err(StoreError::FutureFormat { found: version });
    }
    let (offset, len) = (le_u64(bytes, 16), le_u64(bytes, 24));
    let out_of_file = || StoreError::Corrupt {
        detail: format!(
            "core range {offset}+{len} lies outside the {}-byte file",
            bytes.len()
        ),
    };
    let start = usize::try_from(offset).map_err(|_| out_of_file())?;
    let len = usize::try_from(len).map_err(|_| out_of_file())?;
    let end = start.checked_add(len).ok_or_else(out_of_file)?;
    if start < PREAMBLE_LEN || end > bytes.len() {
        return Err(out_of_file());
    }
    if start % CORE_ALIGN != 0 {
        return Err(StoreError::Corrupt {
            detail: format!("core offset {start} is not {CORE_ALIGN}-byte aligned"),
        });
    }
    Ok(start..end)
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(b)
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(b)
}

// ============================================================================
// Read — mmap zero-copy
// ============================================================================

/// Owns the mmap'd bytes and exposes a borrowed `&ArchivedContainer` view.
///
/// The archived view is reborrowed out of the `core` range of `bytes` via
/// `rkyv::access`. We hold the `Mmap` alive for the lifetime of
/// `MmapContainer` so the borrow stays valid; the `archived()` accessor returns
/// a borrow tied to `&self`, which is exactly what the caller wants — they
/// can't outlive the mmap.
pub struct MmapContainer {
    bytes: Mmap,
    /// The rkyv core inside `bytes`, as named by the preamble (`split_file`).
    core: Range<usize>,
}

impl MmapContainer {
    /// Open a `.gmap` file zero-copy. Reads the preamble first (old, future
    /// and foreign files fail here, before rkyv), then validates the archive
    /// and its header magic + version; the file stays mmap'd for the life of
    /// the returned struct.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let f = File::open(path)?;
        // Safety: the file is opened read-only and Mmap is read-only. Other
        // processes mutating this exact path while we hold the mmap could
        // surprise us, but the write path uses tmp+rename so concurrent
        // writers replace the inode rather than mutate it in place — our
        // mmap stays pinned to the old inode until we drop.
        let bytes = unsafe { MmapOptions::new().map(&f)? };
        let core = split_file(&bytes)?;
        let s = Self { bytes, core };
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
    /// `rkyv::access` call over the core range. Cache the result if you call it
    /// in a hot loop. An archive that does not validate is `Corrupt`: the
    /// preamble already matched this build's version, so the only causes left
    /// are a same-number dev build with another layout or a damaged file.
    pub fn archived(&self) -> Result<&ArchivedContainer, StoreError> {
        let core = self.bytes.get(self.core.clone()).ok_or_else(|| StoreError::Corrupt {
            detail: format!("core range {:?} outside the mapped file", self.core),
        })?;
        rkyv::access::<ArchivedContainer, rkyv::rancor::Error>(core).map_err(|_| {
            StoreError::Corrupt {
                detail: ARCHIVE_INVALID.to_string(),
            }
        })
    }

    /// Raw mmap byte length (preamble included) — useful for diagnostics and
    /// size-on-disk asserts.
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

    let bytes = encode_file(&container)?;
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
        let bytes = encode_file(&container)?;
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

    fn empty_container() -> Container {
        Container {
            header: Header::for_code(),
            repo: RepoId::from_canonical("test://empty"),
            nodes: Vec::new(),
            edges: Vec::new(),
            code_nav: CodeNavStore::default(),
            symbols: SymbolTableStore::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
        }
    }

    /// Encode, split, and copy the core into a 16-aligned buffer (a plain
    /// `Vec<u8>` carries no alignment guarantee; an mmap base is page aligned).
    fn encoded_core(c: &Container) -> rkyv::util::AlignedVec<16> {
        let file = encode_file(c).unwrap();
        let range = split_file(&file).unwrap();
        let mut core = rkyv::util::AlignedVec::<16>::new();
        core.extend_from_slice(&file[range]);
        core
    }

    #[test]
    fn header_round_trips_via_rkyv() {
        let core = encoded_core(&empty_container());
        let archived =
            rkyv::access::<ArchivedContainer, rkyv::rancor::Error>(&core).unwrap();
        assert_eq!(archived.header.magic, MAGIC);
        assert_eq!(archived.header.version.to_native(), FORMAT_VERSION);
        assert_eq!(archived.header.graph_type.as_str(), "code");
    }

    #[test]
    fn empty_container_round_trips() {
        let core = encoded_core(&empty_container());
        let archived =
            rkyv::access::<ArchivedContainer, rkyv::rancor::Error>(&core).unwrap();
        assert_eq!(archived.nodes.len(), 0);
        assert_eq!(archived.edges.len(), 0);
    }

    #[test]
    fn preamble_layout_is_fixed() {
        let file = encode_file(&empty_container()).unwrap();
        assert_eq!(&file[0..8], b"GLIAGMAP");
        assert_eq!(&file[8..12], &FORMAT_VERSION.to_le_bytes());
        assert_eq!(&file[12..16], &[0u8; 4], "flags are reserved as 0");
        assert_eq!(&file[16..24], &(PREAMBLE_LEN as u64).to_le_bytes());
        let core_len = (file.len() - PREAMBLE_LEN) as u64;
        assert_eq!(&file[24..32], &core_len.to_le_bytes());
        assert_eq!(split_file(&file).unwrap(), PREAMBLE_LEN..file.len());
        // Deterministic: byte_identical builds depend on it.
        assert_eq!(file, encode_file(&empty_container()).unwrap());
    }

    #[test]
    fn split_file_classifies_what_it_cannot_read() {
        let file = encode_file(&empty_container()).unwrap();
        assert!(matches!(split_file(b""), Err(StoreError::OldFormat { found: None })));
        assert!(matches!(
            split_file(b"def f():\n    return 1\n pre-0.5.0 out-of-line bytes"),
            Err(StoreError::OldFormat { found: None })
        ));
        assert!(matches!(split_file(&file[..20]), Err(StoreError::Corrupt { .. })));
        let mut v1 = file.clone();
        v1[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert!(matches!(split_file(&v1), Err(StoreError::OldFormat { found: Some(1) })));
        let mut v9 = file.clone();
        v9[8..12].copy_from_slice(&9u32.to_le_bytes());
        assert!(matches!(split_file(&v9), Err(StoreError::FutureFormat { found: 9 })));
        let mut overlap = file.clone();
        overlap[16..24].copy_from_slice(&16u64.to_le_bytes());
        assert!(matches!(split_file(&overlap), Err(StoreError::Corrupt { .. })));
        let mut huge = file;
        huge[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(split_file(&huge), Err(StoreError::Corrupt { .. })));
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
        let bytes = encode_file(&container).unwrap();
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
