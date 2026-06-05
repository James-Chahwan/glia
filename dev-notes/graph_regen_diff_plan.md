# Graph-regenerated diff (Lever #4) — implementation plan

The highest-leverage remaining edit-content lever. Bypasses the model's raw-diff
emission step entirely; model emits a **structured replace-block spec**, glia's
tree-sitter machinery locates the target qname's AST node, replaces its body
with correct indentation/geometry, and emits the resulting unified diff via
`git diff`.

## Why this lever now

Cycle 3.0 holdout data (first 5 instances) shows the dominant failure mode:

| Instance | Error | Hunks |
|---|---|---|
| xarray-4248 | post-apply SyntaxError: unexpected indent | 1 |
| flask-4045 | post-apply SyntaxError: bare-`*` arg | 2 |
| astropy-14995 | post-apply SyntaxError: invalid syntax | 2 |

3/3 APPLY-FAILs are post-apply syntax errors. Fix #4 (ast.parse check) catches
the symptom; this lever **prevents** the failure by ensuring the diff is
geometry-correct by construction. Model never writes diff syntax; glia does.

Also: PASS instances (marshmallow / django / pytest) all used single-hunk
emissions. Multi-hunk is correlated with breakage. The structured spec format
trivially expresses N independent edits without requiring N hunk geometries to
be individually correct.

## Locked design decisions (from session 2026-05-23 Q&A)

1. **Spec format**: structured replace-block per edit (file + qname + new body)
2. **Multi-edit**: list of N independent specs, each self-contained
3. **Language coverage v1**: Python only (all SWE-bench Lite is Python)
4. **On failure**: fall back to existing raw-diff emission path

## Spec format (model-facing)

```
## Edits (N)

### Edit 1
file: src/flask/blueprints.py
target: Blueprint::__init__
```python
def __init__(self, name, import_name, ...):
    # full new body, properly Python-indented at column 0
    self.name = name
    ...
```

### Edit 2
file: src/flask/blueprints.py
target: Blueprint::register
```python
def register(self, app, options):
    ...
```
```

**Parser rules**:
- Each `### Edit K` block has 3 required fields: `file:`, `target:`, fenced code block
- `target:` uses glia `::` qname convention (`Class::method`, `module::func`, `Class::Inner::method`)
- Module-level targets: special `target: MODULE` (defer to v1.1)
- Fenced block must be `python` and contain a syntactically valid function/method/class definition starting at column 0
- Decorators included in the fenced block ARE preserved on replacement

## Components

### 1. New Rust bin: `projection-text/src/bin/synth_graph_diff.rs`

**Args**:
- `--src <repo>` — repo working tree
- `--spec <path>` — model's emitted spec file (stdin if `-`)
- `--out-diff <path>` — write resulting unified diff here
- `--out-err <path>` — write failure reason if regen fails
- `--validate-ast` — run ast.parse on synthesized file before emitting diff (default on)

**Algorithm**:
1. Parse spec into `Vec<EditSpec { file, qname, new_body }>`
2. For each spec:
   a. Read `<repo>/<file>` source
   b. Parse with tree-sitter-python (use existing parser from `parsers/code/python`)
   c. Locate the qname's node:
      - Walk class/function definitions
      - Match by qname segments (`A::B::method` walks `class A` → `class B` → `def method`)
      - Include `decorated_definition` wrapper if present
      - Async functions (`async def`) are `function_definition` with first child `async`
   d. Get byte range of matched node (start of decorator if any, end of body)
   e. Detect indent context (column of the matched node's start)
   f. Reindent `new_body` from column-0 to match
   g. Splice into source (byte-range replace)
3. Write all modified files to disk (each only once even if multiple specs touch it)
4. If `--validate-ast`: ast.parse each touched .py file — fail if any SyntaxError
5. `git diff` in repo to produce unified diff
6. Write diff to `--out-diff`; on any failure, write reason to `--out-err` and exit non-zero

**Build location**: `projection-text/Cargo.toml` adds `synth_graph_diff` as a
`[[bin]]` entry. Reuses `repo-graph-parsers-code-python` crate dep for
tree-sitter and Python qname walking helpers.

### 2. Tree-sitter qname resolution

Existing pattern in `parsers/code/python/src/lib.rs:34` and `:1390` shows the
language setup. New helper module `projection-text/src/graph_diff/locate.rs`:

```rust
pub struct Located {
    pub byte_start: usize,       // includes decorator if present
    pub byte_end: usize,
    pub indent_col: usize,       // column of definition start
}

pub fn locate_qname(source: &str, qname: &str) -> Result<Located> {
    // Parse with tree-sitter-python
    // Split qname on "::" → segments
    // Walk top-level definitions matching first segment by `name` field
    // Recurse into class body / function body for remaining segments
    // Return byte range + indent column
    // Error if not unique or not found
}
```

**Uniqueness**: if more than one node matches (e.g. two `def __init__` in
sibling classes both named `Foo` — extremely rare), error out. Better to fall
back to raw diff than silently pick the wrong one.

### 3. Indent correction

Model emits new body at column 0 (top-level Python). Glia must re-indent to
match the target's context (4 spaces per class nesting level).

```rust
fn reindent(body: &str, target_indent_col: usize) -> String {
    // 1. Find minimum leading-whitespace among non-empty lines of body
    //    (should be 0 for well-formed model output, but be defensive)
    // 2. Strip that prefix from all lines
    // 3. Add target_indent_col spaces to all non-empty lines
    // 4. Preserve blank lines as-is
}
```

**Edge cases**:
- Body contains mixed tabs+spaces → reject (force ast.parse failure)
- Body's first non-blank line has non-zero indent → strip relative indent before re-adding target
- Trailing newline preserved if present

### 4. Prompt template (graph-diff suffix)

New suffix template, env-gated by `GLIA_GRAPH_DIFF=1`. Replaces the diff-emission
section of plan-then-edit. Plan step unchanged.

```
You've analyzed the bug. Now emit the fix as a structured edit specification.

Format:
  ## Edits (N)

  ### Edit 1
  file: <path/to/file.py>
  target: <Class::method>  (use :: between qname segments)
  ```python
  <full new function/method body, starting at column 0>
  ```

Rules:
- Emit ONLY the new function body, not a diff.
- Start every function/method at column 0 (we re-indent for you).
- Include decorators if the function has them.
- One ### Edit block per function you're changing.
- The target must be the EXISTING qname; we will replace its body.

## Edits (
```

Open paren at end primes the model to fill in count.

### 5. Integration in `run_instance.py`

New env: `GLIA_GRAPH_DIFF=1`. Order of operations:

```python
if os.environ.get("GLIA_GRAPH_DIFF") == "1":
    # 1. Build graph-diff suffix (planning step same as plan-then-edit)
    # 2. Run inference → spec.txt
    # 3. Run synth_graph_diff:
    sh([
        str(GLIA / "target/release/synth_graph_diff"),
        "--src", str(repo_dir),
        "--spec", str(spec_path),
        "--out-diff", str(out_path),
        "--out-err", str(workdir / "graph_diff_err.txt"),
        "--validate-ast",
    ])
    # 4. If exit 0 and out_path non-empty → use as the diff
    # 5. If exit non-zero → fall through to existing raw-diff emission
```

Fallback path: when graph-diff fails, re-invoke `run_llama_pathB.py` with the
existing raw-diff suffix. Logs the fallback reason for analyzer ingestion.

### 6. Validation gates

Three gates inside `synth_graph_diff`:
1. **Spec parses**: all `### Edit K` blocks have file + target + fenced block
2. **Qname locates**: every spec's target qname resolves to a unique node
3. **ast.parse clean**: synthesized file parses as Python

Any gate failure → exit non-zero with reason in `--out-err`.

A fourth gate runs in `run_instance.py`:
4. **Diff applies**: `git apply --check` on the synthesized diff (sanity — should
   always pass since we generated it via `git diff`, but cheap to verify)

## File map

```
projection-text/
  Cargo.toml                                          edit (add synth_graph_diff bin)
  src/
    graph_diff/
      mod.rs                                          NEW
      locate.rs                                       NEW (tree-sitter qname resolution)
      reindent.rs                                     NEW (indent correction)
      spec.rs                                         NEW (spec format parser)
      apply.rs                                        NEW (byte-range splice + git diff)
    bin/
      synth_graph_diff.rs                             NEW (CLI driver)

bench/latent/out/
  run_instance.py                                     edit (GLIA_GRAPH_DIFF flag + integration)
  prompts/graph_diff_suffix.py                        NEW (suffix builder)

bench/lens/cycle/
  cycle_3_1_graph_diff.sh                             NEW (re-run holdout with GLIA_GRAPH_DIFF=1)

dev-notes/
  graph_regen_diff_plan.md                            THIS FILE
```

## Smoke + validation sequence

### Phase A: bin works in isolation
1. Build `cargo build --release -p repo-graph-projection-text --bin synth_graph_diff`
2. Hand-craft a spec.txt for a known-good holdout instance (e.g. xarray-4248
   with the GOLD function bodies)
3. Run `synth_graph_diff --src <repo> --spec spec.txt --out-diff diff.patch`
4. Verify diff is identical (up to whitespace) to the gold patch from SWE-bench
5. Verify `git apply --check diff.patch` succeeds

### Phase B: integration works on cycle 2.1 sentinel
1. Set `GLIA_GRAPH_DIFF=1` in run_instance.py invocation
2. Run on **marshmallow-1359** (known PASS sentinel) — should still PASS
3. Run on **pytest-7373** (cycle 2.1 PASS) — should still PASS
4. If either regresses, the format/prompt isn't ready

### Phase C: holdout re-run (cycle 3.1)
1. Re-run 10 holdout instances with `GLIA_GRAPH_DIFF=1` + full lever stack
2. Compare APPLY-FAIL counts against cycle 3.0
3. Expected: 3/3 APPLY-FAIL post-apply-SyntaxError instances flip — at minimum
   to FAIL/PASS (no longer syntax errors)
4. If they flip to FAIL: edit-content quality is the remaining wall (Lever #4
   solved the geometry problem, not the semantics problem). Different lever
   needed for that.
5. If they flip to PASS: large generalization win

### Phase D: cross-cycle comparison
Run failure_mode_classifier on cycle 3.1; expected distribution shifts:
- APPLY-FAIL ↓ significantly (geometry-error class eliminated)
- NO-DIFF ↑ slightly (spec format more constrained; model may refuse on hard cases)
- FAIL ↔ (edit-content quality unchanged; orthogonal)
- PASS ↑ if some cycle 3.0 APPLY-FAILs would have been correct edits

## Risk register

| Risk | Mitigation |
|---|---|
| Model can't learn structured spec format → NO-DIFF explosion | Phase B sentinel catches it; fall back to raw diff (env flag off) |
| tree-sitter-python misses qname for edge cases (nested closures, lambdas, dynamic class attrs) | Limit v1 to top-level functions + class methods; fall back for misses |
| Indent correction breaks multi-line strings / heredocs | ast.parse catches it; gate prevents bad emission |
| New-function inserts (no existing qname) unsupported | v1 limitation; document in spec format. Most bugs are MODIFY not INSERT |
| Decorator handling subtle (multi-decorator, decorator with args) | Tree-sitter `decorated_definition` covers all cases; include full wrapper |
| Module-level constant edits (e.g. `__version__ = "1.2.3"`) | v1 doesn't support; defer to MODULE target in v1.1 |

## What this lever does NOT solve

This lever ONLY fixes the **geometry/syntax** failure class. It does NOT help when:
- Model picks the wrong target qname (sphinx-10325 case → Fix #2 file_priors)
- Model emits semantically wrong code (correct file, wrong logic → needs Lever #6 partial-pass rerank or self-distillation)
- Substrate issues (sklearn-10508 → substrate fixes)

The expected uplift is ~2-3 instances on holdout (the 3 post-apply-SyntaxError
APPLY-FAILs), possibly converting them to PASS if the model's body was
semantically right and only geometrically wrong. xarray-4248's
"unexpected indent" smells exactly like this — model wrote correct logic with
wrong indent depth.

## Sequencing

1. Build synth_graph_diff bin (Phase A) — Rust crate work, no inference
2. Wire run_instance.py integration (Phase B) — Python, env-gated
3. Sentinel run (marshmallow + pytest) — confirm no regression
4. Cycle 3.1 holdout re-run (Phase C) — measure uplift
5. Failure-mode analyzer on cycle 3.1 — confirm geometry class eliminated

## Links

- `[[project-cycle-30-launched-4-targeted-fixes]]` — cycle that surfaced the pattern
- `[[project-per-instance-content-failure-root-causes]]` — the 4 distinct root causes
- `[[feedback-plan-then-edit-markdown-section-trap]]` — sibling prompt-format hazard
- `dev-notes/cycle21_content_failure_analysis.md` — sibling analysis
