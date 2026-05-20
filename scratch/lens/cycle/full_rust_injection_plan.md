# Full Rust injection pipeline — away from JSON

Plan for cycle 0.3+. Written 2026-05-21 night while James asleep. Tractable cycle-0.2 piece built on top (see `synth_traceback_target.rs` below).

## Current state (with the JSON dependency that needs to die)

```
.gmap (binary, rkyv)
  ↓
[10 Rust synth bins in projection-text/src/bin/]
  ↓
seeds.json / summaries.json / summaries-aplus.json / source_cells.json     ← JSON BOUNDARY
  ↓
run_llama_pathB.py (Python, llama-cpp-python)
  ↓
out.txt (raw diff text)
  ↓
diff_healer.py + apply_and_test (Python; touches pytest, must stay Python)
```

The JSON boundary exists because the Rust synth bins output JSON (their CLI contract) and `run_llama_pathB.py` reads JSON. Everything before JSON is Rust; everything after is Python. The injection MEDIUM is JSON.

## What "away from JSON" looks like

Two architectural options. Both lose the JSON intermediate. They differ on where the Rust↔Python boundary sits.

### Option A — all-Rust through inference (full move)

```
.gmap → Rust synth bins (in-memory, no JSON) → Rust generation bin (llama-cpp-sys-2) → out.txt → apply_test (Python)
```

- Replaces `run_llama_pathB.py` with a Rust binary (`synth_generate.rs` or extend `scratch/lens/` to do full generation, not just 2 forward passes).
- Rust binary owns: .gmap read, activation, synth cells, latent embed, decode loop, write out.txt.
- Python is gone from injection. Stays only for apply_test (pytest harness).
- Scope: substantial. scratch/lens/ already proves the FFI works; needs greedy/sample loop + chat template + EOS detection + the embedding-pool splicing. ~6-10 hours of Rust.

### Option B — binary intermediate (incremental move)

```
.gmap → Rust synth bins → BINARY pool file (rkyv) → Python reads via rkyv-py → embed → splice → out.txt → apply_test
```

- Rust still emits a file; just rkyv-binary instead of JSON.
- Python (run_llama_pathB) reads via `rkyv-py` (Python bindings to rkyv) or msgpack.
- Scope: smaller. ~2-3 hours of Rust + Python.
- Doesn't lose the file-on-disk intermediate but removes JSON.
- Trade-off: still has the file-boundary, but the format is binary + zero-copy mmap.

### Recommendation

**Option A is the right end state.** Glia owns the inference loop, neuropil consumes outputs, Python is reserved for pytest harness only. Aligns with `[[feedback-runtime-stack-llamacpp-only]]` (Rust + llama.cpp for speed).

**Option B is the right intermediate step** if Option A blows scope. Ship binary intermediate first → port run_pathB to Rust later.

## Implementing the L1-L5 candidates GRAPH-SIDE

With the JSON pipeline gone, each L candidate becomes a new pass in Rust:

| # | Candidate | Implementation | Crate / file |
|---|---|---|---|
| **L1** | Target cell from attribute-access-mismatch | New `synth_target_directive.rs`: scan activated nodes for METHOD body using `arg.Y` where `arg`'s type doesn't define `Y`. Synth a target cell. Requires type-inference on graph. | `projection-text/src/bin/synth_target_directive.rs` |
| **L2** | Traceback-driven seed boost | New `synth_traceback_target.rs`: parse issue.txt for `File "...", line N` patterns; match against POSITION cells (`[[reference-position-cells]]`); emit high-rank target cells. NO type inference needed. **Tractable tonight.** | `projection-text/src/bin/synth_traceback_target.rs` |
| **L3** | Multi-cell coordinated directive | Extend L1 to emit (target, problem, fix-path) triples. Three cells per detected mismatch. | extends L1 |
| **L4** | Activation-layer score boost from traceback | Modify `activation` crate: seed mass boost on traceback-matched nodes BEFORE PPR runs. PPR propagates the boost through the graph natively. | `activation/src/lib.rs` |
| **L5** | Anti-target cells | New `synth_antitarget.rs`: detect siblings of target with similar name/signature; emit "do not edit" cells. | `projection-text/src/bin/synth_antitarget.rs` |

L2 + L4 are the cleanest graph-side interventions because they use existing primitives (POSITION cells, PPR seeds). L1/L3/L5 require new analyses.

## Tonight: synth_traceback_target.rs (L2 implementation)

Built into `projection-text/src/bin/`. Reads (CLI args):
- `--src <repo_path>` — the .gmap location is implicit
- `--issue <issue.txt>` — the SWE-bench issue text
- `--seeds <seeds.json>` — to identify the activated set
- `--out <summaries-aplus.json>` — appends target cells

Logic:
1. Parse issue text with regex for Python traceback: `File "(.+)", line (\d+), in (\w+)`
2. For each match, search graph nodes (via .gmap) whose POSITION cell satisfies:
   - file path ends with matched file
   - start_line ≤ matched line ≤ end_line
3. For each matched node:
   - Build a synth target cell:
     ```
     {
       "id": <synth id>,
       "qname": "synth::Target::<original-qname>",
       "score": 999.0,
       "summary": "TARGET: <qname> at <file>:<line>. The issue traceback names this function. Fix the bug here, not in similarly-named siblings."
     }
     ```
4. Append target cells to the input summaries-aplus.json (compat with existing pipeline)

OUTPUT IS STILL JSON. But the CONTENT is graph-derived (POSITION cells matched against traceback). When the full Rust refactor lands, this binary's output becomes a binary tuple stream instead.

### Why this is graph-side, not text-side

- The targeting comes from POSITION cells living in the .gmap (graph data).
- The mapping from traceback line → node is a graph lookup (POSITION cell range containment).
- No hand-written directive text per instance; the directive is auto-synthesized from graph data.
- Works for ANY SWE-bench instance with a traceback in the issue. Generalizes.

### Test plan

1. Build `synth_traceback_target.rs`.
2. Run for marshmallow: outputs augmented summaries-aplus.json.
3. Re-run `run_llama_pathB.py` with augmented pool + GENERIC suffix (NO prescriptive text).
4. Heal + apply + F2P.
5. If PASS with generic suffix → graph-side win, latent injection controls target.
6. If FAIL → run lens to diagnose at what layer model commits to wrong target.

## Cycle 0.3+ roadmap (in priority order)

1. **L4: PPR seed mass boost** (~2h) — purest graph-side intervention. Modify `activation/src/lib.rs` to take a seed-mass override. seed_from_issue or a Python wrapper computes the override from traceback. PPR redistributes mass natively.
2. **Option B binary intermediate** (~3h) — rkyv pool file. Removes JSON without rebuilding inference.
3. **L1: type-mismatch detection** (~4-6h) — needs lightweight type inference on the graph. The hardest L candidate.
4. **Option A all-Rust inference** (~6-10h) — extend lens crate or new generation bin.
5. **L3 + L5** (~2h each after L1 lands) — extensions of the type-mismatch pass.

## Pyo3 path (alternative, not in scope tonight)

run_llama_pathB.py could call into glia via `repo-graph-py` pyo3 bindings to fetch activated cells in-memory. The `py/` crate at `/home/ivy/Code/glia/py/` already exposes `MergedGraph` operations via `[[project-glia-upgrade-spec-g14-to-g22]]` (added `cluster_key_for`, `node_id_by_qname`, etc.). Extending it to expose "activated nodes + their CODE cells" is ~30 LOC of pyo3.

This avoids the binary-intermediate path entirely. Python calls Rust functions directly, gets `(qname, score, cell_text)` tuples in-process. No file boundary at all.

Probably the cleanest middle-ground: Rust owns the data + analysis, Python orchestrates the inference loop (it already has llama-cpp-python tuned), pyo3 is the bridge. Python becomes a thin orchestrator.

## Decision matrix for morning James

- If you want to GO FAST tonight: synth_traceback_target.rs (L2 tractable) lands + tested by morning. JSON pool still exists; the content gets smarter.
- If you want CLEAN ARCHITECTURE first: Option A all-Rust inference. Pause new candidates until that lands. ~1-2 sessions.
- If you want INCREMENTAL: Option B binary intermediate first, then L1/L3/L5 in Rust on top of it. ~3-4 cycles.

**My recommendation**: ship L2 (synth_traceback_target.rs) tonight as the cycle 0.2 deliverable. It's a real graph-side win, testable on marshmallow tonight, and lays the pattern for L1/L3/L4/L5 in the same architecture. Then morning-James decides between Option A (big refactor) and continuing L-candidate buildout on the existing JSON pipeline.
