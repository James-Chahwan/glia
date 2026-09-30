# R2 — `glia watch` (0.5.2 bet A)

Research only, at glia `2170ff8`. Line numbers drift; find symbols by name. Timings are from the DEBUG binary
(`GLIA_NO_PERSIST=1 ./target/debug/glia build`, as the brief requires) on git-archive copies under
`research/probe/r2-*`. Raw timestamped logs: `research/timing/*.log`; the phase splitter is `research/phases.py`
and the driver `research/bench_watch.sh`. Release numbers come later from 0.5.1's CA-9 (per-phase build timers).

## 1. What a one-file change reruns today

The only incremental seam is the per-file parse cache. Everything after the parse reruns in full.

| step | where | scope | cached today? |
|---|---|---|---|
| parse-cache load (bincode, whole sidecar) | engine/src/build/mod.rs:141 `ParseCache::load` | repo | n/a |
| walk + read every file | mod.rs:203 `walk_source_files` (walk.rs:217) | repo | no |
| route + parse | assemble.rs:152 `parse_repo_files` (route.rs:43); cache.rs:323 `get` | per file | **yes** (content hash) |
| const-table scan | assemble.rs:37-58 `build_const_table` | per file, folded per repo | no |
| post-cache grafts | grafts.rs:55-178 `apply_post_cache`: ModuleQnames plan (:71, the whole file list), endpoint fold (:82, repo const table), Next pages (:86), queue const fold (:90, sequential), RPC needles (:100, per file with a parse, against the BUILD-wide proto service set), owner qualify (:167), IMPORTS filter (:175, whole repo's declarations) | repo-wide inputs | no |
| per-language graph build | lang_build.rs:74-175 `build_language_graphs`; TS family last on the calling thread (:151-158); `build_go` = merge → symbol table → imports → calls → refs → implicit implements (graph/src/build.rs:82-110) | per language group | no |
| region / project / docs graphs | mod.rs:231-241 | repo | no |
| CODE_PASSES: 15 resolvers, external edges, 8 post-passes, evidence fill, sort | engine/src/profile.rs:110-238, run by mod.rs:243-244 | global (every graph) | no |
| external cells | mod.rs:245 | global | no |
| parse-cache save (serialise + compare) | mod.rs:143, cache.rs:456 (LC.11 write-if-changed) | repo | skips the write only |
| persist | persist.rs:199 `persist_result`; store/src/layout.rs:479-490 skips unchanged shard WRITES, but serialises every shard to hash it | per shard | writes only |

Locality:

- **Per file** (content-hash keyable): parse, const scan, RPC needle output (given the service set, the twirp flag
  and the file's module id).
- **Per repo, cross-file**: the ModuleQnames plan (a pure function of the walked list; LB.9b / LB.13 same-stem
  naming), the const table, the endpoint / queue folds, the owner index (roots), the IMPORTS filter, `rust_crates`,
  `ts_aliases`, the Go module set. In a multi-repo build the proto service set is the union across repos
  (mod.rs:379-382), so a `.proto` edit in repo A changes needles in repo B.
- **Per language group**: `build_*`. A group's graph is a pure function of its POST-graft parses plus
  `rust_crates` / `ts_aliases` (lang_build.rs:178-195). The cache holds PRE-graft parses (grafts.rs:79-81).
- **Global**: every resolver indexes all graphs by kind (graph/src/resolvers/mod.rs:61-63, 81-86). Post-passes
  mutate node confidence and cells in place: `demote_unmatched_http_nodes` records `pass_undo`
  (graph/src/merged.rs:25-39, undo at :55-71); `fill_test_cells` and `tag_synthetic_provenance` add cells.

The key enabling fact: `merge_layouts` already re-runs `CODE_PASSES` over graphs that went through them once
(undo, drop `resolver:*` / `pass:*` cross edges, re-run: engine/src/merge.rs:224-227, :245, :310) and
`engine/tests/merge_layouts.rs:165 merge_equals_building_together` asserts the bytes equal one joint build.
So "reuse some graphs, recompute the global tail" is already proven byte-identical for layouts.
`dev-notes/incremental_gmap_plan.md:217-229` (§9) deferred exactly this per-language RepoGraph cache as "YAGNI until
measured".

## 2. Measured: warm incremental vs clean (debug binary, 16 cores)

Median of 3 runs per row. "warm0" = incremental, no change. "warm1" = incremental after appending one comment line
to one file (grpc-go `server.go`; glia `engine/src/merge.rs`), so exactly one file reparses. Phases are cut at the
stderr markers (`[parallel] walk`, `routed`, `[imports] local-filter`, `language graphs on`, `[doclink]`,
`[passes] domain=code`, `[gmap] meta`, `[gmap] wrote`).

| run | load+walk | route/parse | const+grafts | lang-build | resolvers + early post | evidence + late post + sort | cache save | persist | total (s) |
|---|---|---|---|---|---|---|---|---|---|
| grpc-go clean, default threads | 0.03 | 2.16 | 0.29 | 1.58 | 0.21 | 0.44 | 0.01 | 0.36 | 5.09 |
| grpc-go warm0, default | 0.25 | 0.03 | 0.29 | 1.56 | 0.20 | 0.42 | 0.17 | 0.34 | 3.30 |
| grpc-go warm1, default | 0.25 | 0.17 | 0.29 | 1.57 | 0.20 | 0.42 | 0.18 | 0.33 | 3.51 |
| grpc-go clean, GLIA_THREADS=1 | 0.03 | 18.46 | 1.85 | 1.59 | 0.20 | 0.44 | 0.00 | 0.37 | 22.97 |
| grpc-go warm1, GLIA_THREADS=1 | 0.25 | 0.23 | 1.85 | 1.54 | 0.20 | 0.43 | 0.18 | 0.34 | 5.06 |
| glia clean, default | 0.15 | 3.14 | 1.92 | 2.61 | 0.28 | 0.68 | 0.01 | 0.53 | 9.40 |
| glia warm0, default | 0.56 | 0.05 | 1.90 | 2.60 | 0.28 | 0.67 | 0.33 | 0.50 | 6.96 |
| glia warm1, default | 0.57 | 0.09 | 1.91 | 2.78 | 0.29 | 0.72 | 0.38 | 0.53 | 7.32 |
| glia clean, GLIA_THREADS=1 | 0.17 | 27.67 | 4.44 | 2.98 | 0.28 | 0.70 | 0.00 | 0.55 | 37.34 |
| glia warm1, GLIA_THREADS=1 | 0.58 | 0.20 | 4.43 | 2.73 | 0.27 | 0.68 | 0.34 | 0.50 | 9.78 |

grpc-go: 938 routed files, 901 Go, one Go group, 35,426 edges, 30 MB layout. glia: 1,630 routed files, 19 groups,
56,738 edges.

Where the warm time goes:

- **Parsing is solved.** A one-file change reparses in 0.09-0.23 s. The rest (3.3 s grpc-go, 7.2 s glia) is the
  post-parse pipeline, which the parse cache never touches.
- **One language group dominates.** grpc-go's single Go build is 1.57 s of 3.51 s. On glia the Rust group is
  2.57 s of the 2.78 s lang-build (`[roles] folded 2` at 2.738 → `[evidence-lines] lang=rust` at 5.305 in
  `timing/glia-default-warm1-1.log`). Rebuilding "only the changed group" saves little when the edit is in the
  dominant language, and a lot when it is not.
- **Grafts are not cached.** glia's 1.9 s const+grafts is mostly the LA.4 queue const fold (`[nav-pages]` 0.712 →
  `[queue-const]` 2.090 = 1.38 s). It is the one graft not on the pool: it loops `files` sequentially and re-scans
  every queue-holding file (grafts.rs:337-420). The RPC needle pass re-parses every text-gated file on every build
  (703 of 938 grpc-go files, rpc_needles.rs:80-105). At GLIA_THREADS=1 these grafts cost 1.85 s (grpc-go) and 4.4 s
  (glia).
- **Evidence fill is global.** `fill_evidence_sites` over every edge is 0.42 s (grpc-go) and 0.70 s (glia)
  (`[contract-link]` → `[evidence]` in the glia log).
- **Fixed per-invocation costs** that a warm process avoids: cache load 0.22-0.41 s, cache save 0.17-0.38 s (serialise
  and compare, even with nothing to write), re-reading every file in the walk.
- A first clean run on a cold 284 MB debug binary took 9.4 s against a 5.0 s steady state
  (`timing/grpc-clean-default.log`). Take medians only.

The pragmatic floor for a one-file edit in the dominant group (debug estimates, not measurements): grpc-go ≈ group
1.57 + resolvers 0.20 + incremental evidence ~0.05 + memoised grafts ~0.05 + delta ~0.1 ≈ 2.0 s (against 3.5 s).
glia Rust edit ≈ 3.2 s (against 7.3 s). glia edit in a minor group (a Python file under `bench/`) ≈ 0.7 s (against
7.3 s). A no-change event costs ~0, against 3.3-7.0 s today. Going below "one group rebuild" needs intra-group
incrementality (§5, A2).

## 3. Consumers today

- **repo-graph MCP.** watchdog runs in-process (repo_graph/watcher.py:80-119). The debouncer calls `on_change()`
  with no paths (:102-113); `_watch_rebuild` (server.py:134-141) calls `_build_graph`, which runs
  `glia_py.generate(target, incremental=True)` and `save_to_default` (server.py:108-131). Every write event costs a
  full load + walk + build + passes + save + persist. The 1.2 TB read-event loop (2026-09-19) came from this path;
  WRITE_EVENTS (watcher.py:34) is the wrapper's fix.
- **neuropil** builds single-repo with `generate_one` (neuropil state.rs:443). The engine offers
  `generate_one_with_cache` for its hot reload (engine/src/build/mod.rs:149-158).
- **TODO.md:34, decided:** "There is no central daemon (`glia serve`). The shape is the CLI plus a post-commit
  hook." So watch is either a foreground CLI that dies with its terminal, or a library the consumer's own
  long-lived process embeds (the MCP server already is one).

## 4. Prior art

- **Salsa** (rust-analyzer): queries K → V, a revision counter, red-green verification, backdating (early cutoff:
  a re-executed query that returns the same value does not invalidate its dependents), and durability classes
  (skip validating queries that read only high-durability inputs).
  https://salsa-rs.github.io/salsa/reference/algorithm.html, https://github.com/salsa-rs/salsa,
  https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html
- **Glean** (Meta): stacked immutable databases. An increment hides the facts owned by changed units, and derived
  facts get ownership `O1 && ... && On`. Its incrementality targets whole-repo re-index cost, not IDE latency.
  https://glean.software/docs/implementation/incrementality/, https://glean.software/blog/incremental/
- **Stack Graphs** (GitHub, archived 2025-09-09): file-local partial resolution graphs. This is 0.5.3's bet and the
  natural home of intra-group incrementality.
- **notify / notify-debouncer-full** is the standard Rust watcher. inotify emits 3-5 events per save.
  https://github.com/notify-rs/notify, https://docs.rs/notify-debouncer-full/

## 5. Design options

**A1 — pragmatic: warm session, group-level early cutoff.** A `WatchSession` holds the in-memory ParseCache, the
walked file table (path → hash, mtime and size, source), the repo-level inputs, each language group's PRE-pass
RepoGraph with a fingerprint of its post-graft inputs, and the last post-pass MergedGraph. An update:

1. Classify each changed path by durability. HIGH: `go.mod`, `tsconfig*.json`, Cargo / npm / pyproject manifests,
   `.gitignore`, `.glia/overlay.toml` and other `.glia` inputs, `.proto` (the build-wide service set). A HIGH change
   rebuilds everything from the warm cache. LOW: source files.
2. Re-read only the changed files. Recompute the ModuleQnames plan when a file is added or removed, and re-route every
   file whose module id moved.
3. Parse the cache misses.
4. Re-run the grafts over all parses. Per-file memos keep this near the changed file's cost: RPC needles keyed by
   (content hash, service-set fingerprint, twirp flag, module id), const scan keyed by content hash, queue fold
   keyed by (content, const-table fingerprint).
5. A group is dirty when any post-graft parse in it differs from last time. Rebuild the dirty groups and clone the
   clean ones' cached pre-pass graphs.
6. Reassemble in clean-build order: sorted languages, JVM join, TS last (lang_build.rs:88-158); parse errors in
   clean-build order.
7. Run CODE_PASSES and the external cells.
8. `located_delta(prev, next)` (engine/src/delta.rs:158) gives the answer. It can be scoped to dirty shards plus
   cross edges.
9. Optionally persist. Unchanged shard writes are skipped already.

A1 is correct by construction to the same degree the merge path is: everything global reruns. The early cutoff is at
group granularity. Its weakness: a single-language repo still rebuilds its one big group.

**A2 — A1 plus a body-only fast path inside a group.** When the changed file's declaration surface is unchanged
(node ids, kinds, nav names, field types, imports), keep the symbol table, drop that file's old edges and
re-resolve only its calls and refs. Needs (a) canonical intra-edge order in RepoGraph (today intra edges are
emission-ordered; only cross edges get `canonical_edge_cmp`, merged.rs:84-86), a one-time content change of every
shard's bytes; (b) a partial-resolution entry in each of the 8 builders (graph/src/build.rs:33-238, calls.rs);
(c) a declaration-surface fingerprint per FileParse. This is Salsa's backdating at the "declarations" query,
hand-rolled, and the same file-local seam Stack Graphs wants in 0.5.3.

**A3 — Salsa-style.** Recast walk / parse / graft / symbol-table / per-file-resolution / per-mechanism resolver
indexes as salsa queries with durability for manifests. Maximum reuse, but it restructures 8 builders and 15
resolvers, adds the `salsa` crate (a Cargo.lock change moves PARSER_STAMP), and byte-identity then rests on
canonical assembly order rather than on "the same code ran". 3-6k LOC.

## 6. Recommendation

**Ship A1 in 0.5.2. Defer A2 to 0.5.3 with Stack Graphs. Do not do A3.**

Reasons:

1. The measurements put 90-97% of a warm one-file rebuild in post-parse phases. A1 removes the fixed costs (load,
   save, walk, un-memoised grafts, global evidence fill) and every non-dirty group. That is ≈40-55% on the two
   repos measured and ≈90% for minor-language edits.
2. A1 reuses a proven byte-identity argument (merge re-runs the passes) and needs no resolver changes.
3. A2's canonical-order change and per-builder partial resolution are exactly 0.5.3's Stack Graphs seam; doing them
   twice is waste.
4. A3's cost is out of proportion before CA-9 shows release-build post-parse phases matter on big repos.

## 7. API shape

- **Library** (`glia_engine::watch`, a new public module slot):
  - `WatchSession::open(root, &BuildOptions) -> Result<Self, String>` loads the disk sidecar once and builds.
  - `apply(&mut self, changed: &[PathBuf]) -> Result<WatchUpdate, String>`, for a caller that owns its own watcher
    (repo-graph's watchdog).
  - `poll(&mut self) -> Result<Option<WatchUpdate>, String>` stats the file table and re-lists directories: no
    watcher dependency.
  - `result(&self) -> &GenerateResult`, `persist(&self, dir)`, `save_cache(&self)`.
  - `WatchUpdate { seq, files: CacheDiff, groups_rebuilt, full_rebuild: Option<reason>, nodes: Vec<DeltaNode>,
    edges: Vec<DeltaEdge>, counts, phase_ms }`. `phase_ms` comes from CA-9.
- **pyo3** (`py/src/watch.rs`): `glia_py.WatchSession(repo, overlay=True)` with `.apply(paths) -> dict` (a native
  dict per LD.2, the rule that answers are native dicts / lists and only `*_json` returns text), `.poll()`,
  `.snapshot() -> PyGraph` and `.persist()`. PyGraph owns its MergedGraph (py/src/graph.rs:27-28), so a snapshot is
  a clone per update. That matches what the wrapper pays today with a fresh `generate`. The wrapper's Debouncer must
  accumulate paths instead of dropping them (watcher.py:102-113).
- **CLI** (`glia watch <repo> [--persist] [--poll-ms N] [--debounce-ms N]`): a foreground loop that prints one
  NDJSON object per update on stdout, in the delta rows' existing sorted order (delta.rs sorts nodes by
  `(change, qname, id)` and edges by `(change, category, from, to)`). stderr carries `[watch]` fired_on markers.
  It exits on SIGINT or stdin EOF. There is no daemon, no socket and no pidfile, which honours TODO.md:34.

## 8. The incremental == clean guarantee

- Gate: `engine/tests/watch_parity.rs`. Scripted edit sequences, one per input class: body edit, declaration edit,
  file add / delete / rename, same-stem sibling add (plan change), go.mod, tsconfig paths, Cargo member, overlay,
  `.proto`, markdown. Seeded SplitMix64 random sequences run over the fixture repos. After every step, the persisted
  layout bytes must equal a clean `generate_one` + `persist_result` of the same tree. The existing
  `byte_identical.rs:81 incremental_build_is_byte_identical_to_clean_on_disk` is the one-shot version of this.
- Also asserted: parse_errors order and `pass_undo`, which are in the manifest via `layout_meta`.
- Markers: an update prints only the rebuilt groups' per-build lines (`[evidence-lines]`, `[recv]` inputs...). That
  is a declared marker difference. The graph bytes do not change.

## 9. Components (LOC, crate)

Core:

- Build-stage refactor (engine/src/build/*): expose walk reuse, per-repo assembly over an injected file set, and a
  callable per-group build; reassemble in clean order. 200-350 LOC, `glia-engine`.
- `engine/src/watch.rs`, the WatchSession: durability classes, file table plus stat-poll, group fingerprints and
  cache, error / marker ordering, passes and external cells, scoped `located_delta`, `[watch]` markers. 550-850 LOC,
  `glia-engine`.
- CLI `glia watch` (cli/src/cmd/watch.rs plus `cli/surface/watch.txt`). 220-350 LOC, `glia-cli`.
- pyo3 `WatchSession` (py/src/watch.rs plus `py/api_surface/watch.txt` and a surface test). 150-250 LOC, `glia-py`.

Speed-ups:

- Graft memos plus the queue const fold on the pool (grafts.rs, rpc_needles.rs, assemble.rs). 200-350 LOC,
  `glia-engine`. The pool half helps every build.
- Split the evidence fill: intra edges filled at group build and cached with the group; cross edges in Finalize.
  Must stay byte-identical. 120-220 LOC, `glia-engine` (passes.rs, lang_build.rs).

Tests:

- Parity sequences, random edits, CLI NDJSON shape, pyo3 surface. 450-750 LOC.

Totals: core 1,120-1,800; speed-ups 320-570; tests 450-750. **All in: 1,890-3,120** (§4's 800-1,500 covered the core
only, without tests). A2, if pulled forward: +1,150-1,900. A3: 3-6k.

## 10. 0.5.1 packets it needs first

- **CA-9 per-phase build timers.** `WatchUpdate.phase_ms`, and release-build proof that the post-parse phases
  dominate on big repos. The numbers above are debug.
- **CA-7 TS family inside the engine pool.** It rewrites the group scheduling in lang_build.rs:130-158; watch's group
  cache must be built on the post-CA-7 ordering.
- **CD-7 FORMAT_VERSION 3** (EVIDENCE interning, CODE cells as spans, lz4 parse cache). The parity gate compares
  format-3 bytes. The lz4 cache and the interned evidence change what the session holds and what the evidence split
  must reproduce.
- **CE-2 shared / remote cache.** It adds per-entry content addresses and cache import / export to cache.rs; the
  session's in-memory cache must use the same entry API.
- **CD-5 time-travel graph** ("one incremental build per rev") would reuse the session as its builder if it lands
  first. It is a coordination point, not a hard dependency.
- The next 0.5.2 wave 0 declares the `watch` slots (engine, py, cli). 0.5.1's C0.* does not.

## 11. Open questions only James can answer

1. How should `glia watch` detect changes? Option (a): stat-poll the walked file table plus periodic directory
   re-lists. No new dependency, and deterministic. Option (b): the `notify` crate. Any Cargo.lock change moves
   PARSER_STAMP once, and event storms are the class of bug behind repo-graph's 1.2 TB loop.
2. Should `--persist` write the layout after every update? That keeps the MCP cold start and hooks fresh but costs a
   full serialise per update (0.33-0.53 s debug). The alternative is persisting on exit or on an interval.
3. Is multi-repo watch (`--with`) in 0.5.2, or single-repo only? The proto service set is build-wide
   (mod.rs:379-382), so it touches every repo.
4. Does the repo-graph wrapper adopt the embedded WatchSession (a handoff item for its session), or keep calling
   `generate(incremental=True)`?
5. A2 (body-only fast path) in 0.5.2, or with Stack Graphs in 0.5.3?

## 12. Spikes worth running before packets

- **S1.** After CA-9, run the same table on the release binary and on the largest local repos (quokka-stack, Kina,
  glia, grpc-go), plus one 20k+-file monorepo, at GLIA_THREADS=1 and default. Decides whether the speed-ups are
  core.
- **S2.** Group-graph clone cost vs rebuild. Re-run CODE_PASSES over post-pass graphs after `undo_pass_mutations` on
  glia and grpc-go, as merge does; if the bytes are identical, the session can skip keeping pre-pass clones.
- **S3.** A throwaway A1 prototype on a scratch branch: one-file edit parity plus timing on grpc-go and glia.
- **S4.** Replay the last ~200 commits of glia and quokka-stack as file edits and count the share whose declaration
  surface is unchanged. Sizes A2's payoff.
- **S5.** Put the queue const fold on `par_map_ordered`: a measured 1.38 s debug on glia, sequential today. This may
  belong in 0.5.1 next to CA-9, since it speeds every build.

## 13. Risks

- **Single-language repos** keep one big group. On grpc-go the floor stays ≈ the Go build (1.57 s of 3.51 s debug).
  A2 or Stack Graphs is the only fix.
- **Hidden cross-file inputs** are the audit-2026-06-10 #2 / #3 bug class (repo identity, go.mod set invisible to
  per-file hashes). Anything a group build reads besides its parses (`rust_crates`, `ts_aliases`, roots, overlay,
  the service set) must be in its key. The parity matrix must exercise each input class.
- **Order-sensitive outputs**: shard index = group order, parse_errors order in the manifest, `pass_undo`. A slip is
  a silent byte drift.
- **Memory**: pre-pass groups plus the current graph plus the previous graph for the delta is ≈3× one graph. The MCP
  process already sat at 1 GB RSS (2026-09-20 measurement).
- **Markers** differ between a watch update and a clean build. Declare it, and never gate on marker equality.

## 14. Side findings (not watch-specific)

- The LA.4 queue const fold is sequential and costs 1.38 s debug on glia (grafts.rs:337-420). Every build pays it.
- RPC needles re-parse every text-gated file on every build (rpc_needles.rs:80-105). Nothing caches them.
- A no-change incremental build still spends 0.17-0.38 s serialising the parse cache to compare it.

Sources: https://salsa-rs.github.io/salsa/reference/algorithm.html · https://github.com/salsa-rs/salsa ·
https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html ·
https://glean.software/docs/implementation/incrementality/ · https://glean.software/blog/incremental/ ·
https://github.com/notify-rs/notify · https://docs.rs/notify-debouncer-full/
