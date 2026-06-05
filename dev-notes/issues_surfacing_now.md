# Issues surfacing now — plans to address

Live observations from cycle 2.0-gpu-14b (in flight) + codebase review.

## Issue 1 — pool-cap may be too aggressive

**Symptom**: matplotlib-22711 (1156-entry pool, ~136K tokens) still NO-DIFF even with pool-cap engaged. Inference ran 657s (model loaded the prompt) but emitted no diff.

**Root cause hypothesis**: pool-cap drops from tail by score rank. For matplotlib-22711 the TAIL of the score-sorted pool includes ~25K tokens of low-score but possibly-relevant entries. After cap, model sees ~131K tokens of mostly high-score pool + prefix + suffix — but with the directive's gold qname maybe missing from the surviving pool.

**Plan**:
- **A1** Instrument: log which pool entries were dropped (by qname/file) in pool-cap path. Confirm whether gold-related entries are being culled.
- **A2** Smarter cap: instead of strict tail-drop, **always-keep entries whose qname matches a directive PRIMARY target**. Build a "protected set" from directive at run_instance.py side, pass as env to pathB, use as do-not-drop filter.
- **A3** Fallback: if pool cap forces dropping >50% of entries, switch to mean-pool mode for the dropped slice (lose per-token info but keep some signal) instead of removing entirely.

**Effort**: A1 is small (3-line print). A2 is the real fix.

## Issue 2 — normalize_diff produces ≥ original consistently

**Symptom**: cycle 2.0 logs show "produced 647c (≥ original 597c); keeping original" repeatedly. Normalizer fires but never compresses.

**Diagnosis**: function works as designed. The compression case (model's wrong-line-numbers → git re-emits at correct numbers → smaller output) only triggers when the model's diff had line drift. For diffs that ALREADY apply cleanly at the right place, git diff re-emits the same content at the same line numbers → same size. Plus git's default `--unified=3` may include slightly more context than the model emitted if model used --unified=1 or 2.

**Plan**:
- **B1** This isn't a bug — log message should say "no compression available; keeping original" not "produced N≥M; keeping". Clearer semantics.
- **B2** Add a SEPARATE code path that runs normalize_diff WHEN APPLY-FAILS but keeps original WHEN APPLIES CLEAN. Different from current "always try, only keep if shorter".
- **B3** Maybe also use `--unified=1` in the git diff call to actually compress context blocks.

**Effort**: small. B1+B3 are 2-line changes; B2 needs a small refactor.

## Issue 3 — edit-content levers shipped but NOT tested

**Symptom**: cycle 2.0 has substrate fixes + normalize + full-hunk exemplars active, but **GLIA_PLAN_THEN_EDIT / GLIA_BEHAVIORAL_TARGET / GLIA_SKIP_VALIDATOR_PASS / GLIA_ATTN_INJECTION are all OFF**. We shipped 5 levers, then ran a cycle without enabling them.

**Plan**: cycle 2.1 with all levers enabled:
```bash
GLIA_TWO_PASS=1 GLIA_SAGE_RUNTIME=1 GLIA_RUNTIME_EVIDENCE=1 \
GLIA_PER_TOKEN_POOL=1 GLIA_SAMPLES=3 GLIA_COMPOSITIONAL=1 \
GLIA_NORMALIZE_DIFF=1 GLIA_EXEMPLAR_FULL_HUNK=1 \
GLIA_PLAN_THEN_EDIT=1 GLIA_BEHAVIORAL_TARGET=1 \
GLIA_SKIP_VALIDATOR_PASS=1 \
GLIA_ATTN_INJECTION=1 GLIA_ATTN_ALPHA=0.3 \
N_GPU_LAYERS=99 \
./cycle/run_cycle_loop.sh 2.1-edit-levers-attn 14b-q4
```

Wall will be heavy (plan pass + attn-bias pass per sample = ~3× baseline). But this is the test of whether ANY of the edit-content work moves the needle.

## Issue 4 — matplotlib substrate still partial

**Symptom**: cycle 2.0 matplotlib-22835 FAIL wall=1028s — substrate fix made build_ext run, but final result is still FAIL. (vs cycle 1.5 where it was NO-RUN.)

**Diagnosis**: build_ext works now (chmod bundled freetype scripts succeeded). But matplotlib tests STILL might be failing for either:
- Wrong reason (the test the model's fix should pass, just doesn't)
- Test infrastructure issue (image comparison precision, environment variable, etc.)

**Plan**:
- **D1** Pull the actual test_log.txt from cycle 2.0 matplotlib-22835 — see what the failing test reports.
- **D2** Could be content-wrong (model's fix is wrong); could be substrate-wrong (different test fail).

## Issue 5 — Lens-attention-bias smoke not yet run

**Symptom**: lens-bias wire-in shipped (487f020) but never invoked. Smoke script lens_bias_smoke.sh exists; needs GPU.

**Plan**: after cycle 2.0 closes, run smoke on marshmallow:
```bash
ssh pod 'bash /home/ivy/Code/glia/bench/lens/cycle/lens_bias_smoke.sh'
```
Read the alpha sweep table, find the knee where injection moves output without degenerating.

## Issue 6 — classifier NO-RUN bucket fix shipped but cycle 1.4/1.5 not re-classified

**Symptom**: failure_mode_classifier.py was patched (commit 25e483c) to bucket NO-RUN distinctly. But auto8h's existing cycle-1.4-no-channels-failure-modes.md was generated PRE-fix and shows inflated RIGHT-TARGET-WRONG-EDIT count.

**Plan**:
- **F1** Re-run classifier on cycles 1.4 and 1.5 with patched code → corrected counts → update the cycle comparison memory.

**Effort**: trivial. Single command per cycle.

## Issue 7 — Phase 2 design blocked on neuropil API

**Symptom**: Phase 2 (per-file delta) needs a resolver invalidation registry, but the actual API shape depends on neuropil's call patterns. Currently designing in a vacuum.

**Plan**:
- **G1** Capture neuropil's actual call surface before committing Phase 2 design. Questions to answer:
  - Does neuropil call `apply_file_deltas()` directly, or watch a filesystem and get woken?
  - Sync or async API?
  - What's the read pattern after a write — immediate or eventual?
- **G2** Block Phase 2 implementation on this conversation. Continue with Phase 1 polish + lens-bias validation in the meantime.

## Plan priority order

1. **Run cycle 2.1** with all levers (Issue 3) — biggest signal
2. **Run lens-bias smoke** (Issue 5) — validates the wire-in
3. **A1 + A2 pool-cap instrumentation + protected set** (Issue 1) — unlocks matplotlib-22711
4. **D1 matplotlib test_log diagnosis** (Issue 4) — disambiguate content vs substrate
5. **F1 re-classify auto8h cycles** (Issue 6) — clean comparison table
6. **B1 + B3 normalize_diff polish** (Issue 2) — clearer logs + better compression
7. **G1 neuropil API conversation** (Issue 7) — unblock Phase 2

The first 2 are pod-bound (need GPU); the rest can be CPU-side or planning.
