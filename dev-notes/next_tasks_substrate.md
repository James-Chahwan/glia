# Next tasks — substrate work for neuropil

Snapshot 2026-05-22 post-substrate-pivot. Phase 1 + profile + lens-bias wire-in are landed; this is the queue for next session(s).

## Validated this session
- `.gmap` incremental rebuild — Phase 1 ✓ (skip-when-unchanged + test)
- PPR profile baseline ✓ (4.59ms @ 10K nodes — already under target)
- Lens-attention-bias wired into beam ✓ (parallel candidate channel)

## Queued — substrate / neuropil-readiness

### Tier S (high-value, well-scoped)

1. **Cycle 2.0 + 2.1 validation runs**
   - Cycle 2.0 in flight — finishes any moment, expect 3/7 → 4/7 PASS via matplotlib+sklearn substrate fixes
   - Cycle 2.1: `GLIA_ATTN_INJECTION=1 GLIA_PLAN_THEN_EDIT=1 GLIA_BEHAVIORAL_TARGET=1` — all new levers active, alpha=0.3 default
   - Cycle 2.2: alpha sweep across {0.1, 0.3, 0.5, 1.0} on marshmallow only — find the steering knee

2. **Phase 2 — per-file delta** (~3-5d design est, **don't estimate but ship in atomic slices**)
   - Insert resolver invalidation registry: each cross-graph resolver declares consumed/produced NodeKinds
   - `apply_file_deltas(dir, deltas)` function: drop-then-add by POSITION cell origin
   - Bounded intra-repo re-resolution (BFS 1-2 hops)
   - Tests: edit one function, only python.gmap + cross_stack.gmap change

3. **Lens-bias alpha-knee finding**
   - Run `lens_bias_smoke.sh` after cycle 2.0 closes (no GPU contention)
   - Find alpha where injected output diverges meaningfully from baseline but doesn't degenerate
   - Document the knee in lens-bias attention bias notes

### Tier A (good leverage but bigger lift)

4. **Phase 3 — live shadow journal** (LSM-tree mutable layer)
   - Sub-10ms reads for per-token-steering use case
   - Open design Qs: tombstone scope, cross-shard dropped endpoints, RwLock vs DashMap, compaction trigger
   - **Needs neuropil API conversation** before committing — what do they actually call?

5. **Activation `top_k` direct API**
   - Current: `activate()` returns full score vector, caller filters
   - Better: callers ask for top-K seed-relevant nodes only; iterate to early-exit when top-K converged
   - PR likely cuts the 50K/100K rows from 32ms/66ms to single-digit ms

6. **Lens-bias diagnostic dashboard**
   - Extend the smoke script into a 3-renderer artifact (JSONL + PNG + ASCII)
   - Per (instance × alpha × layer × position) measure injection-effect
   - Goal: identify a "good" alpha per instance, see if it can be auto-tuned

### Tier B (research-y, needs separate planning)

7. **cb_eval Python hook via ctypes** — for true per-token attention bias inside Python pathB. Currently the parallel-Rust approach (Lever #8 shipped) gives us the same result without the hack, but ctypes hook would let us put bias INSIDE the same forward pass as the beam sample.

8. **SIMD power iteration** — for 100K+ node graphs the scalar loop is the bottleneck. Need `wide` or `std::simd` crate; vectorize the `sum += incoming[i].w * score[from]` inner loop.

9. **Pre-computed PPR cache at .gmap build time** — for common seeds (e.g. by NodeKind frequency), precompute top-K results, serialize alongside the shard. Sub-millisecond lookups for popular queries.

10. **Graph-regenerated diff** (Tier-3 edit-content lever from earlier menu) — model emits text-spec, glia tree-sitter parses + regenerates clean diff. Eliminates APPLY-FAIL class entirely. Requires synth_diff_regen Rust bin.

## Decision points for next session

- Continue substrate work OR pivot back to running edit-content levers (#1-#7)?
- Phase 2 or Phase 3 first? (depends on neuropil use case)
- Stay with Qwen 2.5 Coder family or test Qwen3-Coder when its 256K ctx is needed?

## Honest open questions

- **Does lens-attention-bias actually steer output?** Substrate is shipped; needs the smoke run + alpha sweep to confirm the lever exists before integration is justified.
- **What's neuropil's actual API surface?** Phase 3 design depends on whether they want sync get_node calls, PPR queries, or live injection. Get that conversation before committing Phase 3 implementation.
- **Are the 5 edit-content levers actually helping?** Cycle 2.1 (planned but not yet run) will A/B them against cycle 2.0. Skipped due to substrate pivot.

## Commits this session (substrate segment)

- 25e483c: edit-content levers (5 features)
- c3e74a7: Phase 1 incremental store
- cb5e989: incremental rebuild plan
- e6091d0: lens-bias smoke script
- 7fd198c: PPR profile harness
- 487f020: lens-attention-bias wire-in
