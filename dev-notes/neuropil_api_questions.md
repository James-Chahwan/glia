# Neuropil ↔ glia integration — API questions to resolve before Phase 2

Phase 2 (per-file delta) and Phase 3 (live shadow journal) design depends on what neuropil ACTUALLY calls. The plan in `incremental_gmap_plan.md` covers a hypothetical surface; this doc enumerates the questions to settle with neuropil before committing to specifics.

## 1. Read pattern

- **One-shot queries** (find symbol, walk callers, get PPR scores) — does neuropil cache results or re-query?
- **Streaming queries** during inference — per-token? per-batch? per-prompt?
- **Bulk pre-fetch** — neuropil reads entire shard once, then operates from in-memory copy?

Implementations diverge: Phase 1 + Phase 2 sufficient for read-heavy / cache-able; Phase 3 (live shadow) only needed for sub-100ms incremental visibility.

## 2. Write trigger

- **Filesystem watcher** — does neuropil watch glia's `.gmap` and react to mtime changes?
- **Explicit call** — does neuropil call `apply_file_deltas()` directly when it knows code changed?
- **Push from glia** — does glia notify neuropil (via socket, pipe, callback) when the graph updates?

If watcher → Phase 1 + Phase 2 is enough (just write fewer files, atomic-rename guarantees correctness).
If explicit call → API shape needs to be designed (sync vs async, batched vs single-file).
If push → glia needs an event-emitter abstraction.

## 3. Sync vs async

- **Sync read**: `let node = graph.get_node(id)` — blocking, returns when value resolved
- **Async read**: `let node = graph.get_node(id).await` — for use inside an async task
- **Streaming subscription**: `for update in graph.subscribe() { ... }` — long-lived stream of changes

Sync is simplest. Async needed if neuropil's inference loop is async-driven and needs to interleave queries with other I/O. Streaming is the gold standard but pulls in pub-sub complexity.

## 4. Mutability

- **Read-only consumer** — neuropil never writes to the graph
- **Read-write consumer** — neuropil can add nodes (e.g. "I observed this runtime type") or edges (e.g. "this call is dominant in production traffic")
- **Shadow-write** — neuropil writes to its own private layer, glia's shard is unchanged

Phase 2 supports the first two; Phase 3's MutableLayer supports the third trivially.

## 5. Concurrency

- Single-reader, single-writer (glia builds, neuropil reads) — easy
- Multi-reader, single-writer — needs RwLock or arc-swap pattern
- Multi-writer (multiple agents writing concurrently) — needs serialization or CRDT

Most likely: single-reader (neuropil), single-writer (glia build pipeline). Multi-reader if multiple neuropil instances share a graph.

## 6. Failure semantics

- **Eventual consistency OK**: neuropil tolerates stale reads if the write is in flight
- **Read-your-writes**: after `apply_file_deltas` returns, the next read must reflect the delta
- **Linearizable**: all readers see the same total order of writes

Atomic file rename (Phase 1) + mmap re-open gives eventual consistency. Read-your-writes needs an explicit barrier (e.g. mmap pointer swap). Linearizable requires versioning + sequence numbers.

## 7. Schema evolution

- **Stable schema**: glia and neuropil agree on `.gmap` layout; changes require coordinated upgrades
- **Versioned schema**: manifest carries `schema_version`; loader rejects unknown versions
- **Forward-compatible**: glia can write a `.gmap` neuropil can read even at older schema version (additive changes only)

Phase 1 already has `MANIFEST_VERSION` constant. Need to confirm neuropil reads + respects it.

## 8. Performance budget

- Per-query latency target: **<10ms? <1ms?** (depends on use case)
- Throughput target: **K queries/sec? per-token? per-batch?**
- Memory ceiling: **how big can mmap'd `.gmap` get? bounded by host RAM? by VRAM if used by inference?**

Sub-10ms is what we currently have for 10K-node graphs (PPR profile says 4.59ms median). Sub-1ms would require precomputed-cache + SIMD; possible but not built.

## 9. Live-injection use case (Phase 3 specific)

- Does neuropil want to **inject attention bias at L25 during inference**, parameterized by which symbol's embed to bias toward?
- If yes — neuropil needs glia to expose: (a) qname → embed lookup, (b) compose into model's residual stream at decode time
- Currently lens-attention-bias bin does this STANDALONE; need an embeddable Rust crate / FFI layer for inline use

This is the most speculative use case. Confirm before designing.

## 10. Cross-process or in-process

- **In-process (Rust ↔ Rust)**: neuropil links glia as a crate, calls `.gmap` APIs directly. Sub-microsecond overhead.
- **Cross-process via shared mmap**: neuropil reads the `.gmap` file directly via its own mmap; no IPC needed for reads
- **Cross-process via socket/pipe**: full IPC, serialization costs per call

Default assumption: same Rust workspace, in-process. Cross-process if neuropil is a long-lived daemon with multiple language clients.

## Next steps

1. Get answers (or strong opinions) on Q1-Q3 first — they shape Phase 2's API surface
2. Q4-Q5 determine MutableLayer (Phase 3) requirements
3. Q6 + Q7 are correctness contracts; can be deferred to integration testing
4. Q8 is a measurable target — once API shape is settled, can be benchmarked
5. Q9 + Q10 only matter if Phase 3 / cross-process is required

Until these are answered, Phase 2 remains spec'd but not implemented. Phase 1 (skip-when-unchanged) is sufficient for most read-side use cases.
