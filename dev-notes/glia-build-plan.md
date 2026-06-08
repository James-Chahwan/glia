# glia build plan — consolidated upstream roadmap (2026-06-09)

Substrate-side mirror of `repo-graph/dev-notes/glia-requirements.md`, reconciled with the
2026-06-05 Engram/neuropil upstream-needs audit. One ranked roadmap; each item grounded in
verified 0.4.14 code state, not memory. **Sequencing is proposed — James is the release gate.**

## Boundary rule (governs every item below)

**Anything of cross-consumer benefit lives in glia (Rust), always.** Logic that >1 consumer
(repo-graph / Engram / neuropil) needs goes in a shared crate (`graph` / `store` / `core` /
`projection-text` / `activation` / `engine`), surfaced through the Rust `py/` binding. The
Python wrapper keeps **only transport**: MCP server loop, tool registration, arg parsing,
response shaping. Nothing that touches the graph lives in Python.

Corollaries: (a) `py/src/lib.rs` is *Rust* — "pyo3 work" is not "Python work." (b) Push the
boundary further into Rust than a consumer asks when the logic is cross-benefit (see WP-B's
sniffer). (c) The wrapper growing path-guessers / decode-tables / sniffers is a *parallel
Python implementation* — exactly what `CLAUDE.md`'s "No Python fallback" forbids. (d) Only
exception: features that drag a heavy dep tree back in (neuropil embed vectors → candle/llama,
split out to `bench/` on purpose). See `[[feedback_cross_benefit_belongs_in_glia]]`.

## Reconciliation (old backlog ↔ new GR doc)

The two sources overlap heavily. De-duplicated:

| Engram/neuropil audit item | repo-graph GR | Status |
|---|---|---|
| #4 per-node POSITION in nodes_json | **GR-1** (path+spans) | SAME — GR-1 is the fuller spec, supersedes #4 |
| #1 incremental per-file delta | **GR-4** (BLAKE3) | SAME big item — ONE feature, 3 consumers (repo-graph perf + Engram `--since` + neuropil v1.1 hot-reload) |
| #2 publish v5 wheel | (delivery of every GR) | The wheel-cut mechanism; CI blocker now fixed (`eedcd65`/`9d67e29`) |
| #5 Engram line spans (prime_text) | overlaps GR-1 | span DATA satisfied by GR-1; prime_text rendering is Engram-side |
| #10 gmap cache version stamp | subsumed by GR-4 | BLAKE3 per-file hash = content version |
| #6 Go import-prefix leak | — | Engram-only, glia-owned, still live |
| #7 DOCUMENTS edges (.md→symbol) | — | Engram-only, glia-owned, still missing |
| #3 kind/edge id→name pyo3 | — | RE-ELEVATED by boundary rule — every consumer decodes ids; → WP-I |
| #8 structured pyo3 cells | — | RE-ELEVATED by boundary rule — cross-benefit cell access; → WP-J |
| #9 neuropil embed vectors | — | post-1.0, gated, re-adds candle dep tree; OUT of scope |
| — | **GR-2** resolve_signal | NEW P0 |
| — | **GR-3** subset/prose projection | NEW P1 |
| — | **GR-5** profile presets | NEW P2 |
| — | **GR-6** with/without bench axis | NEW P1 (glia-side) |
| resolver determinism fix | (rides cut A) | DONE 2026-06-06, uncommitted |

## Work packages (verified scope, LOC not time)

**WP-0 · resolver determinism fix** — DONE, uncommitted (`graph`+`py`+`cli`). impact/trace
flapping → `pick_primary`. Rides the first wheel cut.

**WP-A · GR-1 node spans · P0 · ~30 LOC pyo3.** VERIFIED CHEAP: the `{file,start_line,end_line}`
JSON already lives in the `POSITION` cell (`CellType(3)`) on every source node via
`repo_graph_doc::position_json`, and cells round-trip through `.gmap`. Work = parse that cell in
`nodes_json()`, surface `path`/`start_line`/`end_line`, `null` when absent. **Decision needed:**
POSITION rows are **0-indexed** (tree-sitter `.row`); GR-1 spec asks **1-based inclusive** —
+1 on emit, or document 0-based? (Recommend +1 to match the spec.)

**WP-B · GR-2 resolve_signal (incl. sniffer) · P0 · ~180-300 LOC (graph crate + pyo3).** Doc
cites `stack_resolvers.rs` — that's the cross-graph-resolver *integration test*. The real logic
is the traceback→POSITION-range matcher in
`projection-text/src/bin/synth_traceback_target.rs` (Python-traceback-only, bench-bin home).
Work = lift into `graph` crate as `resolve_signal(text, kind)`, generalize Python-only →
multi-language frames, add `test` (`path::test_name`→test node) and `diff`/file-list kinds,
pyo3 bind. `resolve_span` (hardened in WP-0) is the symbol/path→node primitive it builds on.
**Boundary-rule override of the GR doc:** the doc parks the `auto` *sniffer* (is this text a
stacktrace? a diff? a test-id?) in the Python wrapper — but signal-type detection is
cross-benefit (Engram + neuropil both seed from signals), so it comes into glia:
`resolve_signal(text, kind="auto")` does detection in Rust. Wrapper keeps zero resolution
logic. (+~30-50 LOC vs the doc's split.)

**WP-C · GR-3 subset/prose projection · P1 · ~60-120 LOC + pyo3.** `projection-text::render_merged`
is whole-graph only; needs a node-id-subset entry point reusing existing passes. Add
`dense_text_subset(node_ids, full)` + `prose(node_ids)`. Verify which passes assume whole-graph.

**WP-D · GR-4 incremental indexing · P1 · ~300-550 LOC · THE BIG ONE.** store+core+engine.
Per-file BLAKE3 hash in the container; stat+hash → re-parse only changed files; dirty
propagation marks downstream stale; mtime fast-path on the existing skip set. Surface
`generate(repo, incremental=True)` + incremental reload. Aligns with Phase 2 of
`dev-notes/incremental_gmap_plan.md` (resolver-invalidation registry = bulk of the work).
3-consumer win; **blocks neuropil v1.1**. Subsumes #10.

**WP-E · GR-6 SWE-bench with/without axis · P1 · glia bench, no engine LOC.** 3×2 → 3×2×2 on
the existing Qwen harness. **≥100-instance → WORKFLOW §3 pre-flight REQUIRED** (inode test,
repo coverage, N_CTX≤32K cap, single-inst dry-run) pasted before launch. Produces the headline
"repo-graph lifts solve rate by N%".

**WP-F · GR-5 profile presets · P2 · ~40-80 LOC.** Expose `activate(..., profile=)` →
edge-weight preset over `ActivationConfig` (`code_activation_defaults` already exists as the
base). presets: default/repair/review/onboard.

**WP-G · #6 Go import-prefix leak · ~60-110 LOC.** `engine/src/lib.rs` Go `parse_file` passes
`""` as `module_import_prefix` → internal pkg names leak into `Symbol.imports`. Engram-only.

**WP-H · #7 DOCUMENTS edges · ~80-150 LOC.** `DOCUMENTS` edge cat (`EdgeCategoryId(6)`) exists
but v5 `.md` doc nodes emit none → islands. Engram-only.

**WP-I · #3 canonical id→name decode · ~60-110 LOC (code-domain + pyo3).** RE-ELEVATED by the
boundary rule: glia owns the kind/edge `u32`→name map in `code-domain`; not exposed → the
wrapper hardcodes a Python decode table frozen at id 40/31 that goes stale every new kind. The
textbook "glia has it, doesn't expose it, Python fakes it" case. Add `kind_names()` /
`category_names()` from `code-domain`. Cross-benefit (every consumer decodes ids); also fixes
glia's OWN stale projection-text fallthrough to 'Node' for ids >9. Bundle into Cut A.

**WP-J · #8 structured cell access · ~40-70 LOC pyo3.** RE-ELEVATED: imports / state-var / doc
cells are reachable only by string-scraping `dense_text` today. Expose them structured off the
node. Cross-benefit (Engram emission + neuropil + any client). Bundle with WP-I.

## Proposed sequencing (wheel cuts)

1. **Cut A — v5 wheel, finally (0.4.15):** WP-0 + WP-A + WP-I + WP-J + WP-G + WP-H →
   bump+tag+publish. All cheap Rust-exposure/emission items; ships the long-overdue v5 wheel
   (gates the whole repo-graph cluster), lights up `read`, and kills the wrapper's
   path-guesser + frozen decode table in one go. **Pushback on repo-graph's "GR-1+GR-2
   together":** don't couple cheap exposures to the ~250-LOC GR-2 port and stall the v5 wheel —
   ship A now, B next.
2. **Cut B (0.4.16):** WP-B → lights up `locate`.
3. **Cut C (0.4.17):** WP-C → prose / scoped-dense cluster. Fold in WP-F.
4. **WP-D incremental** as its own focused project (parallel-track; biggest, 3-consumer).
5. **WP-E bench** alongside D (own harness, pre-flight gated) → the headline number.

## Answers to repo-graph's open questions
1. **`activate` shape:** `Vec<(id: u64, score: f64)]`, score-sorted, `top_k` applied. impact/
   locate will reuse the same shape. ✓
2. **edge weight in `edges_json`?** Currently `{from,to,category}` only; weight is derived in
   activation, not a stored edge property. Keep engine-internal unless GR-5 lands — then expose a
   `category→weight` table for the active profile rather than per-edge. Recommend: keep internal.
3. **`read` truncation:** wrapper caps the span; no engine change. dense_text already truncates. ✓

## Build status — v0.4.15 cut (2026-06-09)

Branch `feat/v0.4.15-cut`, **not pushed/tagged** (release gate = James). 10 of 11
WPs shipped, each its own commit + tests; full workspace suite green except the
**pre-existing** `py_smoke_fixture::every_code_entity_has_code_and_position_cells`
(fails identically on base `eedcd65` — confirmed, not caused by this cut).

Done: WP-0 resolver fix · WP-A spans · WP-I decode tables · WP-J cells ·
WP-G Go import leak (note: needed both prefix threading AND a domain-aware
`library_name` — prefix alone wasn't enough) · WP-F activate profiles ·
WP-C subset/prose · WP-B resolve_signal+sniffer · WP-H DOCUMENTS edges ·
WP-WHEEL 0.4.15 bump.

### WP-D incremental indexing — PARKED, needs a direction call

Not a code-stuck park — it's a multi-hour, format-touching feature whose shape is
a release-gate decision. What I found in the store:
- Manifest already has `schema_version` + per-**shard** `content_hash` (**xxhash64**,
  not blake3) and a write-side skip-unchanged-shards step. But shards are by
  **kind, not file** — there is no per-file artifact to reuse, so the read/build
  path re-parses everything.
- GR-4's acceptance ("touch 1 file → re-parse ~1 file, byte-identical to clean
  rebuild") needs (a) a new persisted **per-file** parse cache, and (b) resolver
  re-run regardless (cross-graph resolvers are global) — the
  resolver-invalidation registry the incremental plan calls "the bulk of the work."

**Decisions needed before I build it:**
1. **Hash:** use the existing `xxhash64` (no new dep, consistent with shards) or
   add `blake3` per GR-4's literal text? (Recommend xxhash64.)
2. **Architecture:** persist a per-file `FileParse` cache under
   `.ai/repo-graph/parses/` keyed by content hash (reuse unchanged files, re-merge
   + re-run resolvers) — vs an in-memory-only `generate_incremental(prev, changed)`
   (what neuropil wants, but reconstructing per-file contributions from a merged
   graph is lossy). (Recommend the persisted per-file cache; it also backs Engram's
   `export-engram --since`.)
3. **Scope now:** parse-only speedup (skip re-parsing unchanged files; resolvers
   still global) is the safe, high-value first slice. Full resolver-invalidation
   (skip resolvers when no cross-cutting node changed) is a follow-up. OK to ship
   the parse-only slice first and keep "byte-identical" as the test?
4. **Manifest bump:** adding per-file hashes bumps `MANIFEST_VERSION`; old caches
   rebuild once (graceful). OK?

## Cross-cutting caveat (unchanged)
None of this reaches the live MCP until repo-graph deletes its stale vendored `rust/` fork
(`git rm -r rust/`, zero glia LOC, repo-graph session's job). The wheel is necessary but not
sufficient. See `[[feedback_separate_sessions_connected_repos]]`.
