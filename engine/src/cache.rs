//! WP-D incremental parse cache.
//!
//! Caches the per-file `FileParse` (main-parser output) keyed by a content hash
//! so an unchanged source file skips tree-sitter on the next build. The graph is
//! still rebuilt fully from the cached + freshly-parsed parses, so the output is
//! byte-identical to a clean build — the cache only elides the expensive *parse*
//! step, never the (cheap, global) resolve/merge step. See
//! `dev-notes/incremental_gmap_plan.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use repo_graph_code_domain::FileParse;

/// Tied to the WORKSPACE release version (`[workspace.package]` in the root
/// Cargo.toml — the same single line that versions the wheel): a different glia
/// release may parse differently, so a cache written by another version is
/// discarded (one-time full rebuild after upgrade — also the fix for "stale
/// cache after upgrade", backlog #10). The engine crate inherits the workspace
/// version precisely so this stamp can never lag a release (audit 2026-06-10
/// #4: a py-only bump left this frozen at 0.4.13).
const CACHE_VERSION: &str = env!("CARGO_PKG_VERSION");
const CACHE_FILE: &str = "parse_cache.bin";

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
#[derive(Default, Clone, Copy, Debug)]
pub struct CacheStats {
    pub reused: usize,
    pub reparsed: usize,
    pub evicted: usize,
}

/// Per-repo cache of main-parser `FileParse`s. Hold one in memory across edits
/// (neuropil) or persist it next to the `.gmap` (CLI / pyo3).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ParseCache {
    version: String,
    /// Repo identity the cached parses were built under (`file://<repo_path>`,
    /// the exact string fed to `RepoId::from_canonical`). Every cached
    /// `FileParse` has that RepoId baked into its NodeIds, but the per-file
    /// content hash can't see it — so a path-spelling change (`.` vs absolute)
    /// or a moved repo must discard the cache, or reused nodes silently carry
    /// the old identity (audit 2026-06-10 #2).
    repo_canonical: String,
    /// `go.mod` module path the cached parses were built under. It changes how
    /// every `.go` file parses (internal-vs-library imports, WP-G) without
    /// changing any `.go` content hash (audit 2026-06-10 #3).
    go_prefix: String,
    entries: HashMap<String, CacheEntry>,
    #[serde(skip)]
    pub stats: CacheStats,
}

impl Default for ParseCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION.to_string(),
            repo_canonical: String::new(),
            go_prefix: String::new(),
            entries: HashMap::new(),
            stats: CacheStats::default(),
        }
    }
}

impl ParseCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Discard every entry if the build context differs from the one the cache
    /// was written under, then adopt the new context. Per-file content hashes
    /// can't see either value, so a mismatch means every entry is suspect.
    /// Called at the top of each build — covers the disk sidecar AND a
    /// long-lived in-memory cache (neuropil) being pointed at a different repo.
    pub fn validate_context(&mut self, repo_canonical: &str, go_prefix: &str) {
        if self.repo_canonical != repo_canonical || self.go_prefix != go_prefix {
            if !self.entries.is_empty() {
                eprintln!(
                    "[incremental] build context changed (repo path or go.mod module), discarding {} cached parses",
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

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load `<repo>/.ai/repo-graph/parse_cache.bin`. Returns an empty cache if
    /// missing, unreadable, corrupt, or written by a different engine version.
    pub fn load(repo_path: &str) -> ParseCache {
        let path = gmap_dir(repo_path).join(CACHE_FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            return ParseCache::new();
        };
        match bincode::deserialize::<ParseCache>(&bytes) {
            Ok(c) if c.version == CACHE_VERSION => c,
            _ => ParseCache::new(),
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

    /// Persist atomically next to the `.gmap`. Best-effort: the cache is an
    /// optimization, never load-bearing.
    pub fn save(&self, repo_path: &str) -> std::io::Result<()> {
        let dir = gmap_dir(repo_path);
        std::fs::create_dir_all(&dir)?;
        let bytes = bincode::serialize(self).map_err(std::io::Error::other)?;
        let tmp = dir.join(format!("{CACHE_FILE}.tmp"));
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, dir.join(CACHE_FILE))?;
        Ok(())
    }
}
