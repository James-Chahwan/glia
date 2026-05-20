# Overnight loop — cycle log

Newest cycle at top. Append-only. Never rewrite.

Conventions:
- `cycle N`: header
- `phase 1 hypothesis | phase 2 implementation | phase 3 test | phase 4 analyze`: one line per phase, terse
- numbers not vibes; no celebration before BREAKTHROUGH (5+ gold pass on working set)
- regression = call it a regression, no softening

## cycle 0 — marshmallow calibration · 2026-05-21 (overnight)

**Goal:** establish whether marshmallow-1359 still passes gold on the current pipeline. Discovered the historical sentinel was broken at the apply-path AND semantic levels. Two fixes shipped.

**Results:**
- apply-path regression (GNU patch 2.8 default fuzz too strict): fixed with `--fuzz=5` in `run_instance.py:904`. Without this, every SWE-bench instance whose diff needs fuzz≥3 silently APPLY-FAILs. See `[[feedback-swebench-apply-check-fuzz-first]]`.
- semantic regression (model picks wrong target when given generic suffix): iter1 prescriptive suffix gets **F2P PASS + 77/77 regression suite green**. Full iteration log in `cycle/marshmallow_log.md`.

**Lever discovered for cycle 0.2+:**
The prefix already contains the diagnosis (derived_notes: "DateTime in List has no .opts") + the fix path (access_paths: "self.root.opts"). Model ignores them under generic suffix; uses them under prescriptive suffix. Pipeline change: auto-derive a "Required fix location" suffix block from existing prefix content. Estimated ~50 LOC in Python.

**Artefacts:**
- `cycle/marshmallow_log.md` — full iteration trace
- `inst-marshmallow-code__marshmallow-1359-7b-q4-iter1-prescriptive/out.txt` — the winning diff
- `[[reference-marshmallow-1359-diff-anatomy]]` — bug structure
- `[[feedback-swebench-apply-check-fuzz-first]]` — apply-path diagnosis protocol

**Next:** cycle 0.1 setup waits on James's morning review. Two decisions: (1) keep iter1's prescriptive suffix as a generic instruction (low risk, immediate gain) vs auto-derive (real sage-feature, more work). (2) pick the 6 sentinel-companions to marshmallow for the 7-instance loop.

Goal: establish gold-pass baseline on the 21-instance working set using the existing v0.4.13 pipeline (run_instance.py + diff_healer + apply_and_test) on Qwen 2.5 Coder 7B Q4.

Source of truth:
- working set: `manifests/working_set.json` (21 instances)
- holdout: `manifests/holdout.json` (10, NEVER TOUCHED)
- per-instance results: `cycle/results_history.jsonl` (append-only)
- per-instance lens telemetry: `out/cycle-0/<instance>.{jsonl,png}`

Baseline run output goes to `scratch/latent/out/instance_results.jsonl` (run_instance.py's default) AND is tagged with `cycle: 0` in the cycle results.
