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
// `parses_by_lang` (`build::build_graphs_for_repo`), never from this map, and
// `engine/tests/byte_identical.rs` already held with the HashMap. What it buys is
// "same input, same bytes", which makes the sidecar diffable and
// content-addressable — what a future Engram `--since` diff would stand on.
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use repo_graph_code_domain::FileParse;

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
pub(crate) const CACHE_VERSION: &str = repo_graph_stamp::BUILD_STAMP;
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
/// writer's live staging file. The leftovers are small and live in a gitignored
/// directory (`.gitignore` `**/.ai/repo-graph/`).
fn tmp_name() -> String {
    format!("{CACHE_FILE}.{}.tmp", std::process::id())
}

/// Conventional cache location, mirroring `repo_graph_store::default_gmap_dir`
/// (`<repo>/.ai/repo-graph`). The engine is store-independent, so the literal is
/// replicated here rather than depending on the store crate.
fn gmap_dir(repo_path: &str) -> PathBuf {
    Path::new(repo_path).join(".ai").join("repo-graph")
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
    parse: FileParse,
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
pub struct CacheStats {
    pub reused: usize,
    pub reparsed: usize,
    pub evicted: usize,
}

/// One cached file as [`ParseCache::iter`] yields it, in path order.
#[derive(Debug, Clone, Copy)]
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
    /// (`repo_graph_code_domain::walk_gating::repo_identity`, LB.1). Every
    /// cached `FileParse` has that RepoId baked into its NodeIds, but the
    /// per-file content hash can't see it — so a cache pointed at another
    /// identity must discard, or reused nodes silently carry the old one
    /// (audit 2026-06-10 #2). The key is path-independent, so a re-spelled
    /// path (`.` vs absolute), a moved checkout or a second clone of one remote
    /// keeps its cache. (Until LB.1 this held `file://<repo_path>`, and any
    /// respelling discarded a valid sidecar.)
    repo_canonical: String,
    /// `go.mod` module path the cached parses were built under. It changes how
    /// every `.go` file parses (internal-vs-library imports, WP-G) without
    /// changing any `.go` content hash (audit 2026-06-10 #3).
    go_prefix: String,
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
            go_prefix: String::new(),
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
    /// module. Per-file content hashes can't see either value, so a mismatch
    /// means every entry is suspect. Called at the top of each build — covers
    /// the disk sidecar AND a long-lived in-memory cache (neuropil) being
    /// pointed at a different repo.
    pub fn validate_context(&mut self, repo_canonical: &str, go_prefix: &str) {
        if self.repo_canonical != repo_canonical || self.go_prefix != go_prefix {
            if !self.entries.is_empty() {
                eprintln!(
                    "[incremental] build context changed (repo identity or go.mod module), discarding {} cached parses",
                    self.entries.len()
                );
            }
            self.entries.clear();
            repo_canonical.clone_into(&mut self.repo_canonical);
            go_prefix.clone_into(&mut self.go_prefix);
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

    /// Load `<repo>/.ai/repo-graph/parse_cache.bin`. Returns an empty cache if
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
    pub fn save(&self, repo_path: &str) -> std::io::Result<()> {
        let dir = gmap_dir(repo_path);
        std::fs::create_dir_all(&dir)?;
        let bytes = bincode::serialize(self).map_err(std::io::Error::other)?;
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
}
