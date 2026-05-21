# Cycle 0.4 autoregressive lens findings · marshmallow-1359 · 2026-05-21

## Setup

Ran the new `lens --mode generate --max-new 64` (cycle 0.4) on two marshmallow conditions:
- `cycle-0-calib-fuzz5/` — default generic suffix (no directive)
- `iter5-graph-text-srcline/` — graph-derived directive prepended to suffix

Both use the same prefix + same summaries-aplus.json (which the lens doesn't inject — pool isn't the differentiator anyway, per [[reference-latent-pool-mechanism]]).

Output: per-(generated-token, layer) JSONL = 64 tokens × 28 layers = 1792 records per condition.

## Headline: divergence is at generated-token position 24, not position 0

| Position | Baseline emits | Iter5 emits | KL @ L27 |
|---|---|---|---|
| 0-22 | identical | identical | 0.00 |
| 23 | `index` | `@@` | 1.33 |
| **24** | `1` (of `index 12345...`) | `-` (of `@@ -1113`) | **11.67** |
| 25+ | hallucinated hex tokens | proper hunk + def + super call | 15-20+ |

The slice-1.5 prompt-position-0 lens showed KL≈0 between baseline and iter5 because the **first** generated token (`\`\`\``) is the same in both. The actual divergence is 23 tokens INTO generation.

## What baseline emits (positions 23-50)

```
``` diff
diff --git a/src/marshmallow/fields.py b/src/marshmallow/fields.py
index 12345678..abcdef0064
--- a/src/marshmallow/fields.py
+++ b/src/marshmallow/fields.py
[hallucinates further hex/numbers without coherent diff content]
```

The baseline emits an `index ...` line (forbidden by the suffix rules) and then can't recover — it loops back to re-emitting `--- a/...` and `+++ b/...` instead of writing real hunk content.

## What iter5 emits (positions 23-64)

```
``` diff
diff --git a/src/marshmallow/fields.py b/src/marshmallow/fields.py
@@ -1113,7 +1113,7 @@ def _bind_to_schema(self, field_name, schema):
     super()._bind_to_schema(field_name, schema)
     self.format = (
        ...
```

Iter5 follows the rules (no `index` line), writes a real hunk header at the directive's target line (1113), and emits the function context correctly.

## Mechanism revealed

The directive's effect is NOT primarily "point the model at the right function." It's:

**The suffix-text directive raises the salience of the suffix's rules during decoding. Baseline ignores the rules and hallucinates; iter5 obeys the rules and emits well-formed output.**

Evidence:
- Both conditions agreed on the diff header through position 22 — the model's ability to identify the file IS already present in baseline.
- Divergence happened at the FIRST POINT where the model has to choose between an "easy hallucination" (emit an `index` line, which it's seen in countless training-corpus diffs) and "follow the suffix rule" (skip to `@@`).
- Iter5's directive doesn't say anything about `index` lines explicitly — but its many strong directives ("Edit ONE function", "Do NOT modify", "Edit ONLY") make the model attend to suffix rules generally, including unrelated ones.

The targeting effect (iter5 targets DateTime._bind_to_schema correctly) is downstream of the well-formedness effect — once the model emits `@@ -1113` it's committed to a specific function's line range, so the directive's line-number signal lands.

## Cycle 0.5+ implications

1. **The directive's strength is its RULE-FOLLOWING enforcement, not its targeting per se.** A simpler experiment: would just adding "REMEMBER to follow ALL output rules above. Do not emit an index line." (no graph-derived directive at all) get marshmallow gold? That's testable cheaply (~5 min: hand-edit suffix.txt, re-run inference). If yes, the cycle 0.2/0.3 graph-derived directive is OVERKILL — the lever is just suffix-rule emphasis.

2. **For no-traceback instances (3 of 7 in cycle 0.3 FAILed because of this), the lever isn't graph-side at all** — it's whatever channel raises suffix-rule salience. The graph-derived content might be a force multiplier, but the rule-following IS the primary lever.

3. **Position-24 commitment matters more than prompt encoding.** A "real" latent injection that biases attention at the layer where the model commits to its first line (around layer 22-27, position 23-25 per this lens data) would be a much more surgical intervention than text channels. Cycle 0.5+.

## Cycle 0.5 candidate experiments (cheap)

| # | Experiment | Hypothesis | Cost |
|---|---|---|---|
| A | Replace iter5 directive with "REMEMBER: follow ALL output rules. Do not emit an index line." | If marshmallow still PASSes, the directive's targeting content was redundant; the lever is suffix-rule emphasis | 5 min hand-edit suffix + ~3 min inference |
| B | Strip just the "Do NOT emit an `index` line" rule from baseline suffix | If baseline now also passes, that rule is the SINGLE lever | 3 min |
| C | Run lens generate-mode on a no-traceback FAILed instance (django-10914) with a hand-crafted "raise the salience of the rules" directive | Tests whether the well-formedness lever transfers to non-marshmallow instances | 15 min |

## Cost / wall-clock

- Per condition: 64-token generation on 7B Q4 ≈ 12 min wall-clock (12 min for prompt-decode of 594 tokens + 64 × ~10s/token greedy generation).
- For comparing 2 conditions: ~25 min total.
- For comparing N conditions: N × 12 min.

Reasonable for tonight; not feasible for "scan 100 conditions × 21 instances" — that'd be hours per such matrix. Cycle 0.5 should focus on the 1-2 hypotheses most likely to invalidate the cycle 0.3 framing.
