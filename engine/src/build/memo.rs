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
//! and the parses the needles read depend on the go context.
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
//!   `[graft-memo] const hit=<h>/<n> needles hit=<h>/<n> noncode hit=<h>/<n> repo=<label>`
//! `n` = hits + misses: files with a source language (const), files a needle
//! text gate passed with a source language other than `.proto` (needles; 0
//! when the build knows no proto service), files a `PURE_NONCODE` branch
//! routed (noncode).

use std::collections::{HashMap, HashSet};

use glia_code_domain::FileParse;
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
        Self { map: HashMap::new() }
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
    #[cfg(test)]
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
}

/// Every DC.4 memo of one repo's builds, held by its `ParseCache`.
#[derive(Default)]
pub(crate) struct BuildMemo {
    pub(crate) consts: ConstMemo,
    pub(crate) needles: NeedleMemo,
    pub(crate) noncode: NonCodeMemo,
    pub(crate) stats: MemoStats,
}

impl BuildMemo {
    /// Drop every entry (the build context changed). The stats are kept: they
    /// describe the last build, which the next one resets.
    pub(crate) fn clear(&mut self) {
        self.consts.clear();
        self.needles.clear();
        self.noncode.clear();
    }

    /// Drop the entries of paths not in `live` (this build's walked set).
    pub(crate) fn retain(&mut self, live: &HashSet<&str>) {
        self.consts.retain(live);
        self.needles.retain(live);
        self.noncode.retain(live);
    }

    /// Paths held across the three memos (a path may be in several).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.consts.len() + self.needles.len() + self.noncode.len()
    }

    /// The `[graft-memo]` line of the last build (module doc).
    pub(crate) fn marker(&self, repo_label: &str) -> String {
        let s = &self.stats;
        format!(
            "[graft-memo] const hit={}/{} needles hit={}/{} noncode hit={}/{} repo={repo_label}",
            s.const_hit,
            s.const_hit + s.const_miss,
            s.needle_hit,
            s.needle_hit + s.needle_miss,
            s.noncode_hit,
            s.noncode_hit + s.noncode_miss,
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
        assert_ne!(services_fp(&base), services_fp(&[svc("Greeter", &["Hi", "Bye"])]));
        assert_ne!(services_fp(&base), services_fp(&[svc("Farewell", &["Hi"])]));
        assert_ne!(services_fp(&base), services_fp(&[]));
        let two = [svc("A", &[]), svc("B", &[])];
        assert_ne!(services_fp(&two), services_fp(&[svc("A", &[])]));
    }

    #[test]
    fn a_path_memo_hits_only_under_its_key_and_retains_live_paths() {
        let mut m: PathMemo<u64, u32> = PathMemo::default();
        m.insert("a".into(), 1, 10);
        m.insert("b".into(), 2, 20);
        assert_eq!(m.get("a", &1), Some(&10));
        assert_eq!(m.get("a", &2), None, "another key misses");
        m.insert("a".into(), 3, 30);
        assert_eq!((m.get("a", &1), m.get("a", &3)), (None, Some(&30)), "overwritten");
        assert_eq!(m.len(), 2, "one entry per path");
        m.retain(&HashSet::from(["b"]));
        assert_eq!((m.len(), m.get("b", &2)), (1, Some(&20)));
    }
}
