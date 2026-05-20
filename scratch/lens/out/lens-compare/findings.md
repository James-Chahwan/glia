# Lens 4-condition compare findings · marshmallow-1359 · 2026-05-21

## Setup

Ran the slice-1.5 lens (`scratch/lens/target/release/lens`) at output-position 0 across 28 layers, four prompt conditions:
- `baseline`: default suffix (no directive)
- `iter1`: hand-written prescriptive suffix (cycle 0 win)
- `iter4`: graph-derived deepest-frame target + anti-targets
- `iter5`: graph-derived target + buggy line + exception parse

JSONLs at `scratch/lens/out/lens-compare/<cond>.jsonl`.

## Headline finding

Position-0 residuals (first generated token = "diff" in all conditions):

| Comparison | KL midpoint (layers 10-22) | Interpretation |
|---|---|---|
| baseline vs iter1 | **5-19** | iter1 perturbs prompt encoding heavily |
| baseline vs iter4 | 0.0-1.5 | iter4 nearly identical |
| baseline vs iter5 | 0.0-1.1 | **iter5 nearly identical** |
| iter1 vs iter5 | 1.5-8 | iter1 ≠ iter5 at position 0 |
| iter4 vs iter5 | 0.0-1.0 | iter4 ≈ iter5 at position 0 |

**iter5 PASSES F2P. Baseline FAILS. Yet their position-0 residuals are nearly identical.**

This contradicts the naive hypothesis that "the directive changes the model's processing of the prompt." For iter5, the directive's effect is INVISIBLE at position 0. It must operate later in generation, when the model is actually emitting the diff body.

## Per-layer top-1 token (selected layers)

| Layer | baseline | iter1 | iter4 | iter5 |
|---|---|---|---|---|
| 0 | meye | izzazione | meye | meye |
| 11 | 📐 0.73 | byss 0.21 | 📐 0.56 | 📐 0.52 |
| 17 | 📐 0.78 | .Disclaime 0.19 | 📐 0.78 | 📐 0.77 |
| 21 | diffs 0.11 | changes 0.43 | diffs 0.25 | diffs 0.18 |
| 23 | diff 0.41 | diff 0.55 | diff 0.40 | diff 0.36 |
| 27 | `` 0.44 | diff 0.83 | `` 0.60 | `` 0.66 |

Most layers iter4/iter5/baseline all show the same top-1 token. iter1 sits in a completely different region of embedding space.

## What this means

1. **The slice-1.5 lens is the WRONG diagnostic for multi-token generation differentiation.** It inspects residuals for ONE position (the model's prediction of the FIRST generated token). With marshmallow, the first generated token is "diff" in every condition because the suffix forces it. The pass/fail differentiator is in tokens 5-50, not token 1.

2. **iter4/iter5 steer the model via a different mechanism than iter1.** iter1 changes prompt encoding heavily (high KL vs baseline). iter4/iter5 barely change prompt encoding (low KL vs baseline) but still produce different final diffs. The auto-derived directives must operate during decoding, not prompt encoding.

3. **No "wasted channels" in iter5 detected via position-0 lens.** Position-0 lens shows iter5 ≈ iter4 ≈ baseline. iter5's extra channels (buggy line, exception class) must matter at the generated-tokens stage, not at prompt-token-0.

## Cycle 0.4 — autoregressive lens extension

Slice 1.5 lens needs to be extended to capture residuals during AUTOREGRESSIVE GENERATION, not just at prompt positions. Concrete:

- Run model: prompt → generate token 1 → append → generate token 2 → ... → EOS
- At each generated position, capture l_out-N residuals via cb_eval
- Compare baseline-gen vs iter5-gen residuals at generated positions 1, 2, 5, 10, 20, 50

This is where the directive's actual effect on the diff body lives. Requires extending `scratch/lens/src/runtime.rs::forward_capture` to do greedy generation with per-step residual capture. ~4-6h Rust.

## Cycle 0.3 immediate action (no lens needed)

Run iter5's auto-derived directive on the 4 traceback-having companion instances (matplotlib-22711, matplotlib-22835, sklearn-10508, plus 1 control with no traceback). The position-0 lens told us nothing actionable for steering iter5; the real test is whether iter5 GENERALIZES to other instances. That's an inference + F2P run, not another lens run.

## Honest counter-take

Maybe the position-0 lens IS showing something real and we should trust it:

- iter1's massive residual perturbation suggests it works by **changing how the model encodes the prompt**, while iter4/iter5 work by **changing how the model attends to specific suffix tokens during decoding**. Two different mechanisms, both successful.
- If iter5's mechanism is "attention-during-decode-to-specific-suffix-tokens," then a future stronger lever is **direct attention biasing at specific layers** (cycle 0.5+). The lens already showed us layers 22-26 (where the model commits to "diff") are the decision band.
- For pure-text-channel work, the directive seems to NOT need to perturb prompt encoding to work. So channel-pruning could remove channels with zero observable KL impact (iter6 candidate: ship a minimal iter5 with only target + buggy line, drop the exception-parse + anti-target list, see if pass holds).
