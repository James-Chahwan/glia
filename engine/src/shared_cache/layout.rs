//! Whole-layout objects (CE.2d): a clean checkout at a tree another machine
//! already built takes that machine's finished `.glia/graph/` layout instead
//! of building. The engine keys a layout ([`layout_key`]), hands out the files
//! of a fresh one ([`export_layout`]) and installs offered files only after
//! re-deriving the key and loading the result ([`install_layout`]). The CLI
//! packs, signs and moves the files (`glia cache push|pull --layout`); nothing
//! here does network I/O or authenticates bytes.
//!
//! # The key
//!
//! A layout is shared only for a CLEAN work tree: `git status` (tracked and
//! untracked, `.glia` excluded) prints nothing, so the tracked files are
//! exactly `HEAD^{tree}` (`git_rev::clean_head_tree`). The key is blake3
//! `derive_key("glia layout object v1")` over these fields, each framed as a
//! `u64` LE length and its bytes:
//!
//! 1. the build stamp (`BUILD_STAMP`): the code that built it;
//! 2. the repo identity key (`walk_gating::repo_identity`), in every NodeId;
//! 3. the `HEAD` tree id;
//! 4. the `.glia` input fingerprint (`glia_store::external_inputs_fingerprint`,
//!    the map the manifest records) as sorted `path\0hash\n` lines;
//! 5. the target facts `arch=<ARCH> pointer_width=<bits> endian=<little|big>
//!    path_sep=<sep>`: shard bytes are rkyv archives, reused only on the same
//!    layout target, and a walked path is spelled with the platform separator;
//! 6. `overlay=on`: only the default layout dir is shared, and it always holds
//!    the overlay-applied graph (`--no-overlay` builds need `--out`);
//! 7. the walk digest: what the build sees beyond the tracked tree. The walk
//!    turns every gated directory present on disk (a gitignored `target/`, an
//!    empty `dist/`, an initialised submodule) into a REGION node, reads project
//!    manifests by name whether git tracks them or not, and the C/C++ build reads
//!    `compile_commands.json` from gitignored build dirs; so the digest hashes the
//!    REGION nodes (id and ORIGIN cell), the project roots and every
//!    `compile_commands.json` directly in the repo root, a project root or one of
//!    their child directories (a superset of the files `c_includes` reads).
//!    A walked source or doc file git does not track (hidden from `status` by
//!    `.git/info/exclude` or a global excludes file, which the walk never reads)
//!    makes the checkout dirty.
//!
//! Not in the key: other off-walk reads of gitignored files (a tsconfig
//! `extends` resolved into `node_modules`). Submodule content is not walked.
//!
//! # Export and install
//!
//! [`export_layout`] ships `manifest.json` and every file it names (shards,
//! `cross_stack.gmap`, foreign shards), each checked against its manifest hash,
//! of a layout that is fresh for the checkout (`is_gmap_stale` false) and holds
//! this repo alone. The parse cache (shared per file, CE.2a / CE.2b), the
//! timeline sidecar (history, not tree: it names commits) and `.gitignore`
//! (written by the install) stay home.
//!
//! [`install_layout`] re-derives the key locally and refuses another one,
//! checks every name (a plain `manifest.json` or `*.gmap` file name, so no path
//! leaves the layout dir) and that the manifest names every other entry, and
//! rewrites the manifest's `repos` to what a local build writes
//! (`persist::layout_meta`: this checkout's label, its root relative to the
//! layout dir, its `HEAD` commit). The files are staged in the sibling
//! `<layout>.pull.<pid>.tmp/` and moved in shards first, `manifest.json` last,
//! with every file they replace or orphan kept aside there; the self-ignoring
//! `.gitignore` is written; the manifest's mtime is set to the moment the
//! install began, so a source edited while it ran reads as newer. Then
//! `persist::load_or_rebuild(dir, root, rebuild = false)` must serve the layout
//! as it is (stamp, `.glia` inputs, source mtimes, every shard's hash, every
//! CODE span resolved); otherwise the new files are removed, the kept-aside
//! ones restored byte for byte, and the result is [`LayoutInstall::Rejected`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use glia_code_domain::project_roots::ProjectRoot;
use glia_code_domain::walk_gating::repo_identity;
use glia_core::{CellPayload, RepoId};
use glia_store::{
    MANIFEST_NAME, Manifest, TIMELINE_FILE, external_inputs_fingerprint, is_gmap_stale,
};

use super::key::CacheKey;
use crate::BUILD_STAMP;
use crate::persist::{
    LoadOutcome, default_layout_dir, layout_meta, load_or_rebuild, write_self_ignore,
};

/// blake3 `derive_key` context of [`layout_key`]. A change to the field list
/// or their framing is a new context string, never a reuse.
const LAYOUT_CONTEXT: &str = "glia layout object v1";
/// Most bytes one exported layout holds, summed over its files.
pub const LAYOUT_MAX_BYTES: u64 = 1 << 30;
/// Most bytes one file of an exported layout holds.
pub const LAYOUT_MAX_ENTRY_BYTES: u64 = 512 << 20;
/// Most files one exported layout holds.
pub const LAYOUT_MAX_ENTRIES: usize = 4096;
/// Longest file name of an exported layout.
pub const LAYOUT_MAX_NAME: usize = 128;
/// The C/C++ compilation database `c_includes` reads off the walk.
const COMPILE_COMMANDS: &str = "compile_commands.json";

/// What a checkout's layout is keyed by ([`layout_key`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayoutKey {
    /// Not shareable, and why: uncommitted or untracked changes, a walked file
    /// git does not track, no commit, not a git work tree, no git.
    Dirty(String),
    /// A clean work tree at `tree` (the `HEAD` tree id), under `key`.
    Clean { key: CacheKey, tree: String },
}

/// What a checkout can offer a store ([`export_layout`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayoutExport {
    /// The work tree is not clean (see [`LayoutKey::Dirty`]).
    Dirty(String),
    /// Clean, but its layout is missing, stale, mid-write or not this repo's
    /// alone: `reason` says which (the fix is `glia build`).
    Stale {
        key: CacheKey,
        tree: String,
        reason: String,
    },
    /// The layout's files, `(name, bytes)` sorted by name: `manifest.json`
    /// and every file it names.
    Ready {
        key: CacheKey,
        tree: String,
        entries: Vec<(String, Vec<u8>)>,
    },
}

/// What [`install_layout`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayoutInstall {
    /// The work tree is not clean: nothing was written.
    Dirty(String),
    /// The offered files were refused (another key, a bad name, a manifest
    /// that does not load): nothing of them is left and the previous layout
    /// is as it was.
    Rejected(String),
    /// Installed and verified: `entries` files, `bytes` bytes.
    Installed {
        tree: String,
        entries: usize,
        bytes: u64,
    },
}

/// The layout key of `repo_path`'s checkout as it is on disk now (see the
/// module doc). Walks the repo (parses nothing) and runs three read-only git
/// commands. `Err` only when `repo_path` is not a directory; every reason not
/// to share (git missing included) is [`LayoutKey::Dirty`].
pub fn layout_key(repo_path: &str) -> Result<LayoutKey, String> {
    let root = Path::new(repo_path);
    if !root.is_dir() {
        return Err(format!("not a directory: {repo_path}"));
    }
    let tree = match crate::git_rev::clean_head_tree(root) {
        Ok(Some(tree)) => tree,
        Ok(None) => {
            return Ok(LayoutKey::Dirty(
                "uncommitted or untracked changes (`git status` is not clean)".to_string(),
            ));
        }
        Err(e) => return Ok(LayoutKey::Dirty(e)),
    };
    let tracked = match crate::git_rev::tracked_paths(root) {
        Ok(t) => t,
        Err(e) => return Ok(LayoutKey::Dirty(e)),
    };
    let walk = match walk_digest(root, &tracked) {
        Ok(d) => d,
        Err(path) => {
            return Ok(LayoutKey::Dirty(format!(
                "the walk reads {path}, which git does not track (an excludes file hides it from `git status`)"
            )));
        }
    };
    let mut inputs = Vec::new();
    for (path, hash) in &external_inputs_fingerprint(root) {
        inputs.extend_from_slice(path.as_bytes());
        inputs.push(0);
        inputs.extend_from_slice(hash.as_bytes());
        inputs.push(b'\n');
    }
    let ident = repo_identity(root);
    let target = format!(
        "arch={} pointer_width={} endian={} path_sep={}",
        std::env::consts::ARCH,
        usize::BITS,
        if cfg!(target_endian = "big") {
            "big"
        } else {
            "little"
        },
        std::path::MAIN_SEPARATOR
    );
    let mut h = blake3::Hasher::new_derive_key(LAYOUT_CONTEXT);
    for field in [
        BUILD_STAMP.as_bytes(),
        ident.key.as_bytes(),
        tree.as_bytes(),
        inputs.as_slice(),
        target.as_bytes(),
        b"overlay=on".as_slice(),
        walk.as_slice(),
    ] {
        frame(&mut h, field);
    }
    let key = CacheKey::from_bytes(*h.finalize().as_bytes());
    Ok(LayoutKey::Clean { key, tree })
}

/// `field` framed as a `u64` LE length and its bytes.
fn frame(h: &mut blake3::Hasher, field: &[u8]) {
    h.update(&(field.len() as u64).to_le_bytes());
    h.update(field);
}

/// Item 7 of the key (module doc): the digest of the walk's REGION nodes,
/// project roots and compilation databases, or `Err(path)` for the first walked
/// source or doc file git does not track.
fn walk_digest(root: &Path, tracked: &BTreeSet<String>) -> Result<[u8; 32], String> {
    let (files, regions, md, roots) = crate::walk::walk_source_files(root);
    for (rel, _) in files.iter().chain(&md) {
        let rel = rel.replace(std::path::MAIN_SEPARATOR, "/");
        if !tracked.contains(&rel) {
            return Err(rel);
        }
    }
    let mut h = blake3::Hasher::new();
    let regions = crate::walk::build_region_graph(&regions, RepoId(0));
    frame(&mut h, &(regions.nodes.len() as u64).to_le_bytes());
    for node in &regions.nodes {
        frame(&mut h, &node.id.0.to_le_bytes());
        for cell in &node.cells {
            frame(&mut h, &cell.kind.0.to_le_bytes());
            let (tag, bytes): (&[u8], &[u8]) = match &cell.payload {
                CellPayload::Text(s) => (b"text", s.as_bytes()),
                CellPayload::Json(s) => (b"json", s.as_bytes()),
                CellPayload::Bytes(b) => (b"bytes", b.as_slice()),
            };
            frame(&mut h, tag);
            frame(&mut h, bytes);
        }
    }
    frame(&mut h, &(roots.len() as u64).to_le_bytes());
    for r in &roots {
        for field in [
            r.rel_path.as_str(),
            r.label.as_str(),
            r.ecosystem,
            r.manifest.as_str(),
        ] {
            frame(&mut h, field.as_bytes());
        }
    }
    for (rel, hash) in compile_databases(root, &roots) {
        frame(&mut h, rel.as_bytes());
        frame(&mut h, hash.as_bytes());
    }
    Ok(*h.finalize().as_bytes())
}

/// Every `compile_commands.json` directly in the repo root, a project root or
/// a child directory of either, as `(repo-relative path, blake3)` in path
/// order: a superset of what `build::c_includes` reads off the walk.
fn compile_databases(root: &Path, roots: &[ProjectRoot]) -> Vec<(String, blake3::Hash)> {
    let dirs: BTreeSet<&str> = std::iter::once("")
        .chain(roots.iter().map(|r| r.rel_path.as_str()))
        .collect();
    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let base = root.join(dir);
        candidates.push(base.join(COMPILE_COMMANDS));
        let children = std::fs::read_dir(&base).into_iter().flatten().flatten();
        candidates.extend(
            children
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path().join(COMPILE_COMMANDS)),
        );
    }
    let mut out: Vec<(String, blake3::Hash)> = candidates
        .into_iter()
        .filter_map(|p| {
            let bytes = std::fs::read(&p).ok()?;
            let rel = p
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            Some((rel, blake3::hash(&bytes)))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Why `name` cannot be a layout file: not a plain file name of at most
/// [`LAYOUT_MAX_NAME`] bytes of `[A-Za-z0-9._+-]` (so never `.`, `..` or a
/// path), or neither `manifest.json` nor a `*.gmap` shard (the timeline
/// sidecar excluded). Every file this admits is also one the `.glia` input
/// fingerprint skips, so a staged or kept-aside file never reads as an input.
fn bad_name(name: &str) -> Option<&'static str> {
    let plain = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'));
    if name.is_empty() || name.len() > LAYOUT_MAX_NAME || !plain || name == "." || name == ".." {
        return Some("not a plain file name");
    }
    if name != MANIFEST_NAME && (!name.ends_with(".gmap") || name == TIMELINE_FILE) {
        return Some("not manifest.json or a shard");
    }
    None
}

/// The files a manifest names, in manifest order: its shards and cross shard.
fn named_files(m: &Manifest) -> Vec<String> {
    m.shards
        .iter()
        .chain(m.cross.as_ref())
        .map(|e| e.path.clone())
        .collect()
}

/// `RepoId.0` of the checkout at `root`, as a build keys it.
fn local_repo_id(root: &Path) -> u64 {
    RepoId::from_canonical(&repo_identity(root).key).0
}

/// xxhash64 of `bytes`, 16 lower-case hex characters: a manifest's shard hash.
fn shard_hash(bytes: &[u8]) -> String {
    use core::hash::Hasher;
    let mut h = twox_hash::XxHash64::with_seed(0);
    h.write(bytes);
    format!("{:016x}", h.finish())
}

/// The files of `repo_path`'s default layout (`<repo>/.glia/graph/`) for a
/// store, keyed for the checkout as it is now (see the module doc). `Err` when
/// a file cannot be read or the layout breaks a cap ([`LAYOUT_MAX_ENTRIES`],
/// [`LAYOUT_MAX_ENTRY_BYTES`], [`LAYOUT_MAX_BYTES`], a bad file name).
pub fn export_layout(repo_path: &str) -> Result<LayoutExport, String> {
    let root = Path::new(repo_path);
    let (key, tree) = match layout_key(repo_path)? {
        LayoutKey::Dirty(reason) => return Ok(LayoutExport::Dirty(reason)),
        LayoutKey::Clean { key, tree } => (key, tree),
    };
    let dir = default_layout_dir(root);
    let stale = |reason: String| {
        Ok(LayoutExport::Stale {
            key,
            tree: tree.clone(),
            reason,
        })
    };
    let manifest_path = dir.join(MANIFEST_NAME);
    if !manifest_path.is_file() {
        return stale(format!(
            "no layout at {}: run `glia build` first",
            dir.display()
        ));
    }
    if is_gmap_stale(&dir, root) {
        return stale("the layout is stale for this checkout: run `glia build` first".to_string());
    }
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let manifest_bytes = read(&manifest_path)?;
    let manifest: Manifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(m) => m,
        Err(e) => return stale(format!("{}: {e}", manifest_path.display())),
    };
    let own = manifest.repos.len() == 1
        && manifest.repos[0].id == local_repo_id(root)
        && manifest.members.is_empty()
        && manifest.external_inputs_by_repo.is_empty();
    if !own {
        return stale(format!(
            "the layout holds {} repo(s) or a merge, not this checkout alone: run `glia build {repo_path}`",
            manifest.repos.len()
        ));
    }
    let hashes: BTreeMap<&str, &str> = manifest
        .shards
        .iter()
        .chain(manifest.cross.as_ref())
        .map(|e| (e.path.as_str(), e.content_hash.as_str()))
        .collect();
    let mut names = named_files(&manifest);
    names.push(MANIFEST_NAME.to_string());
    names.sort();
    if names.windows(2).any(|w| w[0] == w[1]) {
        return stale("the manifest names one file twice".to_string());
    }
    if names.len() > LAYOUT_MAX_ENTRIES {
        return Err(format!(
            "layout has {} files, over the cap of {LAYOUT_MAX_ENTRIES}",
            names.len()
        ));
    }
    let mut entries = Vec::with_capacity(names.len());
    let mut total = 0u64;
    for name in names {
        if let Some(why) = bad_name(&name) {
            return Err(format!("layout file {name:?}: {why}"));
        }
        let bytes = if name == MANIFEST_NAME {
            manifest_bytes.clone()
        } else {
            let path = dir.join(&name);
            let len = std::fs::metadata(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .len();
            if len > LAYOUT_MAX_ENTRY_BYTES {
                return Err(format!(
                    "layout file {name} is {len} bytes, over the cap of {LAYOUT_MAX_ENTRY_BYTES}"
                ));
            }
            let bytes = read(&path)?;
            if hashes
                .get(name.as_str())
                .is_none_or(|h| *h != shard_hash(&bytes))
            {
                return stale(format!(
                    "{name} does not match its manifest hash (a build is writing the layout?)"
                ));
            }
            bytes
        };
        total += bytes.len() as u64;
        if total > LAYOUT_MAX_BYTES {
            return Err(format!(
                "layout is over the cap of {LAYOUT_MAX_BYTES} bytes"
            ));
        }
        entries.push((name, bytes));
    }
    Ok(LayoutExport::Ready { key, tree, entries })
}

/// Install `entries`, a layout offered under `key`, as `repo_path`'s default
/// layout (see the module doc). [`LayoutInstall::Dirty`] and
/// [`LayoutInstall::Rejected`] write nothing that stays. `Err` only when the
/// layout dir cannot be staged, or a failed install could not put the
/// previous layout back (the message names both).
pub fn install_layout(
    repo_path: &str,
    key: &CacheKey,
    entries: Vec<(String, Vec<u8>)>,
) -> Result<LayoutInstall, String> {
    let started = SystemTime::now();
    let root = Path::new(repo_path);
    let tree = match layout_key(repo_path)? {
        LayoutKey::Dirty(reason) => return Ok(LayoutInstall::Dirty(reason)),
        LayoutKey::Clean { key: local, tree } if local == *key => tree,
        LayoutKey::Clean { .. } => {
            return Ok(LayoutInstall::Rejected(
                "key mismatch: the layout was built for another tree, repo, build, target or .glia inputs"
                    .to_string(),
            ));
        }
    };
    let dir = default_layout_dir(root);
    let manifest = match check_offer(root, &dir, &entries) {
        Ok(m) => m,
        Err(reason) => return Ok(LayoutInstall::Rejected(reason)),
    };
    let count = entries.len();
    let bytes: u64 = entries.iter().map(|(_, b)| b.len() as u64).sum();
    let mut swap = Swap::stage(&dir, entries, &manifest)?;
    match swap.apply(started).and_then(|()| verify(&dir, root)) {
        Ok(()) => {
            swap.finish();
            Ok(LayoutInstall::Installed {
                tree,
                entries: count,
                bytes,
            })
        }
        Err(reason) => match swap.roll_back() {
            Ok(()) => Ok(LayoutInstall::Rejected(reason)),
            Err(e) => Err(format!(
                "{reason}; restoring the previous layout failed: {e}"
            )),
        },
    }
}

/// The checks on an offered layout before anything is written: names, caps,
/// duplicates, a manifest of this build and this repo that names every other
/// entry. Returns the manifest's bytes with `repos` rewritten to what a local
/// build of `root` writes into `dir`.
fn check_offer(root: &Path, dir: &Path, entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if entries.len() > LAYOUT_MAX_ENTRIES {
        return Err(format!(
            "{} files, over the cap of {LAYOUT_MAX_ENTRIES}",
            entries.len()
        ));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for (name, bytes) in entries {
        if let Some(why) = bad_name(name) {
            return Err(format!("entry {name:?}: {why}"));
        }
        if !seen.insert(name.as_str()) {
            return Err(format!("entry {name:?} is offered twice"));
        }
        let len = bytes.len() as u64;
        total += len;
        if len > LAYOUT_MAX_ENTRY_BYTES || total > LAYOUT_MAX_BYTES {
            return Err("the layout is over the size caps".to_string());
        }
    }
    let Some((_, manifest_bytes)) = entries.iter().find(|(n, _)| n == MANIFEST_NAME) else {
        return Err("no manifest.json".to_string());
    };
    let mut m: Manifest =
        serde_json::from_slice(manifest_bytes).map_err(|e| format!("manifest.json: {e}"))?;
    if m.build_stamp != BUILD_STAMP {
        return Err(format!("written by another glia build ({})", m.build_stamp));
    }
    let id = local_repo_id(root);
    if m.repos.len() != 1 || m.repos[0].id != id || !m.members.is_empty() {
        return Err("the manifest is not a single-repo layout of this repo".to_string());
    }
    let named: BTreeSet<String> = named_files(&m).into_iter().collect();
    if let Some(stray) = seen
        .iter()
        .find(|n| **n != MANIFEST_NAME && !named.contains(**n))
    {
        return Err(format!("entry {stray:?} is not named by the manifest"));
    }
    let labels = crate::arch::repo_label_map(&[(id, root.to_string_lossy().into_owned())]);
    let roots = BTreeMap::from([(id, root.to_string_lossy().into_owned())]);
    m.repos = layout_meta(&labels, &roots, &[], dir).repos;
    serde_json::to_vec_pretty(&m).map_err(|e| format!("manifest.json: {e}"))
}

/// The post-swap check: the layout loads as it is, fresh for `root`.
fn verify(dir: &Path, root: &Path) -> Result<(), String> {
    match load_or_rebuild(dir, Some(root), false) {
        Ok((_, LoadOutcome::Fresh)) => Ok(()),
        Ok((_, outcome)) => Err(format!(
            "the installed layout did not load as it is ({outcome:?})"
        )),
        Err(e) => Err(format!("the installed layout does not load: {e}")),
    }
}

/// One install's file moves, undoable until [`Swap::finish`]: `new/` holds
/// the staged files, `old/` every file of the layout dir they replaced or
/// orphaned.
struct Swap {
    dir: PathBuf,
    stage: PathBuf,
    /// Staged names, shards sorted, `manifest.json` last.
    order: Vec<String>,
    /// Old files the new manifest no longer names.
    orphans: Vec<String>,
    /// Names now holding a staged file in the layout dir.
    placed: Vec<String>,
    /// Names moved (a shard) or copied (the manifest) into `old/`.
    kept: Vec<String>,
}

impl Swap {
    /// Write `entries` (the manifest as `manifest`) into
    /// `<dir>.pull.<pid>.tmp/new/`, and note the old layout's files the new
    /// manifest drops.
    fn stage(dir: &Path, entries: Vec<(String, Vec<u8>)>, manifest: &[u8]) -> Result<Self, String> {
        let io = |p: &Path, e: std::io::Error| format!("stage the layout at {}: {e}", p.display());
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        let mut stage_name = dir.file_name().unwrap_or_default().to_os_string();
        stage_name.push(format!(".pull.{}.tmp", std::process::id()));
        let stage = dir.with_file_name(stage_name);
        let _ = std::fs::remove_dir_all(&stage);
        for sub in ["new", "old"] {
            std::fs::create_dir_all(stage.join(sub)).map_err(|e| io(&stage, e))?;
        }
        let mut order = Vec::with_capacity(entries.len());
        for (name, bytes) in entries {
            let bytes = if name == MANIFEST_NAME {
                manifest
            } else {
                bytes.as_slice()
            };
            let path = stage.join("new").join(&name);
            if let Err(e) = std::fs::write(&path, bytes) {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(io(&path, e));
            }
            if name != MANIFEST_NAME {
                order.push(name);
            }
        }
        order.sort();
        let new_names: BTreeSet<&str> = order.iter().map(String::as_str).collect();
        let orphans = old_named_files(dir)
            .into_iter()
            .filter(|n| !new_names.contains(n.as_str()) && dir.join(n).is_file())
            .collect();
        order.push(MANIFEST_NAME.to_string());
        Ok(Swap {
            dir: dir.to_path_buf(),
            stage,
            order,
            orphans,
            placed: Vec::new(),
            kept: Vec::new(),
        })
    }

    /// Move the staged files in (shards first, the manifest last, replacing
    /// it atomically), set aside the orphans, write the self-ignore, and date
    /// the manifest `started`.
    fn apply(&mut self, started: SystemTime) -> Result<(), String> {
        let io = |what: &str, p: &Path, e: std::io::Error| format!("{what} {}: {e}", p.display());
        for name in self.order.clone() {
            let live = self.dir.join(&name);
            let kept = self.stage.join("old").join(&name);
            if live.is_file() {
                if name == MANIFEST_NAME {
                    std::fs::copy(&live, &kept).map_err(|e| io("keep", &live, e))?;
                } else {
                    std::fs::rename(&live, &kept).map_err(|e| io("keep", &live, e))?;
                }
                self.kept.push(name.clone());
            }
            let staged = self.stage.join("new").join(&name);
            std::fs::rename(&staged, &live).map_err(|e| io("install", &live, e))?;
            self.placed.push(name);
        }
        for name in self.orphans.clone() {
            let live = self.dir.join(&name);
            std::fs::rename(&live, self.stage.join("old").join(&name))
                .map_err(|e| io("remove", &live, e))?;
            self.kept.push(name);
        }
        write_self_ignore(&self.dir).map_err(|e| io("write the self-ignore in", &self.dir, e))?;
        let manifest = self.dir.join(MANIFEST_NAME);
        std::fs::File::options()
            .write(true)
            .open(&manifest)
            .and_then(|f| f.set_modified(started))
            .map_err(|e| io("date", &manifest, e))
    }

    /// Remove every placed file and put every kept one back.
    fn roll_back(self) -> Result<(), String> {
        let mut failed = Vec::new();
        for name in &self.placed {
            if let Err(e) = std::fs::remove_file(self.dir.join(name))
                && e.kind() != std::io::ErrorKind::NotFound
            {
                failed.push(format!("{name}: {e}"));
            }
        }
        for name in &self.kept {
            if let Err(e) = std::fs::rename(self.stage.join("old").join(name), self.dir.join(name))
            {
                failed.push(format!("{name}: {e}"));
            }
        }
        if failed.is_empty() {
            let _ = std::fs::remove_dir_all(&self.stage);
            Ok(())
        } else {
            Err(format!(
                "kept in {}: {}",
                self.stage.display(),
                failed.join(", ")
            ))
        }
    }

    /// Drop the staging dir and what it kept.
    fn finish(self) {
        let _ = std::fs::remove_dir_all(&self.stage);
    }
}

/// The shard files the layout at `dir` names now, read from its manifest
/// whatever its schema; only names [`bad_name`] admits, so nothing else in
/// the directory is ever moved.
fn old_named_files(dir: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(dir.join(MANIFEST_NAME)) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let shards = v
        .get("shards")
        .and_then(|s| s.as_array())
        .into_iter()
        .flatten();
    let cross = v.get("cross").into_iter();
    let mut out: Vec<String> = shards
        .chain(cross)
        .filter_map(|e| e.get("path")?.as_str().map(str::to_string))
        .filter(|n| n != MANIFEST_NAME && bad_name(n).is_none())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_layout_files() {
        for ok in [
            "manifest.json",
            "cross_stack.gmap",
            "repo-1-02.gmap",
            "docs+v2.gmap",
        ] {
            assert_eq!(bad_name(ok), None, "{ok}");
        }
        let long = format!("{}.gmap", "a".repeat(LAYOUT_MAX_NAME));
        for bad in [
            "",
            ".",
            "..",
            "../evil",
            "a/b.gmap",
            "..\\x.gmap",
            "x y.gmap",
            "parse_cache.bin",
            "timeline.gmap",
            ".gitignore",
            "repo-1.gmap.tmp",
            long.as_str(),
        ] {
            assert!(bad_name(bad).is_some(), "{bad:?} admitted");
        }
    }

    #[test]
    fn old_named_files_reads_any_schema_and_skips_non_shards() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(old_named_files(tmp.path()).is_empty());
        std::fs::write(
            tmp.path().join(MANIFEST_NAME),
            r#"{"schema_version":1,"shards":[{"path":"repo-7.gmap"},{"path":"../x.gmap"},{"path":"timeline.gmap"}],"cross":{"path":"cross_stack.gmap"}}"#,
        )
        .expect("manifest");
        assert_eq!(
            old_named_files(tmp.path()),
            ["cross_stack.gmap", "repo-7.gmap"]
        );
    }
}
