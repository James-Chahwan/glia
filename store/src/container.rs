//! The `.gmap` container: the domain-free rkyv core (`Container` / `Header`),
//! the named byte-range sections every domain writes beside it, the atomic
//! write primitive, the zero-copy `MmapContainer` reader, single-file cell
//! mutation, and the accessors on `ArchivedContainer`.
//!
//! This is the domain seam (LC.5b). The core is header + repo + nodes + edges +
//! `node_kinds` (every domain has kinds, and a reader cannot name a node
//! without them) + the section table. Anything a domain owns beyond that - the
//! code domain's nav maps, symbol table and unresolved refs, a video domain's
//! frame index - is its own rkyv archive, written as a named section between the
//! preamble and the core and read back with `MmapContainer::section`. Nothing in
//! this file names a domain type: the code domain's section and its
//! `RepoGraph` codec live in `code_section`.

use std::{
    fs::{File, OpenOptions, rename},
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
};

use memmap2::{Mmap, MmapOptions};
use repo_graph_core::{Cell, CellPayload, CellTypeId, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId};
use rkyv::util::AlignedVec;

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
/// [32 .. core_offset)                        the sections, each its own rkyv
///                                            archive at a 16-aligned offset,
///                                            zero-padded, located by the core's
///                                            section table (absolute offsets)
/// [core_offset .. core_offset + core_len)   the rkyv archive of `Container`
/// ```
///
/// A file with no sections has `core_offset` 32. Bytes 0..12 are frozen across
/// every future version: a reader must always be able to name the version of a
/// file it cannot read. This number is glia's store format and is unrelated to
/// engram-core's `GMAP_FORMAT_VERSION`.
pub const FORMAT_VERSION: u32 = 2;

/// First 8 bytes of every `.gmap` since format 2.
pub(crate) const PREAMBLE_MAGIC: [u8; 8] = *b"GLIAGMAP";
/// Preamble length. A multiple of 16, so a core placed right after it stays
/// 16-aligned in a page-aligned mmap (rkyv's `AlignedVec<16>` requirement).
pub(crate) const PREAMBLE_LEN: usize = 32;
/// Alignment the core offset and every section offset honour for
/// `rkyv::access` to validate (`AlignedVec<16>`, over a page-aligned mmap).
const CORE_ALIGN: usize = 16;
/// `Corrupt` detail for a same-version archive that does not validate.
const ARCHIVE_INVALID: &str = "archive does not validate (same format version, different \
layout: a dev build from another commit, or a damaged file)";

/// Top-level on-disk shape: the domain-free core. Owned form = what the writer
/// builds; Archived form = `&ArchivedContainer`, what mmap returns.
#[derive(Debug, Clone, PartialEq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Container {
    pub header: Header,
    pub repo: RepoId,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Every node's kind, sorted by id (the writer sorts it) so
    /// `ArchivedContainer::kind` binary-searches it. Core state, not a domain
    /// section: every domain has kinds, and `Header.node_kind_registry` names
    /// them.
    pub node_kinds: Vec<(NodeId, NodeKindId)>,
    /// The section table. Filled by the writer (`write_container` / the
    /// `RepoGraph` codec) with the layout it chose; ignored on input.
    pub sections: Vec<SectionEntry>,
}

/// One named section: an rkyv archive at an absolute, 16-aligned file offset
/// between the preamble and the core.
#[derive(Debug, Clone, PartialEq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct SectionEntry {
    pub name: String,
    pub offset: u64,
    pub len: u64,
}

/// A section's archived bytes, ready to lay out: what `encode_section` builds
/// and what `read_to_owned` copies back out of a file, verbatim.
#[derive(Debug, Clone)]
pub struct EncodedSection {
    pub name: String,
    pub bytes: AlignedVec<16>,
}

impl PartialEq for EncodedSection {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.bytes.as_slice() == other.bytes.as_slice()
    }
}

/// A whole file read back into owned form: the core, plus every section's
/// bytes in table order. Mutation re-encodes the core and writes the sections
/// back verbatim, so a cell edit never touches a domain's section.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedFile {
    pub core: Container,
    pub sections: Vec<EncodedSection>,
}

/// File header. Magic + version are checked on load. The three registries make
/// the file self-describing (LC.4): they name every node-kind, edge-category
/// and cell-type id the writing domain knows, sorted by id, so a reader
/// (`inspect_path`, `glia inspect`) labels a file's ids from the file alone,
/// with no domain crate linked, and a file written by a newer build names the
/// ids an older reader has never heard of. Not load-bearing for decoding:
/// `Header::new` writes them empty, `Header::for_domain` fills them.
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

impl Header {
    /// A header for a graph of `graph_type` (`"code"`, `"toy"`, ...): this
    /// build's magic and format version, empty registries. A file written with
    /// it names none of its ids; `for_domain` is the self-describing form.
    pub fn new(graph_type: impl Into<String>) -> Self {
        Self {
            magic: MAGIC,
            version: FORMAT_VERSION,
            graph_type: graph_type.into(),
            cell_registry: Vec::new(),
            edge_category_registry: Vec::new(),
            node_kind_registry: Vec::new(),
        }
    }

    /// A self-describing header for a domain (LC.4): this build's magic and
    /// format version, and the domain's node-kind, edge-category and
    /// cell-type tables as `(id, name)` pairs, each copied into its registry
    /// sorted by id (the input order does not matter, so the bytes are
    /// deterministic). An id that appears twice in one table is `Corrupt`
    /// ("duplicate registry id ..."): a reader could not tell which name is
    /// meant. The same id in two different tables is fine - they are separate
    /// id spaces.
    pub fn for_domain(
        graph_type: impl Into<String>,
        kinds: &[(u32, &str)],
        categories: &[(u32, &str)],
        cells: &[(u32, &str)],
    ) -> Result<Header, StoreError> {
        Ok(Self {
            node_kind_registry: registry("node_kind", kinds)?,
            edge_category_registry: registry("edge_category", categories)?,
            cell_registry: registry("cell_type", cells)?,
            ..Self::new(graph_type)
        })
    }
}

/// `pairs` as registry entries sorted by id; a repeated id is `Corrupt`.
fn registry(table: &str, pairs: &[(u32, &str)]) -> Result<Vec<RegistryEntry>, StoreError> {
    let mut out: Vec<RegistryEntry> = pairs
        .iter()
        .map(|(id, name)| RegistryEntry { id: *id, name: (*name).to_string() })
        .collect();
    out.sort_by_key(|e| e.id);
    if let Some(w) = out.windows(2).find(|w| w[0].id == w[1].id) {
        return Err(StoreError::Corrupt {
            detail: format!(
                "duplicate registry id {} in the {table} table ({} and {})",
                w[0].id, w[0].name, w[1].name
            ),
        });
    }
    Ok(out)
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
// File encoding — fixed preamble + sections + rkyv core (see FORMAT_VERSION)
// ============================================================================

/// Archive one domain's section value on its own: `rkyv::to_bytes` into a
/// 16-aligned buffer, so it can be laid out at a 16-aligned file offset and
/// read back zero-copy with `MmapContainer::section`. The store never looks
/// inside the bytes.
pub fn encode_section<T>(name: &str, value: &T) -> Result<EncodedSection, StoreError>
where
    T: for<'a> rkyv::Serialize<
        rkyv::api::high::HighSerializer<
            AlignedVec,
            rkyv::ser::allocator::ArenaHandle<'a>,
            rkyv::rancor::Error,
        >,
    >,
{
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(value)?;
    Ok(EncodedSection { name: name.to_string(), bytes })
}

/// Round `at` up to the next multiple of `CORE_ALIGN`.
fn align_up(at: usize) -> Result<usize, StoreError> {
    at.checked_next_multiple_of(CORE_ALIGN).ok_or_else(|| StoreError::Corrupt {
        detail: format!("file layout overflows at offset {at}"),
    })
}

/// Lay `sections` out after the preamble, in slice order, each at a 16-aligned
/// offset; return the table and the (16-aligned) offset the core starts at.
/// Section names must be unique within a file.
fn layout_sections(sections: &[EncodedSection]) -> Result<(Vec<SectionEntry>, usize), StoreError> {
    let mut table: Vec<SectionEntry> = Vec::with_capacity(sections.len());
    let mut at = PREAMBLE_LEN;
    for s in sections {
        if table.iter().any(|e| e.name == s.name) {
            return Err(StoreError::Corrupt {
                detail: format!("duplicate section name '{}' in one file", s.name),
            });
        }
        at = align_up(at)?;
        table.push(SectionEntry {
            name: s.name.clone(),
            offset: at as u64,
            len: s.bytes.len() as u64,
        });
        at = at.checked_add(s.bytes.len()).ok_or_else(|| StoreError::Corrupt {
            detail: format!("section '{}' overflows the file layout", s.name),
        })?;
    }
    Ok((table, align_up(at)?))
}

/// Serialise a core and its sections into the bytes of one `.gmap` file:
///
/// ```text
/// [preamble 32B][section 0, zero-padded to 16][section 1 ...][core archive]
/// ```
///
/// Sections are laid out first, so their absolute offsets are known when the
/// core (which carries the table) is archived; the preamble's
/// `core_offset` / `core_len` then locate the core. `core.sections` is
/// overwritten with that table and `core.node_kinds` is sorted by id (a stable
/// sort) - both are the writer's to fill. Every writer goes through here, so no
/// file can be written without the preamble. Deterministic: the same core and
/// sections always encode to the same bytes (the shard `content_hash` covers
/// the whole file).
pub(crate) fn encode_file(
    core: &mut Container,
    sections: &[EncodedSection],
) -> Result<Vec<u8>, StoreError> {
    let (table, core_offset) = layout_sections(sections)?;
    core.sections = table;
    core.node_kinds.sort_by_key(|(id, _)| id.0);
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(&*core)?;
    let mut out = Vec::with_capacity(core_offset + archive.len());
    out.extend_from_slice(&PREAMBLE_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(core_offset as u64).to_le_bytes());
    out.extend_from_slice(&(archive.len() as u64).to_le_bytes());
    for (entry, s) in core.sections.iter().zip(sections) {
        // Offsets come from `layout_sections` over these same lengths, so the
        // pad is exact; `resize` only ever grows here.
        out.resize(entry.offset as usize, 0);
        out.extend_from_slice(&s.bytes);
    }
    out.resize(core_offset, 0);
    out.extend_from_slice(&archive);
    Ok(out)
}

/// Write a core and its sections to `path` atomically (`<path>.tmp` + rename).
/// The entry point for a domain that is not code: build a `Container` (its own
/// `Header::new(graph_type)`, nodes, edges, `node_kinds`), archive its state
/// with `encode_section`, and write both here. `core.sections` comes back
/// holding the table that was written; nothing is written if the table is
/// invalid (a duplicate section name).
pub fn write_container(
    path: &Path,
    core: &mut Container,
    sections: &[EncodedSection],
) -> Result<(), StoreError> {
    let bytes = encode_file(core, sections)?;
    write_atomic(path, &bytes)
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

/// Owns the mmap'd bytes and exposes a borrowed `&ArchivedContainer` view,
/// plus the named sections beside it.
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
    /// The section table, resolved to byte ranges and checked once at `open`
    /// (in bounds, 16-aligned, in order, clear of the preamble and the core,
    /// unique names), so a section lookup never re-validates the core.
    sections: Vec<(String, Range<usize>)>,
}

impl MmapContainer {
    /// Open a `.gmap` file zero-copy. Reads the preamble first (old, future
    /// and foreign files fail here, before rkyv), then validates the core
    /// archive, its header magic + version and its section table; the file
    /// stays mmap'd for the life of the returned struct. A section's own
    /// archive is validated when it is read (`section`).
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let f = File::open(path)?;
        // Safety: the file is opened read-only and Mmap is read-only. Other
        // processes mutating this exact path while we hold the mmap could
        // surprise us, but the write path uses tmp+rename so concurrent
        // writers replace the inode rather than mutate it in place — our
        // mmap stays pinned to the old inode until we drop.
        let bytes = unsafe { MmapOptions::new().map(&f)? };
        let core = split_file(&bytes)?;
        let mut s = Self { bytes, core, sections: Vec::new() };
        s.validate_header()?;
        s.sections = s.resolve_sections()?;
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

    /// Turn the archived section table into checked byte ranges. Every entry
    /// must lie between the preamble and the core, start 16-aligned, follow its
    /// predecessor without overlap, and carry a name no other entry has; any
    /// other table is `Corrupt`. Offsets are absolute file offsets and every
    /// sum is checked, so a damaged table can never slice out of bounds.
    fn resolve_sections(&self) -> Result<Vec<(String, Range<usize>)>, StoreError> {
        let archived = self.archived()?;
        let mut out: Vec<(String, Range<usize>)> = Vec::with_capacity(archived.sections.len());
        let mut prev_end = PREAMBLE_LEN;
        for entry in archived.sections.iter() {
            let name = entry.name.as_str();
            let (offset, len) = (entry.offset.to_native(), entry.len.to_native());
            let bad = |why: &str| StoreError::Corrupt {
                detail: format!("section '{name}' at {offset}+{len} {why}"),
            };
            let start = usize::try_from(offset).map_err(|_| bad("lies outside the file"))?;
            let len = usize::try_from(len).map_err(|_| bad("lies outside the file"))?;
            let end = start.checked_add(len).ok_or_else(|| bad("lies outside the file"))?;
            if start < prev_end || end > self.core.start {
                return Err(bad(&format!(
                    "is not between the previous section (ending {prev_end}) and the core \
                     (at {})",
                    self.core.start
                )));
            }
            if start % CORE_ALIGN != 0 {
                return Err(bad(&format!("is not {CORE_ALIGN}-byte aligned")));
            }
            if out.iter().any(|(n, _)| n == name) {
                return Err(bad("repeats a section name"));
            }
            out.push((name.to_string(), start..end));
            prev_end = end;
        }
        Ok(out)
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

    /// Every section this file carries, as `(name, byte length)`, in table
    /// order.
    pub fn section_names(&self) -> Result<Vec<(String, u64)>, StoreError> {
        Ok(self
            .sections
            .iter()
            .map(|(name, range)| (name.clone(), range.len() as u64))
            .collect())
    }

    /// The raw bytes of section `name`, or `None` when the file has no such
    /// section. The range was bounds- and alignment-checked at `open`; the
    /// slice is 16-aligned in memory because the mmap base is page-aligned.
    pub fn section_bytes(&self, name: &str) -> Result<Option<&[u8]>, StoreError> {
        let Some((_, range)) = self.sections.iter().find(|(n, _)| n == name) else {
            return Ok(None);
        };
        self.bytes.get(range.clone()).map(Some).ok_or_else(|| StoreError::Corrupt {
            detail: format!("section '{name}' range {range:?} outside the mapped file"),
        })
    }

    /// Section `name` validated and borrowed as the archived type `A` the
    /// caller names (the domain owns the type; the store never does). `None`
    /// when the file has no such section; `Corrupt` when its bytes do not
    /// validate as `A`.
    pub fn section<A>(&self, name: &str) -> Result<Option<&A>, StoreError>
    where
        A: rkyv::Portable
            + for<'a> rkyv::bytecheck::CheckBytes<
                rkyv::api::high::HighValidator<'a, rkyv::rancor::Error>,
            >,
    {
        let Some(bytes) = self.section_bytes(name)? else {
            return Ok(None);
        };
        rkyv::access::<A, rkyv::rancor::Error>(bytes).map(Some).map_err(|_| {
            StoreError::Corrupt {
                detail: format!("section '{name}': {ARCHIVE_INVALID}"),
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

/// Deserialize a `.gmap` back to owned form: the core, and every section's
/// bytes copied out verbatim in table order. Used by mutation operations that
/// modify the core and re-write the file with its sections untouched.
pub fn read_to_owned(path: &Path) -> Result<OwnedFile, StoreError> {
    let mmap = MmapContainer::open(path)?;
    owned_file(&mmap)
}

/// `read_to_owned` over an already-open file.
pub(crate) fn owned_file(mmap: &MmapContainer) -> Result<OwnedFile, StoreError> {
    let archived = mmap.archived()?;
    let core: Container = rkyv::deserialize::<Container, rkyv::rancor::Error>(archived)?;
    let mut sections = Vec::with_capacity(mmap.sections.len());
    for (name, _) in &mmap.sections {
        let mut bytes = AlignedVec::<16>::new();
        if let Some(raw) = mmap.section_bytes(name)? {
            bytes.extend_from_slice(raw);
        }
        sections.push(EncodedSection { name: name.clone(), bytes });
    }
    Ok(OwnedFile { core, sections })
}

/// Set `cell_type` on node `node_id` of `core`: replace the payload of the
/// node's cell of that type, or append one. `NodeNotFound` if no node has the
/// id.
pub(crate) fn set_cell(
    core: &mut Container,
    node_id: NodeId,
    cell_type: CellTypeId,
    payload: CellPayload,
) -> Result<(), StoreError> {
    let node = core
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
    Ok(())
}

/// Insert or replace a cell on a node. Full read-modify-write cycle:
/// opens the `.gmap`, deserializes to owned form, mutates, re-serializes
/// atomically with every section copied verbatim. If a cell of the same type
/// already exists on the node, its payload is replaced; otherwise a new cell
/// is appended.
pub fn upsert_cell(
    path: &Path,
    node_id: NodeId,
    cell_type: CellTypeId,
    payload: CellPayload,
) -> Result<(), StoreError> {
    let mut file = read_to_owned(path)?;
    set_cell(&mut file.core, node_id, cell_type, payload)?;
    let bytes = encode_file(&mut file.core, &file.sections)?;
    write_atomic(path, &bytes)
}

/// Remove a cell of a given type from a node. Returns `Ok(true)` if a cell
/// was removed, `Ok(false)` if the node existed but had no cell of that type.
/// Returns `NodeNotFound` if the node id isn't in the container. Sections are
/// written back verbatim.
pub fn remove_cell(
    path: &Path,
    node_id: NodeId,
    cell_type: CellTypeId,
) -> Result<bool, StoreError> {
    let mut file = read_to_owned(path)?;
    let node = file
        .core
        .nodes
        .iter_mut()
        .find(|n| n.id == node_id)
        .ok_or(StoreError::NodeNotFound(node_id))?;

    let before = node.cells.len();
    node.cells.retain(|c| c.kind != cell_type);
    let removed = node.cells.len() < before;

    if removed {
        let bytes = encode_file(&mut file.core, &file.sections)?;
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

    /// Look up a node id's kind via binary search on the core's `node_kinds`
    /// (sorted by id by the writer). Domain-free: every domain has kinds.
    pub fn kind(&self, id: NodeId) -> Option<NodeKindId> {
        let pairs = &self.node_kinds;
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
            header: Header::new("code"),
            repo: RepoId::from_canonical("test://empty"),
            nodes: Vec::new(),
            edges: Vec::new(),
            node_kinds: Vec::new(),
            sections: Vec::new(),
        }
    }

    /// Encode, split, and copy the core into a 16-aligned buffer (a plain
    /// `Vec<u8>` carries no alignment guarantee; an mmap base is page aligned).
    fn encoded_core(c: &Container) -> rkyv::util::AlignedVec<16> {
        let file = encode_file(&mut c.clone(), &[]).unwrap();
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
        assert_eq!(archived.header.node_kind_registry.len(), 0, "Header::new names nothing");

        // LC.4: a for_domain header's registries survive the archive, sorted.
        let mut c = empty_container();
        c.header = Header::for_domain("toy", &[(2, "B"), (1, "A")], &[(1, "E")], &[]).unwrap();
        let core = encoded_core(&c);
        let archived =
            rkyv::access::<ArchivedContainer, rkyv::rancor::Error>(&core).unwrap();
        let kinds: Vec<(u32, &str)> = archived
            .header
            .node_kind_registry
            .iter()
            .map(|e| (e.id.to_native(), e.name.as_str()))
            .collect();
        assert_eq!(kinds, vec![(1, "A"), (2, "B")]);
        assert_eq!(archived.header.edge_category_registry.len(), 1);
        assert_eq!(archived.header.cell_registry.len(), 0);
    }

    #[test]
    fn for_domain_rejects_a_duplicate_id_within_one_table() {
        let err = Header::for_domain("toy", &[(3, "X"), (3, "Y")], &[], &[]).unwrap_err();
        match err {
            StoreError::Corrupt { detail } => {
                assert!(detail.contains("duplicate registry id 3"), "{detail}");
                assert!(detail.contains("node_kind"), "{detail}");
            }
            other => panic!("expected Corrupt, got {other:?}"),
        }
        // One id in two different tables is two id spaces, not a duplicate.
        let h = Header::for_domain("toy", &[(1, "K")], &[(1, "C")], &[(1, "T")]).unwrap();
        assert_eq!(h.magic, MAGIC);
        assert_eq!(h.version, FORMAT_VERSION);
        assert_eq!(h.graph_type, "toy");
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
        let file = encode_file(&mut empty_container(), &[]).unwrap();
        assert_eq!(&file[0..8], b"GLIAGMAP");
        assert_eq!(&file[8..12], &FORMAT_VERSION.to_le_bytes());
        assert_eq!(&file[12..16], &[0u8; 4], "flags are reserved as 0");
        assert_eq!(&file[16..24], &(PREAMBLE_LEN as u64).to_le_bytes());
        let core_len = (file.len() - PREAMBLE_LEN) as u64;
        assert_eq!(&file[24..32], &core_len.to_le_bytes());
        assert_eq!(split_file(&file).unwrap(), PREAMBLE_LEN..file.len());
        // Deterministic: byte_identical builds depend on it.
        assert_eq!(file, encode_file(&mut empty_container(), &[]).unwrap());
    }

    #[test]
    fn split_file_classifies_what_it_cannot_read() {
        let file = encode_file(&mut empty_container(), &[]).unwrap();
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

    /// Hand-assemble a file whose core carries `table` verbatim (bypassing
    /// `encode_file`, which always writes a valid table), with the core at
    /// `core_offset` and zero bytes before it.
    fn file_with_table(core_offset: usize, table: Vec<SectionEntry>) -> Vec<u8> {
        let mut core = empty_container();
        core.sections = table;
        let archive = rkyv::to_bytes::<rkyv::rancor::Error>(&core).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(&PREAMBLE_MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(core_offset as u64).to_le_bytes());
        out.extend_from_slice(&(archive.len() as u64).to_le_bytes());
        out.resize(core_offset, 0);
        out.extend_from_slice(&archive);
        out
    }

    fn open_bytes(bytes: &[u8]) -> Result<MmapContainer, StoreError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.gmap");
        std::fs::write(&path, bytes).unwrap();
        MmapContainer::open(&path)
    }

    fn entry(name: &str, offset: u64, len: u64) -> SectionEntry {
        SectionEntry { name: name.into(), offset, len }
    }

    #[test]
    fn a_damaged_section_table_is_corrupt() {
        // Control: a well-formed hand-built table opens and reads back.
        let ok = open_bytes(&file_with_table(64, vec![entry("a", 32, 8), entry("b", 48, 16)]))
            .expect("valid table");
        assert_eq!(ok.section_bytes("b").unwrap().map(<[u8]>::len), Some(16));
        assert_eq!(ok.section_names().unwrap(), vec![("a".into(), 8), ("b".into(), 16)]);

        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("runs into the core", file_with_table(48, vec![entry("a", 32, 17)])),
            ("inside the preamble", file_with_table(48, vec![entry("a", 16, 8)])),
            ("misaligned", file_with_table(64, vec![entry("a", 40, 8)])),
            ("overlapping", file_with_table(96, vec![entry("a", 32, 32), entry("b", 48, 8)])),
            ("duplicate name", file_with_table(64, vec![entry("a", 32, 8), entry("a", 48, 8)])),
            ("offset overflow", file_with_table(48, vec![entry("a", u64::MAX, 2)])),
            ("past the file", file_with_table(48, vec![entry("a", 1 << 40, 8)])),
        ];
        for (what, bytes) in cases {
            match open_bytes(&bytes) {
                Err(StoreError::Corrupt { detail }) => {
                    assert!(detail.contains("section 'a'") || detail.contains("section 'b'"), "{what}: {detail}")
                }
                other => panic!("{what}: expected Corrupt, got {:?}", other.err()),
            }
        }
    }

    #[test]
    fn a_section_that_is_not_the_named_type_is_corrupt() {
        let mut core = empty_container();
        let s = encode_section("s", &7u32).unwrap();
        let bytes = encode_file(&mut core, &[s]).unwrap();
        let m = open_bytes(&bytes).unwrap();
        assert_eq!(*m.section::<rkyv::Archived<u32>>("s").unwrap().unwrap(), 7);
        assert!(m.section::<rkyv::Archived<u32>>("none").unwrap().is_none());
        match m.section::<rkyv::Archived<String>>("s") {
            Err(StoreError::Corrupt { detail }) => assert!(detail.contains("section 's'"), "{detail}"),
            other => panic!("expected Corrupt, got {:?}", other.map(|o| o.is_some())),
        }
    }

    fn make_test_container() -> Container {
        Container {
            header: Header::new("code"),
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
            node_kinds: Vec::new(),
            sections: Vec::new(),
        }
    }

    fn write_test_gmap(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("test.gmap");
        let mut container = make_test_container();
        let bytes = encode_file(&mut container, &[]).unwrap();
        write_atomic(&path, &bytes).unwrap();
        path
    }

    #[test]
    fn read_to_owned_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_gmap(dir.path());
        let owned = read_to_owned(&path).unwrap().core;
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

        let owned = read_to_owned(&path).unwrap().core;
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

        let owned = read_to_owned(&path).unwrap().core;
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

        let owned = read_to_owned(&path).unwrap().core;
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
