# Interrupt state — live

Read this if you wake up mid-cycle. Tells you where the loop is RIGHT NOW.

## Status snapshot · 2026-05-21 ~08:00

- branch: `overnight-loop-cycle-0`
- cycle: 0.2 COMPLETE — graph-derived directive gold pass on marshmallow
- next: decide cycle 0.3 scope (integrate + generalize) vs deeper architectural shift

## The headline result

**Marshmallow-1359 GOLD via auto-derived directive.** Zero hand-written instance-specific text. Same `synth_traceback_target.rs` bin will work on any SWE-bench instance with a Python traceback + AttributeError.

- F2P: PASS
- regression: 77/77 green
- diff: `getattr(schema.opts, ...)` → `getattr(schema.root.opts, ...)` in `DateTime._bind_to_schema`
- channel: text directive auto-derived from .gmap POSITION cells + issue.txt regex

## What was tried tonight (the actual research)

5-iter progression on marshmallow:

| iter | channel | F2P |
|---|---|---|
| 1 (cycle 0) | hand-written prescriptive suffix | PASS (text engineering, not glia) |
| 2 (L2) | high-score JSON pool cells | FAIL — pool channel is dead |
| 3 | graph candidate list (5 functions) | FAIL — first-listed bias |
| 4 | deepest-frame target + anti-targets | FAIL (right function, wrong line) |
| 5 | + buggy line + exception parse | **PASS** |

## Important negative result: latent JSON pool is structurally dead for steering

`run_llama_pathB.py` mean-pools each summary to ONE D-dim vector, concatenates the N vectors with prefix/suffix tokens, decodes. **Score field is unused.** Pool entries are anonymous to the model — can't differentiate a target cell from background. See `[[reference-latent-pool-mechanism]]`.

This bounds the project: graph→pool→embed cannot steer target selection in current mechanism. Steering happens via prefix/suffix TEXT. The L2 pool injection at score 999 produced byte-identical output to baseline (zero injection).

## What's in the working tree

Committed on `overnight-loop-cycle-0`:
- `745892d` cycle 0 — apply-path fuzz fix + prescriptive suffix win
- (latest) cycle 0.2 — graph-derived directive via `synth_traceback_target.rs`

Uncommitted in working tree:
- `engine/src/lib.rs` (mixed with prior-session panic-isolation work, untouched tonight)

## Decisions awaiting morning James

1. **Integrate `synth_traceback_target.rs` into `run_instance.py`?** Currently a manual subprocess + suffix concatenation. Real integration = pipeline step between `synth_derived_notes` and suffix assembly. ~30 min Python.
2. **Run on the 21-instance working set?** ~5-9h depending on per-instance time. Tests generalization across django/matplotlib/pytest/sphinx/sklearn/etc. Some instances may have no traceback or non-AttributeError exception — those fall back to baseline behavior.
3. **Extend exception parsing** to TypeError/KeyError/ValueError? Each needs its own substitution-hint logic (no `'X' has no attribute 'Y'` pattern). ~1-2h each.
4. **Latent mechanism upgrade**? Per-token embedding + marker tokens around pool cells (replacing mean-pool). The "real" graph-injection channel. ~4-6h Rust in `scratch/lens/` extension.
5. **Lens diagnostic comparing iter1/iter4/iter5**? Run slice 1.5 multi-position lens on the three generations to see at which layer each channel commits the model. Informs whether iter6+ text channels are worth pursuing OR if it's time to switch to latent. ~1-2h.

## Recommendation order

1. (2) — run on 21 instances first. Cheap and informative. Tells us if this generalizes.
2. (1) — integrate into run_instance.py once we know (2) is worth shipping.
3. (3) — extend exception parsing only for instances in the working set that needed it.
4. (5) — lens diagnostic to inform any further investment.
5. (4) — only if (5) shows the text channel is converging on diminishing returns.

## Cycle 0.3 partial work (overnight, post-iter5)

- **Extended synth_traceback_target.rs**: ExcInfo enum (Attribute / UnexpectedKwarg / ArityMismatch / NotSubscriptable / IndexOutOfRange / Generic), Windows-path support, deepest-graph-matched-frame walk-back. 3 companion smoke directives verified (matplotlib-22711 IndexError, matplotlib-22835 ValueError WITH known raise-line caveat, sklearn-10508 TypeError after path+walkback fix).
- **6-companion loop set** picked + locked at `manifests/cycle_loop_set.json`: marshmallow + matplotlib×2 + sklearn-10508 + django-10914 + pytest-11143 + sphinx-10325. Spans 4 traceback-having instances and 3 no-traceback controls.
- **Lens 4-condition diagnostic** ran (baseline / iter1 / iter4 / iter5 at position 0, 28 layers). Findings at `out/lens-compare/findings.md`. Headline: position-0 lens **can't see iter5's mechanism** — iter5 residuals ≈ baseline at position 0, but iter5 passes F2P. The directive operates on generated tokens 5-50, not the first token. Slice-1.5 lens is the wrong instrument for multi-token differentiation. Cycle 0.4 needs an autoregressive-generation lens (extend `scratch/lens/src/runtime.rs::forward_capture` to do greedy decode with per-step residual capture, ~4-6h Rust).

## Critical caveat for matplotlib-22835

Its deepest traceback frame is a `raise ValueError(...)` line. synth_traceback_target.rs currently targets that line. **The bug isn't IN the raise** — the fix is upstream (something should prevent reaching this raise). Directive needs a "is the buggy line a raise statement?" check that redirects to the caller frame. TODO in `noticed_but_not_fixed.md`.

## Decision matrix for morning, refreshed

Highest-yield next step is **NOT another lens iteration on marshmallow** — the position-0 lens gave us all it can. It's also **NOT iter6 text-channel addition** — iter5 already passes marshmallow and the lens shows no obvious channel to add or prune at position 0.

The two pulled-forward priorities are:

1. **Generalization test**: run iter5-style auto-derived directives on the 4 traceback-having companion instances + 3 no-traceback controls. ~2-3h wall-clock. Tells us if the directive works on more than marshmallow. This is the real cycle 0.3 deliverable.
2. **Autoregressive lens (cycle 0.4)**: extend `forward_capture` to do greedy decode with per-step residual capture. Required infrastructure for any further lens-based diagnosis of how the directive influences DECODING (not prompt encoding). ~4-6h.

Lower priority but still in scope:
3. **Raise-line detection** in synth_traceback_target.rs (matplotlib-22835 fix).
4. **Integration**: synth_traceback_target.rs as a pipeline stage in run_instance.py (not subprocess).

## Sacred rules — still in force

- 10 holdout instances at `manifests/holdout.json` NEVER touched
- `cycle_log.md` + `marshmallow_log.md` + `results_history.jsonl` (when it exists) APPEND-ONLY
- No celebrating; numbers only

## Pending ideas

(see `cycle/pending_ideas.md` — empty so far; you can append before bed)

## Noticed but not fixed (already 3 items)

(see `cycle/noticed_but_not_fixed.md` — the `--fuzz=5` + `-l` follow-ups, diff_healer header reconstruction, trailing newline enforcement)
