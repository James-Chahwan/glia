# Marshmallow-1359 iteration log

Cycle 0 loop until gold passes. Newest at top. Append-only.

## iter1 — prescriptive suffix · 2026-05-21 06:1X · 🟢 PASS

**Hypothesis:** the model has the diagnosis (derived_notes already says `DateTime inside List → no .opts`) AND the access path (`self.root.opts` listed) in the prefix, but generic suffix lets it default to a wrong-target edit. Prescriptive suffix that names the function + the substitution should fix it.

**Change:** Replaced the default suffix.txt in `inst-marshmallow-code__marshmallow-1359-7b-q4-iter1-prescriptive/` with a prescriptive block:
- "Edit ONE function: `DateTime._bind_to_schema` in `src/marshmallow/fields.py`."
- "Use `self.root.opts` instead (Field.root walks .parent until outermost Schema)."
- "Edit ONLY src/marshmallow/fields.py. Do NOT modify List._bind_to_schema."

**Result:**
- generation: 108 tok in 57.9s (1.9 tok/s)
- model output:
```diff
-        or getattr(schema.opts, self.SCHEMA_OPTS_VAR_NAME)
+        or getattr(self.root.opts, self.SCHEMA_OPTS_VAR_NAME)
```
- targets `DateTime._bind_to_schema` correctly (line 1113 model / 1117 actual)
- apply: applied with `patch -p1 --forward --fuzz=10 -l` (whitespace + indent mismatch needed `-l`)
- F2P: `tests/test_fields.py::TestParentAndName::test_datetime_list_inner_format PASSED`
- regression suite (`pytest -rA tests/test_fields.py`): **77 passed, 0 failed, 1 warning**

**Comparison:**
| Run | apply | f2p | reg | reg_fail |
|---|---|---|---|---|
| Default suffix (4 prior history) | applied | FAIL | 76 | 1 |
| Default suffix (tonight 2026-05-21) | applied(fuzz) | FAIL | 60 | 16 |
| iter1 prescriptive | applied(fuzz) | PASS | 77 | 0 |

**Generalizable sage-feature derived:**
The prefix's `derived_notes` already contains "X has no Y attribute" + access-paths section lists candidate paths resolving to Y. Pipeline should auto-emit a **Required fix location** suffix block synthesizing these into a directive. The model has the diagnosis; it just needs to be told to *use* it.

Concrete: extend `run_instance.py::run_pipeline` (or wherever the prompt assembly is) to detect cases where derived_notes has an `X has no Y` pattern + access_paths contains a `self.Z.Y` resolution, then auto-emit a prescriptive suffix block. ~50 LOC in Python.

**Decision:** STOP iteration. Gold passes on marshmallow with iter1. The lever is real. Next cycle (0.2+) builds the auto-derivation. Wake-up state captured in INTERRUPT_STATE.md.

**Artefacts:**
- prefix: `inst-marshmallow-code__marshmallow-1359-7b-q4-iter1-prescriptive/prefix.txt` (UNCHANGED from default — proves prefix already had everything needed)
- suffix: `inst-marshmallow-code__marshmallow-1359-7b-q4-iter1-prescriptive/suffix.txt` (the prescriptive change)
- diff: `inst-marshmallow-code__marshmallow-1359-7b-q4-iter1-prescriptive/out.txt`
- healed patch: `/tmp/iter1.patch`

## iter0 — calibration baseline · 2026-05-21 06:0X · 🔴 FAIL

- Default suffix, current pipeline, fuzz=5 fix in run_instance.py:904 (after discovering GNU patch 2.8 default fuzz too strict)
- F2P FAIL (0/1), reg=60, reg_fail=16
- Model produced wrong-target diff (renamed param in List._bind_to_schema, not DateTime)
- See `[[reference-marshmallow-1359-diff-anatomy]]` for why
