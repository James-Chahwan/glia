# Incremental .gmap rebuild — design plan

Status as of 2026-05-22: Phase 1 shipped (commit c3e74a7). Phase 2 + 3 in planning.

## Phase 1 — skip-when-unchanged (shipped)

Per-shard + manifest hash check before write_atomic. If serialized container bytes match the prior manifest's `content_hash` AND the file exists, skip the write. Verified by `write_sharded_skips_unchanged_shards_on_rewrite` test.

Effect: no-op rewrites cost ~3-5ms (hash compute) instead of ~50-200ms (write + fsync). Consumers watching mtime don't get false wakeups.

## Phase 2 — per-file delta (PLAN)

Goal: edit one Python file → 50-100ms graph update (vs 5-10s full reparse).

### API surface

```rust
pub struct FileDelta {
    pub file_path: PathBuf,           // e.g. "src/marshmallow/fields.py"
    pub shard_name: String,            // which shard owns this file (e.g. "python")
    pub new_items: ExtractedItems,     // re-parsed output for this file
}

pub fn apply_file_deltas(
    dir: &Path,
    deltas: Vec<FileDelta>,
) -> Result<Manifest, StoreError>;
```

Caller responsibilities:
- Detect changed files (file watcher, git diff, etc — orthogonal)
- Re-parse changed files via the relevant language parser
- Bundle (file → ExtractedItems) pairs

Glia responsibilities:
- Load existing shard(s) for the named shards
- Drop nodes whose `cells[POSITION].file == file_path` for each delta
- Drop edges referencing dropped nodes (both endpoints checked)
- Add new nodes/edges from `new_items`
- Re-run **bounded** intra-repo resolution (only walk BFS one-two hops from changed nodes — most calls within a file are intra-file)
- Re-run cross-graph resolvers (HttpStack, GraphQL, etc) for the affected shards — each resolver is idempotent + can be invalidated independently
- Re-serialize and write_atomic the affected shards (Phase 1's skip-when-unchanged handles the no-op case)

### Implementation details

1. **POSITION cell as origin tracker**: every node already carries a `POSITION` cell with `(file, start_line, end_line)`. This is the "owns this node" stamp. Edges don't carry POSITION directly but inherit from their `from` or `to` node's POSITION.

2. **Resolver invalidation registry**: each cross-graph resolver registers which `NodeKind`s it consumes/produces. When deltas modify a node of consumed kind, resolver re-runs over only the affected neighborhood.

3. **Concurrency-safety**: write_atomic + manifest version bump means in-flight readers see either old or new state, never torn. No locking needed.

4. **Test coverage**: per-delta unit tests + a "edit one function, all other shards unchanged" integration test.

### Estimated effort

3-5 days. Bulk of the work is the resolver invalidation registry — current resolvers don't declare their input/output kinds explicitly.

## Phase 3 — live shadow journal (PLAN)

Goal: per-token-steering use case from neuropil — sub-10ms reads with edits visible immediately.

### Architecture

LSM-tree-inspired:

```
                    ┌───────────────────────────────┐
                    │  MutableLayer (in-memory)     │
                    │  ┌─────────────────────────┐  │
                    │  │ pending_nodes: HashMap  │  │
                    │  │ pending_edges: HashMap  │  │
                    │  │ tombstones: HashSet     │  │
                    │  └─────────────────────────┘  │
                    └────────────┬──────────────────┘
                                 │ reads consult delta first
                                 ▼
                    ┌───────────────────────────────┐
                    │  ShardedMmap (immutable)      │
                    │  rkyv + mmap, zero-copy       │
                    └───────────────────────────────┘
                                 ▲
                                 │ compaction (background)
                    ┌───────────────────────────────┐
                    │  CompactionWorker             │
                    │  - every N seconds / M edits  │
                    │  - merges MutableLayer into   │
                    │    new ShardedMmap, swaps     │
                    │    pointer, clears delta      │
                    └───────────────────────────────┘
```

### API surface

```rust
pub struct LiveGraph {
    mmap: ShardedMmap,          // immutable snapshot
    delta: Arc<RwLock<MutableLayer>>,
    compactor: BackgroundWorker,
}

impl LiveGraph {
    pub fn open(dir: &Path) -> Result<Self>;
    pub fn insert_node(&self, node: Node);
    pub fn remove_node(&self, id: NodeId);
    pub fn upsert_edge(&self, edge: Edge);
    pub fn get_node(&self, id: NodeId) -> Option<NodeView>;
    pub fn ppr(&self, seeds: &[NodeId]) -> Vec<f32>;  // delta-aware PPR
    pub fn flush(&self) -> Result<()>;  // force compaction
}
```

### Hot-path read latency budget

- Delta HashMap lookup: ~50ns
- Mmap'd Archived<Node> deserialization: ~200ns
- PPR power iteration (precomputed cache miss): ~1-5ms for 10K-node graph
- **Total**: sub-10ms achievable IF PPR cache hits common queries.

### Implementation order

1. **MutableLayer + insert/remove/upsert API** — pure in-memory, no compaction (~2 days)
2. **Delta-aware reads** — get_node, get_edges_from/to consult MutableLayer first (~1 day)
3. **Compaction worker** — background thread, debounced trigger (~2 days)
4. **Atomic swap** — replace mmap pointer atomically; old readers finish on old snapshot (~1-2 days, tricky)
5. **PPR with delta** — power iteration that reads through MutableLayer (~1-2 days)
6. **Pressure tests** — write rate vs read rate, GC stability under churn (~2 days)

### Estimated effort

~1-2 weeks for stable v1.

### Open design questions

- **Tombstone scope**: per-node id or per-(shard, node-id)? Per-shard is cleaner but requires shard-aware deletion API.
- **Cross-shard edges with one tombstoned endpoint**: drop the edge in delta? Lazy filter at read time?
- **Compaction trigger**: time-based, count-based, or memory-pressure-based? Probably hybrid.
- **Reader-writer concurrency**: RwLock on MutableLayer is simple; lock-free MutableLayer (DashMap-style) better for high write rates.
- **What if compaction fails mid-flight?**: fall back to old snapshot; queue the delta for retry; alert via metric.

## Mapping to neuropil's needs (TBD)

What does neuropil actually need from glia at sub-10ms latency?

| Use case | Phase needed |
|---|---|
| One-shot graph queries (find symbol, walk callers) | Phase 1 is enough |
| Build-time graph generation w/ caching | Phase 1 + 2 |
| Live editor integration (LSP-like) | Phase 2 |
| Per-token latent injection during inference | Phase 3 |
| Streaming code analysis (CI bot watching commits) | Phase 2 |

Need to confirm with neuropil team which they actually want. The plan supports the most ambitious (Phase 3) but Phase 1+2 is enough for most uses.
