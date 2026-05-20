# Interrupt state — live

Read this if you wake up mid-cycle. Tells you where the loop is RIGHT NOW.

## Status snapshot · 2026-05-21 ~06:15

- branch: `overnight-loop-cycle-0`
- cycle: 0 (CALIBRATION) — COMPLETE
- phase: idle, awaiting morning review
- last result: **marshmallow-1359 GOLD PASS** via prescriptive suffix (iter1)

## What was achieved tonight

1. Slice 1.5 multi-position lens shipped (commit `2acb337`)
2. Slice 1.6 lens-batch + Slice 1.7 lens-analyze built
3. Cycle harness scaffolded in `cycle/`
4. **Cycle 0 marshmallow loop:**
   - iter0 (default suffix) → F2P FAIL, 16 regressions
   - iter1 (prescriptive suffix) → **F2P PASS + 77/77 regression green** 🎯
5. Two real bugs surfaced + fixed:
   - apply-path: GNU patch 2.8 default fuzz too strict → `--fuzz=5` in `run_instance.py:904`
   - semantic: model picks wrong target without explicit suffix directive

## What's in the working tree, uncommitted

- `scratch/latent/out/run_instance.py` — the `--fuzz=5` fix at line 904
- `inst-marshmallow-code__marshmallow-1359-7b-q4-iter1-prescriptive/` — the winning iteration
- `cycle/marshmallow_log.md` — full iter trace
- `cycle/cycle_log.md` — updated with cycle 0 summary
- `engine/src/lib.rs` — still mixed with prior-session panic-isolation work (untouched tonight)

## Generalizable sage-feature derived from iter1

The prefix's `derived_notes` already contains the diagnosis ("DateTime in List has no .opts") + access paths section lists `self.root.opts`. The model has the answer in front of it but defaults to easier-looking edits under a generic suffix. **Auto-deriving a "Required fix location" suffix from existing prefix content is the cycle 0.2+ target.**

Pseudocode:
- if derived_notes contains "X has no Y attribute" AND access_paths contains a `self.Z.Y` path:
  - emit suffix block: "Edit the function where Y is missing. Use self.Z.Y instead."

Estimate: ~50 LOC in `run_instance.py`'s prefix-assembly step. Could also live in `synth_derived_notes` binary (Rust).

## Decisions awaiting morning James

1. **Adopt iter1 prescriptive suffix as the default for cycle 0.1+?** Risk: it's hand-crafted for marshmallow. Generalizing means writing the auto-derivation. Direct path: ship the generalization THEN cycle 0.1 = re-run 7-instance loop with auto-derived suffixes; expect multiple instances to now pass.
2. **Pick the 6 sentinel-companions to marshmallow** for the 7-instance loop. Candidates in `manifests/working_set.json` (21 entries).

## What to do if you wake here

If "wake to see if it worked": yes, marshmallow gold PASSED with iter1. Read `cycle/marshmallow_log.md` for the trace + diff.

If "continue the loop": iter1 + apply-path-fix are the cycle 0 deliverables. Cycle 0.1 starts with the auto-derivation work + 7-instance run. Don't need to keep grinding marshmallow alone — the lever is found.

If "something went wrong": check `cycle/marshmallow_log.md` for the artefact paths. The repo at `/home/ivy/swe-work/marshmallow-code__marshmallow-1359` is currently in the iter1-patched state (after pytest). `git checkout -- .` to reset.

## Sacred rules — still in force

- 10 holdout instances at `manifests/holdout.json` NEVER touched
- `cycle_log.md` + `results_history.jsonl` (when it exists) APPEND-ONLY
- No celebrating in cycle log; numbers only — "F2P PASS, 77/77 regression green" is the strongest language allowed

## Pending ideas

(see `cycle/pending_ideas.md` — empty so far)

## Noticed but not fixed

(see `cycle/noticed_but_not_fixed.md`)

- `run_instance.py:904` patch fallback should also use `-l` (ignore whitespace) — model's indentation drift (4-space vs 8-space) breaks even fuzz match. Without `-l` the iter1 patch wouldn't have applied. Add to cycle 0.2 patches.
- model output sometimes lacks `--- a/...` / `+++ b/...` header lines; `diff_healer.py` recovers but worth a directness flag too.
