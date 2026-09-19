# CODE_RULES.md (glia engine)

Operational conventions that emerged from cycles. Complements `CLAUDE.md` (architecture) — these are daily workflow rules. When in doubt, the conventions below have already been argued through and shipped; deviate only with stronger reason than convenience.

## 1. The `synth_*` bin pattern

Every directive / content extractor under `projection-text/src/bin/synth_*.rs` follows the same shape. Match it for any new bin.

**Args struct** (`#[derive(Parser)]`):
- `--src <PathBuf>` — repo source root
- `--text-out <PathBuf>` — markdown directive output (the primary channel)
- `--repo-canonical <String>` — default `"seeds"` (clap kebab-cases the field; see §2)
- Domain-specific inputs (e.g. `--issue`, `--test-patch`)
- `--max-targets <usize>` — cap on emitted bullets (heuristic ranking when over)

**Entry sequence**:
1. Read input file(s)
2. Call `build_repo_graph(&args.src, &args.repo_canonical)` from `glia_projection_text::driver_utils`
3. Build a tail-index (§5)
4. Resolve identifiers / line numbers against graph qnames
5. Emit `## Required fix target (graph-derived from <source>)` markdown block via `--text-out`
6. On no matches, write a single-line inert directive: `(no <thing> matched ...)` so the orchestrator can detect it

**Canonical reference**: `projection-text/src/bin/synth_traceback_target.rs` (the longest), `projection-text/src/bin/synth_test_expectation.rs` (the simplest).

**Cargo.toml**: every `[[bin]]` entry MUST set `required-features = ["research"]`. The `research` feature (named `driver` before 0.5.0) gates clap/serde/regex/walkdir/parser-python so the library stays slim for the pyo3 layer. A synth pass that other code runs in-process is also a `SynthHook` in `projection-text/src/hooks.rs`, and its bin runs that hook through `ActivationPlan::synthesize`.

## 2. clap kebab-case gotcha

`#[arg(long)]` on a Rust field `repo_canonical` becomes `--repo-canonical` on the CLI (snake → kebab is automatic in clap derive).

**Rule**: when one Rust bin invokes another via subprocess, the arg name in `Command::new("…").arg("--repo-canonical")` MUST use the kebab form. Passing `--repo_canonical` silently fails with clap exit code 2 — the composer hit this in cycle 0.6 (commit `6543564` fix).

**Test**: any new orchestrator that subprocess-spawns synth bins should be smoke-tested against a fixture; clap exit-code 2 = "unrecognized flag" = check kebab-vs-snake first.

## 3. `driver_utils` API

`projection-text/src/driver_utils.rs` is the single entry to graph construction for synth bins. Never rebuild the .gmap path directly; always go through:

- `build_repo_graph(src, repo_canonical) -> Result<RepoGraph>` — parses, builds, logs `[parse] N files / [graph] M nodes, K edges` to stderr
- `extract_position_cell(node) -> Option<&str>` — JSON payload `{file, start_line, end_line}` (forward slashes, 0-indexed lines from tree-sitter)
- `extract_code_cell(node) -> Option<&str>` — source text payload
- `read_json` / `write_json` — pretty-printed serde_json IO

`RepoGraph` has `nodes: Vec<Node>` (deterministic order) and `nav: NavigationIndex` with `qname_by_id: HashMap` (random hasher — never iterate). For qname → node lookups iterate `graph.nodes` and consult `nav.qname_by_id` per node.

## 4. POSITION cell semantics

Payload: `{"file":"<rel_path>","start_line":N,"end_line":N}`. Paths are repo-relative with forward slashes. Lines are **0-indexed** (tree-sitter convention).

Tracebacks are 1-indexed; SWE-bench gold patch hunk headers are 1-indexed. **One line convention (LD.1): stored POSITION rows are 0-based; the `line` of every answer record (blast-radius / trace / resolve / docs-for / contracts, CLI and pyo3) and `nodes_json`'s `start_line` / `end_line` are 1-based, converted once in `Locator::locate` (engine `answers.rs`).** A record builder takes `Located.line` as-is and never adds 1 again; code comparing a 1-based line (traceback frame, hunk header) against a stored row subtracts 1 first, as `resolve_frame` does (`line_1based - 1`). ENDPOINT_HIT / JSON ROUTE_METHOD cells store a 1-based `line`; `endpoint::http_node_span` turns it into a row.

Path matching for traceback frames and changed-file seeds is **path-suffix preferred, basename fallback** (A6.9): `resolve_frame` / `resolve_file` first take every node whose POSITION path is a `/`-boundary-aligned suffix of the query (or vice versa, so an absolute frame `/repo/svc_b/utils.py` matches the repo-relative POSITION `svc_b/utils.py` while `svc_a/utils.py` does not), and only fall back to bare-basename matching when that pass is empty. A query with no directory component therefore behaves exactly as the old basename-only rule did. Windows `\` is normalised to `/` on the query side; POSITION is canonical (repo-relative, forward slashes). Repo-relative full paths are used for non-traceback resolvers.

## 5. Tail-index pattern

Identifier → graph qname resolution uses a tail-index built once per graph:

```rust
fn build_tail_index(graph: &RepoGraph) -> BTreeMap<&str, Vec<(NodeId, &str)>> {
    let mut out = BTreeMap::new();
    for n in &graph.nodes {  // Vec, deterministic
        let Some(&kind) = graph.nav.kind_by_id.get(&n.id) else { continue };
        if kind != METHOD && kind != FUNCTION && kind != CLASS { continue }
        let Some(qn) = graph.nav.qname_by_id.get(&n.id) else { continue };
        let tail = qn.as_str().rsplit("::").next().unwrap_or(qn);
        out.entry(tail).or_default().push((n.id, qn.as_str()));
    }
    out
}
```

Canonical: `projection-text/src/bin/synth_call_chain.rs:218-237`. Used identically in synth_test_expectation, synth_prose_mention.

**Tie-break**: shortest qname wins (least nested = closest to public surface), then lexical. Cap with `--max-targets` (default 5-6).

**STOP list**: when extracting identifiers from text, filter Python keywords + common nouns (`self`, `cls`, `test`, `assert`, `pytest`). See `synth_test_expectation.rs:84-90`.

## 6. The composer pattern (subprocess orchestration)

`synth_directive.rs` is the canonical example. To compose multiple synth bins:

1. Define a `--bin-dir` arg (default: derived from `std::env::current_exe`'s parent)
2. For each channel, build `Command::new(bin_dir.join("<bin_name>"))`, forward `--src`, `--repo-canonical`, source inputs, and `--text-out <channels_dir>/<channel>.md`
3. Read each emitted block, score (4 per resolved bullet, +2 for "Edit ONE" phrasing, +1 per channel-specific bonus, -1 per inert marker)
4. Sort by score; positive-score blocks compose into the final directive; primary verbatim, secondary blocks stripped-of-H2-header and appended under "## Additional graph-derived signals"

Scoring is **heuristic, not load-bearing** — see `project_b4_composer_scoring_needs_tuning` memory. Cycle 0.7+ evidence will tune the weights.

## 7. Append-only files (NEVER rewrite)

| File | Convention |
|---|---|
| `bench/lens/cycle/cycle_log.md` | INSERT-AFTER-H1 markdown summary per cycle |
| `bench/lens/cycle/results_history.jsonl` | One JSON line per instance per cycle |
| `bench/lens/cycle/marshmallow_log.md` | Iteration trace per marshmallow cycle |
| `bench/lens/cycle/cycle-<N>-results.jsonl` | Per-cycle aggregated results |

**Rule**: corrections land as new entries with explicit `CORRECTED:` markers, never as edits to existing lines. The append-only convention prevents accidental reinterpretation of results during analysis. Git history is the recovery mechanism.

## 8. Honest-failure framing

Cycle entries report exact pass/fail/error counts without softening:

> ✗ "pass=1 / fail=6 / error=0 / total wall=1328s"
> ✗ NOT: "mostly working, needs tuning"

Positive milestones use 🟢 PASS emoji + strong framing ("Real glia win.") but only after F2P + regression suite confirms. **No BREAKTHROUGH claims before PASS ≥ 5/7 on the working set** (per `reference-overnight-loop-conventions`).

Negative results are first-class evidence. The cycle 0.5 A/D FAILs are explicitly preserved in memory as evidence of what the lever isn't.

## 9. Sacred holdout

`bench/lens/manifests/holdout.json` contains 10 instances NEVER touched until cycle 1.0 validation. Every cycle driver asserts loop-set ∩ holdout = ∅ at start (`run_cycle_loop.sh:32-46`). Violation = abort.

Memory references (`reference_swebench_lite_gold_patches`, `reference_overnight_loop_conventions`) state this in stronger terms. The holdout is the only defense against overfitting to the 21-instance working set.

## 10. Inference stack rules

- **llama.cpp only** (no candle, no Python in the inference loop). Python is allowed in eval harness (`run_instance.py`, `eval_specs.py`), the latent injection path (`run_llama_pathB.py`), and per-cycle drivers.
- **No JSON for injection** when there's a graph-side alternative. The latent JSON pool (`summaries-aplus.json`) is a Rust↔Python boundary to refactor away (`feedback_no_json_for_injection`). Graph-derived text directives are the current sanctioned channel; per-token embed injection is the next.

## 11. Lens runtime conventions

`bench/lens/src/runtime.rs` exposes the `LensRuntime` trait. Two impls:

- **FakeRuntime** — synthetic embeddings for testing the pipeline end-to-end without llama.cpp.
- **LlamaCppRuntime** — uses `ggml_backend_sched_eval_callback` (cb_eval) to capture residual streams.

**cb_eval rules** (from `reference_llamacpp_cb_eval_pattern`):
- Per-op callback fires for every tensor; filter by name (`l_out-N`, `result_norm`, `result_output`)
- cb_eval fires per **ubatch**, not per forward — accumulate by appending rows, not overwriting
- Per-layer captured-row count varies (late layers may have logits=1 row) — use `row = captured_rows - 1 - offset_from_end` formula
- KV cache must be cleared between passes: `llama_memory_clear(llama_get_memory(ctx), true)`
- Quantized tensors (output.weight is Q4_K_M) require type-aware dequant via `ggml_get_type_traits().to_float`; element count is `ne[0]*ne[1]*ne[2]*ne[3]`, not `nbytes/4`

**Write access**: returning `false` from the cb_eval callback indicates "I modified this tensor"; ggml-backend then re-uploads it. This is the surgical write path for K/V cache injection at L25-27 (Bundle C3 / spitball B3).

## 12. JSONL schema (lens)

`bench/lens/src/jsonl.rs::LensStep`:

```rust
struct LensStep {
    layer: u32,
    run: String,           // "baseline" or "with_injection"
    position: u32,         // 0 = first generated token
    top_k: Vec<TopKEntry>,
    gold_token_id: Option<u32>,
    gold_rank: Option<u32>,     // 0-indexed; 0 = top-1
    gold_prob: Option<f32>,
    kl_vs_baseline: Option<f32>,// only with_injection rows
    #[serde(skip_serializing_if = "Option::is_none")]
    instance_id: Option<String>,  // batch aggregation tag
}
```

Line-delimited, `#[serde(tag = "kind", content = "data")]` envelope (`{"kind":"LensStep","data":{...}}`) mirrors neuropil's RecordedFlow shape.

## 13. CycleHarness contract

`bench/lens/cycle/run_cycle_loop.sh <cycle_tag> [model]` — drives the 7-instance loop set.

- Calls `run_instance.py --instance-id … --split … --model … --tag cycle-<N>` per instance
- Parses last line of `bench/latent/out/instance_results.jsonl` for F2P field
- Appends to `cycle/cycle-<N>-results.jsonl` AND `cycle/results_history.jsonl` (both append-only)
- Updates `cycle_log.md` via INSERT-AFTER-H1 summary block

**Tag convention**: `cycle-<N>` where N is the cycle number (e.g. `cycle-0.7`). Per-instance workdirs land at `bench/latent/out/inst-<id>-<model>-cycle-<N>/`.

## 14. Memory system rules

Memory file rules live in the auto-memory system prompt (and `MEMORY.md` indexes them). Quick rules for this repo:

- **NEVER** save code patterns, file paths, or git-derivable facts as memory
- **DO** save: feedback (user corrections + validated approaches), project state (cycle results, plans, deadlines), references (canonical pointers)
- Bracketed `[[memory-name]]` links freely; they may dangle until the linked memory is written
- Memory is for cross-conversation continuity. Within-session state belongs in plan files (`/home/ivy/.claude/plans/<name>.md`) or task lists

## 15. CLAUDE.md split

This file is **operational conventions**. CLAUDE.md is **architecture** (workspace layout, .gmap format, parser-vs-graph split, design decisions, roadmap). Don't duplicate; cross-reference. New rules emerging from cycles land here. New architectural decisions land in CLAUDE.md or `dev-notes/glia-memory/`.

## 16. Bin invocation conventions

Workspace-local binaries live at `target/release/<name>` after `cargo build --release -p <crate> --features research --bin <name>`. Tests, harness scripts, and the orchestrator (`run_instance.py`) reference these by absolute path with a `<bin>.exists()` check before invocation — never assume `$PATH` includes target/release.

Output paths: `/tmp/<descriptive>.md` for one-off smoke tests; `<workdir>/<channel>.md` for per-instance pipeline output (cleaned up by run_instance.py if it owns the workdir).

## 17. Public API types (LD.9)

Adding a field to a public struct breaks every struct literal of it outside the crate. So every struct and enum that the engine or graph facade makes public, and that is defined in that crate, must meet one of three conditions. "Makes public" means named by a `pub use` in `engine/src/lib.rs` or `graph/src/lib.rs` (explicit, single or `*` glob), or declared in a `pub mod` slot there. The three conditions:

- **`#[non_exhaustive]`**. Outside the crate, callers get the type from its producing function (`generate_one`, `blast_radius_by_qname`, ...) or from `Default` plus field assignment. Derive `Default` when a caller builds one, as the cli tests do with `ServiceMap`. A struct literal and `..Default::default()` are both refused (E0639). A `match` on an enum needs a wildcard arm. Constructing a variant is still allowed.
- **A private field**: the type is already unconstructible outside the crate (`ParseCache`, `Locator`, `HttpRouteMatcher`).
- **An allowlist entry** in `engine/tests/api_stability.rs`, with its reason:
  - The data model: `MergedGraph`, `RepoGraph` and `SymbolTable`. The store and the tests build these by literal, and LC.2 owns their shape.
  - Unit resolver structs pass by rule, because the engine constructs them.
  - Re-exports from another crate are skipped. `ActivationConfig` and `DomainProfile` stay exhaustive because domains build them by literal.
  - The rest are stopgaps, each naming the test that builds the type by literal. Delete an entry once its type is marked.

`cargo test -p glia-engine --test api_stability` enforces this. It prints `[api_stability] scan types=… offenders=…`, and on failure it lists each offender as `file:line`. It also fails loudly on a stale allowlist entry or on a facade shape it cannot follow. Four `compile_fail` doctests prove that the attribute refuses a literal: `GenerateResult`, `BlastAnswer`, `ServiceMap` and `BlastHit`. Each snippet is only the literal with every current field, because stable rustdoc does not check the error code. So when you add a field to one of those four types, add it to its snippet too.

---

*Last updated: 2026-05-21, cycle 0.6 Rust session. Owners: glia engine + bench/{lens,latent,3d-viewer}.*
