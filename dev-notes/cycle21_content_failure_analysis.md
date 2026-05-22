# Cycle 2.1/2.2 content-failure analysis (per-instance)

Substrate is now solid (no more NO-RUN due to env). Content failures are the next wall. Each remaining FAIL has a DIFFERENT root cause — not one fix for all.

## sphinx-10325 — plan-then-edit picked WRONG FILE

**Plan output (first 400c)**:
> 1. The bug is that the `:inherited-members:` option in `automodule` does not support ignoring multiple base classes.
> 2. The symbol carrying the bug is `sphinx::parsers::Parser::app` ...

**Diff**: `sphinx/parsers.py` (matches the plan's wrong target)
**Gold**: `sphinx/ext/autodoc/__init__.py`

**Root cause**: file_priors top-1 for sphinx IS `sphinx/ext/autodoc/__init__.py` (verified during cycle 1.5 work). But plan-then-edit's planning pass picked a different file based on its OWN reasoning, and the diff-emission pass followed the plan, not file_priors.

**Fix direction**: include file_priors evidence IN the planning prompt (not just as a parallel directive bullet). Force the plan to address: "the top-3 historically-touched files for this repo are X, Y, Z — does your plan target one of them, and if not, why?"

## matplotlib-22835 — IndentationError on emitted diff

**Plan**: identifies `BoundaryNorm.inverse` as bug carrier (reasonable — bug is in BoundaryNorm path)
**Diff**: edits `colors.py` at BoundaryNorm
**Apply**: applied (clean apply)
**Result**: `NO-RUN (E IndentationError: unexpected indent)`

**Root cause**: model emitted Python source with broken indentation INSIDE the diff. The diff applies (syntactically valid as a diff), but the resulting Python file fails to parse.

**Fix direction**:
- Post-apply syntax check before running tests — already exists as `synth_check` static_check.json
- BUT static_check.json said STATIC-OK — the model's edited code may have been OK in isolation but contextually broken (indent within an existing block)
- Need a stronger post-apply `python -c "import ast; ast.parse(open(file).read())"` check that catches indent errors

## matplotlib-22711 — A2 protected the WRONG qnames

**Directive structure** (key fragment):
> Causal chain step 1: `lib::matplotlib::widgets::RangeSlider::set_val`
> Required fix target (from test_expectation): `Line2D::get_xdata`, `Line2D::get_ydata`

**A2 took protected qnames from PRIMARY (test_expectation) block** → protected `Line2D` entries
**But the BUG is in `RangeSlider::set_val`** per the causal chain (deepest traceback frame)

So pool-cap protected the WRONG entries. RangeSlider::set_val was dropped from the pool because it wasn't in the "PRIMARY" block.

**Fix direction**:
- A2's qname source should be UNION of all directive blocks (causal_chain + test_expectation + prose_mention + traceback), not just PRIMARY
- OR weight by recency-in-traceback (deepest frame = highest weight)
- Either way, the directive PRIMARY block's targeting is contradicted by causal_chain's analysis — the composer should reconcile these BEFORE A2 reads from it

## sklearn-10508 — 155 warnings + 1 error during test collection

**Plan**: identifies `LabelEncoder.transform` (CORRECT — matches gold)
**Diff**: edits `sklearn/preprocessing/label.py` at transform()
**Apply**: applied
**Result**: `NO-RUN (155 warnings, 1 error during collection)`

**Diagnosis**: substrate is STILL partial. numpy<1.24 fixed np.int; cython<3 fixed pyx compile; setuptools<71 fixed pkg_resources. But there's a residual collection-time error. Need to grep the actual test_log.

**Fix direction**:
- Pull test_log.txt entirely for sklearn — see what the "1 error" is
- May need another substrate pin (scipy version? pytest version? six?)
- Or sklearn's pytest config has incompatibility with modern pytest

## Net summary

| Instance | Root cause | Fix axis |
|---|---|---|
| sphinx-10325 | Plan picked wrong file | Inject file_priors INTO plan prompt |
| matplotlib-22835 | IndentationError on emitted diff | Stronger post-apply syntax check (ast.parse) |
| matplotlib-22711 | A2 protected wrong qnames | A2 source = union of all directive blocks |
| sklearn-10508 | Residual substrate issue | Dig test_log for specific error |

**All four are distinct fixes.** No "one fix unlocks all". But each is targeted and shippable.

Pytest's win was: plan correctly identified target file + target function family → diff emission worked. The same chain breaks at DIFFERENT points for the four FAIL instances.
