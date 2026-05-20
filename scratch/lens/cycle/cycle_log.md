# Overnight loop — cycle log

Newest cycle at top. Append-only. Never rewrite.

Conventions:
- `cycle N`: header
- `phase 1 hypothesis | phase 2 implementation | phase 3 test | phase 4 analyze`: one line per phase, terse
- numbers not vibes; no celebration before BREAKTHROUGH (5+ gold pass on working set)
- regression = call it a regression, no softening

## cycle 0 — baseline (timestamp TBD)

Pending. See `INTERRUPT_STATE.md` for current state.

Goal: establish gold-pass baseline on the 21-instance working set using the existing v0.4.13 pipeline (run_instance.py + diff_healer + apply_and_test) on Qwen 2.5 Coder 7B Q4.

Source of truth:
- working set: `manifests/working_set.json` (21 instances)
- holdout: `manifests/holdout.json` (10, NEVER TOUCHED)
- per-instance results: `cycle/results_history.jsonl` (append-only)
- per-instance lens telemetry: `out/cycle-0/<instance>.{jsonl,png}`

Baseline run output goes to `scratch/latent/out/instance_results.jsonl` (run_instance.py's default) AND is tagged with `cycle: 0` in the cycle results.
