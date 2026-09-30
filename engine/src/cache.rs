//! WP-D incremental parse cache.
//!
//! Caches the per-file `FileParse` (main-parser output) keyed by a content hash
//! so an unchanged source file skips tree-sitter on the next build. The graph is
//! still rebuilt fully from the cached + freshly-parsed parses, so the output is
//! byte-identical to a clean build — the cache only elides the expensive *parse*
//! step, never the (cheap, global) resolve/merge step. See
//! `dev-notes/incremental_gmap_plan.md`.

// BTreeMap, not HashMap: `bincode::serialize` walks the map in iteration
// order, and a HashMap walks a per-instance RandomState, so two runs over
// identical inputs wrote different sidecar bytes. HashSet stays — `retain_paths`
// only needs membership, and it is never serialized.
//
// This is sidecar HYGIENE, not a `.gmap` determinism fix: graph build order comes
// from the name-sorted walk (`walk::walk_source_files`) and the sorted
// `parses_by_lang` (`build::assemble::build_graphs_for_repo`), never from this map, and
// `engine/tests/byte_identical.rs` already held with the HashMap. What it buys is
// "same input, same bytes", which makes the sidecar diffable and
// content-addressable — what a future Engram `--since` diff would stand on.
// The parses inside the entries need the same care: their hash containers are
// written in key order by `canonical_parse` (LC.11), without which no two
// builds wrote equal bytes and `ParseCache::save` rewrote the sidecar forever.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use glia_code_domain::{CodeNav, FileParse};
use glia_core::NodeId;

/// Build identity a cache must match to be reused: `<release>+p<parser stamp>`
/// (see the `stamp` crate).
///
/// History. This was the WORKSPACE release version alone (`[workspace.package]`
/// in the root Cargo.toml — the same line that versions the wheel), so a cache
/// written by another glia release was discarded (the fix for "stale cache after
/// upgrade", backlog #10; the engine inherits the workspace version precisely so
/// it can never lag a release — audit 2026-06-10 #4, where a py-only bump left
/// this frozen at 0.4.13). The release turned out to be the wrong GRANULARITY:
/// a parser or extractor fix merged without a version bump left every
/// incremental consumer serving the pre-fix `FileParse`, while `bench/substrate-gap`
/// (which builds cold) reported the cell fixed. `BUILD_STAMP` folds in a content
/// hash of every graph-shaping source, so a parser change invalidates on its own.
pub(crate) const CACHE_VERSION: &str = glia_stamp::BUILD_STAMP;
const CACHE_FILE: &str = "parse_cache.bin";

/// Staging file name `save` writes through before the atomic rename.
///
/// Per-pid. A fixed `parse_cache.bin.tmp` is shared by every writer, and two
/// builds of the same repo at once — the repo-graph MCP server plus a
/// `glia build`, a routine combination — interleave their writes on it, so the
/// rename publishes whichever partial byte sequence won. It is self-healing (a
/// truncated sidecar fails to deserialize and [`ParseCache::load`] returns an
/// empty cache) but it silently throws the cache away, and it reads as a stamp
/// problem. The rename was already atomic within the directory; the fix is only
/// that two writers no longer share the staging path.
///
/// No stale-tmp sweep: a crashed build leaves a `parse_cache.bin.<pid>.tmp`
/// behind, and sweeping `*.tmp` older than the sidecar races a concurrent
/// writer's live staging file. The leftovers are small and live in the layout
/// directory, which ignores itself (`<repo>/.glia/graph/.gitignore` is `*`).
fn tmp_name() -> String {
    format!("{CACHE_FILE}.{}.tmp", std::process::id())
}

/// Does the file at `path` hold exactly `bytes`? Length first, from the
/// opened handle's metadata, so a cache that changed size costs no read; then
/// the content in fixed chunks, so a 20 MB sidecar is never held twice and a
/// difference stops the read at its chunk. Best-effort like [`ParseCache::save`]:
/// any failure (missing file, unreadable, shorter than stat said) is "differs",
/// which means write.
fn on_disk_equals(path: &Path, bytes: &[u8]) -> bool {
    use std::io::Read;
    const CHUNK: usize = 64 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    if !f.metadata().is_ok_and(|m| m.len() == bytes.len() as u64) {
        return false;
    }
    let mut buf = vec![0u8; CHUNK.min(bytes.len())];
    for want in bytes.chunks(CHUNK) {
        let got = &mut buf[..want.len()];
        if f.read_exact(got).is_err() || got != want {
            return false;
        }
    }
    // The file must end here too: it may have grown since the stat.
    f.read(&mut [0u8; 1]).is_ok_and(|n| n == 0)
}

/// Conventional cache location: the repo's layout directory
/// (`glia_store::default_gmap_dir`, `<repo>/.glia/graph`), so the parse
/// cache sits beside the gmap it is invalidated with.
fn gmap_dir(repo_path: &str) -> PathBuf {
    glia_store::default_gmap_dir(Path::new(repo_path))
}

/// xxhash64 of a source string — the same primitive the store uses for shard
/// content hashes.
pub fn content_hash(source: &str) -> u64 {
    use core::hash::Hasher;
    use twox_hash::XxHash64;
    let mut h = XxHash64::with_seed(0);
    h.write(source.as_bytes());
    h.finish()
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct CacheEntry {
    content_hash: u64,
    lang: String,
    #[serde(serialize_with = "canonical_parse")]
    parse: FileParse,
}

/// `FileParse` serialized with every hash container in key order (LC.11).
///
/// `FileParse::properties` and `CodeNav`'s nine maps are std hash
/// containers, which iterate in a per-instance RandomState order, so the
/// derived serializer wrote the same parse as different bytes on every build
/// and [`ParseCache::save`] could never find the sidecar unchanged. This is
/// the derived wire shape (same fields, same order, a map still a length and
/// its entries) with only the entry order fixed, so old sidecars load and
/// `FileParse`'s own `Deserialize` reads this back; the cache format and
/// [`CACHE_VERSION`] are unchanged. The destructuring below names every
/// field, so a field added to `FileParse` or `CodeNav` fails to compile here
/// rather than drop out of the cache. Vec fields keep their parser order (a
/// `nav_facts` scope's fact list included).
/// Removal path: once `CodeNav` and `properties` are ordered containers
/// (BTreeMap / BTreeSet in code-domain), the derived serializer is canonical
/// and this function and its helpers go.
fn canonical_parse<S: serde::Serializer>(p: &FileParse, s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeStruct;
    let FileParse { nodes, edges, imports, calls, refs, nav, properties } = p;
    let mut props: Vec<&NodeId> = properties.iter().collect();
    props.sort_unstable_by_key(|id| id.0);
    let mut st = s.serialize_struct("FileParse", 7)?;
    st.serialize_field("nodes", nodes)?;
    st.serialize_field("edges", edges)?;
    st.serialize_field("imports", imports)?;
    st.serialize_field("calls", calls)?;
    st.serialize_field("refs", refs)?;
    st.serialize_field("nav", &CanonicalNav(nav))?;
    st.serialize_field("properties", &props)?;
    st.end()
}

/// `CodeNav` in the derived wire shape, each map in key order.
struct CanonicalNav<'a>(&'a CodeNav);

impl serde::Serialize for CanonicalNav<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let CodeNav {
            name_by_id,
            qname_by_id,
            kind_by_id,
            parent_of,
            children_of,
            field_types,
            local_types,
            nav_facts,
            return_types,
        } = self.0;
        let mut st = s.serialize_struct("CodeNav", 9)?;
        st.serialize_field("name_by_id", &by_id(name_by_id, |v| v))?;
        st.serialize_field("qname_by_id", &by_id(qname_by_id, |v| v))?;
        st.serialize_field("kind_by_id", &by_id(kind_by_id, |v| v))?;
        st.serialize_field("parent_of", &by_id(parent_of, |v| v))?;
        st.serialize_field("children_of", &by_id(children_of, |v| v))?;
        st.serialize_field("field_types", &by_id(field_types, by_name))?;
        st.serialize_field("local_types", &by_id(local_types, by_name))?;
        st.serialize_field("nav_facts", &by_id(nav_facts, |v| v))?;
        st.serialize_field("return_types", &by_id(return_types, |v| v))?;
        st.end()
    }
}

/// Entries serialized as a map, in the order held.
struct OrderedMap<K, V>(Vec<(K, V)>);

impl<K: serde::Serialize, V: serde::Serialize> serde::Serialize for OrderedMap<K, V> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(k, v)| (k, v)))
    }
}

/// A `NodeId`-keyed map in id order, each value through `f`.
fn by_id<'a, V, W>(m: &'a HashMap<NodeId, V>, f: impl Fn(&'a V) -> W) -> OrderedMap<&'a NodeId, W> {
    let mut e: Vec<(&NodeId, W)> = m.iter().map(|(k, v)| (k, f(v))).collect();
    e.sort_unstable_by_key(|(k, _)| k.0);
    OrderedMap(e)
}

/// A name-keyed map in name order.
fn by_name(m: &HashMap<String, String>) -> OrderedMap<&String, &String> {
    let mut e: Vec<(&String, &String)> = m.iter().collect();
    e.sort_unstable();
    OrderedMap(e)
}

/// Counters for the build's `[incremental]` marker. Not persisted.
///
/// The lengths of the build's [`CacheDiff`] (LA.12), not a separately kept
/// tally, so the counts and the file lists can never disagree. The diff
/// describes the build's INPUTS: a file whose reparse fails still counts as
/// reparsed (its failure is in the build's `parse_errors`), and a cached file
/// that fails to reparse counts as reparsed, not evicted, although
/// `retain_paths` still drops it from the sidecar. That is the only way these
/// numbers differ from the pre-LA.12 loop counters.
#[derive(Default, Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CacheStats {
    pub reused: usize,
    pub reparsed: usize,
    pub evicted: usize,
}

/// One cached file as [`ParseCache::iter`] yields it, in path order.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct CachedFile<'a> {
    /// Repo-relative path, the key the build walks under.
    pub path: &'a str,
    /// [`content_hash`] of the source this parse was built from.
    pub content_hash: u64,
    /// Language tag the file was parsed as (`detect_language`).
    pub lang: &'a str,
    /// The cached main-parser output.
    pub parse: &'a FileParse,
}

/// File-level delta between what a [`ParseCache`] holds and one build's
/// inputs ([`ParseCache::diff`]). Each list is sorted by path, and a path is
/// in exactly one of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CacheDiff {
    /// In the input with the cached content hash: served from the cache.
    pub reused: Vec<String>,
    /// In the input and either absent from the cache (added) or cached under
    /// another content hash (modified).
    pub reparsed: Vec<String>,
    /// Cached but absent from the input (deleted, or no longer a main-parser
    /// file).
    pub evicted: Vec<String>,
}

/// Per-repo cache of main-parser `FileParse`s. Hold one in memory across edits
/// (neuropil) or persist it next to the `.gmap` (CLI / pyo3).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ParseCache {
    /// Build identity this cache was written under (`CACHE_VERSION`). First
    /// serialized field, so it is the leading bincode string on disk.
    stamp: String,
    /// Repo identity key the cached parses were built under — the exact string
    /// fed to `RepoId::from_canonical`: `git:<remote>[/<rel>]`,
    /// `gitdir:<name>[/<rel>]` or `dir:<basename>`
    /// (`glia_code_domain::walk_gating::repo_identity`, LB.1). Every
    /// cached `FileParse` has that RepoId baked into its NodeIds, but the
    /// per-file content hash can't see it — so a cache pointed at another
    /// identity must discard, or reused nodes silently carry the old one
    /// (audit 2026-06-10 #2). The key is path-independent, so a re-spelled
    /// path (`.` vs absolute), a moved checkout or a second clone of one remote
    /// keeps its cache. (Until LB.1 this held `file://<repo_path>`, and any
    /// respelling discarded a valid sidecar.)
    repo_canonical: String,
    /// The go.mod module set the cached parses were built under: the
    /// module-set key from `GoModules::context_key` (`<root dir>=<module>;..`,
    /// LA.13). It changes how every `.go` file parses (internal-vs-library
    /// imports, WP-G) without changing any `.go` content hash (audit
    /// 2026-06-10 #3), so any go.mod added, removed or edited discards the
    /// cache. (Until LA.13 this held the root go.mod's bare module path; bincode
    /// does not encode field names, so the sidecar layout is unchanged and an
    /// old sidecar simply discards once.)
    go_modules: String,
    entries: BTreeMap<String, CacheEntry>,
    #[serde(skip)]
    pub stats: CacheStats,
    /// The file-level diff of the last build that used this cache
    /// ([`ParseCache::last_diff`]). Not persisted: `serde(skip)` keeps the
    /// sidecar layout unchanged, and a loaded cache starts at `None`.
    #[serde(skip)]
    last_diff: Option<CacheDiff>,
}

impl Default for ParseCache {
    fn default() -> Self {
        Self {
            stamp: CACHE_VERSION.to_string(),
            repo_canonical: String::new(),
            go_modules: String::new(),
            entries: BTreeMap::new(),
            stats: CacheStats::default(),
            last_diff: None,
        }
    }
}

impl ParseCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Discard every entry if the build context differs from the one the cache
    /// was written under, then adopt the new context. The context is the repo
    /// identity KEY (not its path — see `repo_canonical`) plus the go.mod
    /// module set (`go_context`, `GoModules::context_key`; `""` for a repo
    /// with no go.mod). Per-file content hashes can't see either value, so a
    /// mismatch means every entry is suspect. Called at the top of each build —
    /// covers the disk sidecar AND a long-lived in-memory cache (neuropil)
    /// being pointed at a different repo.
    pub fn validate_context(&mut self, repo_canonical: &str, go_context: &str) {
        if self.repo_canonical != repo_canonical || self.go_modules != go_context {
            if !self.entries.is_empty() {
                eprintln!(
                    "[incremental] build context changed (repo identity or go.mod module set), discarding {} cached parses",
                    self.entries.len()
                );
            }
            self.entries.clear();
            repo_canonical.clone_into(&mut self.repo_canonical);
            go_context.clone_into(&mut self.go_modules);
        }
    }

    /// Reuse an unchanged parse for `path` (content hash + language must match),
    /// cloned so the caller can hand it to the builder. `None` on miss.
    pub fn get(&self, path: &str, hash: u64, lang: &str) -> Option<FileParse> {
        let e = self.entries.get(path)?;
        (e.content_hash == hash && e.lang == lang).then(|| e.parse.clone())
    }

    /// Record a freshly-parsed file.
    pub fn put(&mut self, path: String, hash: u64, lang: &str, parse: FileParse) {
        self.entries.insert(
            path,
            CacheEntry { content_hash: hash, lang: lang.to_string(), parse },
        );
    }

    /// Drop entries for files no longer present this build (deletions / files
    /// that stopped being parseable). Records the count in `stats.evicted`.
    pub fn retain_paths(&mut self, live: &HashSet<String>) {
        let before = self.entries.len();
        self.entries.retain(|p, _| live.contains(p));
        self.stats.evicted = before.saturating_sub(self.entries.len());
    }

    /// Every cached file, in path order (the sidecar's `BTreeMap` order).
    pub fn iter(&self) -> impl Iterator<Item = CachedFile<'_>> + '_ {
        self.entries.iter().map(|(path, e)| CachedFile {
            path,
            content_hash: e.content_hash,
            lang: &e.lang,
            parse: &e.parse,
        })
    }

    /// Classify one build's inputs (`(path, content_hash)` pairs) against
    /// what the cache holds. Pure: the cache is not touched. Duplicate paths
    /// collapse, the last pair winning.
    ///
    /// The language is deliberately not compared: a path's language is a
    /// function of the path, and a change to language detection changes the
    /// build stamp, which already discards every entry on load.
    pub fn diff(&self, current: &[(String, u64)]) -> CacheDiff {
        let now: BTreeMap<&str, u64> = current.iter().map(|(p, h)| (p.as_str(), *h)).collect();
        let mut d = CacheDiff::default();
        for (&path, &hash) in &now {
            match self.entries.get(path) {
                Some(e) if e.content_hash == hash => d.reused.push(path.to_string()),
                _ => d.reparsed.push(path.to_string()),
            }
        }
        d.evicted = self
            .entries
            .keys()
            .filter(|p| !now.contains_key(p.as_str()))
            .cloned()
            .collect();
        d
    }

    /// The file-level diff of the last build that used this cache; `None`
    /// until one has (a freshly loaded sidecar included).
    pub fn last_diff(&self) -> Option<&CacheDiff> {
        self.last_diff.as_ref()
    }

    /// Record a build's diff and fill `stats` from its lengths. Called by the
    /// build (`route::parse_repo_files`) after `retain_paths`.
    pub(crate) fn record_diff(&mut self, d: CacheDiff) {
        self.stats = CacheStats {
            reused: d.reused.len(),
            reparsed: d.reparsed.len(),
            evicted: d.evicted.len(),
        };
        self.last_diff = Some(d);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load `<repo>/.glia/graph/parse_cache.bin`. Returns an empty cache if
    /// missing, unreadable, corrupt, or written by a different build identity.
    pub fn load(repo_path: &str) -> ParseCache {
        let path = gmap_dir(repo_path).join(CACHE_FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            return ParseCache::new();
        };
        match bincode::deserialize::<ParseCache>(&bytes) {
            Ok(c) if c.stamp == CACHE_VERSION => c,
            // A stamp mismatch is the parse cache doing its job, and it is the
            // one discard a human needs to see (it explains a slow build), so it
            // is never silent.
            Ok(c) => {
                eprintln!(
                    "[incremental] cache stamp mismatch (disk={} build={CACHE_VERSION}) — full reparse",
                    c.stamp
                );
                ParseCache::new()
            }
            Err(_) => ParseCache::new(),
        }
    }

    /// Delete the on-disk sidecar. Called when the user explicitly asks for a
    /// non-incremental build (`--no-incremental` / `incremental=False`): a
    /// forced clean build must be a real escape hatch — without this, the NEXT
    /// default-on build would reuse whatever cache the user was escaping.
    pub fn purge(repo_path: &str) -> std::io::Result<()> {
        match std::fs::remove_file(gmap_dir(repo_path).join(CACHE_FILE)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Persist atomically next to the `.gmap`, staging through a per-process
    /// tmp file (see [`tmp_name`]). Best-effort: the cache is an optimization,
    /// never load-bearing.
    ///
    /// The directory gets its self-ignoring `.gitignore` here too (as from
    /// `persist::persist_result`), so a cache-only build (pyo3 `generate` under
    /// `GLIA_NO_PERSIST=1`) never shows up in the repo's `git status` either.
    ///
    /// Written only when it changed (LC.11), as the store already skips an
    /// unchanged shard or manifest: when the sidecar on disk holds exactly
    /// these bytes nothing is written, so a build that changed no entry keeps
    /// the file's inode and mtime. A watcher rebuilding on every inotify event
    /// otherwise rewrote the whole cache each time (~1.2 TB in a day on a
    /// 21 MB sidecar). The comparison is against the file actually on disk,
    /// not a digest kept from `load`: another writer may have replaced it
    /// since, and then ours is written, as before. The mtime staying put
    /// cannot make a layout look stale: `store::scan_for_newer` skips the
    /// layout directory by prefix.
    pub fn save(&self, repo_path: &str) -> std::io::Result<()> {
        let dir = gmap_dir(repo_path);
        std::fs::create_dir_all(&dir)?;
        crate::persist::write_self_ignore(&dir)?;
        let bytes = bincode::serialize(self).map_err(std::io::Error::other)?;
        if on_disk_equals(&dir.join(CACHE_FILE), &bytes) {
            eprintln!(
                "[incremental] unchanged {} entries - parse_cache.bin not rewritten",
                self.entries.len()
            );
            return Ok(());
        }
        let tmp = dir.join(tmp_name());
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, dir.join(CACHE_FILE))?;
        eprintln!(
            "[incremental] saved {} entries (btree) via {}",
            self.entries.len(),
            tmp.file_name().unwrap_or_default().to_string_lossy()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::{Mount, NavFact};

    /// A cache with `n` entries inserted in a fixed order, under a fixed build
    /// context. Every `FileParse` is `default()` (empty vecs/maps), so the only
    /// thing that can vary between two of these is the order `entries` walks.
    fn filled(n: usize) -> ParseCache {
        let mut c = ParseCache::new();
        c.validate_context("dir:a13", "example.com/a13");
        for i in 0..n {
            c.put(format!("src/f{i:02}.rs"), i as u64, "rust", FileParse::default());
        }
        c
    }

    fn sidecar(dir: &Path) -> PathBuf {
        gmap_dir(dir.to_string_lossy().as_ref()).join(CACHE_FILE)
    }

    /// Sidecar hygiene (NOT a `.gmap` determinism fix — graph order comes from
    /// the name-sorted walk and the sorted `parses_by_lang`, never from this
    /// map). Same input, same bytes makes the sidecar diffable and
    /// content-addressable, which a future `--since` diff can stand on.
    #[test]
    fn sidecar_bytes_are_stable_for_identical_content() {
        let a = tempfile::tempdir().expect("tempdir a");
        let b = tempfile::tempdir().expect("tempdir b");
        filled(32).save(a.path().to_string_lossy().as_ref()).expect("save a");
        filled(32).save(b.path().to_string_lossy().as_ref()).expect("save b");
        let ba = std::fs::read(sidecar(a.path())).expect("read a");
        let bb = std::fs::read(sidecar(b.path())).expect("read b");
        assert_eq!(
            ba, bb,
            "two caches with identical content serialised to different bytes — \
             `entries` is walking a per-instance hash order"
        );
    }

    fn owned(pairs: &[(&str, u64)]) -> Vec<(String, u64)> {
        pairs.iter().map(|(p, h)| ((*p).to_string(), *h)).collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn diff_classifies_reused_reparsed_evicted() {
        let mut c = ParseCache::new();
        c.put("a".into(), 1, "python", FileParse::default());
        c.put("b".into(), 2, "python", FileParse::default());
        c.put("c".into(), 3, "python", FileParse::default());

        // Unsorted input: a unchanged, b modified, d added, c gone.
        let d = c.diff(&owned(&[("d", 9), ("b", 20), ("a", 1)]));
        assert_eq!(
            d,
            CacheDiff {
                reused: strings(&["a"]),
                reparsed: strings(&["b", "d"]),
                evicted: strings(&["c"]),
            }
        );
        assert_eq!(c.len(), 3, "diff must not mutate the cache");
        assert!(c.last_diff().is_none(), "diff is pure; only record_diff stores one");

        // Duplicates collapse, the last pair winning.
        let dup = c.diff(&owned(&[("a", 7), ("b", 2), ("c", 3), ("a", 1)]));
        assert_eq!(dup.reused, strings(&["a", "b", "c"]));
        assert!(dup.reparsed.is_empty() && dup.evicted.is_empty(), "{dup:?}");

        c.record_diff(d.clone());
        assert_eq!(c.last_diff(), Some(&d));
        assert_eq!((c.stats.reused, c.stats.reparsed, c.stats.evicted), (1, 2, 1));
    }

    #[test]
    fn iter_is_path_ordered() {
        let mut c = ParseCache::new();
        for (p, h) in [("src/z.rs", 3), ("a.go", 1), ("src/m.py", 2)] {
            let lang = p.rsplit('.').next().unwrap_or_default();
            c.put(p.to_string(), h, lang, FileParse::default());
        }
        let rows: Vec<(&str, u64, &str)> = c.iter().map(|f| (f.path, f.content_hash, f.lang)).collect();
        assert_eq!(rows, [("a.go", 1, "go"), ("src/m.py", 2, "py"), ("src/z.rs", 3, "rs")]);
    }

    /// Two builds of the same repo at once (the repo-graph MCP server plus a
    /// `glia build`) must not share a staging path, or the rename publishes
    /// whichever partial byte sequence won.
    #[test]
    fn tmp_file_is_per_process() {
        let d = tempfile::tempdir().expect("tempdir");
        let repo = d.path().to_string_lossy().into_owned();
        filled(3).save(&repo).expect("save");

        let dir = gmap_dir(&repo);
        assert!(dir.join(CACHE_FILE).is_file(), "sidecar was not published");
        assert!(
            !dir.join(format!("{CACHE_FILE}.tmp")).exists(),
            "a fixed-name staging file was left behind"
        );

        let name = tmp_name();
        assert_ne!(
            name,
            format!("{CACHE_FILE}.tmp"),
            "staging path is the fixed name shared by every concurrent writer"
        );
        assert!(
            name.contains(&std::process::id().to_string()),
            "staging name {name} does not carry this pid"
        );
    }

    /// A parse with every field filled: `n` entries in each hash container
    /// (inserted in `order`), one row in each Vec, so a field serialized out
    /// of place or dropped changes the bytes.
    fn rich_parse(n: u64, order: &[u64]) -> FileParse {
        use glia_code_domain::{
            CallQualifier, CallSite, ImportStmt, ImportTarget, UnresolvedRef, edge_category, node_kind,
        };
        use glia_core::{Cell, CellPayload, CellTypeId, Confidence, Edge, Node, NodeId, RepoId};
        let mut p = FileParse::default();
        let cell = Cell { kind: CellTypeId(1), payload: CellPayload::Text("code".into()) };
        let node = Node { id: NodeId(1), repo: RepoId(7), confidence: Confidence::Strong, cells: vec![cell] };
        p.nodes.push(node);
        p.edges.push(Edge {
            from: NodeId(1),
            to: NodeId(2),
            category: edge_category::CALLS,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        p.imports.push(ImportStmt {
            from_module: "m".into(),
            target: ImportTarget::Module { path: "p".into(), alias: None },
            line: 3,
        });
        p.calls.push(CallSite { from: NodeId(1), qualifier: CallQualifier::Bare("f".into()), line: 4 });
        p.refs.push(UnresolvedRef {
            from: NodeId(1),
            from_module: NodeId(9),
            qualifier: CallQualifier::SelfMethod("g".into()),
            category: edge_category::CALLS,
            line: 5,
        });
        for &i in order.iter().filter(|&&i| i < n) {
            let id = NodeId(100 + i);
            p.nav.name_by_id.insert(id, format!("n{i}"));
            p.nav.qname_by_id.insert(id, format!("m::q{i}"));
            p.nav.kind_by_id.insert(id, if i % 2 == 0 { node_kind::FUNCTION } else { node_kind::METHOD });
            p.nav.parent_of.insert(id, NodeId(1000 + i));
            p.nav.children_of.insert(id, vec![NodeId(2000 + i), NodeId(3000 + i)]);
            for &j in order.iter().filter(|&&j| j < n) {
                p.nav.field_types.entry(id).or_default().insert(format!("f{j}"), format!("T{j}"));
                p.nav.local_types.entry(id).or_default().insert(format!("l{j}"), format!("U{j}"));
            }
            // CB.6: one fact per scope; its Vec keeps parser order.
            let fact = NavFact::DeclaresFn { ns: format!("ns{i}"), name: format!("proto{i}") };
            p.nav.record_fact(id, fact);
            // CA.2a: one result type per callable.
            p.nav.record_return_type(id, &format!("pkg.R{i}"));
            p.properties.insert(id);
        }
        p
    }

    /// Every [`NavFact`] variant, each [`Mount`] shape among them, in the
    /// order a parser would record them on one scope.
    fn every_nav_fact() -> Vec<NavFact> {
        vec![
            NavFact::DeclaresFn {
                ns: String::new(),
                name: "codec_encode".into(),
            },
            NavFact::UsingNamespace {
                within: String::new(),
                ns: "shop".into(),
            },
            NavFact::UsingName {
                within: "app".into(),
                ns: "shop".into(),
                name: "total".into(),
            },
            NavFact::MountArg {
                line: 12,
                callee: "RegisterUsers".into(),
                arg: 0,
                mount: Mount::Const("/api/v2".into()),
            },
            NavFact::MountArg {
                line: 13,
                callee: "RegisterAdmin".into(),
                arg: 2,
                mount: Mount::Param {
                    fn_qname: "api::server::NewServer".into(),
                    index: 1,
                    suffix: "/admin".into(),
                },
            },
            NavFact::FieldMount {
                owner: "api::Server".into(),
                field: "admin".into(),
                mount: Mount::Field {
                    owner: "api::Server".into(),
                    field: "root".into(),
                    suffix: "/x".into(),
                },
            },
            NavFact::InternalLinkage {
                name: "clamp".into(),
            },
            NavFact::ClientHost {
                via: "graphql".into(),
                host: "api.example.com".into(),
                line: 4,
            },
        ]
    }

    /// CB.6: `CodeNav::nav_facts` (build-time, never in the store) survives a
    /// save and a load of the parse cache with every variant, per scope and
    /// in parser order, and the sidecar bytes do not depend on the order the
    /// scopes went into the map.
    #[test]
    fn nav_facts_survive_the_parse_cache() {
        let facts = every_nav_fact();
        let fill = |order: &[u64]| {
            let mut p = FileParse::default();
            for &i in order {
                for f in &facts[..=(i as usize % facts.len())] {
                    p.nav.record_fact(NodeId(500 + i), f.clone());
                }
            }
            p
        };
        let fwd: Vec<u64> = (0..16).collect();
        let rev: Vec<u64> = (0..16).rev().collect();
        let parse = fill(&fwd);
        assert_eq!(
            parse.nav.nav_facts[&NodeId(507)],
            facts,
            "one scope holds every variant"
        );

        let d = tempfile::tempdir().expect("tempdir");
        let repo = d.path().to_string_lossy().into_owned();
        let mut c = ParseCache::new();
        c.put("src/codec.cpp".into(), 7, "cpp", parse.clone());
        c.save(&repo).expect("save");
        let loaded = ParseCache::load(&repo);
        let got = loaded
            .get("src/codec.cpp", 7, "cpp")
            .expect("the cached parse loads");
        assert_eq!(got.nav.nav_facts, parse.nav.nav_facts);

        let mut other = ParseCache::new();
        other.put("src/codec.cpp".into(), 7, "cpp", fill(&rev));
        assert!(
            bincode::serialize(&other).expect("ser rev")
                == std::fs::read(sidecar(d.path())).expect("read sidecar"),
            "nav_facts filled in another scope order serialized to other bytes"
        );
    }

    /// LC.11: equal parses serialize to equal bytes whatever order their hash
    /// containers were filled in (`CodeNav`'s maps and `properties` iterate in
    /// a per-instance RandomState order), or no rebuild could ever match the
    /// sidecar on disk.
    #[test]
    fn equal_parses_serialize_to_equal_bytes() {
        let fwd: Vec<u64> = (0..24).collect();
        let rev: Vec<u64> = (0..24).rev().collect();
        let mut a = ParseCache::new();
        let mut b = ParseCache::new();
        a.put("src/x.rs".into(), 1, "rust", rich_parse(24, &fwd));
        b.put("src/x.rs".into(), 1, "rust", rich_parse(24, &rev));
        let ba = bincode::serialize(&a).expect("serialize a");
        let bb = bincode::serialize(&b).expect("serialize b");
        assert!(ba == bb, "equal parses serialized to different bytes ({} vs {} bytes)", ba.len(), bb.len());
    }

    /// The canonical order is a reordering, never a new wire shape: with at
    /// most one entry per hash container (one possible order) it is byte for
    /// byte `FileParse`'s derived serialization, so a field written out of
    /// place, dropped or wrapped fails here. With many entries the derived
    /// deserializer reads it back to equal containers, and an old sidecar
    /// (hash order) still loads.
    #[test]
    fn canonical_parse_is_the_derived_wire_shape() {
        let one = rich_parse(1, &[0]);
        let mut c = ParseCache::new();
        c.put("a".into(), 1, "rust", one.clone());
        let mut derived = bincode::serialize(&c.stamp).expect("stamp");
        derived.extend(bincode::serialize(&c.repo_canonical).expect("repo"));
        derived.extend(bincode::serialize(&c.go_modules).expect("go"));
        derived.extend(bincode::serialize(&1u64).expect("n entries"));
        derived.extend(bincode::serialize("a").expect("key"));
        derived.extend(bincode::serialize(&1u64).expect("hash"));
        derived.extend(bincode::serialize("rust").expect("lang"));
        derived.extend(bincode::serialize(&one).expect("derived parse"));
        let canonical = bincode::serialize(&c).expect("canonical");
        assert!(canonical == derived, "canonical != derived wire shape");

        let order: Vec<u64> = (0..24).rev().collect();
        let many = rich_parse(24, &order);
        let mut c = ParseCache::new();
        c.put("a".into(), 1, "rust", many.clone());
        let back: ParseCache = bincode::deserialize(&bincode::serialize(&c).expect("ser")).expect("de");
        let got = &back.entries["a"].parse;
        assert_eq!(got.nav.name_by_id, many.nav.name_by_id);
        assert_eq!(got.nav.qname_by_id, many.nav.qname_by_id);
        assert_eq!(got.nav.kind_by_id, many.nav.kind_by_id);
        assert_eq!(got.nav.parent_of, many.nav.parent_of);
        assert_eq!(got.nav.children_of, many.nav.children_of);
        assert_eq!(got.nav.field_types, many.nav.field_types);
        assert_eq!(got.nav.local_types, many.nav.local_types);
        assert_eq!(got.nav.nav_facts, many.nav.nav_facts);
        assert_eq!(got.nav.return_types, many.nav.return_types);
        assert_eq!(got.properties, many.properties);
        assert_eq!((&got.nodes, &got.edges, &got.imports), (&many.nodes, &many.edges, &many.imports));
        assert_eq!((&got.calls, &got.refs), (&many.calls, &many.refs));

        // A sidecar an older glia wrote (derived serializer, hash order) loads:
        // the same cache header, then the derived bytes of `many`.
        let header_len = derived.len() - bincode::serialize(&one).expect("one").len();
        let mut old = derived[..header_len].to_vec();
        old.extend(bincode::serialize(&many).expect("derived many"));
        let loaded: ParseCache = bincode::deserialize(&old).expect("old sidecar loads");
        assert_eq!(loaded.entries["a"].parse.nav.field_types, many.nav.field_types);
    }

    /// LC.11: a build that changed no entry writes nothing. The tmp + rename
    /// always lands a new inode, so an unchanged inode proves no write.
    #[cfg(unix)]
    #[test]
    fn save_twice_unchanged_does_not_rewrite() {
        use std::os::unix::fs::MetadataExt;
        let d = tempfile::tempdir().expect("tempdir");
        let repo = d.path().to_string_lossy().into_owned();
        let c = filled(8);
        c.save(&repo).expect("first save");
        let path = sidecar(d.path());
        let before = std::fs::metadata(&path).expect("stat after first save");

        c.save(&repo).expect("second save");
        let after = std::fs::metadata(&path).expect("stat after second save");
        assert_eq!(before.ino(), after.ino(), "an unchanged cache was rewritten (new inode)");
        assert_eq!(
            (before.mtime(), before.mtime_nsec()),
            (after.mtime(), after.mtime_nsec()),
            "an unchanged cache was rewritten (mtime moved)"
        );
        assert!(!gmap_dir(&repo).join(tmp_name()).exists(), "a staging file was left behind");
    }

    /// A changed entry of the same serialized length: the length gate cannot
    /// decide it, the byte comparison must.
    #[test]
    fn save_after_change_rewrites() {
        let d = tempfile::tempdir().expect("tempdir");
        let repo = d.path().to_string_lossy().into_owned();
        let mut c = filled(8);
        c.save(&repo).expect("first save");
        let old = std::fs::read(sidecar(d.path())).expect("read first");

        c.put("src/f03.rs".to_string(), 999, "rust", FileParse::default());
        c.save(&repo).expect("second save");
        let new = std::fs::read(sidecar(d.path())).expect("read second");
        assert_eq!(new.len(), old.len(), "fixture must keep the length so the bytes decide");
        assert_ne!(new, old, "the changed cache was not written");
        assert_eq!(new, bincode::serialize(&c).expect("serialize"), "disk != the new serialization");
    }

    /// Another writer replaced the sidecar with other bytes of the same
    /// length: the comparison is against the disk, so ours is written back.
    #[test]
    fn save_when_on_disk_differs_rewrites() {
        let d = tempfile::tempdir().expect("tempdir");
        let repo = d.path().to_string_lossy().into_owned();
        let c = filled(8);
        c.save(&repo).expect("first save");
        let path = sidecar(d.path());
        let mut tampered = std::fs::read(&path).expect("read");
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        std::fs::write(&path, &tampered).expect("tamper");

        c.save(&repo).expect("second save");
        let now = std::fs::read(&path).expect("read back");
        assert_eq!(now, bincode::serialize(&c).expect("serialize"), "the differing sidecar was kept");
    }
}
