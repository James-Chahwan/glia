//! The on-disk directory layout: where a repo's gmap lives
//! (`DEFAULT_GMAP_SUBDIR`), the sharded `manifest.json` + per-shard `.gmap` +
//! `cross_stack.gmap` format, the `MergedGraph` round-trip, sharded cell
//! mutation, and the `is_gmap_stale` freshness scan.
//!
//! The manifest also carries the layout's metadata (LC.7, [`LayoutMeta`]):
//! each repo's label and root, and the build's parse errors. They describe
//! the LAYOUT (a multi-repo build has several repos and one error list), not
//! any single shard, so they live in the human-readable `manifest.json`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use repo_graph_code_domain::{GRAPH_TYPE, walk_gating};
use repo_graph_core::{CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, NodeId};
use repo_graph_graph::RepoGraph;

use crate::code_section::{decode_repo_graph, encode_repo_graph_counted};
use crate::container::{
    Container, FORMAT_VERSION, MmapContainer, encode_file, hex_xxhash64, read_to_owned, set_cell,
    write_atomic,
};
use crate::error::StoreError;

/// Convention (LC.9): `<repo>/.glia/graph/` holds exactly the engine's OUTPUT
/// for a repo - `manifest.json`, the per-language `.gmap` shards,
/// `cross_stack.gmap`, the parse cache `parse_cache.bin` beside them, and a
/// self-ignoring `.gitignore` - written by one writer
/// (`repo_graph_engine::persist::persist_result`) for `glia build`, the git
/// hooks and pyo3 alike, and read by `load_from_gmap` and the MCP. Everything
/// else under `.glia/` is an INPUT glia reads (`docs-snapshot/`,
/// `overlay.toml`), so the output gets its own subdirectory rather than
/// sharing `.glia/` with checked-in files. The one name to change if the
/// directory ever moves.
pub const DEFAULT_GMAP_SUBDIR: &str = ".glia/graph";

/// Where 0.4.x wrote the sharded layout and parse cache (and where the 0.4.x
/// `mcp-repo-graph` wrapper still does). 0.5.0 neither reads nor writes a
/// layout there: the builder walk and the [`is_gmap_stale`] scan skip it as
/// engine output, so a 0.4.x writer still running beside 0.5.0 never marks the
/// new layout stale, and the writer only reports it.
pub const LEGACY_GMAP_SUBDIR: &str = ".ai/repo-graph";

/// Resolve the conventional gmap directory for a repo. Does NOT create the
/// directory — callers decide whether to write.
pub fn default_gmap_dir(repo_path: &Path) -> PathBuf {
    repo_path.join(DEFAULT_GMAP_SUBDIR)
}

/// The inverse of [`default_gmap_dir`] (LF.1d): the repo whose conventional
/// layout `dir` is, found by stripping the [`DEFAULT_GMAP_SUBDIR`] components
/// from its end. `None` for any other directory. Purely lexical, like
/// `default_gmap_dir`: `r/.glia/graph` and `r/.glia/graph/` give `r`, a bare
/// `.glia/graph` gives `.`, and `r/.glia/graph/..` is not a layout dir.
pub fn repo_root_of_gmap_dir(dir: &Path) -> Option<PathBuf> {
    let sub: Vec<Component> = Path::new(DEFAULT_GMAP_SUBDIR).components().collect();
    let comps: Vec<Component> = dir.components().collect();
    let split = comps.len().checked_sub(sub.len())?;
    if comps[split..] != sub[..] {
        return None;
    }
    let root: PathBuf = comps[..split].iter().collect();
    Some(if root.as_os_str().is_empty() { PathBuf::from(".") } else { root })
}

// ============================================================================
// Sharded layout — manifest.json + per-shard .gmap + cross_stack.gmap
// ============================================================================

/// Manifest schema version. Bump on any manifest JSON shape change that an
/// older reader would misread; purely additive `#[serde(default)]` fields do
/// not bump it (serde ignores unknown fields). 2 = 0.5.0, the layout whose
/// shards carry the `GLIAGMAP` preamble (`FORMAT_VERSION` 2): a schema-1
/// layout is reported stale by `is_gmap_stale` and rejected by
/// `ShardedMmap::open` before any shard is opened.
pub const MANIFEST_VERSION: u32 = 2;
/// Filename of the manifest inside a sharded directory.
pub const MANIFEST_NAME: &str = "manifest.json";
/// Filename of the cross-stack edge shard inside a sharded directory.
pub const CROSS_STACK_NAME: &str = "cross_stack.gmap";

/// Top-level manifest for a sharded `.gmap` directory.
///
/// One entry per per-repo `.gmap` plus an optional `cross` entry for the
/// cross-stack edge shard. Each entry carries a content hash so the loader
/// can detect on-disk corruption or stale shards from a partial rewrite.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    /// Workspace release version that wrote this layout (`[workspace.package]`
    /// in the root Cargo.toml). `is_gmap_stale` treats a mismatch as stale so
    /// a graph written by an older (or buggier) engine is regenerated on
    /// upgrade instead of being served until a source file happens to change
    /// (audit 2026-06-10 #9 — a pre-gating engine's 23k-node junk graph
    /// survived the 0.4.16 upgrade because only source mtimes were checked).
    /// `default` so pre-0.4.17 manifests deserialize (as "", always stale).
    #[serde(default)]
    pub engine_version: String,
    /// Build identity of the code that produced this layout:
    /// `<release>+p<16 hex>` (`repo_graph_stamp::BUILD_STAMP`).
    /// `engine_version` above is the RELEASE, which does not move when a
    /// parser fix is merged — so a mismatch *here* is what forces a
    /// regenerate for a graph-shaping change made *within* a release
    /// (audit 2026-06-10 #5 fixed upgrade-between-releases; this is the
    /// finer granularity). `default` so pre-0.4.19 manifests deserialize
    /// (as "", always stale). `MANIFEST_VERSION` is deliberately NOT bumped:
    /// serde ignores unknown fields, so an older glia still loads a newer
    /// manifest instead of hard-erroring, while a newer glia reading an
    /// older manifest simply regenerates once.
    #[serde(default)]
    pub build_stamp: String,
    pub shards: Vec<ShardEntry>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cross: Option<ShardEntry>,
    /// The repos this layout was built from, sorted by id (LC.7): the human
    /// label a `RepoId` hash cannot give back, and the repo root relative to
    /// the manifest's directory. Additive under schema 2, so a manifest
    /// without it deserialises (as empty) and one written without metadata
    /// omits the key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoMeta>,
    /// Files the build that wrote this layout could not parse, as
    /// `"<path>: <reason>"`, in build order (LC.7). A loaded graph reports them
    /// so "no gRPC here" and "the file that had it failed" stay apart.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parse_errors: Vec<String>,
    /// Content fingerprint of the repo's `.glia` control-dir inputs (LF.1d):
    /// repo-relative `/`-joined path -> xxhash64 of the file's bytes, from
    /// [`external_inputs_fingerprint`]. The walk and the mtime scan never enter
    /// `.glia` (`walk_gating::CONTROL_DIR`), so [`is_gmap_stale`] compares this
    /// map instead: gitignore-blind, and an in-place rewrite that moves no
    /// directory mtime still marks stale. Omitted when empty, so a repo without
    /// `.glia` inputs writes the same bytes as before. `MANIFEST_VERSION` is not
    /// bumped (the `build_stamp` precedent above): an older glia ignores the
    /// key, a newer one reading a manifest without it regenerates once for a
    /// repo that has inputs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_inputs: BTreeMap<String, String>,
    /// The node confidences a set-dependent post-pass overwrote in the build
    /// that wrote this layout (LC.10a, `MergedGraph::pass_undo`), sorted by
    /// node id, so a merge of pre-built layouts can undo them before it
    /// re-runs the passes over the union. Omitted when empty, so a layout no
    /// such pass touched writes the same bytes as before; `MANIFEST_VERSION`
    /// is not bumped (additive, the `build_stamp` precedent above).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_undo: Vec<PassUndo>,
    /// The `.glia` inputs fingerprint of EACH repo of a layout that records
    /// two or more repo roots (LC.10b, closing LF.1d's multi-root hand-off):
    /// `RepoId.0` -> the map [`Self::external_inputs`] holds for a one-root
    /// layout, for every recorded root that has inputs. [`is_gmap_stale`] on
    /// such a layout compares the entry of the repo whose recorded root is the
    /// path it is given, so a multi-repo or merged layout is not stale on
    /// every call for a repo that has `.glia` inputs. Omitted when empty (no
    /// root has inputs), so a layout without any writes the same bytes as
    /// before; additive, `MANIFEST_VERSION` is not bumped.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_inputs_by_repo: BTreeMap<u64, BTreeMap<String, String>>,
    /// The members a merge of pre-built layouts combined, in merge order
    /// (LC.10b, `repo_graph_engine::merge`). Omitted for a layout one build
    /// wrote.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<MemberMeta>,
}

/// One member of a merged layout (LC.10b): the name the merge gave it, where
/// its graph came from (`gmap`: a pre-built layout dir, `repo`: a source tree
/// whose own layout was loaded or rebuilt) and the build stamp of the layout
/// it was read from (`""` when unknown).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemberMeta {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub build_stamp: String,
}

/// A shard of a domain other than code (LC.10b): its manifest name, its
/// `graph_type` (the manifest's, which a writer sets from the file's
/// `Header::graph_type`) and the whole `.gmap` file's bytes. A layout carries
/// it verbatim: never decoded into the `MergedGraph`, written back byte for
/// byte, so a non-code graph can ride a code layout through a merge without
/// the code store knowing its sections.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForeignShard {
    pub name: String,
    pub graph_type: String,
    pub bytes: Vec<u8>,
}

/// What a layout holds beside the code graph and its [`LayoutMeta`] (LC.10b):
/// the foreign-domain shards, in manifest order, and the merge members.
/// Written by [`write_merged_sharded_extras`] (the other merged-graph writers
/// write none), read back by [`read_layout_extras`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutExtras {
    pub foreign: Vec<ForeignShard>,
    pub members: Vec<MemberMeta>,
}

/// One entry of [`Manifest::pass_undo`] (LC.10a): a node whose confidence a
/// post-pass overwrote, and the value it had before the pass. Written from
/// and read back into `MergedGraph::pass_undo`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PassUndo {
    /// The node's `NodeId.0`.
    pub node: u64,
    /// Its confidence before the pass: `strong`, `medium` or `weak`.
    pub confidence: String,
}

/// The manifest spelling of a confidence in [`PassUndo::confidence`].
fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

/// Inverse of [`confidence_name`]; `None` for any other spelling.
fn confidence_from_name(s: &str) -> Option<Confidence> {
    match s {
        "strong" => Some(Confidence::Strong),
        "medium" => Some(Confidence::Medium),
        "weak" => Some(Confidence::Weak),
        _ => None,
    }
}

/// `MergedGraph::pass_undo` as manifest entries, sorted by node id.
fn pass_undo_entries(undo: &[(NodeId, Confidence)]) -> Vec<PassUndo> {
    let mut out: Vec<PassUndo> = undo
        .iter()
        .map(|(id, c)| PassUndo { node: id.0, confidence: confidence_name(*c).to_string() })
        .collect();
    out.sort_by_key(|e| e.node);
    out
}

/// Manifest entries back as `MergedGraph::pass_undo`: sorted by id, the first
/// entry per id kept. An unknown confidence spelling is a corrupt manifest
/// (the layout needs a rebuild), never a silently dropped undo.
fn pass_undo_from_entries(entries: &[PassUndo]) -> Result<Vec<(NodeId, Confidence)>, StoreError> {
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let Some(c) = confidence_from_name(&e.confidence) else {
            return Err(StoreError::ManifestJson(<serde_json::Error as serde::de::Error>::custom(
                format!("pass_undo: node {} has unknown confidence {:?}", e.node, e.confidence),
            )));
        };
        out.push((NodeId(e.node), c));
    }
    out.sort_by_key(|(id, _)| id.0);
    out.dedup_by_key(|(id, _)| id.0);
    Ok(out)
}

/// One repo of a layout (LC.7): its `RepoId.0`, its human label (the one
/// `service_map` names services by), and its root.
///
/// `root` is RELATIVE to the layout directory with `/` separators (`../..`
/// for `<repo>/.glia/graph`), so a committed layout carries no absolute
/// path and still resolves after a clone. It is absolute only when no
/// relative path exists (another Windows drive), and absent when the writer
/// did not know it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RepoMeta {
    pub id: u64,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

/// The layout-level metadata a sharded write records beside the shards and a
/// read hands back (LC.7): the repos, sorted by id when written, and the
/// build's parse errors in build order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutMeta {
    pub repos: Vec<RepoMeta>,
    pub parse_errors: Vec<String>,
}

/// Just the schema number, parsed before the full `Manifest` so a manifest of
/// another schema reports `ManifestSchemaVersion` even when its shape no
/// longer deserialises as this build's `Manifest`.
#[derive(serde::Deserialize)]
struct SchemaProbe {
    schema_version: u32,
}

/// The fields of a `manifest.json` that say why a layout cannot be served and
/// where its repos live, parsed from a manifest of ANY schema (LC.8): a 0.4.x
/// manifest (schema 1, no `repos`) or one whose shards no longer open still
/// yields its schema number and build stamp, so a loader can name the reason
/// for a rebuild and find the roots to rebuild from. Every other field is
/// ignored.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[non_exhaustive]
pub struct LenientManifest {
    pub schema_version: u32,
    /// `""` when the manifest predates the field (pre-0.4.19).
    #[serde(default)]
    pub build_stamp: String,
    /// Empty for a manifest written without layout metadata (every 0.4.x one).
    #[serde(default)]
    pub repos: Vec<RepoMeta>,
}

/// [`LenientManifest`] of the layout at `dir`. `None` when `manifest.json` is
/// missing, unreadable, not JSON, or has no numeric `schema_version`.
pub fn read_manifest_lenient(dir: &Path) -> Option<LenientManifest> {
    let bytes = std::fs::read(dir.join(MANIFEST_NAME)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read `<dir>/manifest.json`, checking the schema number first.
fn read_manifest(dir: &Path) -> Result<Manifest, StoreError> {
    let bytes = std::fs::read(dir.join(MANIFEST_NAME))?;
    let probe: SchemaProbe = serde_json::from_slice(&bytes)?;
    if probe.schema_version != MANIFEST_VERSION {
        return Err(StoreError::ManifestSchemaVersion {
            got: probe.schema_version,
            supported: MANIFEST_VERSION,
        });
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShardEntry {
    /// Caller-provided name for this shard ("backend", "frontend", ...).
    pub name: String,
    /// Path to the `.gmap` file relative to the manifest's directory.
    pub path: String,
    /// xxhash64 of the shard's bytes, hex-encoded (16 lowercase hex chars).
    pub content_hash: String,
    /// The domain of the shard's graph (LC.10b): `"code"`
    /// (`repo_graph_code_domain::GRAPH_TYPE`) for every shard the code build
    /// writes, another value for a foreign domain's shard ([`ForeignShard`]),
    /// which readers hash-check but never decode as code. Read as `"code"`
    /// when absent and omitted when `"code"`, so a code-only manifest keeps
    /// the bytes it had before the field.
    #[serde(default = "code_graph_type", skip_serializing_if = "is_code_graph_type")]
    pub graph_type: String,
}

impl ShardEntry {
    /// Is this shard the code domain's (a `RepoGraph` shard)?
    pub fn is_code(&self) -> bool {
        is_code_graph_type(&self.graph_type)
    }
}

fn code_graph_type() -> String {
    GRAPH_TYPE.to_string()
}

fn is_code_graph_type(t: &str) -> bool {
    t == GRAPH_TYPE
}

/// What a sharded write records beyond the shards and the [`LayoutMeta`]:
/// the `.glia` fingerprints (LF.1d, LC.10b), the post-pass undo (LC.10a) and
/// the foreign shards and merge members (LC.10b). Empty for a bare writer.
#[derive(Default)]
struct Recorded<'a> {
    external_inputs: BTreeMap<String, String>,
    external_inputs_by_repo: BTreeMap<u64, BTreeMap<String, String>>,
    pass_undo: Vec<PassUndo>,
    foreign: &'a [ForeignShard],
    members: &'a [MemberMeta],
}

/// A foreign shard `write_sharded_with` refuses: a name that is not a plain
/// file stem, a `graph_type` that is empty or the code domain's (a reader
/// would decode it as code), or a name (so a `<name>.gmap` file) another
/// shard of the layout already has. `taken` holds the names before it.
fn check_foreign(f: &ForeignShard, taken: &[&str]) -> Result<(), StoreError> {
    let bad = |why: &str| {
        Err(StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("foreign shard {:?} (graph_type {:?}): {why}", f.name, f.graph_type),
        )))
    };
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    if f.name.is_empty() || f.name.starts_with('.') || !f.name.chars().all(plain) {
        return bad("the name must be a plain file stem ([A-Za-z0-9._-], no leading '.')");
    }
    if f.graph_type.is_empty() || is_code_graph_type(&f.graph_type) {
        return bad("a foreign shard's graph_type must be set and not the code domain's");
    }
    if format!("{}.gmap", f.name) == CROSS_STACK_NAME || taken.contains(&f.name.as_str()) {
        return bad("another shard of the layout already has this name");
    }
    Ok(())
}

/// Write a sharded `.gmap` layout: one `<name>.gmap` per input graph plus a
/// `cross_stack.gmap` if `cross_edges` is non-empty, plus a `manifest.json`.
/// Returns the manifest that was written so callers can inspect hashes. Each
/// per-graph shard is `encode_repo_graph` (core + `"code"` section);
/// `cross_stack.gmap` is a core with no section. Prints one un-gated
/// `[gmap] layout <dir>: shards=<n> format=<v> sections=code:<k>` line per
/// write, `k` = shards carrying a code section.
///
/// Shard names must be unique and non-empty — duplicates produce a manifest
/// whose loader will reject it. Records no layout metadata: see
/// [`write_sharded_meta`].
pub fn write_sharded(
    shards: &[(&str, &RepoGraph)],
    cross_edges: &[Edge],
    dir: &Path,
) -> Result<Manifest, StoreError> {
    write_sharded_meta(shards, cross_edges, &LayoutMeta::default(), dir)
}

/// [`write_sharded`] plus the layout's metadata in the manifest (LC.7):
/// `meta.repos` sorted by id and `meta.parse_errors` in the order given, so
/// the same build writes the same manifest bytes and the skip-when-unchanged
/// check still holds.
pub fn write_sharded_meta(
    shards: &[(&str, &RepoGraph)],
    cross_edges: &[Edge],
    meta: &LayoutMeta,
    dir: &Path,
) -> Result<Manifest, StoreError> {
    write_sharded_with(shards, cross_edges, meta, dir, Recorded::default())
}

/// The sharded writer behind [`write_sharded_meta`] and the merged-graph
/// writers. `recorded` holds what the manifest records beyond the shards:
/// the `.glia` fingerprint (LF.1d), empty for a layout no repo root is known
/// for, and the per-repo ones of a multi-root layout (LC.10b); the merged
/// graph's post-pass undo record (LC.10a), sorted by node id; and the foreign
/// shards, written verbatim after the code shards and before
/// `cross_stack.gmap` in the order given, and the merge members (LC.10b).
/// Every one of them is empty for a writer of bare shards.
fn write_sharded_with(
    shards: &[(&str, &RepoGraph)],
    cross_edges: &[Edge],
    meta: &LayoutMeta,
    dir: &Path,
    recorded: Recorded<'_>,
) -> Result<Manifest, StoreError> {
    // LC.10b: every foreign shard is checked before anything is written.
    let mut taken: Vec<&str> = shards.iter().map(|(name, _)| *name).collect();
    for f in recorded.foreign {
        check_foreign(f, &taken)?;
        taken.push(&f.name);
    }
    std::fs::create_dir_all(dir)?;

    // Phase 1 incremental rebuild: load prior manifest (if present) and
    // compare per-shard content hashes. Shards whose serialized bytes hash
    // matches the stored content_hash skip the write_atomic call. Cuts
    // write/fsync work for the common case of "this repo's parse output
    // didn't change for these shards" — useful for any consumer that
    // watches mtime or hot-loads on change events.
    //
    // Worst case unchanged: 0 writes saved (every shard hash changed).
    // Best case: parse output identical → only manifest gets a rewrite
    // (and it might also skip via the same check). Typical: 1-3 of N
    // shards change per cycle when a single language tree is edited.
    // A prior manifest of another schema describes shards of another format:
    // none of its hashes can be reused.
    let prior_manifest: Option<Manifest> = read_manifest(dir).ok();

    let mut entries = Vec::with_capacity(shards.len());
    let mut shards_skipped = 0usize;
    let mut code_sections = 0usize;
    for (name, g) in shards {
        let file_name = format!("{name}.gmap");
        let shard_path = dir.join(&file_name);
        let (bytes, has_code) = encode_repo_graph_counted(g)?;
        code_sections += usize::from(has_code);
        let content_hash = hex_xxhash64(&bytes);

        // Skip-when-unchanged: write only if the prior manifest didn't
        // already report this hash for this shard name AND the file on disk
        // still holds exactly these bytes. Existence alone is not enough: a
        // shard damaged after its write keeps its name and its manifest entry,
        // and a rebuild must replace it (LC.8), not skip it forever.
        let unchanged = prior_manifest.as_ref().is_some_and(|m| {
            m.shards.iter().any(|e| e.name == *name && e.content_hash == content_hash)
        }) && on_disk_is(&shard_path, &bytes);
        if !unchanged {
            write_atomic(&shard_path, &bytes)?;
        } else {
            shards_skipped += 1;
        }

        entries.push(ShardEntry {
            name: (*name).to_string(),
            path: file_name,
            content_hash,
            graph_type: code_graph_type(),
        });
    }

    // LC.10b: foreign-domain shards, byte for byte, after the code shards
    // (their names were checked above), skipped when unchanged like them.
    for f in recorded.foreign {
        let file_name = format!("{}.gmap", f.name);
        let shard_path = dir.join(&file_name);
        let content_hash = hex_xxhash64(&f.bytes);
        let unchanged = prior_manifest.as_ref().is_some_and(|m| {
            m.shards.iter().any(|e| e.name == f.name && e.content_hash == content_hash)
        }) && on_disk_is(&shard_path, &f.bytes);
        if unchanged {
            shards_skipped += 1;
        } else {
            write_atomic(&shard_path, &f.bytes)?;
        }
        entries.push(ShardEntry {
            name: f.name.clone(),
            path: file_name,
            content_hash,
            graph_type: f.graph_type.clone(),
        });
    }

    let cross = if cross_edges.is_empty() {
        None
    } else {
        let shard_path = dir.join(CROSS_STACK_NAME);
        let mut container = Container::for_cross_edges(cross_edges.to_vec());
        let bytes = encode_file(&mut container, &[])?;
        let content_hash = hex_xxhash64(&bytes);
        let unchanged = prior_manifest
            .as_ref()
            .and_then(|m| m.cross.as_ref())
            .is_some_and(|c| c.content_hash == content_hash)
            && on_disk_is(&shard_path, &bytes);
        if !unchanged {
            write_atomic(&shard_path, &bytes)?;
        } else {
            shards_skipped += 1;
        }
        Some(ShardEntry {
            name: "cross_stack".to_string(),
            path: CROSS_STACK_NAME.to_string(),
            content_hash,
            graph_type: code_graph_type(),
        })
    };

    let shard_files = entries.len() + usize::from(cross.is_some());
    let mut repos = meta.repos.clone();
    repos.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.label.cmp(&b.label)));
    let manifest = Manifest {
        schema_version: MANIFEST_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
        build_stamp: repo_graph_stamp::BUILD_STAMP.to_string(),
        shards: entries,
        cross,
        repos,
        parse_errors: meta.parse_errors.clone(),
        external_inputs: recorded.external_inputs,
        pass_undo: recorded.pass_undo,
        external_inputs_by_repo: recorded.external_inputs_by_repo,
        members: recorded.members.to_vec(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    // Skip the manifest write only if it's byte-identical to the prior one
    // (so consumers watching the manifest mtime don't get false wakeups).
    let manifest_path = dir.join(MANIFEST_NAME);
    let manifest_unchanged = std::fs::read(&manifest_path)
        .ok()
        .map(|prior| prior == manifest_bytes)
        .unwrap_or(false);
    if !manifest_unchanged {
        write_atomic(&manifest_path, &manifest_bytes)?;
    }
    // LC.5b marker, un-gated: one line per layout write naming how many files
    // carry the code domain's section (cross_stack.gmap never does).
    eprintln!(
        "[gmap] layout {}: shards={shard_files} format={FORMAT_VERSION} sections=code:{code_sections}",
        dir.display()
    );
    // Diagnostic: emit how many shards were skipped (env-gated to keep
    // hot-path output clean by default; opt in via GLIA_STORE_VERBOSE=1).
    if std::env::var("GLIA_STORE_VERBOSE").as_deref() == Ok("1") {
        eprintln!(
            "[store] write_sharded: {} shards skipped (unchanged), {} written, manifest {}",
            shards_skipped,
            shards.len() + recorded.foreign.len() + cross_edges.len().min(1) - shards_skipped,
            if manifest_unchanged { "unchanged" } else { "rewritten" },
        );
    }
    Ok(manifest)
}

/// Does `path` hold exactly `bytes`? The length is checked before the read,
/// so a changed shard of another size costs one `stat`.
fn on_disk_is(path: &Path, bytes: &[u8]) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == bytes.len() as u64)
        && std::fs::read(path).is_ok_and(|b| b == bytes)
}

/// A sharded layout opened zero-copy. Each per-shard `.gmap` is its own mmap'd
/// `MmapContainer`; the manifest is loaded eagerly and verified against the
/// files on disk.
pub struct ShardedMmap {
    pub manifest: Manifest,
    pub shards: Vec<(String, MmapContainer)>,
    pub cross: Option<MmapContainer>,
}

impl ShardedMmap {
    /// Open a sharded directory. Validates the manifest schema version (from
    /// a probe, before the full manifest is parsed), then opens each shard's
    /// `.gmap` and verifies its content hash against the manifest. Returns on
    /// the first hash mismatch or missing file. A foreign-domain shard
    /// (LC.10b, `graph_type` other than `"code"`) is hash-checked but not
    /// opened: `shards` holds the code shards only, and the manifest still
    /// lists every shard.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        let manifest = read_manifest(dir)?;

        let mut shards = Vec::with_capacity(manifest.shards.len());
        for entry in &manifest.shards {
            let shard_path = dir.join(&entry.path);
            verify_hash(entry, &shard_path)?;
            if !entry.is_code() {
                continue;
            }
            let mmap = MmapContainer::open(&shard_path)?;
            shards.push((entry.name.clone(), mmap));
        }

        let cross = if let Some(entry) = &manifest.cross {
            let shard_path = dir.join(&entry.path);
            verify_hash(entry, &shard_path)?;
            Some(MmapContainer::open(&shard_path)?)
        } else {
            None
        };

        Ok(Self {
            manifest,
            shards,
            cross,
        })
    }

    /// Iterate all edges across every shard and the cross-stack shard. Useful
    /// for whole-merged-graph walks without rehydrating owned types.
    pub fn edges_iter(&self) -> impl Iterator<Item = (NodeId, NodeId, EdgeCategoryId)> + '_ {
        let shard_edges = self.shards.iter().flat_map(|(_, mmap)| match mmap.archived() {
            Ok(a) => Some(a.edges_iter()),
            Err(_) => None,
        }).flatten();
        let cross_edges = self
            .cross
            .as_ref()
            .and_then(|m| m.archived().ok())
            .map(|a| a.edges_iter())
            .into_iter()
            .flatten();
        shard_edges.chain(cross_edges)
    }
}

fn verify_hash(entry: &ShardEntry, path: &Path) -> Result<(), StoreError> {
    verified_bytes(entry, path).map(|_| ())
}

/// The bytes of the shard `entry` names, once they match its manifest hash.
fn verified_bytes(entry: &ShardEntry, path: &Path) -> Result<Vec<u8>, StoreError> {
    if !path.exists() {
        return Err(StoreError::ShardMissing(entry.name.clone()));
    }
    let bytes = std::fs::read(path)?;
    let got = hex_xxhash64(&bytes);
    if got != entry.content_hash {
        return Err(StoreError::ContentHashMismatch {
            shard: entry.name.clone(),
            expected: entry.content_hash.clone(),
            got,
        });
    }
    Ok(bytes)
}

// ============================================================================
// MergedGraph round-trip — write/read for the multi-language sharded layout
// ============================================================================

/// Write a merged graph to a sharded directory. Each `RepoGraph` becomes one
/// shard; cross-repo edges go in `cross_stack.gmap`. Shard names use
/// `repo-<u64>-<idx>` so the same repo's multiple language sub-graphs don't
/// collide. Returns the manifest that was written. Records no layout
/// metadata: see [`write_merged_sharded_meta`].
///
/// The `.glia` inputs fingerprint (LF.1d) is recorded when `dir` is a repo's
/// default layout dir ([`repo_root_of_gmap_dir`]), so every default-dir writer
/// (pyo3 `generate`'s auto-persist, `save_to_default`, `glia build`) records
/// it with no call-site change. A custom dir records it only through
/// [`write_merged_sharded_meta`] with exactly one repo root, or
/// [`write_merged_sharded_for_repo`]. A custom-dir manifest written with no
/// root records nothing, so [`is_gmap_stale`] on it, for a repo that HAS
/// `.glia` inputs, reports stale on every call: the safe direction (regenerate,
/// never serve stale). The fingerprint is taken when the manifest is written,
/// not when the build read its inputs, so an input rewritten DURING a long
/// build is recorded as fresh: the same window the mtime scan accepts for a
/// source edited during a build.
pub fn write_merged_sharded(
    merged: &repo_graph_graph::MergedGraph,
    dir: &Path,
) -> Result<Manifest, StoreError> {
    write_merged_sharded_meta(merged, &LayoutMeta::default(), dir)
}

/// [`write_merged_sharded`] plus the layout's metadata (repo labels and roots,
/// parse errors) in the manifest (LC.7). [`read_merged_sharded_meta`] is its
/// inverse.
///
/// The `.glia` inputs fingerprint (LF.1d) is of the repo whose default layout
/// `dir` is, else of the one repo root `meta` records (resolved against
/// `dir`, as [`RepoMeta::root`] is written), else none: see
/// [`write_merged_sharded`].
pub fn write_merged_sharded_meta(
    merged: &repo_graph_graph::MergedGraph,
    meta: &LayoutMeta,
    dir: &Path,
) -> Result<Manifest, StoreError> {
    write_merged_sharded_extras(merged, meta, &[], &[], dir)
}

/// [`write_merged_sharded_meta`] plus what a merge of layouts records
/// (LC.10b): each `foreign` shard written verbatim as `<name>.gmap`, with its
/// `graph_type` and the hash of its bytes, after the code shards and before
/// `cross_stack.gmap`, in the order given; `members` in the manifest. A
/// foreign name that is not a plain file stem or repeats another shard's, or
/// a `graph_type` that is empty or `"code"`, is an `InvalidInput` error and no
/// shard or manifest is written. [`read_merged_sharded_meta`] plus
/// [`read_layout_extras`] read it back.
pub fn write_merged_sharded_extras(
    merged: &repo_graph_graph::MergedGraph,
    meta: &LayoutMeta,
    foreign: &[ForeignShard],
    members: &[MemberMeta],
    dir: &Path,
) -> Result<Manifest, StoreError> {
    // First, so a recorded root relative to `dir` (`../repo`) resolves: the
    // OS walks `dir/..` only when `dir` exists.
    std::fs::create_dir_all(dir)?;
    let root = repo_root_of_gmap_dir(dir).or_else(|| match meta.repos.as_slice() {
        [only] => only.root.as_deref().map(|r| dir.join(r)),
        _ => None,
    });
    let inputs = root.map(|r| external_inputs_fingerprint(&r)).unwrap_or_default();
    write_merged_with(
        merged,
        meta,
        dir,
        Recorded {
            external_inputs: inputs,
            external_inputs_by_repo: inputs_by_repo(meta, dir),
            foreign,
            members,
            ..Recorded::default()
        },
    )
}

/// The per-repo `.glia` fingerprints of a layout whose metadata records two
/// or more repo roots (LC.10b): each root resolved against `dir` as
/// [`RepoMeta::root`] is written, entries with no inputs left out. Empty for
/// a layout of one root, which [`Manifest::external_inputs`] covers.
fn inputs_by_repo(meta: &LayoutMeta, dir: &Path) -> BTreeMap<u64, BTreeMap<String, String>> {
    let rooted: Vec<(u64, PathBuf)> = meta
        .repos
        .iter()
        .filter_map(|r| Some((r.id, dir.join(r.root.as_deref()?))))
        .collect();
    if rooted.len() < 2 {
        return BTreeMap::new();
    }
    rooted
        .into_iter()
        .map(|(id, root)| (id, external_inputs_fingerprint(&root)))
        .filter(|(_, inputs)| !inputs.is_empty())
        .collect()
}

/// [`write_merged_sharded_meta`] recording the `.glia` inputs fingerprint of
/// an explicit `repo_root` (LF.1d), for a layout at a custom dir whose
/// metadata does not name exactly one root.
pub fn write_merged_sharded_for_repo(
    merged: &repo_graph_graph::MergedGraph,
    meta: &LayoutMeta,
    dir: &Path,
    repo_root: &Path,
) -> Result<Manifest, StoreError> {
    let recorded =
        Recorded { external_inputs: external_inputs_fingerprint(repo_root), ..Recorded::default() };
    write_merged_with(merged, meta, dir, recorded)
}

fn write_merged_with(
    merged: &repo_graph_graph::MergedGraph,
    meta: &LayoutMeta,
    dir: &Path,
    recorded: Recorded<'_>,
) -> Result<Manifest, StoreError> {
    let names: Vec<String> = merged
        .graphs
        .iter()
        .enumerate()
        .map(|(i, g)| {
            if merged.graphs.len() == 1 {
                format!("repo-{}", g.repo.0)
            } else {
                format!("repo-{}-{:02}", g.repo.0, i)
            }
        })
        .collect();
    let shards: Vec<(&str, &RepoGraph)> = names
        .iter()
        .zip(merged.graphs.iter())
        .map(|(n, g)| (n.as_str(), g))
        .collect();
    write_sharded_with(
        &shards,
        &merged.cross_edges,
        meta,
        dir,
        Recorded { pass_undo: pass_undo_entries(&merged.pass_undo), ..recorded },
    )
}

/// Read a sharded directory back into an owned `MergedGraph`. Reconstructs
/// every per-language `RepoGraph` from its archived shard (core + code
/// section, `properties` included since LC.7), then attaches the cross-stack
/// edges and the post-pass undo record (LC.10a). Drops the layout metadata:
/// see [`read_merged_sharded_meta`].
///
/// When the layout cannot be served (`StoreError::needs_rebuild`) it prints one
/// `[gmap] needs rebuild: <dir>: <reason>` line before returning the error, so
/// the first 0.5.0 load of a 0.4.x layout says why it is regenerating.
pub fn read_merged_sharded(
    dir: &Path,
) -> Result<repo_graph_graph::MergedGraph, StoreError> {
    read_merged_sharded_meta(dir).map(|(merged, _meta)| merged)
}

/// [`read_merged_sharded`] plus the manifest's layout metadata (LC.7): repo
/// labels and roots as written (roots still relative to `dir`) and the parse
/// errors. A layout written without metadata reads back an empty
/// [`LayoutMeta`]. Prints the same `[gmap] needs rebuild` line on failure.
///
/// The manifest's post-pass undo record (LC.10a) comes back on the graph, as
/// `MergedGraph::pass_undo`, not in the [`LayoutMeta`]: it is the graph's
/// state, and one carrier keeps the write and the read from disagreeing.
///
/// A foreign-domain shard (LC.10b) is hash-checked but not decoded: one
/// `[gmap] skipped foreign shard <name> graph_type=<t>` line is printed per
/// shard, and [`read_layout_extras`] hands back its bytes.
pub fn read_merged_sharded_meta(
    dir: &Path,
) -> Result<(repo_graph_graph::MergedGraph, LayoutMeta), StoreError> {
    let result = read_merged_sharded_inner(dir);
    if let Err(e) = &result
        && let Some(reason) = e.rebuild_reason()
    {
        eprintln!("[gmap] needs rebuild: {}: {reason}", dir.display());
    }
    result
}

/// What the layout at `dir` holds beside its code graph (LC.10b): every
/// foreign-domain shard, hash-checked, as its manifest name, `graph_type` and
/// bytes in manifest order (so a merge carries them on), and the merge members
/// its manifest records. Reads the manifest and the foreign shards only.
pub fn read_layout_extras(dir: &Path) -> Result<LayoutExtras, StoreError> {
    let manifest = read_manifest(dir)?;
    let mut foreign = Vec::new();
    for entry in manifest.shards.iter().filter(|e| !e.is_code()) {
        foreign.push(ForeignShard {
            name: entry.name.clone(),
            graph_type: entry.graph_type.clone(),
            bytes: verified_bytes(entry, &dir.join(&entry.path))?,
        });
    }
    Ok(LayoutExtras { foreign, members: manifest.members })
}

fn read_merged_sharded_inner(
    dir: &Path,
) -> Result<(repo_graph_graph::MergedGraph, LayoutMeta), StoreError> {
    let sharded = ShardedMmap::open(dir)?;
    let mut graphs = Vec::with_capacity(sharded.shards.len());
    for (_name, mmap) in &sharded.shards {
        graphs.push(decode_repo_graph(mmap)?);
    }
    for entry in sharded.manifest.shards.iter().filter(|e| !e.is_code()) {
        eprintln!("[gmap] skipped foreign shard {} graph_type={}", entry.name, entry.graph_type);
    }
    let cross_edges = if let Some(cross_mmap) = &sharded.cross {
        let archived = cross_mmap.archived()?;
        let owned: Container =
            rkyv::deserialize::<Container, rkyv::rancor::Error>(archived)?;
        owned.edges
    } else {
        Vec::new()
    };
    let meta = LayoutMeta {
        repos: sharded.manifest.repos.clone(),
        parse_errors: sharded.manifest.parse_errors.clone(),
    };
    let pass_undo = pass_undo_from_entries(&sharded.manifest.pass_undo)?;
    Ok((
        repo_graph_graph::MergedGraph {
            graphs,
            cross_edges,
            pass_undo,
        },
        meta,
    ))
}

/// Cheap freshness check: is anything under `repo_path` that the BUILDER would
/// look at newer than the manifest in `gmap_dir`? Directory gating is shared
/// with the builder's walk (`repo_graph_code_domain::walk_gating`), so the scan
/// skips exactly the
/// trees the parse skips: VCS/editor metadata, the gmap dir itself, dependency
/// and build-output directories, anything a `.gitignore` (root or nested)
/// matches — directories and files alike — and copied web bundles.
///
/// `.glia` (`walk_gating::CONTROL_DIR`) is hard-skipped by both, so its
/// inputs are compared by CONTENT instead: the manifest's `external_inputs`
/// against a fresh [`external_inputs_fingerprint`] (LF.1d).
///
/// Returns:
/// - `true` if the gmap is missing/unreadable, if its manifest schema is not
///   `MANIFEST_VERSION`, if it was written by another
///   build, if a `.glia` input was added, edited or deleted since it was
///   written, if any un-gated file's mtime is newer than the manifest's, or if a
///   GATED directory's own mtime is newer (the builder emits one REGION node
///   per collapsed directory, so a region appearing or disappearing does change
///   the graph).
/// - `false` if everything the builder would read predates the manifest.
///
/// Walks lazily and stops at the first newer entry. Worst case O(N) over the
/// un-gated tree.
pub fn is_gmap_stale(gmap_dir: &Path, repo_path: &Path) -> bool {
    let manifest_path = gmap_dir.join(MANIFEST_NAME);
    let Ok(manifest_meta) = std::fs::metadata(&manifest_path) else {
        return true;
    };
    let Ok(manifest_mtime) = manifest_meta.modified() else {
        return true;
    };
    // A layout written by a different build is stale regardless of source
    // mtimes — otherwise an old engine's output is served until a source file
    // happens to change. The schema number catches a layout of another format
    // (every 0.4.x layout after LC.1); `engine_version` catches an upgrade
    // between releases; `build_stamp` catches a graph-shaping change within one.
    // Unreadable/unparseable manifest → stale.
    let Ok(manifest_bytes) = std::fs::read(&manifest_path) else {
        return true;
    };
    let Ok(probe) = serde_json::from_slice::<SchemaProbe>(&manifest_bytes) else {
        return true;
    };
    if probe.schema_version != MANIFEST_VERSION {
        // Un-gated, like the build-stamp line below: it explains a regenerate.
        eprintln!(
            "[gmap] stale: manifest schema {} != {MANIFEST_VERSION} - regenerating",
            probe.schema_version
        );
        return true;
    }
    let Ok(m) = serde_json::from_slice::<Manifest>(&manifest_bytes) else {
        return true;
    };
    if m.engine_version != env!("CARGO_PKG_VERSION")
        || m.build_stamp != repo_graph_stamp::BUILD_STAMP
    {
        // Un-gated on purpose (not behind GLIA_STORE_VERBOSE): it fires rarely
        // and it is the only explanation a user gets for an expensive
        // regenerate, matching the `[incremental] build context changed` line.
        eprintln!(
            "[gmap] stale: build stamp mismatch (manifest={}+{} build={}) — regenerating",
            m.engine_version,
            m.build_stamp,
            repo_graph_stamp::BUILD_STAMP
        );
        return true;
    }
    // LF.1d: the control dir's inputs, by content. Un-gated like the lines
    // above; `grep '^\[gmap\] stale: external input changed'` is the marker.
    let now = external_inputs_fingerprint(repo_path);
    if let Some(key) = first_changed_input(recorded_inputs(&m, gmap_dir, repo_path), &now) {
        eprintln!("[gmap] stale: external input changed ({key}) - regenerating");
        return true;
    }

    scan_for_newer(repo_path, gmap_dir, manifest_mtime)
}

/// The `.glia` fingerprint `m` recorded for `repo_path` (LC.10b): when the
/// layout recorded per-repo fingerprints and one of its repos' roots
/// (resolved against `dir`) is `repo_path`, that repo's entry, or an empty map
/// for a rooted repo that had no inputs; otherwise the one-root
/// [`Manifest::external_inputs`].
fn recorded_inputs<'a>(m: &'a Manifest, dir: &Path, repo_path: &Path) -> &'a BTreeMap<String, String> {
    static NONE: BTreeMap<String, String> = BTreeMap::new();
    if m.external_inputs_by_repo.is_empty() {
        return &m.external_inputs;
    }
    let resolved = |p: &Path| {
        std::fs::canonicalize(p)
            .or_else(|_| std::path::absolute(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    let want = resolved(repo_path);
    let repo = m.repos.iter().find(|r| {
        r.root.as_deref().is_some_and(|root| resolved(&dir.join(root)) == want)
    });
    match repo {
        Some(r) => m.external_inputs_by_repo.get(&r.id).unwrap_or(&NONE),
        None => &m.external_inputs,
    }
}

/// Files under `.glia` the store or the parse cache writes itself: never an
/// input, or every layout written into `.glia` would change its own
/// fingerprint and go stale at once (a silent infinite regenerate).
fn is_store_output_name(name: &str) -> bool {
    name == MANIFEST_NAME
        || name == "parse_cache.bin"
        || name == ".gitignore"
        || name.ends_with(".gmap")
        || name.ends_with(".lock")
        || name.ends_with(".tmp")
}

/// Content fingerprint of the inputs under `<repo_root>/.glia`, glia's control
/// directory (LF.1d): repo-relative `/`-joined path -> xxhash64 hex of the
/// bytes, for every regular file (a symlink to one included) except the
/// store's own output: the default layout dir `.glia/graph` and, anywhere,
/// `*.gmap`, `manifest.json`, `parse_cache.bin`, `*.lock`, `*.tmp` and
/// `.gitignore` (so the flat 0.4.x `glia build` output and a custom layout dir
/// inside `.glia` are excluded too). Gitignore-blind by design: a gitignored
/// docs snapshot is an input all the same. Files are hashed in 1 MiB chunks,
/// so a large `vectors.jsonl` is one streaming pass. An unreadable file is
/// left out (glia cannot read it either). Empty when there is no `.glia`.
pub fn external_inputs_fingerprint(repo_root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let control = repo_root.join(walk_gating::CONTROL_DIR);
    let own_layout = repo_root.join(DEFAULT_GMAP_SUBDIR);
    let mut stack = vec![control];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ftype) = entry.file_type() else { continue };
            if ftype.is_dir() {
                if path != own_layout {
                    stack.push(path);
                }
                continue;
            }
            // A symlinked file counts; a symlinked directory is not followed
            // (no cycles).
            let is_file = ftype.is_file()
                || (ftype.is_symlink() && std::fs::metadata(&path).is_ok_and(|m| m.is_file()));
            if !is_file || is_store_output_name(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(repo_root) else { continue };
            let key = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if let Some(hash) = hash_file_streaming(&path) {
                out.insert(key, hash);
            }
        }
    }
    out
}

/// xxhash64 of a file's bytes, read in 1 MiB chunks; the same hex form as
/// `hex_xxhash64`. `None` when the file cannot be read.
fn hash_file_streaming(path: &Path) -> Option<String> {
    use core::hash::Hasher;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = twox_hash::XxHash64::with_seed(0);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.write(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
    Some(format!("{:016x}", hasher.finish()))
}

/// The first path, in sorted order, that was added, edited or deleted between
/// the recorded and the current fingerprint; `None` when they are equal.
fn first_changed_input<'a>(
    recorded: &'a BTreeMap<String, String>,
    now: &'a BTreeMap<String, String>,
) -> Option<&'a str> {
    recorded
        .keys()
        .chain(now.keys())
        .filter(|k| recorded.get(*k) != now.get(*k))
        .min()
        .map(String::as_str)
}

/// Walk `repo_path` for anything newer than the manifest, gating directories
/// exactly as the builder's walk does — both call
/// `repo_graph_code_domain::walk_gating`, so the two cannot drift. Before the
/// shared gate the scan used its own six-name list, so it descended into trees
/// the builder collapses (`dist`, `coverage`, every `.gitignore`d directory)
/// and regenerated the whole gmap for files no parser ever reads — and each
/// wasted regenerate is a wasted full parse of the real tree.
///
/// `.gitignore` semantics are the builder's too (A8.2): each stack entry carries
/// the matcher layers of its ancestors, the directory's own layer is pushed when
/// it is scanned, and a gitignored FILE is skipped just as the builder skips it.
/// The FULL nested stack is used, not a root-only approximation, because any
/// rule the scan honours less precisely than the builder is exactly the
/// builder/store divergence the shared gate exists to close. A `.gitignore`
/// edit is itself an un-ignored file, so changing the rules still marks stale.
///
/// The user's `[walk] skip` patterns (`.glia/overlay.toml`, LF.3a) gate the
/// scan too: the stack is built by `IgnoreStack::for_repo`, the same
/// `with_config` constructor the walk uses, so an excluded directory is a gated
/// region here and an excluded file is never checked. An edit to the skip list
/// changes the `.glia` fingerprint, so it marks stale on its own.
///
/// Returns true at the first newer entry; worst case O(N) over the un-gated
/// tree.
fn scan_for_newer(
    repo_path: &Path,
    gmap_dir: &Path,
    manifest_mtime: std::time::SystemTime,
) -> bool {
    // Our own output, skipped by PREFIX rather than by a directory name. The
    // shards and the parse cache beside them are written after the manifest, so
    // counting them would make every gmap instantly stale — a silent infinite
    // regenerate. Three prefixes, mirroring the builder walk's
    // `is_self_output`: the layout being checked, the repo's default layout
    // `<repo>/.glia/graph` (a layout checked at another dir must not go stale
    // because a hook refreshed the default one), and the legacy
    // `<repo>/.ai/repo-graph` a 0.4.x wrapper may still write into. `.ai` is
    // NOT our output: the engine ingests `.ai/**/*.md` as DOC_SECTIONs, so an
    // edit there MUST mark the gmap stale. `.glia` as a whole is hard-skipped
    // below (`walk_gating::CONTROL_DIR`); its inputs are compared by content in
    // `is_gmap_stale` (LF.1d). Canonicalised so the prefix test survives a
    // relative `repo_path` against an absolute `gmap_dir`.
    let root = std::fs::canonicalize(repo_path).unwrap_or_else(|_| repo_path.to_path_buf());
    let gmap = std::fs::canonicalize(gmap_dir).unwrap_or_else(|_| gmap_dir.to_path_buf());
    let own_output = [gmap, root.join(DEFAULT_GMAP_SUBDIR), root.join(LEGACY_GMAP_SUBDIR)];
    let mut gated = 0usize;
    let mut checked = 0usize;
    let mut stale = false;
    // Each entry owns its ancestors' matcher layers (a Vec of Arcs, so the
    // per-directory clone is O(depth) pointer copies).
    // LF.3a: rooted at the canonical root, the spelling every path below uses.
    let base = walk_gating::IgnoreStack::for_repo(&root);
    let mut stack = vec![(root, base)];
    'walk: while let Some((dir, mut ignores)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        ignores.push_dir(&dir);
        for entry in entries.flatten() {
            let path = entry.path();
            let ftype = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ftype.is_dir() {
                let basename = entry.file_name();
                let bn = basename.to_string_lossy();
                // Hard-skipped dirs and our own output produce no node of any
                // kind, so not even their own mtime is observable — they must
                // NOT reach the gated-dir-mtime rule below.
                let own = own_output.iter().any(|p| path.starts_with(p));
                if own || walk_gating::is_hard_skip(&bn) {
                    gated += 1;
                    continue;
                }
                if walk_gating::dir_is_gated(&path, &bn, &ignores) {
                    gated += 1;
                    // The builder turns a gated dir into ONE region node, so
                    // its existence is graph-visible even though its contents
                    // are not. A directory's own mtime moves when an entry is
                    // created or removed directly inside it — exactly when the
                    // region set can change.
                    if let Ok(meta) = entry.metadata()
                        && let Ok(mtime) = meta.modified()
                        && mtime > manifest_mtime
                    {
                        stale = true;
                        break 'walk;
                    }
                    continue;
                }
                stack.push((path, ignores.clone()));
            } else if ftype.is_file() {
                // The builder never reads a gitignored file, so its churn (a
                // log, a local `.env`, a `*.min.js` rebuild) is not graph churn.
                // Nor a file a user `[walk] skip` pattern excludes (LF.3a).
                if ignores.is_ignored(&path, false) || ignores.is_excluded(&path, false) {
                    continue;
                }
                checked += 1;
                if let Ok(meta) = entry.metadata()
                    && let Ok(mtime) = meta.modified()
                    && mtime > manifest_mtime
                {
                    stale = true;
                    break 'walk;
                }
            }
        }
    }
    // Diagnostic: env-gated like the shard-skip summary above, so the MCP
    // status path stays quiet by default.
    if std::env::var("GLIA_STORE_VERBOSE").as_deref() == Ok("1") {
        let verdict = if stale { "stale" } else { "fresh" };
        eprintln!("[gmap] stale-scan: gated={gated} files={checked} verdict={verdict}");
    }
    stale
}

// ============================================================================
// Sharded cell mutation
// ============================================================================

/// Upsert a cell in a sharded layout. Scans each shard for the target node,
/// deserializes only the matching shard, mutates, and re-writes that shard
/// (its sections copied verbatim) plus the manifest (updated content hash).
/// Other shards stay untouched.
pub fn upsert_cell_sharded(
    dir: &Path,
    node_id: NodeId,
    cell_type: CellTypeId,
    payload: CellPayload,
) -> Result<(), StoreError> {
    let manifest_path = dir.join(MANIFEST_NAME);
    let mut manifest = read_manifest(dir)?;

    // A foreign-domain shard (LC.10b) is carried verbatim, never mutated.
    for entry in manifest.shards.iter_mut().filter(|e| e.is_code()) {
        let shard_path = dir.join(&entry.path);
        let mmap = MmapContainer::open(&shard_path)?;
        let archived = mmap.archived()?;

        let found = archived
            .nodes
            .iter()
            .any(|n| NodeId(n.id.0.to_native()) == node_id);

        if found {
            drop(mmap);
            let mut file = read_to_owned(&shard_path)?;
            set_cell(&mut file.core, node_id, cell_type, payload)?;
            let bytes = encode_file(&mut file.core, &file.sections)?;
            entry.content_hash = hex_xxhash64(&bytes);
            write_atomic(&shard_path, &bytes)?;

            let manifest_out = serde_json::to_vec_pretty(&manifest)?;
            write_atomic(&manifest_path, &manifest_out)?;
            return Ok(());
        }
    }

    Err(StoreError::NodeNotFound(node_id))
}

// ============================================================================
// Tests
// ============================================================================


#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::CodeNav;
    use repo_graph_core::{Node, RepoId};
    use repo_graph_graph::SymbolTable;

    #[test]
    fn upsert_cell_sharded_finds_correct_shard() {
        let dir = tempfile::tempdir().unwrap();
        let shard_dir = dir.path().join("shards");
        std::fs::create_dir_all(&shard_dir).unwrap();

        let repo = RepoId::from_canonical("test://sharded");
        let g1 = RepoGraph {
            repo,
            nodes: vec![Node {
                id: NodeId(100),
                repo,
                confidence: repo_graph_core::Confidence::Strong,
                cells: vec![],
            }],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let g2 = RepoGraph {
            repo,
            nodes: vec![Node {
                id: NodeId(200),
                repo,
                confidence: repo_graph_core::Confidence::Strong,
                cells: vec![],
            }],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };

        write_sharded(&[("shard_a", &g1), ("shard_b", &g2)], &[], &shard_dir).unwrap();

        upsert_cell_sharded(
            &shard_dir,
            NodeId(200),
            CellTypeId(8),
            CellPayload::Text("attention data".into()),
        )
        .unwrap();

        let opened = ShardedMmap::open(&shard_dir).unwrap();
        let shard_b = opened.shards.iter().find(|(n, _)| n == "shard_b").unwrap();
        let archived = shard_b.1.archived().unwrap();
        let node = &archived.nodes[0];
        assert_eq!(node.cells.len(), 1);

        // The upsert round-trips the manifest struct, so the build stamp must
        // survive it — the layout was still produced by THIS build.
        let m: Manifest =
            serde_json::from_slice(&std::fs::read(shard_dir.join(MANIFEST_NAME)).unwrap())
                .unwrap();
        assert_eq!(m.build_stamp, repo_graph_stamp::BUILD_STAMP);
    }

    /// LC.5b: every graph shard with nav carries a `"code"` section,
    /// `cross_stack.gmap` carries none, the loader rebuilds nav from section +
    /// core, and a sharded cell upsert leaves the code section byte-identical.
    #[test]
    fn shards_carry_the_code_section_and_upserts_keep_it() {
        use crate::code_section::{CODE_SECTION, code_section_of};
        use repo_graph_code_domain::node_kind;

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("layout");
        let repo = RepoId::from_canonical("test://lc5b-layout");
        let mut nav = CodeNav::default();
        nav.record(NodeId(100), "m", "pkg::m", node_kind::MODULE, None);
        nav.record(NodeId(101), "f", "pkg::m::f", node_kind::FUNCTION, Some(NodeId(100)));
        let node = |id: u64| Node {
            id: NodeId(id),
            repo,
            confidence: repo_graph_core::Confidence::Strong,
            cells: vec![],
        };
        let g = RepoGraph {
            repo,
            nodes: vec![node(100), node(101)],
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let cross = vec![Edge {
            from: NodeId(101),
            to: NodeId(9),
            category: repo_graph_code_domain::edge_category::HTTP_CALLS,
            confidence: repo_graph_core::Confidence::Strong,
            cells: Vec::new(),
        }];
        write_sharded(&[("a", &g)], &cross, &dir).unwrap();

        let opened = ShardedMmap::open(&dir).unwrap();
        let shard = &opened.shards[0].1;
        let code_before = shard.section_bytes(CODE_SECTION).unwrap().unwrap().to_vec();
        assert_eq!(
            code_section_of(shard).unwrap().unwrap().qname(NodeId(101)),
            Some("pkg::m::f")
        );
        let cross_file = opened.cross.as_ref().unwrap();
        assert!(cross_file.section_names().unwrap().is_empty(), "cross_stack has no section");
        drop(opened);

        let loaded = read_merged_sharded(&dir).unwrap();
        assert_eq!(loaded.graphs[0].nav.qname_by_id, g.nav.qname_by_id);
        assert_eq!(loaded.graphs[0].nav.kind_by_id, g.nav.kind_by_id);
        assert_eq!(loaded.graphs[0].nav.parent_of, g.nav.parent_of);
        assert_eq!(loaded.cross_edges, cross);

        upsert_cell_sharded(&dir, NodeId(101), CellTypeId(8), CellPayload::Text("x".into()))
            .unwrap();
        let reopened = ShardedMmap::open(&dir).unwrap();
        let shard = &reopened.shards[0].1;
        assert_eq!(shard.section_bytes(CODE_SECTION).unwrap().unwrap(), code_before.as_slice());
        assert_eq!(shard.archived().unwrap().kind(NodeId(101)), Some(node_kind::FUNCTION));
    }

    /// LC.7: the layout metadata round-trips through the manifest, repos are
    /// written sorted by id whatever order the caller gives, and a write
    /// without metadata keeps the manifest free of both keys.
    #[test]
    fn layout_meta_round_trips_through_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let g = empty_graph("test://lc7-meta");
        let merged = repo_graph_graph::MergedGraph { graphs: vec![g], ..Default::default() };
        let meta = LayoutMeta {
            repos: vec![
                RepoMeta { id: 9, label: "web".into(), root: Some("../web".into()) },
                RepoMeta { id: 3, label: "api".into(), root: None },
            ],
            parse_errors: vec!["b.py: second".into(), "a.py: first".into()],
        };
        let dir = tmp.path().join("with");
        let written = write_merged_sharded_meta(&merged, &meta, &dir).unwrap();
        assert_eq!(written.repos.iter().map(|r| r.id).collect::<Vec<_>>(), vec![3, 9]);
        let (_, back) = read_merged_sharded_meta(&dir).unwrap();
        assert_eq!(back.repos, written.repos);
        assert_eq!(back.parse_errors, meta.parse_errors, "build order, not sorted");
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).unwrap()).unwrap();
        assert!(json["repos"][0].get("root").is_none(), "an unknown root is omitted: {json}");

        let bare = tmp.path().join("bare");
        write_merged_sharded(&merged, &bare).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bare.join(MANIFEST_NAME)).unwrap()).unwrap();
        assert!(json.get("repos").is_none() && json.get("parse_errors").is_none(), "{json}");
        let (_, empty) = read_merged_sharded_meta(&bare).unwrap();
        assert_eq!(empty, LayoutMeta::default());
    }

    /// LC.10a: the post-pass undo record rides the manifest, written sorted
    /// by node id whatever order the graph holds, read back onto the graph;
    /// an empty record writes no key; an unknown confidence spelling is a
    /// corrupt manifest that needs a rebuild, never a silently dropped undo.
    #[test]
    fn pass_undo_round_trips_through_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut merged = repo_graph_graph::MergedGraph {
            graphs: vec![empty_graph("test://lc10a-undo")],
            ..Default::default()
        };
        merged.pass_undo =
            vec![(NodeId(9), Confidence::Strong), (NodeId(2), Confidence::Medium)];
        let dir = tmp.path().join("undo");
        let written = write_merged_sharded(&merged, &dir).unwrap();
        assert_eq!(
            written.pass_undo,
            vec![
                PassUndo { node: 2, confidence: "medium".into() },
                PassUndo { node: 9, confidence: "strong".into() },
            ]
        );
        let back = read_merged_sharded(&dir).unwrap();
        assert_eq!(
            back.pass_undo,
            vec![(NodeId(2), Confidence::Medium), (NodeId(9), Confidence::Strong)]
        );

        let bare = tmp.path().join("bare");
        merged.pass_undo.clear();
        write_merged_sharded(&merged, &bare).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bare.join(MANIFEST_NAME)).unwrap()).unwrap();
        assert!(json.get("pass_undo").is_none(), "{json}");
        assert!(read_merged_sharded(&bare).unwrap().pass_undo.is_empty());

        let path = dir.join(MANIFEST_NAME);
        let text = std::fs::read_to_string(&path).unwrap().replace("\"strong\"", "\"certain\"");
        std::fs::write(&path, text).unwrap();
        let err = read_merged_sharded(&dir).unwrap_err();
        assert!(err.needs_rebuild(), "{err}");
        assert!(err.to_string().contains("certain"), "{err}");
    }

    /// LC.8: the lenient read yields schema, stamp and roots from any schema,
    /// and nothing from a missing or unparseable manifest.
    #[test]
    fn lenient_manifest_reads_any_schema() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_manifest_lenient(tmp.path()), None, "no manifest");

        let v1 = tmp.path().join("v1");
        std::fs::create_dir_all(&v1).unwrap();
        std::fs::write(
            v1.join(MANIFEST_NAME),
            r#"{"schema_version":1,"engine_version":"0.4.18",
               "build_stamp":"0.4.18+p3d23e8828e7ba01a",
               "shards":[{"name":"repo-7","path":"repo-7.gmap","content_hash":"00"}]}"#,
        )
        .unwrap();
        let m = read_manifest_lenient(&v1).unwrap();
        assert_eq!((m.schema_version, m.build_stamp.as_str()), (1, "0.4.18+p3d23e8828e7ba01a"));
        assert!(m.repos.is_empty());

        let g = empty_graph("test://lc8-lenient");
        let merged = repo_graph_graph::MergedGraph { graphs: vec![g], ..Default::default() };
        let meta = LayoutMeta {
            repos: vec![RepoMeta { id: 3, label: "api".into(), root: Some("../api".into()) }],
            parse_errors: vec![],
        };
        let v2 = tmp.path().join("v2");
        write_merged_sharded_meta(&merged, &meta, &v2).unwrap();
        // Unreadable shards do not matter: only the manifest is read.
        for e in std::fs::read_dir(&v2).unwrap().flatten() {
            if e.file_name().to_string_lossy().ends_with(".gmap") {
                std::fs::write(e.path(), b"garbage").unwrap();
            }
        }
        let m = read_manifest_lenient(&v2).unwrap();
        assert_eq!(m.schema_version, MANIFEST_VERSION);
        assert_eq!(m.build_stamp, repo_graph_stamp::BUILD_STAMP);
        assert_eq!(m.repos, meta.repos);

        std::fs::write(v2.join(MANIFEST_NAME), b"{not json").unwrap();
        assert_eq!(read_manifest_lenient(&v2), None, "unparseable");
        std::fs::write(v2.join(MANIFEST_NAME), br#"{"shards":[]}"#).unwrap();
        assert_eq!(read_manifest_lenient(&v2), None, "no schema_version");
    }

    #[test]
    fn write_sharded_skips_unchanged_shards_on_rewrite() {
        // Phase 1 incremental rebuild test: write a sharded layout twice
        // with identical inputs; verify that the second pass does NOT
        // change the mtime of either shard file (skip-when-unchanged).
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let shard_dir = dir.path();
        let repo = RepoId::from_canonical("test://incremental");
        let g_a = RepoGraph {
            repo,
            nodes: vec![Node {
                id: NodeId(100),
                repo,
                confidence: repo_graph_core::Confidence::Strong,
                cells: vec![],
            }],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let g_b = RepoGraph {
            repo,
            nodes: vec![Node {
                id: NodeId(200),
                repo,
                confidence: repo_graph_core::Confidence::Strong,
                cells: vec![],
            }],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        write_sharded(&[("a", &g_a), ("b", &g_b)], &[], shard_dir).unwrap();
        let shard_a_path = shard_dir.join("a.gmap");
        let shard_b_path = shard_dir.join("b.gmap");
        let manifest_path = shard_dir.join(MANIFEST_NAME);
        let mtime_a_before = std::fs::metadata(&shard_a_path).unwrap().modified().unwrap();
        let mtime_b_before = std::fs::metadata(&shard_b_path).unwrap().modified().unwrap();
        let mtime_m_before = std::fs::metadata(&manifest_path).unwrap().modified().unwrap();
        // Sleep a bit so mtime resolution differences are visible if a
        // write_atomic does fire.
        std::thread::sleep(Duration::from_millis(50));
        // Rewrite with identical inputs.
        write_sharded(&[("a", &g_a), ("b", &g_b)], &[], shard_dir).unwrap();
        let mtime_a_after = std::fs::metadata(&shard_a_path).unwrap().modified().unwrap();
        let mtime_b_after = std::fs::metadata(&shard_b_path).unwrap().modified().unwrap();
        let mtime_m_after = std::fs::metadata(&manifest_path).unwrap().modified().unwrap();
        assert_eq!(mtime_a_before, mtime_a_after, "shard a was rewritten despite unchanged input");
        assert_eq!(mtime_b_before, mtime_b_after, "shard b was rewritten despite unchanged input");
        assert_eq!(mtime_m_before, mtime_m_after, "manifest was rewritten despite unchanged input");
    }

    /// LC.8: a shard damaged after its write keeps its manifest entry, so the
    /// manifest alone says "unchanged". The rewrite must check the bytes on
    /// disk and replace it, or a rebuild never repairs the layout.
    #[test]
    fn write_sharded_rewrites_a_damaged_shard() {
        let tmp = tempfile::tempdir().unwrap();
        let g = empty_graph("test://lc8-damaged");
        let cross = vec![Edge {
            from: NodeId(1),
            to: NodeId(2),
            category: EdgeCategoryId(1),
            confidence: repo_graph_core::Confidence::Strong,
            cells: Vec::new(),
        }];
        write_sharded(&[("a", &g)], &cross, tmp.path()).unwrap();
        let good_a = std::fs::read(tmp.path().join("a.gmap")).unwrap();
        let good_x = std::fs::read(tmp.path().join(CROSS_STACK_NAME)).unwrap();
        std::fs::write(tmp.path().join("a.gmap"), &good_a[..good_a.len() / 2]).unwrap();
        let mut flipped = good_x.clone();
        flipped[0] ^= 0xff;
        std::fs::write(tmp.path().join(CROSS_STACK_NAME), &flipped).unwrap();
        assert!(read_merged_sharded(tmp.path()).is_err(), "damaged layout must not load");

        write_sharded(&[("a", &g)], &cross, tmp.path()).unwrap();
        assert_eq!(std::fs::read(tmp.path().join("a.gmap")).unwrap(), good_a);
        assert_eq!(std::fs::read(tmp.path().join(CROSS_STACK_NAME)).unwrap(), good_x);
        read_merged_sharded(tmp.path()).unwrap();
    }

    #[test]
    fn stale_when_manifest_written_by_other_engine_version() {
        let dir = tempfile::tempdir().unwrap();
        let gmap_dir = dir.path().join("gmap");
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let repo = RepoId::from_canonical("test://stale");
        let g = RepoGraph {
            repo,
            nodes: vec![],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        write_sharded(&[("a", &g)], &[], &gmap_dir).unwrap();
        // Fresh manifest from THIS engine version + no newer sources → fresh.
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));

        // Same layout but stamped by another release → stale regardless of
        // source mtimes (the upgrade / poisoned-cache path).
        let manifest_path = gmap_dir.join(MANIFEST_NAME);
        let mut m: Manifest =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        m.engine_version = "0.0.0".to_string();
        std::fs::write(&manifest_path, serde_json::to_vec(&m).unwrap()).unwrap();
        // Ensure the rewrite itself can't be what makes it stale: mtime check
        // compares repo files (none) to the manifest, so only the version
        // mismatch can trigger here.
        assert!(is_gmap_stale(&gmap_dir, &repo_dir));

        // Pre-0.4.17 manifest (no engine_version field at all) → stale.
        m.engine_version = String::new();
        std::fs::write(&manifest_path, serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(is_gmap_stale(&gmap_dir, &repo_dir));
    }

    #[test]
    fn stale_when_manifest_written_by_other_build_stamp() {
        // `engine_version` is the RELEASE, which does not move when a parser
        // fix is merged. `build_stamp` is the build identity, so a within-a-
        // release parser change forces a regenerate. Patched as raw JSON (not
        // through `Manifest`) so the test exercises what is actually on disk.
        let dir = tempfile::tempdir().unwrap();
        let gmap_dir = dir.path().join("gmap");
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let repo = RepoId::from_canonical("test://stamp");
        let g = RepoGraph {
            repo,
            nodes: vec![],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        write_sharded(&[("a", &g)], &[], &gmap_dir).unwrap();
        let manifest_path = gmap_dir.join(MANIFEST_NAME);

        // (1) The written manifest carries a non-empty build stamp.
        let read_json = || -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap()
        };
        let mut v = read_json();
        let stamp = v["build_stamp"].as_str().unwrap_or("");
        assert!(!stamp.is_empty(), "manifest.json has no build_stamp: {v}");
        assert_eq!(stamp, repo_graph_stamp::BUILD_STAMP);

        // (2) Fresh layout from this build, no newer sources -> not stale.
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));

        // (3) Same release, different build stamp -> stale. Shard hashes and
        // `engine_version` are untouched, so only the stamp can trigger it.
        v["build_stamp"] = serde_json::json!("0.4.18+p0000000000000000");
        assert_eq!(v["engine_version"].as_str().unwrap(), env!("CARGO_PKG_VERSION"));
        std::fs::write(&manifest_path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(is_gmap_stale(&gmap_dir, &repo_dir));

        // (4) Pre-0.4.19 manifest (no `build_stamp` field at all) -> stale.
        v.as_object_mut().unwrap().remove("build_stamp");
        std::fs::write(&manifest_path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(is_gmap_stale(&gmap_dir, &repo_dir));
    }

    // ------------------------------------------------------------------
    // is_gmap_stale gating (audit #17)
    // ------------------------------------------------------------------

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn empty_graph(canonical: &str) -> RepoGraph {
        RepoGraph {
            repo: RepoId::from_canonical(canonical),
            nodes: vec![],
            edges: vec![],
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        }
    }

    /// A repo whose only post-manifest churn is inside directories the builder
    /// collapses to a REGION. The gated directories THEMSELVES predate the
    /// manifest (a real `node_modules` / `dist` / `.venv-eval` was there before
    /// the build that wrote the gmap); the files that move afterwards sit one
    /// level down, so the top-level gated dir's own mtime does not move either.
    /// That separation is what lets the gated-dir-mtime rule (which exists so a
    /// NEW region still marks the gmap stale) coexist with skipping the trees.
    fn gated_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let gmap_dir = dir.path().join("gmap");
        let repo_dir = dir.path().join("repo");

        write_file(&repo_dir.join("src/a.py"), "x = 1\n");
        write_file(&repo_dir.join(".gitignore"), "# local eval venv\n.venv-eval/\n");
        std::fs::create_dir_all(repo_dir.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(repo_dir.join("dist/assets")).unwrap();
        std::fs::create_dir_all(repo_dir.join(".venv-eval/lib")).unwrap();

        write_sharded(&[("a", &empty_graph("test://gating"))], &[], &gmap_dir).unwrap();
        // mtime granularity: make "after the manifest" unambiguous.
        std::thread::sleep(std::time::Duration::from_millis(20));

        write_file(&repo_dir.join("node_modules/pkg/x.js"), "//\n");
        write_file(&repo_dir.join("dist/assets/bundle.js"), "//\n");
        write_file(&repo_dir.join(".venv-eval/lib/y.py"), "y = 1\n");

        (dir, gmap_dir, repo_dir)
    }

    #[test]
    fn stale_scan_skips_gated_dirs() {
        let (_tmp, gmap_dir, repo_dir) = gated_repo();
        // `node_modules` was already skipped; `dist` (always_region) and
        // `.venv-eval` (top-level .gitignore) were not, so the old six-name
        // list regenerated the whole gmap for files no parser ever reads.
        assert!(
            !is_gmap_stale(&gmap_dir, &repo_dir),
            "churn confined to collapsed regions must not mark the gmap stale"
        );
    }

    #[test]
    fn new_gated_dir_still_marks_stale() {
        let (_tmp, gmap_dir, repo_dir) = gated_repo();
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
        // A gated dir is not invisible to the graph: the builder emits one
        // REGION node per collapsed directory, so a region that appears after
        // the build DOES change the graph. Its own mtime is the signal.
        write_file(&repo_dir.join("coverage/index.html"), "<html/>\n");
        assert!(
            is_gmap_stale(&gmap_dir, &repo_dir),
            "a newly-appeared collapsed region must still mark the gmap stale"
        );
    }

    /// The conventional on-disk layout: the gmap lives INSIDE the repo at
    /// `<repo>/.glia/graph`, so `.glia` holds both our output and inputs the
    /// build reads (`overlay.toml`), and `.ai` is a directory the engine
    /// ingests docs from.
    fn glia_layout_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("repo");
        let gmap_dir = repo_dir.join(DEFAULT_GMAP_SUBDIR);
        write_file(&repo_dir.join("src/a.py"), "x = 1\n");
        std::fs::create_dir_all(repo_dir.join("node_modules/pkg")).unwrap();
        write_sharded(&[("a", &empty_graph("test://glia-layout"))], &[], &gmap_dir).unwrap();
        // mtime granularity: make "after the manifest" unambiguous.
        std::thread::sleep(std::time::Duration::from_millis(20));
        (dir, gmap_dir, repo_dir)
    }

    #[test]
    fn default_layout_is_under_dot_glia() {
        assert_eq!(default_gmap_dir(Path::new("r")), Path::new("r/.glia/graph"));
        assert_eq!(LEGACY_GMAP_SUBDIR, ".ai/repo-graph");
    }

    #[test]
    fn stale_scan_ignores_our_own_output() {
        let (_tmp, gmap_dir, repo_dir) = glia_layout_repo();
        // The parse cache is written AFTER the manifest, inside the gmap dir.
        // Counting it would make every gmap permanently stale — an infinite
        // regenerate loop. Churn in a collapsed region is invisible too.
        write_file(&gmap_dir.join("parse_cache.bin"), "cache\n");
        write_file(&gmap_dir.join(".gitignore"), "*\n");
        write_file(&repo_dir.join("node_modules/pkg/x.js"), "//\n");
        assert!(
            !is_gmap_stale(&gmap_dir, &repo_dir),
            "our own output must never mark the gmap stale"
        );
    }

    /// LC.9: a 0.4.x wrapper still writing `<repo>/.ai/repo-graph` beside the
    /// 0.5.0 layout must not mark the new layout stale.
    #[test]
    fn stale_scan_ignores_legacy_dir() {
        let (_tmp, gmap_dir, repo_dir) = glia_layout_repo();
        write_file(&repo_dir.join(LEGACY_GMAP_SUBDIR).join("parse_cache.bin"), "old\n");
        write_file(&repo_dir.join(LEGACY_GMAP_SUBDIR).join("manifest.json"), "{}\n");
        assert!(
            !is_gmap_stale(&gmap_dir, &repo_dir),
            "the legacy layout is engine output, not source"
        );
    }

    /// A layout checked at another dir skips the repo's default layout too:
    /// a hook refreshing `<repo>/.glia/graph` is not source churn.
    #[test]
    fn stale_scan_skips_the_default_layout_for_any_gmap_dir() {
        let (_tmp, gmap_dir, repo_dir) = gated_repo();
        write_file(&repo_dir.join(DEFAULT_GMAP_SUBDIR).join("manifest.json"), "{}\n");
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
    }

    #[test]
    fn stale_scan_sees_ai_docs_beside_the_gmap_dir() {
        let (_tmp, gmap_dir, repo_dir) = glia_layout_repo();
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
        // `.ai` is NOT blanket-skipped: the engine ingests `.ai/**/*.md` as
        // DOC_SECTION nodes, so a doc edit DOES change the graph. Only the
        // legacy `.ai/repo-graph` inside it is (a prefix, not the name `.ai`).
        write_file(&repo_dir.join(".ai/architecture.md"), "# arch\n");
        assert!(
            is_gmap_stale(&gmap_dir, &repo_dir),
            "an ingested .ai doc must mark the gmap stale"
        );
    }

    /// `.glia` is hard-skipped by the mtime scan (LF.1d), but its inputs are
    /// not invisible: `.glia/overlay.toml` is an input the build reads, so a
    /// new one changes the manifest's `external_inputs` fingerprint.
    #[test]
    fn stale_scan_sees_glia_inputs_beside_the_gmap_dir() {
        let (_tmp, gmap_dir, repo_dir) = glia_layout_repo();
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
        write_file(&repo_dir.join(".glia/overlay.toml"), "# overlay\n");
        assert!(
            is_gmap_stale(&gmap_dir, &repo_dir),
            "an edited .glia input must mark the gmap stale"
        );
    }

    /// A8.2: the scan applies the builder's full `.gitignore` semantics — globs,
    /// file-level patterns and nested files — and a nested rule stays scoped to
    /// its own directory.
    #[test]
    fn stale_scan_honours_nested_and_glob_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let gmap_dir = dir.path().join("gmap");
        let repo_dir = dir.path().join("repo");
        write_file(&repo_dir.join("src/a.py"), "x = 1\n");
        write_file(&repo_dir.join(".gitignore"), "*.log\ndist-*\n");
        write_file(&repo_dir.join("pkg/.gitignore"), "generated/\n");
        for d in ["dist-x/sub", "pkg/generated/sub", "src/generated/sub"] {
            std::fs::create_dir_all(repo_dir.join(d)).unwrap();
        }
        write_sharded(&[("a", &empty_graph("test://gi-semantics"))], &[], &gmap_dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Glob dir, nested-file dir, and a file-level pattern: none is read by
        // the builder, so none is graph churn.
        write_file(&repo_dir.join("dist-x/sub/bundle.js"), "//\n");
        write_file(&repo_dir.join("pkg/generated/sub/out.py"), "y = 1\n");
        write_file(&repo_dir.join("server.log"), "GET /\n");
        assert!(
            !is_gmap_stale(&gmap_dir, &repo_dir),
            "churn the builder's .gitignore semantics hide must not mark stale"
        );
        // `pkg/.gitignore` does not reach `src/generated`: that is source.
        write_file(&repo_dir.join("src/generated/sub/real.py"), "z = 1\n");
        assert!(
            is_gmap_stale(&gmap_dir, &repo_dir),
            "a nested rule must not leak to a sibling tree"
        );
    }

    /// LF.3a: the user's `[walk] skip` gates the scan exactly as it gates the
    /// walk. Churn under an excluded directory, or in an excluded file, is not
    /// graph churn; a file beside them still is. The control repo with no
    /// overlay goes stale on the very same edit.
    #[test]
    fn stale_scan_honours_config_skip() {
        let setup = |overlay: bool| {
            let dir = tempfile::tempdir().unwrap();
            let repo_dir = dir.path().join("repo");
            let gmap_dir = default_gmap_dir(&repo_dir);
            write_file(&repo_dir.join("app.py"), "def main():\n    return 1\n");
            write_file(&repo_dir.join("legacy/sub/old.py"), "def legacy_thing():\n    return 1\n");
            write_file(&repo_dir.join("src/api.gen.py"), "x = 1\n");
            if overlay {
                write_file(
                    &repo_dir.join(".glia/overlay.toml"),
                    "version = 1\n[walk]\nskip = [\"legacy\", \"*.gen.py\"]\n",
                );
            }
            write_merged_sharded(&one_graph("test://config-skip"), &gmap_dir).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
            (dir, gmap_dir, repo_dir)
        };

        let (_tmp, gmap_dir, repo_dir) = setup(true);
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir), "fresh right after the write");
        write_file(&repo_dir.join("legacy/sub/old.py"), "def legacy_thing():\n    return 2\n");
        write_file(&repo_dir.join("legacy/sub/new.py"), "y = 1\n");
        write_file(&repo_dir.join("src/api.gen.py"), "x = 2\n");
        assert!(
            !is_gmap_stale(&gmap_dir, &repo_dir),
            "churn under a [walk] skip pattern must not mark stale"
        );
        write_file(&repo_dir.join("app.py"), "def main():\n    return 2\n");
        assert!(is_gmap_stale(&gmap_dir, &repo_dir), "an un-skipped file still marks stale");

        let (_tmp, gmap_dir, repo_dir) = setup(false);
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
        write_file(&repo_dir.join("legacy/sub/old.py"), "def legacy_thing():\n    return 2\n");
        assert!(is_gmap_stale(&gmap_dir, &repo_dir), "control: without the config it is source");
    }

    // ------------------------------------------------------------------
    // LF.1d: `.glia` inputs by content, not by mtime
    // ------------------------------------------------------------------

    fn one_graph(canonical: &str) -> repo_graph_graph::MergedGraph {
        repo_graph_graph::MergedGraph { graphs: vec![empty_graph(canonical)], ..Default::default() }
    }

    fn manifest_json(dir: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).unwrap()).unwrap()
    }

    /// The probe-s2 repo: `.gitignore` holds `.glia/docs-snapshot/` (the
    /// documented default), and the default layout is written with the
    /// snapshot already synced.
    fn docs_snapshot_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("repo");
        let gmap_dir = default_gmap_dir(&repo_dir);
        write_file(&repo_dir.join("m.py"), "def f():\n    return 1\n");
        write_file(&repo_dir.join(".gitignore"), ".glia/docs-snapshot/\n");
        write_file(&repo_dir.join(".glia/docs-snapshot/manifest.jsonl"), "{\"v\":1}\n");
        write_merged_sharded(&one_graph("test://docs-snapshot"), &gmap_dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        (dir, gmap_dir, repo_dir)
    }

    /// Pre-fix (probe s2, HEAD code): `is_stale` was False right after the
    /// persist AND after exactly this rewrite. The dir is gitignored, so it was
    /// GATED and only its own mtime was watched, and an in-place rewrite does
    /// not move it: the MCP warm path kept serving the pre-sync docs.
    #[test]
    fn in_place_docs_snapshot_rewrite_marks_stale() {
        let (_tmp, gmap_dir, repo_dir) = docs_snapshot_repo();
        let snap = repo_dir.join(".glia/docs-snapshot/manifest.jsonl");
        let recorded = manifest_json(&gmap_dir)["external_inputs"].clone();
        assert_eq!(recorded.as_object().map(|o| o.len()), Some(1), "{recorded}");
        assert!(recorded[".glia/docs-snapshot/manifest.jsonl"].is_string(), "{recorded}");
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir), "fresh right after the persist");

        let dir_mtime = || std::fs::metadata(snap.parent().unwrap()).unwrap().modified().unwrap();
        let before = dir_mtime();
        // What doc-sources `write_snapshot` does: `std::fs::write` in place,
        // same length, different bytes.
        std::fs::write(&snap, "{\"v\":2}\n").unwrap();
        assert_eq!(before, dir_mtime(), "an in-place rewrite moves no directory mtime");
        assert!(is_gmap_stale(&gmap_dir, &repo_dir), "an in-place snapshot rewrite must mark stale");
    }

    #[test]
    fn deleting_an_input_marks_stale() {
        let (_tmp, gmap_dir, repo_dir) = docs_snapshot_repo();
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
        std::fs::remove_file(repo_dir.join(".glia/docs-snapshot/manifest.jsonl")).unwrap();
        assert!(is_gmap_stale(&gmap_dir, &repo_dir), "a deleted .glia input must mark stale");
    }

    #[test]
    fn adding_a_nested_input_marks_stale_and_a_rewrite_restores_fresh() {
        let (_tmp, gmap_dir, repo_dir) = docs_snapshot_repo();
        write_file(&repo_dir.join(".glia/history/2026/log.jsonl"), "{}\n");
        assert!(is_gmap_stale(&gmap_dir, &repo_dir), "a new nested .glia input must mark stale");
        // Regenerating records it; the same bytes are fresh again.
        write_merged_sharded(&one_graph("test://docs-snapshot"), &gmap_dir).unwrap();
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
        let fp = external_inputs_fingerprint(&repo_dir);
        let keys: Vec<&str> = fp.keys().map(String::as_str).collect();
        assert_eq!(keys, [".glia/docs-snapshot/manifest.jsonl", ".glia/history/2026/log.jsonl"]);
    }

    /// The store's own writes under `.glia` never enter the fingerprint, or a
    /// layout would go stale the moment it is written (a silent infinite
    /// regenerate).
    #[test]
    fn fingerprint_excludes_gmap_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("repo");
        write_file(&repo_dir.join(".glia/overlay.toml"), "# overlay\n");
        let before = external_inputs_fingerprint(&repo_dir);
        assert_eq!(before.keys().collect::<Vec<_>>(), [".glia/overlay.toml"]);

        let gmap_dir = default_gmap_dir(&repo_dir);
        write_merged_sharded(&one_graph("test://own-output"), &gmap_dir).unwrap();
        write_file(&gmap_dir.join("parse_cache.bin"), "cache\n");
        write_file(&gmap_dir.join("parse_cache.bin.123.tmp"), "half\n");
        write_file(&gmap_dir.join(".gitignore"), "*\n");
        write_file(&gmap_dir.join("notes.txt"), "anything under the layout dir\n");
        // The flat 0.4.x `glia build` output and a custom layout inside .glia.
        write_file(&repo_dir.join(".glia/repo-1.gmap"), "old\n");
        write_file(&repo_dir.join(".glia/custom/manifest.json"), "{}\n");
        write_file(&repo_dir.join(".glia/custom/repo-1.gmap"), "x\n");
        write_file(&repo_dir.join(".glia/custom/.gitignore"), "*\n");
        write_file(&repo_dir.join(".glia/build.lock"), "\n");
        assert_eq!(external_inputs_fingerprint(&repo_dir), before);

        write_merged_sharded(&one_graph("test://own-output"), &gmap_dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_file(&gmap_dir.join("parse_cache.bin"), "cache v2\n");
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir), "our own output must never mark stale");
    }

    #[test]
    fn manifest_without_inputs_has_no_external_inputs_key() {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("repo");
        write_file(&repo_dir.join("m.py"), "x = 1\n");
        let gmap_dir = default_gmap_dir(&repo_dir);
        write_merged_sharded(&one_graph("test://no-inputs"), &gmap_dir).unwrap();
        let bytes = std::fs::read(gmap_dir.join(MANIFEST_NAME)).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("external_inputs"), "{text}");
        // A manifest without the key reads back as an empty map.
        let m: Manifest = serde_json::from_str(&text).unwrap();
        assert!(m.external_inputs.is_empty());
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
    }

    #[test]
    fn custom_dir_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let repo_dir = dir.path().join("repo");
        write_file(&repo_dir.join(".glia/overlay.toml"), "# overlay\n");
        let custom = dir.path().join("out");
        write_merged_sharded(&one_graph("test://custom"), &custom).unwrap();
        assert!(manifest_json(&custom).get("external_inputs").is_none());
        // No root was known, so a repo WITH inputs is stale on every call:
        // regenerate, never serve stale.
        assert!(is_gmap_stale(&custom, &repo_dir));
        assert!(is_gmap_stale(&custom, &repo_dir));

        // An explicit root records its fingerprint.
        write_merged_sharded_for_repo(&one_graph("test://custom"), &LayoutMeta::default(), &custom, &repo_dir)
            .unwrap();
        assert!(manifest_json(&custom)["external_inputs"][".glia/overlay.toml"].is_string());
        assert!(!is_gmap_stale(&custom, &repo_dir));

        // So does metadata naming exactly one root (LC.7 records it relative
        // to the layout dir), which is what the engine's writer passes.
        let meta = LayoutMeta {
            repos: vec![RepoMeta { id: 1, label: "repo".into(), root: Some("../repo".into()) }],
            parse_errors: vec![],
        };
        let other = dir.path().join("out2");
        write_merged_sharded_meta(&one_graph("test://custom"), &meta, &other).unwrap();
        assert!(!is_gmap_stale(&other, &repo_dir));
        write_file(&repo_dir.join(".glia/overlay.toml"), "# overlay v2\n");
        assert!(is_gmap_stale(&other, &repo_dir));
    }

    /// LC.10b (LF.1d's multi-root hand-off): a layout recording several roots
    /// fingerprints each repo's `.glia` inputs, so it is fresh for every root
    /// until one of that root's own inputs changes. Before, it recorded none
    /// and was stale on every call for any root with inputs.
    #[test]
    fn multi_root_layout_fingerprints_each_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = |n: &str| tmp.path().join(n);
        for n in ["a", "b", "c", "d"] {
            write_file(&root(n).join("m.py"), "def f():\n    return 1\n");
        }
        for n in ["a", "b", "d"] {
            write_file(&root(n).join(".glia/overlay.toml"), &format!("# {n}\n"));
        }
        let meta = LayoutMeta {
            repos: ["a", "b", "c"]
                .iter()
                .enumerate()
                .map(|(i, n)| RepoMeta { id: i as u64 + 1, label: n.to_string(), root: Some(format!("../{n}")) })
                .collect(),
            parse_errors: vec![],
        };
        let out = root("out");
        write_merged_sharded_meta(&one_graph("test://multi"), &meta, &out).unwrap();
        let json = manifest_json(&out);
        assert!(json.get("external_inputs").is_none(), "no single root: {json}");
        let by_repo = json["external_inputs_by_repo"].as_object().unwrap();
        assert_eq!(by_repo.keys().collect::<Vec<_>>(), ["1", "2"], "c has no inputs: {json}");
        assert!(by_repo["1"][".glia/overlay.toml"].is_string());
        for n in ["a", "b", "c"] {
            assert!(!is_gmap_stale(&out, &root(n)), "{n}: fresh right after the write");
        }
        write_file(&root("b").join(".glia/overlay.toml"), "# b v2\n");
        assert!(is_gmap_stale(&out, &root("b")), "b's own input changed");
        assert!(!is_gmap_stale(&out, &root("a")), "a's inputs did not");
        write_file(&root("c").join(".glia/overlay.toml"), "# c\n");
        assert!(is_gmap_stale(&out, &root("c")), "c gained an input");
        assert!(is_gmap_stale(&out, &root("d")), "d is no root of the layout: nothing recorded");
    }

    /// LC.10b: a foreign-domain shard rides a layout verbatim (never decoded,
    /// so any bytes do), listed with its graph_type; code readers skip it,
    /// `read_layout_extras` hands it back hash-checked, and a bad one is
    /// refused before anything is written.
    #[test]
    fn foreign_shards_ride_the_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let merged = one_graph("test://foreign");
        let foreign = vec![ForeignShard {
            name: "clip".into(),
            graph_type: "toy".into(),
            bytes: b"not a gmap: never decoded".to_vec(),
        }];
        let members = vec![MemberMeta { name: "cam".into(), source: "gmap".into(), build_stamp: "s".into() }];
        let dir = tmp.path().join("layout");
        let written = write_merged_sharded_extras(&merged, &LayoutMeta::default(), &foreign, &members, &dir)
            .unwrap();
        assert_eq!(written.shards.len(), 2);
        assert_eq!(written.shards[1].path, "clip.gmap");
        assert_eq!(written.shards[1].content_hash, hex_xxhash64(&foreign[0].bytes));
        let json = manifest_json(&dir);
        assert!(json["shards"][0].get("graph_type").is_none(), "a code shard keeps its old JSON: {json}");
        assert_eq!(json["shards"][1]["graph_type"], "toy");
        assert_eq!(json["members"][0]["name"], "cam");
        let old: ShardEntry =
            serde_json::from_str(r#"{"name":"a","path":"a.gmap","content_hash":"00"}"#).unwrap();
        assert!(old.is_code(), "an entry written before the field is code");

        let opened = ShardedMmap::open(&dir).unwrap();
        assert_eq!((opened.shards.len(), opened.manifest.shards.len()), (1, 2), "opened as code: 1");
        assert_eq!(read_merged_sharded(&dir).unwrap().graphs.len(), 1);
        let extras = read_layout_extras(&dir).unwrap();
        assert_eq!((extras.foreign, extras.members), (foreign.clone(), members));
        assert!(matches!(
            upsert_cell_sharded(&dir, NodeId(424242), CellTypeId(1), CellPayload::Text("x".into())),
            Err(StoreError::NodeNotFound(_))
        ));

        std::fs::write(dir.join("clip.gmap"), b"damaged").unwrap();
        assert!(read_layout_extras(&dir).unwrap_err().needs_rebuild());
        assert!(ShardedMmap::open(&dir).is_err(), "a damaged foreign shard fails the layout");

        let code_name = written.shards[0].name.clone();
        for (name, graph_type) in
            [("../x", "toy"), ("", "toy"), (".hidden", "toy"), ("clip", "code"), ("clip", ""), (code_name.as_str(), "toy")]
        {
            let bad = tmp.path().join(format!("bad-{}", graph_type.len() + name.len()));
            let f = ForeignShard { name: name.into(), graph_type: graph_type.into(), bytes: vec![1] };
            let err = write_merged_sharded_extras(&merged, &LayoutMeta::default(), &[f], &[], &bad)
                .expect_err(&format!("{name:?} / {graph_type:?} must be refused"));
            assert!(err.to_string().contains("foreign shard"), "{err}");
            assert!(!bad.join(MANIFEST_NAME).exists(), "{name:?}: nothing written");
        }
    }

    #[test]
    fn repo_root_of_gmap_dir_inverts_default() {
        for r in ["r", "/abs/repo", "a/b/c", "."] {
            assert_eq!(repo_root_of_gmap_dir(&default_gmap_dir(Path::new(r))), Some(PathBuf::from(r)), "{r}");
        }
        assert_eq!(repo_root_of_gmap_dir(Path::new("r/.glia/graph/")), Some(PathBuf::from("r")));
        assert_eq!(repo_root_of_gmap_dir(Path::new(".glia/graph")), Some(PathBuf::from(".")));
        assert_eq!(repo_root_of_gmap_dir(Path::new("/.glia/graph")), Some(PathBuf::from("/")));
        for other in ["r/.glia", "r/graph", "r/.glia/graph/..", "r/.glia/graph/x", "r/glia/graph", "out"] {
            assert_eq!(repo_root_of_gmap_dir(Path::new(other)), None, "{other}");
        }
    }

    #[test]
    fn stale_scan_still_sees_real_source() {
        let (_tmp, gmap_dir, repo_dir) = gated_repo();
        assert!(!is_gmap_stale(&gmap_dir, &repo_dir));
        write_file(&repo_dir.join("src/a.py"), "x = 2\n");
        assert!(
            is_gmap_stale(&gmap_dir, &repo_dir),
            "a touched source file must still mark the gmap stale"
        );
    }
}
