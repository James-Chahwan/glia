//! A checkout's side of a shared-cache pull (CE.2b): which parses it lacks
//! ([`wanted`]) and a verified import of payloads fetched for them
//! ([`import_entries`]) into its parse-cache sidecar
//! (`<repo>/.glia/graph/parse_cache.bin`).
//!
//! Payloads come from another machine, so an import trusts nothing it can
//! check. Every offered payload is decoded under bounds
//! (`CacheEntry::from_payload`: no length prefix reads past the payload) and
//! must name a file this checkout wants, with that file's content hash and
//! language and the MODULE form this checkout's plan gives it; anything else
//! is rejected. What those checks cannot tell apart is a correct parse of the
//! right bytes and a crafted one. For that an import re-parses a uniformly
//! random sample of the accepted entries with the build's own parse code
//! (`route::parse_for_cache`) and compares payload bytes: one mismatch aborts
//! and nothing is written. The sample size is the caller's ([`Verify`]): the
//! CLI (CE.2c) passes 0 for a keyed store, whose objects its MAC already
//! authenticated, and 32 for an explicitly trusted unsigned one. A sample
//! catches bulk poisoning with high probability and one targeted entry only
//! with probability sample / rows; the MAC is the real defence.
//!
//! Nothing here reaches a graph: the next build reads the sidecar through its
//! own checks (stamp, repo identity, go.mod set, content hash, MODULE form).
//! Nothing here does network I/O either; fetching is the CLI's.

use std::collections::BTreeMap;
use std::path::Path;

use glia_code_domain::walk_gating::repo_identity;
use glia_core::RepoId;

use super::export::CacheRow;
use super::key::{CacheKey, file_key};
use crate::cache::{self, CACHE_VERSION, CacheEntry, ParseCache};
use crate::extract::GoModules;
use crate::route::{ModuleQnames, cache_plan, cached_under_other_form, parse_for_cache};

/// Most `[cache] rejected` lines one import prints; the rest are counted.
const MAX_REJECTION_LINES: usize = 20;

/// How many accepted entries [`import_entries`] re-parses and compares with
/// the offered payload before it writes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub enum Verify {
    /// At most this many, chosen uniformly at random (every accepted entry
    /// when fewer were accepted). `Count(0)` checks none.
    Count(usize),
    /// Every accepted entry.
    All,
}

impl Default for Verify {
    fn default() -> Self {
        Verify::Count(0)
    }
}

/// How [`import_entries`] runs. Outside this crate: `ImportOptions::default()`
/// then [`ImportOptions::with_verify`] (or field assignment).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImportOptions {
    /// The re-parse sample (default `Count(0)`).
    pub verify: Verify,
}

impl ImportOptions {
    /// `self` with the re-parse sample set to `verify`.
    pub fn with_verify(mut self, verify: Verify) -> Self {
        self.verify = verify;
        self
    }
}

/// What a checkout's parse cache lacks ([`wanted`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct Wanted {
    /// The build stamp every key hashes (`glia_engine::BUILD_STAMP`).
    pub stamp: &'static str,
    /// The repo's label (its directory name, `arch::repo_label_for`).
    pub repo_label: String,
    /// The rows with no entry a build would reuse, in path order: never
    /// cached, cached from other content or as another language, or under the
    /// other MODULE form; every row when the sidecar was written under
    /// another build stamp, repo identity or go.mod module set.
    pub rows: Vec<CacheRow>,
    /// Rows the local cache already serves.
    pub local_hits: usize,
}

/// What one [`import_entries`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct ImportSummary {
    /// `(key, payload)` pairs offered.
    pub offered: usize,
    /// Payloads that passed every check and went into the sidecar.
    pub accepted: usize,
    /// `offered - accepted`: a key naming no wanted file (or offered twice),
    /// an undecodable payload, or one of other content, language or MODULE
    /// form.
    pub rejected: usize,
    /// Accepted entries re-parsed and found byte-equal to their payload.
    pub verified: usize,
    /// Whether `parse_cache.bin` was written: `false` when nothing was
    /// accepted, or the file already held exactly these bytes (LC.11).
    pub written: bool,
}

/// One language-parser file of the checkout: its row and walked source.
struct Planned<'a> {
    row: CacheRow,
    source: &'a str,
}

/// The checkout as a build sees it: its walked files and the context every
/// key and parse reads.
struct Checkout {
    files: Vec<(String, String)>,
    repo: RepoId,
    repo_key: String,
    go: GoModules,
    go_ctx: String,
    repo_label: String,
}

impl Checkout {
    /// Walk `repo_path` exactly as a build does (the same identity, go.mod
    /// set and walk as `export`'s plan, so a key here is the key a writer
    /// exported for the same bytes).
    fn read(repo_path: &str) -> Result<Self, String> {
        let root = Path::new(repo_path);
        if !root.is_dir() {
            return Err(format!("not a directory: {repo_path}"));
        }
        let ident = repo_identity(root);
        let (files, _regions, _md, roots) = crate::walk::walk_source_files(root);
        let go = crate::build::go_modules_for(root, &roots, repo_path);
        Ok(Self {
            files,
            repo: RepoId::from_canonical(&ident.key),
            go_ctx: go.context_key(),
            go,
            repo_key: ident.key,
            repo_label: crate::arch::repo_label_for(repo_path),
        })
    }

    /// Every language-parser file keyed, in path order.
    fn planned(&self) -> Vec<Planned<'_>> {
        let mut out: Vec<Planned<'_>> = cache_plan(&self.files)
            .into_iter()
            .map(|r| Planned {
                row: CacheRow {
                    key: file_key(
                        CACHE_VERSION,
                        &self.repo_key,
                        r.lang,
                        r.path,
                        &r.module_qname,
                        &self.go_ctx,
                        r.source.as_bytes(),
                    ),
                    content_hash: cache::content_hash(r.source),
                    path: r.path.to_string(),
                    lang: r.lang,
                    module_qname: r.module_qname,
                },
                source: r.source,
            })
            .collect();
        out.sort_by(|a, b| a.row.path.cmp(&b.row.path));
        out
    }

    /// The local sidecar under this checkout's build context: a sidecar of
    /// another identity or go.mod set is emptied, as a build would.
    fn cache(&self, repo_path: &str) -> ParseCache {
        let mut cache = ParseCache::load(repo_path);
        cache.validate_context(&self.repo_key, &self.go_ctx);
        cache
    }
}

/// Is `row` one the local `cache` cannot serve (a build would reparse it)?
fn is_wanted(cache: &ParseCache, row: &CacheRow) -> bool {
    cache
        .peek(&row.path, row.content_hash, row.lang)
        .is_none_or(|fp| cached_under_other_form(fp, &row.path, &row.module_qname))
}

/// The rows of `repo_path`'s checkout, as it is on disk now, that its parse
/// cache cannot serve: what a pull should fetch. Walks and plans the repo and
/// reads the sidecar; parses nothing and writes nothing.
pub fn wanted(repo_path: &str) -> Result<Wanted, String> {
    let co = Checkout::read(repo_path)?;
    let cache = co.cache(repo_path);
    let planned = co.planned();
    let total = planned.len();
    let rows: Vec<CacheRow> = planned
        .into_iter()
        .map(|p| p.row)
        .filter(|row| is_wanted(&cache, row))
        .collect();
    Ok(Wanted {
        stamp: CACHE_VERSION,
        local_hits: total - rows.len(),
        rows,
        repo_label: co.repo_label,
    })
}

/// Check one offered payload against the row its key names: decodable under
/// bounds, of the row's content hash and language, and not parsed under the
/// MODULE form this checkout's plan did not pick. The reason never quotes the
/// payload (it is untrusted text).
fn check(row: &CacheRow, payload: &[u8]) -> Result<CacheEntry, String> {
    let entry = CacheEntry::from_payload(payload)?;
    if entry.lang != row.lang {
        return Err(format!("parsed as another language (the checkout parses it as {})", row.lang));
    }
    if entry.content_hash != row.content_hash {
        return Err("parsed from other content than the checked-out file".to_string());
    }
    if cached_under_other_form(&entry.parse, &row.path, &row.module_qname) {
        return Err(format!(
            "parsed under the other MODULE form (the checkout names it {})",
            row.module_qname
        ));
    }
    Ok(entry)
}

/// Import `fetched` `(key, payload)` pairs into `repo_path`'s parse-cache
/// sidecar, after checking each against the checkout as it is on disk now.
///
/// The rows are re-planned here (a key names the bytes checked out NOW, not
/// when [`wanted`] ran). A key naming no wanted row, a key offered again
/// after one was accepted for it, an undecodable payload, or one whose
/// content hash, language or MODULE form is not the row's is rejected, with
/// one `[cache] rejected <path or key>: <reason>` line on stderr (the first
/// 20). Then `opts.verify` accepted entries, chosen uniformly at random, are
/// re-parsed with the build's own parse code and their payload compared byte
/// for byte with the offered one: ONE mismatch, or a sampled file that does
/// not parse locally, returns `Err` and leaves the sidecar untouched. Which
/// entries are sampled never changes what is written, only whether a
/// poisoned entry is caught. Only then are the accepted entries put and the
/// sidecar saved (not when nothing was accepted; LC.11 skips an unchanged
/// file). The next build reuses them through its ordinary checks.
///
/// fired_on marker, once per completed import:
///   `[cache] import repo=<label> offered=<n> accepted=<a> rejected=<r> verified=<v>`
pub fn import_entries(
    repo_path: &str,
    fetched: Vec<(CacheKey, Vec<u8>)>,
    opts: &ImportOptions,
) -> Result<ImportSummary, String> {
    let co = Checkout::read(repo_path)?;
    let mut cache = co.cache(repo_path);
    let planned: Vec<Planned<'_>> = co
        .planned()
        .into_iter()
        .filter(|p| is_wanted(&cache, &p.row))
        .collect();
    let by_key: BTreeMap<CacheKey, usize> =
        planned.iter().enumerate().map(|(i, p)| (p.row.key, i)).collect();

    // Slot i holds row i's accepted (payload, entry); the first acceptable
    // offer for a row takes it.
    let mut taken: Vec<Option<(Vec<u8>, CacheEntry)>> = planned.iter().map(|_| None).collect();
    let offered = fetched.len();
    let mut rejections = Rejections::default();
    for (key, payload) in fetched {
        let Some(&i) = by_key.get(&key) else {
            rejections.note(&key.to_hex(), "names no file this checkout wants");
            continue;
        };
        let row = &planned[i].row;
        if taken[i].is_some() {
            rejections.note(&row.path, "offered again after an accepted payload");
            continue;
        }
        match check(row, &payload) {
            Ok(entry) => taken[i] = Some((payload, entry)),
            Err(why) => rejections.note(&row.path, &why),
        }
    }
    rejections.finish();

    let accepted: Vec<usize> = (0..planned.len()).filter(|&i| taken[i].is_some()).collect();
    let n = match opts.verify {
        Verify::All => accepted.len(),
        Verify::Count(k) => k.min(accepted.len()),
    };
    let sample: Vec<usize> = sample_indices(accepted.len(), n, &mut SplitMix64::from_clock())
        .into_iter()
        .map(|j| accepted[j])
        .collect();
    let modules = ModuleQnames::plan(&co.files);
    let (checks, _threads) = crate::parallel::par_map_ordered(&sample, |&i| {
        let offered = taken[i].as_ref().map_or(&[][..], |(payload, _)| payload.as_slice());
        verify_one(&planned[i], offered, co.repo, &co.go, &modules)
    });
    // Sample in path order, so the error names the first mismatching path.
    checks.into_iter().collect::<Result<Vec<()>, String>>()?;

    let mut summary = ImportSummary {
        offered,
        accepted: accepted.len(),
        rejected: offered - accepted.len(),
        verified: sample.len(),
        written: false,
    };
    if summary.accepted > 0 {
        for (p, slot) in planned.iter().zip(taken) {
            if let Some((_, entry)) = slot {
                cache.put_entry(p.row.path.clone(), entry);
            }
        }
        summary.written = cache
            .save_changed(repo_path)
            .map_err(|e| format!("{repo_path}: write parse cache: {e}"))?;
    }
    eprintln!(
        "[cache] import repo={} offered={} accepted={} rejected={} verified={}",
        co.repo_label, summary.offered, summary.accepted, summary.rejected, summary.verified
    );
    Ok(summary)
}

/// Re-parse `p` with the build's own parse code and compare the payload that
/// parse would be cached as with the `offered` one, byte for byte.
fn verify_one(
    p: &Planned<'_>,
    offered: &[u8],
    repo: RepoId,
    go: &GoModules,
    modules: &ModuleQnames,
) -> Result<(), String> {
    let path = &p.row.path;
    let parse = match crate::parallel::quiet(|| {
        parse_for_cache(p.source, path, p.row.lang, repo, go, modules)
    }) {
        Ok(Ok((parse, _stats))) => parse,
        Ok(Err(e)) => {
            return Err(format!(
                "cache payload for {path} cannot be checked: the local parse failed ({e}); nothing written"
            ));
        }
        Err(_) => {
            return Err(format!(
                "cache payload for {path} cannot be checked: the local parse panicked; nothing written"
            ));
        }
    };
    let local = CacheEntry::new(p.row.content_hash, p.row.lang, parse)
        .payload()
        .map_err(|e| format!("{path}: {e}"))?;
    if local != offered {
        return Err(format!(
            "cache payload for {path} differs from a local parse; nothing written"
        ));
    }
    Ok(())
}

/// The `[cache] rejected` lines of one import, capped at
/// [`MAX_REJECTION_LINES`].
#[derive(Default)]
struct Rejections {
    count: usize,
}

impl Rejections {
    fn note(&mut self, what: &str, why: &str) {
        self.count += 1;
        if self.count <= MAX_REJECTION_LINES {
            eprintln!("[cache] rejected {what}: {why}");
        }
    }

    fn finish(&self) {
        if self.count > MAX_REJECTION_LINES {
            eprintln!(
                "[cache] rejected {} more (not shown)",
                self.count - MAX_REJECTION_LINES
            );
        }
    }
}

/// SplitMix64 (Steele, Lea and Flood 2014): a small, fast, full-period
/// generator, good enough to pick which entries a store writer cannot
/// predict are checked. It is not a cryptographic generator, and the sample
/// is not the security boundary (the CLI's MAC is).
struct SplitMix64(u64);

impl SplitMix64 {
    /// Seeded from the clock's nanoseconds XOR the pid, so two imports (even
    /// two at once) sample differently. A fixed choice, such as the first N
    /// rows in path order, would tell a store writer which entries are never
    /// checked.
    fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        SplitMix64(nanos ^ u64::from(std::process::id()))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` for `n > 0` (Lemire's multiply-shift; the bias is
    /// below `n / 2^64`).
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }
}

/// `k` distinct indices of `0..n` (all of them when `k >= n`), each `k`-set
/// equally likely: a partial Fisher-Yates shuffle, returned sorted.
fn sample_indices(n: usize, k: usize, rng: &mut SplitMix64) -> Vec<usize> {
    let k = k.min(n);
    let mut idx: Vec<usize> = (0..n).collect();
    for i in 0..k {
        let j = i + rng.below(n - i);
        idx.swap(i, j);
    }
    idx.truncate(k);
    idx.sort_unstable();
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_distinct_bounded_and_cover_every_index() {
        let mut rng = SplitMix64(7);
        assert!(sample_indices(0, 5, &mut rng).is_empty());
        assert_eq!(sample_indices(4, 9, &mut rng), [0, 1, 2, 3]);
        let mut seen = [0usize; 10];
        for _ in 0..2000 {
            let s = sample_indices(10, 3, &mut rng);
            assert_eq!(s.len(), 3);
            assert!(s.windows(2).all(|w| w[0] < w[1]), "{s:?} not sorted and distinct");
            for i in s {
                seen[i] += 1;
            }
        }
        // 600 picks per index expected; a fixed or skewed choice lands far off.
        assert!(seen.iter().all(|&c| (450..=750).contains(&c)), "{seen:?}");
    }

    #[test]
    fn the_clock_seed_moves() {
        let a = SplitMix64::from_clock().next_u64();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = SplitMix64::from_clock().next_u64();
        assert_ne!(a, b);
    }
}
