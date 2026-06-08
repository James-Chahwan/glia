# Incremental graph build + indexing — design plan

Status 2026-06-09: **Phase 1 shipped** (skip-when-unchanged shard writes). **Phase 2
redesigned** around a per-file *parse cache* (this doc) — supersedes the earlier
in-place-`.gmap`-patch Phase 2. **Phase 3 (LiveGraph / streaming) CUT** — neuropil
confirmed in-process bulk-load + cached PPR is enough (no per-token streaming).

This is WP-D / GR-4. Goal (GR-4 acceptance): *touch one file in a ~22k-node repo →
the rebuild re-parses ~1 file, not all, and the resulting graph is byte-identical to a
clean rebuild.*

---

## 1. The key decision: cache at the `FileParse` boundary, then rebuild

Two facts from the code decide the architecture:

1. **Tree-sitter parsing is the expensive step.** `build_<lang>()` (symbol table +
   import/call resolution) and the cross-graph resolvers are HashMap walks over
   nodes/edges — cheap (ms–low-seconds at 22k nodes) relative to parsing every file
   (seconds–tens of seconds).
2. **`FileParse` is a clean, per-file, deterministic boundary.** A parser turns one
   `(source, path)` into one `FileParse` with no cross-file state (the extract-vs-resolve
   split guarantees this — parsers *extract*, the graph crate *resolves*). All of
   `FileParse`'s components already derive `serde` + `rkyv`; only `FileParse` itself needs
   a `serde` derive added.

**So: cache `FileParse` keyed by content hash; on rebuild, reuse unchanged files'
parses and re-parse only changed files; then rebuild the per-language graphs + run all
resolvers + post-passes as today.**

### Why this over the old in-place-patch Phase 2

The earlier plan patched the `.gmap` directly — drop a file's nodes by POSITION cell,
splice in re-parsed nodes, run *bounded* re-resolution, and invalidate resolvers via a
"resolver-invalidation registry" it called "the bulk of the work." That design is:

- **Correctness-risky** — in-place mutation + bounded resolution can drift from a clean
  build; "byte-identical" is hard to guarantee and easy to silently violate.
- **Heavy** — every cross-graph resolver must declare consumed/produced kinds and learn
  to run over a neighbourhood.

The parse-cache design is **byte-identical by construction** (the graph is always built
fresh from the full parse set; only the *parse* of unchanged files is reused) and needs
**no resolver changes at all**. It captures the dominant cost (parsing) and leaves the
cheap, already-correct build/resolve path untouched. Do the simple correct thing first;
optimize the build/resolve step only if profiling demands it (see §9).

---

## 2. Data flow (cache in the loop)

```
            ┌─────────────── incremental walk ───────────────┐
 source ───▶│ enumerate paths + stat (mtime,size)            │
 tree       │   vs FileIndex:                                │
            │     mtime+size match      → REUSE cached parse │ (no read, no parse)
            │     mtime changed, hash same → REUSE, fix mtime│ (read only)
            │     hash differs / new    → re-parse           │ (read + tree-sitter)
            │     in index, gone        → evict              │
            └───────────────────┬───────────────────────────┘
                                 ▼
              per-lang FileParse set (cached ∪ freshly parsed)
                                 ▼
   build_<lang>()  →  RepoGraphs  →  cross-graph resolvers  →  post_passes   (ALL re-run)
                                 ▼
                 MergedGraph ──▶ .gmap (Phase-1 skip-unchanged shards)
                                 │
                 ParseCache + FileIndex ──▶ parse_cache.bin   (rewritten wholesale)
```

The only thing reused across builds is the **`FileParse` of unchanged files**.
Everything downstream is rebuilt → output matches a clean build.

---

## 3. Core types (new; live in `engine`, code-domain-shaped)

```rust
/// One cached per-file parse + the stat/hash that validates it.
struct FileEntry {
    path: String,        // repo-relative
    mtime_ns: u64,
    size: u64,
    content_hash: u64,   // xxhash64 (store's existing primitive; NOT blake3 — see §10)
    lang: String,
}

/// The whole cache: a content-version stamp + per-file entries + parses.
/// Rewritten wholesale each build (glia "write-once, rebuild-whole-file"); ONE
/// file, not one-per-source — inode-safe (cf. the MFS inode-quota incident).
pub struct ParseCache {
    engine_version: String,              // CARGO_PKG_VERSION at write time
    index: Vec<FileEntry>,               // the FileIndex — also powers is_stale (#10) + Engram --since
    parses: HashMap<String, FileParse>,  // path -> cached parse (post-extractors)
}
```

The cached `FileParse` is the **post-extractor** output — i.e. after
`apply_cross_cutting_extractors` + `attach_imports_cell`, the complete per-file
contribution that feeds `build_<lang>`. (Extractors are per-file and source-derived, so
they cache at the same boundary.)

---

## 4. Change detection (the "indexing")

Per candidate file, cheapest-first:

1. **`stat` → (mtime_ns, size).** If both match the `FileEntry` → **unchanged**: reuse
   the cached `FileParse`, never read or parse the file. (The fast path; dominates.)
2. **mtime/size differ → read + `xxhash64`.** If the hash matches the entry →
   **touched-only** (e.g. `git checkout` rewrote mtimes): reuse the parse, refresh
   mtime/size in the index.
3. **hash differs, or path not in index → re-parse** (read + tree-sitter), replace entry.
4. **path in index but absent from the walk → evict** parse + entry.
5. **`engine_version` mismatch → ignore the whole cache, full rebuild.** A new glia
   build may parse differently; a one-time full rebuild after upgrade is correct and
   expected. This folds in backlog **#10** (gmap cache content-version stamp): `is_stale`
   becomes content-aware via the FileIndex instead of mtime-only.

Regions (collapsed dirs) and `.md` docs stay full-rebuild in v1 — cheap (directory
probes / markdown chunking, no tree-sitter). Cache them later only if measured.

---

## 5. Serialization & storage

- **Format: `bincode` + `serde`.** Add `#[derive(serde::Serialize, serde::Deserialize)]`
  to `FileParse` (every field already has it). bincode is compact + fast; the cache is
  **regenerable**, so we deliberately *don't* use the rkyv/mmap zero-copy path (that's for
  the queryable `.gmap`, not a transient sidecar). Documented deviation from the
  store's rkyv convention, justified by transience.
- **Location: `<repo>/.ai/repo-graph/parse_cache.bin`** — sibling of the `.gmap` shards,
  already git-ignored. **One file**, rewritten wholesale each build. Serializing all
  `FileParse`s ≪ tree-sitter time, so wholesale rewrite is still a large net win and keeps
  us inode-safe.
- The `.gmap` `FORMAT_VERSION` (rkyv node/edge layout) is **untouched** — lower risk.
  Only the new sidecar carries a version, and the manifest is unchanged.

---

## 6. API surface (the three consumers)

```rust
// engine — the core is in-memory + cache-threaded:
pub fn generate_one(repo: &str) -> Result<GenerateResult, String>;            // clean build (unchanged)
pub fn generate_one_with_cache(repo: &str, cache: &mut ParseCache)            // reuse + update cache in place
        -> Result<GenerateResult, String>;
pub fn generate_one_incremental(repo: &str) -> Result<GenerateResult, String>;// disk: load → build → save cache
```

- **CLI / pyo3 (disk-backed):** `generate(repo, incremental=True)` (default on) →
  `generate_one_incremental`. `incremental=False` → `generate_one` (clean). A
  `glia build --no-incremental` flag mirrors it.
- **neuropil (in-process, no `.gmap`):** holds a `ParseCache` across edits and calls
  `generate_one_with_cache(repo, &mut cache)` → owned `MergedGraph`, tree-sitter skipped
  for unchanged files, zero disk. This is exactly neuropil's "owned MergedGraph, no
  rkyv" ask — the in-memory cache plays the role of `prev`.
- **Engram (`export-engram --since`):** the `FileIndex` (path→hash) is the diff key.
  `--since <prev parse_cache.bin | git ref>` compares indexes → changed files → emit only
  those symbols. Follow-on (§8 Phase 2c); the index is designed to support it now.

---

## 7. Determinism & the byte-identical test (the gate)

Incremental output equals a clean build **iff** (a) `FileParse` round-trips losslessly
through bincode (it's plain data — holds), (b) NodeIds are content-hashed (already
deterministic), and (c) the build/resolve/store path is order-deterministic.

(c) is the real bar. WP-0 already killed HashMap-iteration-order in node resolution;
incremental is a **forcing function** for the rest. The acceptance test:

```
clean  = generate_one(fixture)                  → serialize shards → bytes_A
warm   = generate_one_incremental(fixture)      (cold cache, full build)
edit one file
inc    = generate_one_incremental(fixture)      → serialize shards → bytes_B
assert bytes_A == bytes_B                         // byte-identical
assert reparsed_count == 1                        // only the edited file
```

Any diff is a residual determinism bug (edge ordering, HashMap-ordered tie-break) — we
fix it, same spirit as WP-0. That hardening is a feature of shipping incremental.

---

## 8. Phasing (LOC, glia style — markers + tests each)

- **Phase 2a — parse-cache core (~150–220 LOC).** serde on `FileParse`; `ParseCache`/
  `FileEntry`; stat→hash→parse change detection; `build_graphs_for_repo` cached variant;
  `generate_one_with_cache`. Marker: `[incremental] reused H/T parses, reparsed M,
  evicted D (cache fresh|stale:version|absent)`. Tests: hit/miss, touched-only reuse,
  deletion eviction, version invalidation.
- **Phase 2b — disk + surfaces (~80–120 LOC).** bincode load/save `parse_cache.bin`;
  `generate_one_incremental`; pyo3 `generate(incremental=)` + `glia build
  --no-incremental`; content-aware `is_stale` (#10). Test: touch 1 file → 1 reparse +
  **byte-identical** `.gmap`.
- **Phase 2c — Engram `--since` (~80–120 LOC, follow-on).** FileIndex diff → changed-file
  export. Gated on the Engram session's need.
- **Phase 3 — CUT.** LiveGraph/MutableLayer/streaming; revisit only if a per-token
  use case returns.

Total for shippable incremental (2a+2b): ~230–340 LOC, no resolver changes, no `.gmap`
format change.

---

## 9. Efficiency notes / future optimization (YAGNI until measured)

- v1 re-runs `build_<lang>` + resolvers fully. If profiling a huge repo shows *that* is
  the bottleneck (not parsing), add a **per-language RepoGraph cache**: if no file of
  language L changed, reuse L's prior built `RepoGraph` (it's a pure function of its
  `FileParse`s). Cross-graph resolvers still re-run globally. Strictly additive; defer.
- Cross-graph resolvers always re-run (they're global by nature). They're cheap; not
  worth invalidating until proven otherwise — and invalidation is exactly the complexity
  this design was chosen to avoid.

---

## 10. Decisions (made, with rationale — release gate can override)

| # | Decision | Choice | Why |
|---|----------|--------|-----|
| 1 | Hash primitive | **xxhash64** | Already the store's primitive; no new dep. GR-4 said BLAKE3, but consistency + zero-dep wins for a content-equality check (not crypto). |
| 2 | Cache vs in-place patch | **Parse cache + full rebuild** | Byte-identical by construction; zero resolver changes; avoids the "bulk of the work" registry. |
| 3 | Serialization | **bincode + serde** | Transient/regenerable cache; rkyv/mmap is for the queryable `.gmap`, not this. |
| 4 | Cache layout | **Single `parse_cache.bin`, wholesale rewrite** | Inode-safe (MFS-quota lesson); matches "rebuild-whole-file"; rewrite ≪ parse cost. |
| 5 | Invalidation stamp | **`CARGO_PKG_VERSION`** | Correct after any upgrade (parsers may change); folds in #10. One-time rebuild post-upgrade is acceptable. |
| 6 | v1 scope | **2a+2b (parse-only speedup), byte-identical test as gate** | Captures the dominant cost; per-language-graph cache deferred to §9. |

## 11. glia-style conformance

- **Extract-vs-resolve split** — the cache boundary *is* the split: cache extraction
  (`FileParse`), always re-resolve.
- **Write-once / rebuild-whole-file** — cache rewritten wholesale; no in-place mutation.
- **Locked formats** — `.gmap` `FORMAT_VERSION` + registries untouched; new sidecar owns
  its own version.
- **Domain-agnostic core** — cache lives in `engine` over code-domain `FileParse`; `core`
  stays clean.
- **fired_on marker + tests** — greppable `[incremental] …` line; byte-identical +
  reparse-count tests are the gate.
- **Honest failure** — version-mismatch / absent-cache fall back to a full build and say
  so in the marker.
```
