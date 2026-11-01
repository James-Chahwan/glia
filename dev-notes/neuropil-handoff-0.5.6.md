# neuropil handoff - glia 0.5.5 -> 0.5.6

For: neuropil. **Pin: `=0.5.6`** on the six glia path crates (`glia-py==0.5.6` for any Python tooling). Apply
after the 0.5.4 and 0.5.5 handoffs (`dev-notes/neuropil-handoff-0.5.4.md`, `dev-notes/neuropil-handoff-0.5.5.md`).

**Status: written at glia HEAD `d3a264bf` (after wave W12 of 0.5.6), before the bump.** The version bump and the
`v0.5.6` tag are James's release steps after these docs; take the pin **once James tags `v0.5.6`** (at HEAD the
crates read 0.5.5, so `version = "=0.5.6"` on a path dependency fails resolution until the tag).

**Where neuropil stands.** Read read-only at HEAD `b140d1e` (the commit every handoff since 0.5.3 read): its
root `Cargo.toml:112-117` still path-depends on the pre-0.5.0 names (`repo-graph-core` .. `repo-graph-projection-text`),
so the 0.5.0 handoff's rename (section 2 of `neuropil-handoff-0.5.4.md`) is still the first step. Act on
neuropil only.

0.5.6 finishes started work (plan `dev-notes/next-leap-0.5.6.md`, packets `dev-notes/leap-056-packets.json`,
waves W0-W13): GA watch (the body-only fast path, multi-repo watch, one memory budget), GB concept search over
flows, GD per-language precision and recall, GH the host graph (plugin hosts and environment hosts), GP build
speed and hygiene. No registry id (next free stays node 51 / edge 39 / cell 28), no `FORMAT_VERSION` (3) or
`MANIFEST_VERSION` (2) change, no Cargo.lock move in glia.

Sources, in order of authority: the compile check below; the code at `d3a264bf`; each packet's landed result
(its `breaking` text) and commit body; the `HANDOFF FROM ..` paragraphs routed to GZ.2 in
`dev-notes/leap-056-corrections.json`. The hash after a packet id is the glia commit that landed it.

---

## 1. The compile check at this commit

`CARGO_BUILD_JOBS=8 bash scripts/check-neuropil.sh`, run as-is against `../neuropil` and this glia tree (it
copies neuropil into `~/.cache/glia-consumer-check/`, applies the 0.5.0 rename to the copy, and runs
`cargo check --workspace --all-targets`; nothing under neuropil is written):

```
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under /home/ivy/.cache/glia-consumer-check/neuropil/src (0 unknown token(s) left)
[neuropil-check] note - neuropil still uses the pre-0.5.0 names; checked a renamed copy
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 36.1s against glia 0.5.5+p220fe51725f584ef (neuropil b140d1e, lock drift +45 -26)
```

- **`new=0`**: neuropil compiles against the 0.5.6 tree once the 0.5.0 rename is in. None of the Rust API
  changes in section 3 reaches a neuropil source file.
- **Lock drift +45 / -26** (0.5.4: +43 / -26). No fastembed, ort or tokenizers entry enters neuropil's lock.

## 2. Cargo: the pin

| where | change |
|---|---|
| `Cargo.toml:112-117` | after the 0.5.0 rename (`repo-graph-<x>` -> `glia-<x>`, projection-text feature `driver` -> `research`): `version = "=0.5.6"` on `glia-core`, `glia-code-domain`, `glia-graph`, `glia-engine`, `glia-activation`, `glia-projection-text` |
| `crates/neuropil-app/Cargo.toml` | nothing beyond the 0.5.0 rename |

**Stamps.** PARSER_STAMP / BUILD_STAMP moved at almost every packet: the first `generate_one` after the upgrade
reparses, and a `.glia/graph/` layout written by another build rebuilds once. Every shard's bytes change at
least once (GA.2, below), with no content change from that alone.

## 3. Rust API changes, against neuropil

None needs a neuropil edit today (section 1). These are the changes an in-process caller could meet:

| packet | change | who must act |
|---|---|---|
| GA.18 evidence re-stamps are delta rows (`fbeb57f8`) | `glia_activation` `DeltaOptions` gains `edge_cell: Option<CellTypeId>` and `edge_cell_key: Option<EdgeCellKey>` (`EdgeCellKey = fn(&CellPayload) -> Option<Vec<u8>>`, plus `whole_payload`). `DeltaOptions` is **not** `#[non_exhaustive]`: a struct literal without `..DeltaOptions::default()` stops compiling. `GraphDelta` gains `restamped_edges`, `DeltaCounts` `edges_restamped`, engine delta rows a `restamped` change and `rule` / `was_emitter` / `was_rule`; `RevDelta` gains `renames` | a `DeltaOptions { .. }` literal (none in neuropil at `b140d1e`); a `match` on a delta row's change string |
| GD.8, GD.11, GD.18, GD.31, GD.40, GH.6 (NavFact variants) | `glia_code_domain::NavFact` gains appended variants `ReExportHide` (GD.8 `d67f2698`), `ContainerRegister`, `ContainerInject`, `Mixin` (GD.18 `6115d1f3`), `EvmContract`, `EvmImportSite` (GD.31 `3bd4d914`), `IncludeMount` (GD.40 `6b19e684`), `HostScriptRef` (GH.6 `dfc62ecd`); GD.11 (`ec4946df`) writes a new `ExportLocal { local, "default" }` shape. `Mount` gains `Mount::Module` and a new public enum `IncludeTarget` (GD.40). NavFacts live only in the parse cache; `FORMAT_VERSION` stays 3 | an exhaustive `match` on `NavFact` or `Mount` (none in neuropil) |
| GD.25 nested `build/` / `out/` source dirs (`190d347d`) | `glia_code_domain::walk_gating`: new `gate_dir_in`, `is_build_output_home`, `is_build_or_out`, `IgnoreStack::root`, `GateCounts::record_named`; `GateCounts` gains the pub field `build_source`; `always_region("build")` / `always_region("out")` now return None | a `GateCounts { .. }` struct literal |
| GB.3 the fused semantic concept tier (`9bb8ca3d`) | `ConceptHit` drops `Eq` (it holds an `f64` `similarity`); `ConceptOptions` / `TraceOptions` gain `semantic` and `rank_cache` and compare their Arcs by pointer; `ConceptTier::Semantic`; `TraceAnswer.resolved_by` may read `concept_semantic` | a `HashSet<ConceptHit>` or `Eq` bound |
| GB.1, GB.2 (`4598c291`, `6eea7286`) | new `glia_engine::trace::{concept_flows, concept_flows_with_live, ConceptOptions, ConceptHit, ConceptTier, DEFAULT_CONCEPT_LIMIT}` and `glia_engine::find::{flows_through, find_flows, find_flows_with_live, FindFlows, FlowsThrough, FlowRef, DEFAULT_FLOWS_PER_HIT}`; `resolved_by` may read `concept` | optional (section 6) |
| GA.4, GA.9, GA.11, GA.12 watch (`6a52dff4`, `cbc88def`, `a0e52d65`, `2c204a31`) | `WatchUpdate` gains `body_only: Vec<String>`; `WatchSession::{open_many, roots, set_memory_budget, memory_budget, memory}`; `WatchMemory`; `glia_engine::memory::{MemoryBudget, BUDGET_ENV}` and a public `memory::rss_kb`; `WatchSession::open` reads `GLIA_MEMORY_BUDGET_MB` | none (all `#[non_exhaustive]` or additive) |
| GA.13, GA.14, GA.16 rules (`f2c8c3f5`, `60fca954`, `1fb0ec17`) | `glia_rules` `Options.max_bytes`, `RulesError::MemoryBudget`, `EvalStats.shortest_fixed`; engine `QueryOptions.byte_budget` (from `GLIA_MEMORY_BUDGET_MB`); the row budget charges intermediate bindings and its text says `bindings` | only if neuropil sets `GLIA_MEMORY_BUDGET_MB` or runs rules near the row budget |
| GA.2 canonical per-graph order (`af92cd26`) | `RepoGraph::canonicalize()` -> `CanonicalMoves`; every graph's intra edges and unresolved lists are stored sorted, so iteration over a graph's edges comes in (from qname, from kind, to qname, to kind), category, confidence, cells order | a view that depended on emission order |
| GA.3, GA.6, GA.7 body re-stitching (`4055c44c`, `f82594d1`, `617f47b0`) | `glia_graph::{rebuild_python_bodies, rebuild_ts_bodies, rebuild_go_bodies, BodyEdit, Refused, RebodyStats}`; no Rust fast path (FA.1: `Rust: FAIL`, `dev-notes/surface-replay-0.5.6.md`) | none |
| GP.4 pooled persist (`54144a43`) | `glia_store::{PersistPool, PersistTimes, write_merged_sharded_pooled}`; the `[timing] persist` line gains ` spans_ms= encode_ms= write_ms=` after `dir=` | a stderr parser ending at `dir=` |
| GH.2-GH.20 the host table | `glia_code_domain::hosts` (new module: `HostRow`, `Gate`, `Shape`, `Callback`, `LAUNCHERS`, `normalise_argv`, `host_command_qname`, `host_event_qname`, ..) | none |
| GH.10 (`db342162`) | `glia_engine::health::CODE_LIKE_EXTS` `[&str; 36]` -> `[&str; 32]` | a caller naming the array type |
| GA.1 surface replay (`4e8edc9d`) | `glia_engine::cache::{Part, surface_class, surface_language, surface_fingerprint, surface_fingerprint_strict}` | none |

## 4. What re-keys (persisted view state keyed by qname orphans)

| packet | old -> new |
|---|---|
| GD.40 Django mounts | Django ROUTE qnames gain their leading `/` and include prefixes (`ANY workspaces/<str:slug>/views/` -> `ANY /api/workspaces/<str:slug>/views/`); regex anchors dropped; `ANY <include prefix>` and `PATCH <mock target>` ROUTEs gone |
| GD.41 Flask / FastAPI mounts (`fb5a02d9`) | `POST /filter` -> `POST /artifacts/filter` |
| GD.42 TS mounts (`beb6fc62`) | `GET /health` -> `GET /api/health` under `app.use('/api', ..)`; NestJS global prefixes and RouterModule paths |
| GD.28 gin `.Use(..)` groups (`4242b652`) | `POST /createAuthority` -> `POST /authority/createAuthority` |
| GD.14 Go routes (`f5898fcc`) | field-mount routes re-keyed under clause-named owners |
| GD.16 OpenAPI `servers:` (`681d9b4a`) | `contract::<doc>::GET:/x` -> `contract::<doc>::GET:<base>/x` |
| GD.19 GraphQL (`57893857`) | `graphql_op:client.query` / `graphql_op:request` / `graphql_op:useQuery` .. gone where the call holds its document; Dart anonymous / by-value documents renamed to their op name |
| GD.18 Ruby Bunny | `queue_producer:unresolved:rabbitmq` / `queue_consumer:unresolved:rabbitmq` -> `queue_producer:<exchange>` / `queue_consumer:<exchange>` |
| GD.11 TS memo / forwardRef consts | STATE_VAR / standalone COMPONENT -> FUNCTION, same qname, new NodeId |
| GD.38 / GD.39 TSX and JSX (`0c14aec4`, `f32a3248`) | locals the old grammar hoisted to module-level FUNCTIONs and standalone COMPONENT / HOOK markers are gone |
| GD.17 Elixir (`b0d29de6`) | a behaviour module that also defines functions: PACKAGE -> INTERFACE, qname kept |
| GD.23 Solidity (`c03557db`) | undocumented literal-constant STATE_VARs are gone |
| GH.13 Dockerfile images (`1bf73a3e`) | `infra:image:--platform=..` gone; generic-dir images renamed (`infra:image:src` -> `infra:image:cartservice`) |
| GD.25 / a11ec18d regions | a nested `build/` / `out/` REGION `build_output` becomes that directory's nodes; `.yarn/` becomes a vendored REGION with no nodes under it |

## 5. Graph content: new nodes and edges (no code change)

- **New qname families** from the host graph: `host_cmd:<host>:<name>` (CLI_COMMAND), `host_event:<host>:<event>`
  (EVENT_HANDLER), `config:<host>:<key>` (CONFIG_KEY), `infra:process:<type>@<dir>`, `infra:unit:<stem>`,
  `infra:program:<name>` (INFRA_RESOURCE), `cli_invoke:<bin>` from Dockerfiles, nginx conf MODULEs with ROUTE
  `ANY <path>`. A HUD that colours by qname prefix sees them as new families.
- **New edges** under emitter `resolver:host` (the 19th Resolve pass): infra start -> code HANDLED_BY, script
  USES, manifest-binary HANDLED_BY; INFRA_REFERENCES image -> base and compose service -> image.
- **New language nodes:** TS interface member METHODs, Rust trait METHODs, Swift protocol requirement METHODs,
  Elixir protocol heads, `.mjs` / `.cjs` / `.mts` / `.cts` MODULEs (GH.10), PACKAGE_DEP from more manifest
  tables (GD.37).
- **ROLE cells:** derived Flutter widgets gain COMPONENT (GD.10 `e427dea5`); JSX components gain COMPONENT
  from the AST (GD.39).
- **ENTRYPOINT cells** with sources `host:<h>`, `shebang`, `script_arg`, `main_guard` (TS / JS too), and
  `host` / `trigger` keys on host starts (GH.4, GH.6, GH.8-GH.11, GH.18): more nodes are live, fewer dead.
- **Removed edges** (no node goes with them): false Rust MODULE -IMPLEMENTS-> trait edges (GD.5), qualified
  framework heritage and self-loops (GD.7), wrong `tail_unique` IMPORTS (GD.34), cross-directory TESTS (GD.36);
  a Rust `use` in an inline `mod` now starts at the inline PACKAGE (GD.35).
- The full per-packet list is section 4 of `dev-notes/repo-graph-handoff-0.5.6.md`.

## 6. Optional: new in-process primitives

- `WatchSession::open_many(&[root, ..])` and `set_memory_budget` for a held multi-repo build bounded by RSS.
- `glia_engine::trace::concept_flows` / `glia_engine::find::find_flows` for "which flows touch this word /
  node" in the inspector strip.

## 7. Re-test

1. Apply the 0.5.0 rename, set `=0.5.6` (after the tag), `cargo check --workspace --all-targets`.
2. Open a TS or Python repo with mounted routes: route node labels carry their prefix.
3. A repo with a Dockerfile or compose file: `infra:*` nodes join the code they start.

## 8. Not in 0.5.6

`glia place` / placement (FAIL, `dev-notes/place-eval-2026-11.md`), a Rust body-only watch path, and docs as a
second graph (GP.9 FAIL; parked for 0.5.7).
