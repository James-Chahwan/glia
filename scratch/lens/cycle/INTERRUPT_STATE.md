# Interrupt state — live

Read this if you wake up mid-cycle. Tells you where the loop is RIGHT NOW.

## Status

- branch: `overnight-loop-cycle-0`
- cycle: 0 (BASELINE)
- phase: SETUP (writing harness; no cycles fired yet)
- next planned: cycle 0 phase 3 (test run on 21 working-set instances via run_instance.py)

## What just finished

- slice 1.5/1.6/1.7 logit-lens crate committed (`2acb337`)
- G17/G18/G19/G20/G22 MergedGraph helpers committed (`7048eab`)
- G14 SERVICE extractor module committed (`a9199d9`)
- RepoGraph.properties stale-fixture fix committed (`4d1d4ff`)
- Branch `overnight-loop-cycle-0` created off main
- 21+10 manifests written to `manifests/{working_set,holdout}.json`
- run_instance.py smoke on matplotlib__matplotlib-22711 KICKED OFF (background) — see `scratch/latent/out/inst-matplotlib__matplotlib-22711-7b-q4-overnight-smoke/`

## Uncommitted, in working tree

- `engine/src/lib.rs` — mixed pre-existing panic-isolation work + my G14 services::extract_service_nodes wiring. Left for review (don't want to take credit for prior work). G14 wiring is at the END of `apply_cross_cutting_extractors`; verify by `git diff engine/src/lib.rs`. The lens crate doesn't depend on this being committed.

## What to do if you wake here

If the matplotlib smoke is STILL running:
- Check `/tmp/claude-1000/-home-ivy-Code-glia/7788db8c-7519-48b1-9985-0f9c870681ee/tasks/bo1zy2zs2.output` for live progress
- ~15-20min expected wall-clock on 7B Q4 CPU
- If >40min: probably stuck on a docker / pytest env step; check the log

If the smoke FINISHED (success or fail), see `cycle/cycle_log.md` for the latest entry.

## Sacred rules

- NEVER touch the 10 holdout instances. They live in `manifests/holdout.json`. Any cycle that runs against an instance in that list = abort + log as violation.
- NEVER delete `cycle/cycle_log.md` or `cycle/results_history.jsonl`. Append only.
- NEVER softening regressions. Number went down = "regression, pass count N→M". Period.

## Pending ideas / noticed-but-not-fixed

- `cycle/pending_ideas.md` (append if interrupting James leaves new ideas mid-cycle)
- `cycle/noticed_but_not_fixed.md` (append if you spot bugs outside the injection/derivation pipeline)
