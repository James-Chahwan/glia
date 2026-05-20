# Overnight loop — cycle log

Newest cycle at top. Append-only. Never rewrite.

Conventions:
- `cycle N`: header
- `phase 1 hypothesis | phase 2 implementation | phase 3 test | phase 4 analyze`: one line per phase, terse
- numbers not vibes; no celebration before BREAKTHROUGH (5+ gold pass on working set)
- regression = call it a regression, no softening

## cycle 0.2 — graph-derived directive · 2026-05-21 ~07:5X · 🟢 PASS

**Real glia win.** Auto-derived directive from `synth_traceback_target.rs --text-out` (new Rust bin in `projection-text/src/bin/`). Reads .gmap POSITION cells + issue.txt traceback/exception regex; emits a graph+issue-derived text directive concatenated into the suffix. NO hand-written instance-specific text.

Marshmallow-1359 on Qwen 2.5 Coder 7B Q4:
- apply: succeeded with fuzz 1
- F2P: PASS
- regression: 77/77 green

Channels (all auto-derived, 5-iter progression iter1→iter5; iter5 was the final lock):
1. target function = deepest traceback frame matched via POSITION cells
2. anti-target list = other matched frames (call-stack ancestors)
3. buggy line = source excerpt parsed from issue traceback
4. exception info = (class, receiver type, missing attr) parsed from `'X' object has no attribute 'Y'`
5. substitution hint = template using parsed missing-attr → "e.g. self.root.<attr>"

The L2 (JSON pool injection) test along the way proved the latent pool channel is dead-on-arrival for steering — `run_llama_pathB.py` mean-pools each cell to one anonymous vec; score field unused. The actionable channel is suffix-text. See `cycle/marshmallow_log.md` for the full iter trace, `[[reference-latent-pool-mechanism]]` for mechanism audit, `[[project-graph-text-directive-progression]]` for the channel-by-channel breakdown.

**Cycle 0.3 candidates:**
1. Integrate synth_traceback_target into run_instance.py (auto-pipe directive into suffix)
2. Run synth_traceback_target on the 21-instance working set → see how many of those instances have Python tracebacks + how many get gold via this directive alone
3. Extend exception parsing to TypeError/KeyError/ValueError (only AttributeError works today)
4. Lens diagnostic comparing iter1/iter4/iter5 generations to see at which layer each channel commits the model

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
