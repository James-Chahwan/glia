//! DC.4: the build memos a held [`crate::cache::ParseCache`] keeps in memory
//! across builds, so the per-file work that runs on every build and is not a
//! parse (the A11.1 const-table scan, the A5.2 / A5.3 / LA.17 RPC needles, the
//! non-code routing branches) is paid about once per changed file.
//!
//! The memos ride the cache as `#[serde(skip)]`: `parse_cache.bin` never holds
//! them, so its bytes and the CE.2 shared-cache payloads are unchanged, and a
//! cache loaded from disk starts with an empty memo. Every caller that holds a
//! cache across builds gains them: a watch session, a `generate_one_with_cache`
//! holder, the after-build and rev-build of `delta` / `review` /
//! `contract-breaks`, `timeline` (one build per rev) and the overlay-loop trial
//! (base, candidate and every leave-one-out on one cache).
//!
//! Each memo is one entry per walked path, `(key, value)`, overwritten when
//! the key changes, so memory is bounded by the file count; entries of paths no
//! longer walked are dropped ([`BuildMemo::retain`]), and the whole memo is
//! cleared with the cache's entries when the build context (repo identity or
//! go.mod module set) changes: the values carry the RepoId in their NodeIds,
//! and the parses the needles read depend on the go context. A watch session
//! past its memory budget clears it too (GA.12, `[watch] rss .. memo_cleared=1`):
//! the next build recomputes every value as a miss does.
//!
//! Lookups run on pool workers against `&BuildMemo`, read-only; misses are
//! computed as before; inserts happen in each stage's sequential fold, after
//! the pool returns, so no lock is taken and no order reaches the graph. A hit
//! hands back a clone and the fold consumes it exactly as it consumes a fresh
//! result, so the graph is byte-identical with or without the memo.
//!
//! CONTRACT for later packets. A memo key must carry every input its value is
//! computed from, or a hit replays a stale result in silence:
//! - [`ConstKey`]: `ConstTable::scan_file` reads the file text and its
//!   language only.
//! - [`NeedleKey`]: `needle_one` (rpc_needles.rs) reads the file text, path,
//!   language, MODULE id, the build's proto service set ([`services_fp`]), the
//!   repo's Twirp flag, and the file's own parse (its CLASS / STRUCT / METHOD /
//!   FUNCTION nodes, a function of the same file, its MODULE qname and the go
//!   context, which clears the memo). A packet that gives `needle_one` a new
//!   input adds it to [`NeedleKey`] in the same commit.
//! - the non-code memo: a `route.rs` branch in `PURE_NONCODE` reads only
//!   `(path, source, repo)`. A packet that makes one read anything else (the
//!   module plan, the go context, another file) removes it from `PURE_NONCODE`
//!   in the same commit.
//! - [`ImportsMemo`] (GA.20, the A16.4 IMPORTS filter of
//!   `grafts::filter_imports_cells`): the repo's `LocalModuleIndex` is a
//!   function of what `LocalModuleIndex::add_parse` reads off every parse
//!   that carries an IMPORTS cell (each MODULE / PACKAGE / CLASS / STRUCT /
//!   INTERFACE / ENUM / FUNCTION's kind, name and MODULE / PACKAGE qname),
//!   the Cargo package names and the tsconfig alias keys, fingerprinted as
//!   [`IndexFp`]; one file's filter output ([`ImportsOut`]) is a function of
//!   its `FileParse::imports`, its language and that index, all three in
//!   [`ImportsKey`]. A packet that makes the index or the filter read
//!   anything else adds it to the fingerprint or the key in the same commit.
//! - [`WrapperMemo`] (GA.21, `external::infer_entity_wrappers`): per walked
//!   file of an inference language (the HTTP families and Go), the file's
//!   CA.4 direct collection candidates (`direct_candidates`), its FH.4b
//!   direct HTTP candidates (`direct_http`: candidates, unparsed calls and
//!   the FH.17 phantom gate), its GD.26 heritage class record (classes,
//!   spans, `extends` bases, members and value imports) and, per forwarding
//!   depth, the frontier calls its text holds with their `forwarding_*`
//!   outputs. Each of those reads exactly the file's path (its row's
//!   path), its text, its language and its own parse, found by its LB.9b
//!   MODULE id; the parse is a function of the same text, language and
//!   MODULE qname under the build context (which clears the memo), so
//!   [`WrapperKey`] carries the language, the text's content hash and the
//!   MODULE id. A forwarding depth's outputs are kept with a fingerprint of
//!   that depth's whole frontier (every call and its shape / name argument,
//!   in order) and replayed only under an equal one. A packet that gives
//!   `direct_candidates`, `direct_http`, the class reader or a
//!   `forwarding_*` reader another input adds it to [`WrapperKey`] (or, for
//!   a repo-wide input of the forwarding pass, to the frontier
//!   fingerprint) in the same commit.
//! - [`ScanMemo`] (GA.21, `external::WrapperPass::scan`): per walked file the
//!   LF.2e stage scans, what the scan read off it: skipped by the stanza
//!   gate, or its sites and GD.26 ambiguous heritage calls (or a caught
//!   panic). The scan reads the file's text and language, the ordered stanza
//!   set (each stanza's declaration, kind, FH.4b shape and whether it is a
//!   heritage stanza: everything `scan_file` / `read_site` /
//!   `scan_heritage` read) and the file's own GD.26 heritage scopes, so
//!   [`ScanKey`] carries the language, the content hash, a fingerprint of
//!   the stanza set and one of the file's scopes. A packet that makes the
//!   gate, `scan_file`, `read_site` or `scan_heritage` read anything else
//!   adds it to [`ScanKey`] in the same commit.
//!
//! What a hit does not replay: the stderr lines an extractor prints itself
//! from inside the call (`[code-guard]`, `[grpc-client] dial hosts`,
//! `[cron] k8s`, `[config] settings` / `defined=`, `[contracts] openapi
//! servers`, ...), exactly as a parse-cache hit replays none of a code file's
//! extractor lines. The lines a non-code branch hands its fold (`SynthTally`'s
//! `lines`: `[cli] manifest`, `[migration]`, ...) and every per-build counter
//! line are replayed, in walk order.
//!
//! fired_on marker, once per repo build that holds a cache
//! ([`BuildMemo::marker`]):
//!   `[graft-memo] const hit=<h>/<n> needles hit=<h>/<n> noncode hit=<h>/<n> imports hit=<h>/<n> index=<hit|built|none> wrappers hit=<h>/<n> scan hit=<h>/<n> repo=<label>`
//! `n` = hits + misses: files with a source language (const), files a needle
//! text gate passed with a source language other than `.proto` (needles; 0
//! when the build knows no proto service), files a `PURE_NONCODE` branch
//! routed (noncode), parses carrying an IMPORTS cell that the filter rewrote
//! (imports, GA.20); `index` says whether the repo's `LocalModuleIndex` was
//! reused or built (`none` only when the filter did not run); walked files
//! of an inference language (wrappers, GA.21) and files the LF.2e stage
//! scanned, every file with a code language (scan, GA.21; 0 when the stage
//! did not run).
//!
//! The two GA.21 memos hold the wrapper stages' own per-file records
//! (private types of `external::infer_wrappers` / `external::wrappers`) as
//! one row table per stage ([`StageMemo`]), type-erased (`Box<dyn Any + Send
//! + Sync>`) and downcast by their one reader: a table of another type reads
//! as a cold memo, never as a value. Each run rewrites its table from this
//! build's walked files, so [`BuildMemo::retain`] leaves them alone.

use std::any::Any;
use std::collections::{HashMap, HashSet};

use glia_code_domain::{FileParse, ImportStmt, LocalModuleIndex};
use glia_code_extractors::constants::ConstTable;
use glia_code_extractors::grpc::{self, ProtoServiceRef};
use glia_core::NodeId;

use crate::route::SynthTally;

/// One memo: per walked path, the key its value was computed under and the
/// value. A lookup hits only when the stored key equals the build's.
pub(crate) struct PathMemo<K, V> {
    map: HashMap<String, (K, V)>,
}

impl<K, V> Default for PathMemo<K, V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<K: PartialEq, V> PathMemo<K, V> {
    /// The value of `path` iff it was computed under `key`.
    pub(crate) fn get(&self, path: &str, key: &K) -> Option<&V> {
        self.map.get(path).filter(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Record `path`'s value under `key`, replacing whatever it held.
    pub(crate) fn insert(&mut self, path: String, key: K, value: V) {
        self.map.insert(path, (key, value));
    }

    /// Paths held.
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    fn retain(&mut self, live: &HashSet<&str>) {
        self.map.retain(|p, _| live.contains(p.as_str()));
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

/// What the A11.1 scan of one file was computed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConstKey {
    pub(crate) content_hash: u64,
    pub(crate) lang: &'static str,
}

/// Per path: the file's own `ConstTable::scan_file`, `None` when the scan
/// panicked (replayed as the same `PANIC (const table scan)` line).
pub(crate) type ConstMemo = PathMemo<ConstKey, Option<ConstTable>>;

/// What one file's needle halves were computed from (module doc: CONTRACT).
/// The parse's slot in `parses_by_lang` is NOT part of it: it shifts whenever
/// a file is added or removed, so a hit re-derives it from this build's parse
/// index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NeedleKey {
    pub(crate) content_hash: u64,
    pub(crate) lang: &'static str,
    pub(crate) module_id: NodeId,
    pub(crate) services_fp: u64,
    pub(crate) twirp: bool,
}

/// One file's three needle halves, as `needle_one` computes them: each `None`
/// when its text gate kept it off the file, else its nodes or the
/// `parse_errors` line of a caught panic.
#[derive(Clone)]
pub(crate) struct NeedleHalves {
    pub(crate) client: Option<Result<grpc::GrpcNodes, String>>,
    pub(crate) server: Option<Result<grpc::GrpcNodes, String>>,
    pub(crate) proto_rpc: Option<Result<grpc::ProtoRpcNodes, String>>,
}

/// Per path: the halves, `None` when the file had no parse (there is no
/// module to hang a marker on).
pub(crate) type NeedleMemo = PathMemo<NeedleKey, Option<NeedleHalves>>;

/// One non-code branch's outcome: the parts of `Routed::NonCode` (the lang
/// key it files under, its parse, its marker counters and lines).
#[derive(Clone)]
pub(crate) struct NonCodeOut {
    pub(crate) key: &'static str,
    pub(crate) fp: Option<FileParse>,
    pub(crate) tally: SynthTally,
}

/// Per path, keyed by the file's content hash: what a `PURE_NONCODE` branch
/// routed it to.
pub(crate) type NonCodeMemo = PathMemo<u64, NonCodeOut>;

/// GA.20: a 128-bit fingerprint of what one repo's `LocalModuleIndex` was
/// built from (module doc: CONTRACT).
pub(crate) type IndexFp = (u64, u64);

/// GA.20: what one file's A16.4 filter output was computed from (module doc:
/// CONTRACT). The imports are held whole, so a hit is an exact match, never a
/// hash collision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportsKey {
    pub(crate) lang: &'static str,
    pub(crate) index: IndexFp,
    pub(crate) imports: Vec<ImportStmt>,
}

/// GA.20: one file's A16.4 filter output, replayed onto its parse on a hit:
/// the IMPORTS cell payload every node carries (`None` when the parse holds
/// no node), the `(kept, dropped)` counts and the IMPORT_BINDINGS cell
/// (`code_domain::import_bindings_cell`: payload, rows, capped rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportsOut {
    pub(crate) imports_json: Option<String>,
    pub(crate) kept: usize,
    pub(crate) dropped: usize,
    pub(crate) bindings: Option<(String, usize, usize)>,
}

/// GA.20: the A16.4 IMPORTS filter's memo. The index is the last build's;
/// the files are keyed by their parse's MODULE id (its first node, the id the
/// LB.9b plan gives the file) and hold exactly the last build's parses: the
/// filter rebuilds the map on every build, so memory is bounded by the file
/// count and a file no longer walked drops out.
#[derive(Default)]
pub(crate) struct ImportsMemo {
    pub(crate) index: Option<(IndexFp, LocalModuleIndex)>,
    pub(crate) files: HashMap<NodeId, (ImportsKey, ImportsOut)>,
}

/// GA.21: what one file's wrapper-inference outputs were computed from
/// (module doc: CONTRACT). The content hash is the DC.4 one
/// (`cache::content_hash`, xxh64), the hash the parse cache keys the same
/// file's parse by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapperKey {
    pub(crate) lang: &'static str,
    pub(crate) content_hash: u64,
    pub(crate) module_id: NodeId,
}

/// GA.21: the inference stage's per-file records
/// (`external::infer_wrappers`): its [`MemoRow`]s, type-erased (module doc).
pub(crate) type WrapperMemo = StageMemo;

/// GA.21: what one file's LF.2e scan was computed from (module doc:
/// CONTRACT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanKey {
    pub(crate) lang: &'static str,
    pub(crate) content_hash: u64,
    /// The ordered stanza set's fingerprint.
    pub(crate) stanzas: u64,
    /// The file's own GD.26 heritage scopes' fingerprint.
    pub(crate) scopes: u64,
}

/// GA.21: the scan's per-file outcomes (`external::wrappers`): its
/// [`MemoRow`]s, type-erased (module doc).
pub(crate) type ScanMemo = StageMemo;

/// GA.21: one row of a stage's memo: a walked path, the key its value was
/// computed under and the value (the stage's own record type).
pub(crate) struct MemoRow<K, V> {
    pub(crate) path: String,
    pub(crate) key: K,
    pub(crate) value: V,
}

/// GA.21: a per-path memo its stage rewrites whole on every run: the last
/// run's rows (`Vec<MemoRow<K, V>>`, in walk order), kept as one type-erased
/// value. A run takes them out ([`StageMemo::take`]) and puts back exactly
/// its own ([`StageMemo::put`]), so a path no longer walked drops out at the
/// stage's next run and memory is bounded by the file count. The rows are
/// read positionally ([`RowTake`]): the walk order rarely moves between two
/// builds of one repo, so an unchanged tree hashes no path.
#[derive(Default)]
pub(crate) struct StageMemo {
    rows: Option<Box<dyn Any + Send + Sync>>,
    len: usize,
}

impl StageMemo {
    /// The last run's rows, out of the memo; `None` when it holds none (or
    /// rows of another type, which read as a cold memo).
    pub(crate) fn take<K: 'static, V: Default + 'static>(&mut self) -> Option<RowTake<K, V>> {
        self.len = 0;
        let rows = *self.rows.take()?.downcast::<Vec<MemoRow<K, V>>>().ok()?;
        Some(RowTake {
            taken: vec![false; rows.len()],
            rows,
            next: 0,
            index: None,
        })
    }

    /// This run's rows, replacing whatever the memo held.
    pub(crate) fn put<K: Send + Sync + 'static, V: Send + Sync + 'static>(
        &mut self,
        rows: Vec<MemoRow<K, V>>,
    ) {
        self.len = rows.len();
        self.rows = Some(Box::new(rows));
    }

    /// Paths held.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    fn clear(&mut self) {
        self.rows = None;
        self.len = 0;
    }
}

/// GA.21: the rows of a [`StageMemo`] taken out for one run.
pub(crate) struct RowTake<K, V> {
    rows: Vec<MemoRow<K, V>>,
    taken: Vec<bool>,
    /// The row after the last one taken: the next path's, when the walk
    /// order did not move.
    next: usize,
    /// Path -> row, built on the first positional miss. Looked up, never
    /// iterated.
    index: Option<HashMap<String, usize>>,
}

impl<K: Copy + Sync, V: Default + Sync> RowTake<K, V> {
    /// Whether this run's walked paths, in order, are exactly the last run's
    /// rows (the paths compared on the engine pool). Then [`RowTake::row_at`]
    /// hands out row `k` for the `k`-th path with no per-row lookup.
    pub(crate) fn aligned(&self, paths: &[&str]) -> bool {
        if paths.len() != self.rows.len() {
            return false;
        }
        let at: Vec<usize> = (0..paths.len()).collect();
        let rows = &self.rows;
        crate::parallel::par_map_ordered(&at, |&k| {
            rows.get(k).map(|r| r.path.as_str()) == paths.get(k).copied()
        })
        .0
        .into_iter()
        .all(|same| same)
    }

    /// Row `k`, out of the take, when [`RowTake::aligned`] held.
    pub(crate) fn row_at(&mut self, k: usize) -> Option<MemoRow<K, V>> {
        let taken = self.taken.get_mut(k)?;
        if *taken {
            return None;
        }
        *taken = true;
        let r = self.rows.get_mut(k)?;
        Some(MemoRow {
            path: std::mem::take(&mut r.path),
            key: r.key,
            value: std::mem::take(&mut r.value),
        })
    }

    /// The row of `path`, out of the take (`None` when the last run held
    /// none, or it was taken already).
    pub(crate) fn row(&mut self, path: &str) -> Option<MemoRow<K, V>> {
        let at = match self.rows.get(self.next) {
            Some(r) if r.path == path => self.next,
            _ => {
                let rows = &self.rows;
                *self
                    .index
                    .get_or_insert_with(|| {
                        rows.iter()
                            .enumerate()
                            .map(|(i, r)| (r.path.clone(), i))
                            .collect()
                    })
                    .get(path)?
            }
        };
        self.next = at + 1;
        self.row_at(at)
    }
}

/// GA.21: an xxh64 fingerprint over `parts`, each followed by a 0 byte so two
/// lists never concatenate to the same bytes (the [`services_fp`] rule). The
/// wrapper memos fingerprint `Debug` strings of types that hold no hash
/// container, so equal values give equal fingerprints.
pub(crate) fn parts_fp<'a>(parts: impl IntoIterator<Item = &'a str>) -> u64 {
    use core::hash::Hasher;
    use twox_hash::XxHash64;
    let mut h = XxHash64::with_seed(0);
    for p in parts {
        h.write(p.as_bytes());
        h.write(&[0]);
    }
    h.finish()
}

/// The last build's lookups, per memo. Reset at the start of each repo build
/// that holds a cache.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoStats {
    pub(crate) const_hit: usize,
    pub(crate) const_miss: usize,
    pub(crate) needle_hit: usize,
    pub(crate) needle_miss: usize,
    pub(crate) noncode_hit: usize,
    pub(crate) noncode_miss: usize,
    pub(crate) imports_hit: usize,
    pub(crate) imports_miss: usize,
    /// The repo's `LocalModuleIndex`: reused (`Some(true)`), built
    /// (`Some(false)`), or `None` when the filter did not run.
    pub(crate) index_hit: Option<bool>,
}

/// GA.21: the last build's lookups in the two wrapper memos, beside
/// [`MemoStats`] (reset by `grafts::apply_post_cache` before the inference).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapperStats {
    /// Walked files of an inference language whose record was replayed /
    /// recomputed.
    pub(crate) wrappers_hit: usize,
    pub(crate) wrappers_miss: usize,
    /// Files the LF.2e scan replayed / rescanned.
    pub(crate) scan_hit: usize,
    pub(crate) scan_miss: usize,
}

/// Every DC.4 memo of one repo's builds, held by its `ParseCache`.
#[derive(Default)]
pub(crate) struct BuildMemo {
    pub(crate) consts: ConstMemo,
    pub(crate) needles: NeedleMemo,
    pub(crate) noncode: NonCodeMemo,
    pub(crate) imports: ImportsMemo,
    pub(crate) wrappers: WrapperMemo,
    pub(crate) scan: ScanMemo,
    pub(crate) stats: MemoStats,
    pub(crate) wrapper_stats: WrapperStats,
}

impl BuildMemo {
    /// Drop every entry (the build context changed). The stats are kept: they
    /// describe the last build, which the next one resets.
    pub(crate) fn clear(&mut self) {
        self.consts.clear();
        self.needles.clear();
        self.noncode.clear();
        self.imports = ImportsMemo::default();
        self.wrappers.clear();
        self.scan.clear();
    }

    /// Drop the entries of paths not in `live` (this build's walked set).
    pub(crate) fn retain(&mut self, live: &HashSet<&str>) {
        self.consts.retain(live);
        self.needles.retain(live);
        self.noncode.retain(live);
        // GA.21's stage memos are rewritten from the walked files by their
        // stage's next run (`StageMemo`): nothing to drop here.
    }

    /// Paths held across the five path memos (a path may be in several) plus
    /// the files of the imports memo: `WatchMemory.memo_paths` (GA.12). The
    /// two GA.21 stage memos count the rows their stage's last run kept.
    pub(crate) fn len(&self) -> usize {
        self.consts.len()
            + self.needles.len()
            + self.noncode.len()
            + self.imports.files.len()
            + self.wrappers.len()
            + self.scan.len()
    }

    /// The `[graft-memo]` line of the last build (module doc).
    pub(crate) fn marker(&self, repo_label: &str) -> String {
        let s = &self.stats;
        let w = &self.wrapper_stats;
        format!(
            "[graft-memo] const hit={}/{} needles hit={}/{} noncode hit={}/{} imports hit={}/{} index={} wrappers hit={}/{} scan hit={}/{} repo={repo_label}",
            s.const_hit,
            s.const_hit + s.const_miss,
            s.needle_hit,
            s.needle_hit + s.needle_miss,
            s.noncode_hit,
            s.noncode_hit + s.noncode_miss,
            s.imports_hit,
            s.imports_hit + s.imports_miss,
            match s.index_hit {
                Some(true) => "hit",
                Some(false) => "built",
                None => "none",
            },
            w.wrappers_hit,
            w.wrappers_hit + w.wrappers_miss,
            w.scan_hit,
            w.scan_hit + w.scan_miss,
        )
    }
}

/// xxh64 over a proto service set's `Debug` strings, in the order given
/// (`RpcContext` holds it sorted and deduplicated, so equal sets give equal
/// fingerprints). `ProtoServiceRef` holds no hash container, so its `Debug` is
/// a pure function of its content. The needle memo's [`NeedleKey`] carries it;
/// a watch session's group key can reuse it.
pub(crate) fn services_fp(services: &[ProtoServiceRef]) -> u64 {
    use core::hash::Hasher;
    use twox_hash::XxHash64;
    let mut h = XxHash64::with_seed(0);
    for s in services {
        h.write(format!("{s:?}").as_bytes());
        // A separator no Debug string ends in, so two lists never concatenate
        // to the same bytes.
        h.write(&[0]);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str, rpcs: &[&str]) -> ProtoServiceRef {
        ProtoServiceRef {
            name: name.to_string(),
            package: Some("shop".to_string()),
            go_package: None,
            java_package: None,
            csharp_namespace: None,
            rpcs: rpcs.iter().map(|r| (*r).to_string()).collect(),
        }
    }

    #[test]
    fn services_fp_tracks_every_field() {
        let base = [svc("Greeter", &["Hi"])];
        assert_eq!(services_fp(&base), services_fp(&[svc("Greeter", &["Hi"])]));
        assert_ne!(
            services_fp(&base),
            services_fp(&[svc("Greeter", &["Hi", "Bye"])])
        );
        assert_ne!(services_fp(&base), services_fp(&[svc("Farewell", &["Hi"])]));
        assert_ne!(services_fp(&base), services_fp(&[]));
        let two = [svc("A", &[]), svc("B", &[])];
        assert_ne!(services_fp(&two), services_fp(&[svc("A", &[])]));
    }

    #[test]
    fn the_marker_names_the_imports_memo() {
        let mut m = BuildMemo::default();
        assert!(
            m.marker("r").ends_with(
                " noncode hit=0/0 imports hit=0/0 index=none wrappers hit=0/0 scan hit=0/0 repo=r"
            ),
            "{}",
            m.marker("r")
        );
        m.stats.imports_hit = 3;
        m.stats.imports_miss = 1;
        m.stats.index_hit = Some(true);
        assert!(
            m.marker("r")
                .contains(" imports hit=3/4 index=hit wrappers")
        );
        m.stats.index_hit = Some(false);
        assert!(m.marker("r").contains(" index=built wrappers"));
    }

    #[test]
    fn the_marker_names_the_wrapper_memos_before_the_repo() {
        let mut m = BuildMemo::default();
        m.wrapper_stats.wrappers_hit = 9;
        m.wrapper_stats.wrappers_miss = 1;
        m.wrapper_stats.scan_hit = 4;
        m.wrapper_stats.scan_miss = 2;
        assert!(
            m.marker("r")
                .ends_with(" wrappers hit=9/10 scan hit=4/6 repo=r"),
            "{}",
            m.marker("r")
        );
    }

    fn row(path: &str, key: u32) -> MemoRow<u32, u32> {
        MemoRow {
            path: path.to_string(),
            key,
            value: key * 10,
        }
    }

    #[test]
    fn the_wrapper_memos_count_and_clear() {
        let mut m = BuildMemo::default();
        m.wrappers.put(vec![row("a.ts", 1), row("b.ts", 2)]);
        m.scan.put(vec![row("a.ts", 3)]);
        assert_eq!(m.len(), 3, "memo_paths counts both");
        m.retain(&HashSet::from(["a.ts"]));
        assert_eq!(
            (m.wrappers.len(), m.scan.len()),
            (2, 1),
            "rewritten by their stage, not by retain"
        );
        m.clear();
        assert_eq!(m.len(), 0, "cleared with the other memos");
        assert!(m.wrappers.take::<u32, u32>().is_none());
    }

    #[test]
    fn a_stage_memo_of_another_type_reads_cold() {
        let mut m = StageMemo::default();
        m.put(vec![row("a", 1)]);
        assert!(m.take::<u32, u8>().is_none());
        assert_eq!(m.len(), 0, "and is gone");
    }

    #[test]
    fn row_take_matches_by_position_then_by_path() {
        let mut m = StageMemo::default();
        m.put(vec![row("a", 1), row("b", 2), row("c", 3), row("d", 4)]);
        let mut t = m.take::<u32, u32>().expect("rows");
        assert_eq!(m.len(), 0, "taken out whole");
        let got = |r: Option<MemoRow<u32, u32>>| r.map(|r| (r.path, r.key, r.value));
        assert_eq!(got(t.row("a")), Some(("a".into(), 1, 10)), "positional");
        // "b" was removed from the walk, "x" added: the path index.
        assert_eq!(got(t.row("c")), Some(("c".into(), 3, 30)));
        assert_eq!(got(t.row("x")), None);
        assert_eq!(got(t.row("d")), Some(("d".into(), 4, 40)));
        assert_eq!(got(t.row("a")), None, "a row is taken once");
        // Out of order, too.
        m.put(vec![row("a", 1), row("b", 2)]);
        let mut t = m.take::<u32, u32>().expect("rows");
        assert_eq!(got(t.row("b")), Some(("b".into(), 2, 20)));
        assert_eq!(got(t.row("a")), Some(("a".into(), 1, 10)));
        // Aligned: by index.
        m.put(vec![row("a", 1), row("b", 2)]);
        let mut t = m.take::<u32, u32>().expect("rows");
        assert!(!t.aligned(&["a"]), "another length");
        assert!(!t.aligned(&["b", "a"]), "another order");
        assert!(t.aligned(&["a", "b"]));
        assert_eq!(got(t.row_at(1)), Some(("b".into(), 2, 20)));
        assert_eq!(got(t.row_at(1)), None, "taken once");
    }

    #[test]
    fn parts_fp_separates_the_parts() {
        assert_eq!(parts_fp(["ab", "c"]), parts_fp(["ab", "c"]));
        assert_ne!(parts_fp(["ab", "c"]), parts_fp(["a", "bc"]));
        assert_ne!(parts_fp(["a"]), parts_fp(["a", ""]));
        assert_ne!(parts_fp([]), parts_fp([""]));
    }

    #[test]
    fn a_path_memo_hits_only_under_its_key_and_retains_live_paths() {
        let mut m: PathMemo<u64, u32> = PathMemo::default();
        m.insert("a".into(), 1, 10);
        m.insert("b".into(), 2, 20);
        assert_eq!(m.get("a", &1), Some(&10));
        assert_eq!(m.get("a", &2), None, "another key misses");
        m.insert("a".into(), 3, 30);
        assert_eq!(
            (m.get("a", &1), m.get("a", &3)),
            (None, Some(&30)),
            "overwritten"
        );
        assert_eq!(m.len(), 2, "one entry per path");
        m.retain(&HashSet::from(["b"]));
        assert_eq!((m.len(), m.get("b", &2)), (1, Some(&20)));
    }
}
