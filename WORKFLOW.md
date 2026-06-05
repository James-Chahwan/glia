# WORKFLOW.md (glia engine)

How a working session runs. Three docs, three jobs — don't duplicate, cross-reference:

| Doc | Answers |
|---|---|
| `CLAUDE.md` | **What the system is** — architecture, workspace layout, `.gmap` format, parser-vs-graph split, roadmap |
| `CODE_RULES.md` | **How the code is written** — synth_* bin pattern, clap kebab-case, append-only files, inference-stack rules |
| `WORKFLOW.md` (this) | **How a session runs** — the orient→work→record loop, knowledge routing, pre-flight, house style |

Run **`/orient`** at the start of every session. It executes the Orient checklist below and prints a one-screen state-of-the-world before any code is touched.

This file travels with the repo through the v0.5.0 `git filter-repo` split, same as `CLAUDE.md`.

---

## 1. The session loop: Orient → Work → Record

### Orient (session start — `/orient` runs this)

1. **repo-graph MCP `status`** — load the structural map (node/edge counts, kinds, entry points). Ask the graph before grepping or reading files. For a feature flow use `activate` from seed qnames; for "what calls X" use `trace`/`neighbours`; for full context use `dense_text`.
2. **mempalace `status`** (loads the palace overview + AAAK dialect), then **`search` / `kg_query`** the subsystem you're about to touch. *Query before you speak* — never assert a fact about past decisions, people, or cycles from memory; verify it.
3. **Native auto-memory** is loaded into context automatically (its own `MEMORY.md` indexes it). Skim it for live cycle/sprint/project state relevant to the task.
4. **Open work** — read `dev-notes/issues_surfacing_now.md` plus the relevant `dev-notes/*_plan.md`. These are the live to-do surface.
5. **Constraints** — skim the sections of `CLAUDE.md` (architecture) and `CODE_RULES.md` (operational) that govern the subsystem you'll touch. Confirm what's *locked* before you change anything (see §4).
6. **If the task is a ≥100-instance cycle** — run the Pre-flight checklist (§3) and paste its output *before* launching. Do not launch on faith.
7. **State the orientation** — one screen: where we are, what you'll touch, what's locked, what could bite. Then start.

### Work (the discipline)

- **Follow `CODE_RULES.md`.** Extract-vs-resolve split is locked (parsers extract, graph resolves); mirror the `synth_*` bin pattern; kebab-case clap args across subprocess boundaries; no `unwrap()`/`panic!()` in non-test code; `required-features = ["driver"]` on every synth `[[bin]]`.
- **Verify-driven, every time.** Ship a grep-able **`fired_on` marker** with every feature/lever/flag. If you can't grep the logs for proof it ran, assume it's silent dead code. No bare `try/except … pass` wrapping a whole feature — it hides the corpse.
- **Scope in LOC / tokens / GPU-mem — never in time.** Group options by capability family, mechanism, or expected uplift. No `~1h`, no `~2 days`, no week-by-week.
- **Honest-failure framing.** Report exact counts: `pass=1 / fail=6 / error=0 / wall=1328s`, never "mostly working." No BREAKTHROUGH claims before PASS ≥ 5/7 on the working set **and** the regression suite confirms.
- **Push back on bad sequence.** If a requested step depends on an earlier one that would break, say so explicitly — not as a neutral "option A vs B." The push-back is wanted.
- **Don't invent ceremony.** When the ask is "simplify," the scope *is* the simplification — no PR-count breakdowns, phase plans, or timelines unless asked.

### Record (session end)

- **`dev-notes/issues_surfacing_now.md`** — mark issues resolved, add ones that surfaced. This is the handoff surface for the next session.
- **Append-only result files** (`CODE_RULES.md §7`) — `cycle_log.md`, `results_history.jsonl`, per-cycle JSONL, `marshmallow_log.md`. Corrections land as *new* entries with `CORRECTED:` markers. Never rewrite a line; git history is the recovery path.
- **Native auto-memory** — write durable `feedback` / `project` / `reference` facts and add the one-line pointer to its `MEMORY.md`. Never save code patterns, file paths, or git-derivable facts (`CODE_RULES.md §14`).
- **mempalace** — `diary_write` what happened / what you learned; `kg_add` new entity facts; `kg_invalidate` the old fact when something changed (don't just add a contradiction).
- **Walkthrough before any push or publish** — summarize files changed, validation results, and the exact shell commands, then get a greenlight. This applies to `git push`, `twine`/`maturin` upload, and `gh release`.

---

## 2. Knowledge routing — four systems, one rule each

Four overlapping systems hold project knowledge. Route by *kind of fact*, not by habit.

| Fact kind | Read from | Write to |
|---|---|---|
| **Live cycle / sprint state** (PASS/FAIL, wall, regr, current plan) | Native auto-memory (`MEMORY.md` + topic files) — authoritative for *now* | Native auto-memory at session end |
| **Architecture contracts** (NodeKind/EdgeCategory/CellType IDs, `.gmap` format version, registries) | `dev-notes/glia-memory/reference_*.md` for the frozen v0.4.x spec; native auto-memory for v5+ upgrades (the shipped contract wins) | Native auto-memory (`glia_vN_shipped`) — **not** dev-notes (frozen) |
| **Design rationale / gotchas / principles** | `dev-notes/glia-memory/feedback_*.md` (frozen, principled); native auto-memory for new discoveries | Native auto-memory `feedback_*` |
| **Cross-session narrative / who-knows-what / decision history** | mempalace `kg_query` (structured) + `search` (semantic) | mempalace `kg_add` + `diary_write`; `kg_invalidate` on change |
| **Code structure** (call graph, qname resolution, scope, impact) | repo-graph MCP (`status`/`activate`/`trace`/`neighbours`/`dense_text`) | **Never** — generated, rebuild-whole-file only |

**Decision tree.** Current cycle/sprint state → native memory. Locked spec (IDs, format) → dev-notes `reference_*` first, native memory for upgrades. "What calls what / where does this qname resolve" → repo-graph MCP (always, before grep). "Who decided X, when" → mempalace `kg_query`. A gotcha or principle → dev-notes `feedback_*` (old) or native memory (new).

**The repo-graph → glia rename seam.** Native memory and mempalace were populated while the project was called *repo-graph*; the engine is renaming to *glia* at v0.5.0. When searching either system for state, grep **both** terms. `dev-notes/glia-memory/` is frozen at the 2026-04-18 snapshot and will *seed* the standalone glia repo's memory after the filter-repo split — don't write new architecture into it now; it would go stale on split.

---

## 3. Pre-flight — before any ≥100-instance cycle

Bench launches have burned us repeatedly by skipping these. **Codify them as a script and run it; until the script exists, run each check by hand and paste the output.** Do not launch on faith.

1. **Disk + inode budget.** Purge stale `inst-*` workdirs from prior cycles first. The MFS per-user **inode quota** silently returns 0-byte writes when exhausted (a manifest writes "successfully" as empty, the spawn exits clean) — verify with a real 50KB write test before launching. Do the disk math for N instances × repos.
2. **Repo coverage.** Derive the repo list from the manifest/parquet, **not** a hardcoded list — full SWE-bench Lite is 18 unique repos, and a missing one isn't noticed until a shard reaches it. Pre-clone all of them.
3. **Local mirror for clones.** `ensure_repo` must clone from `/workspace/repos/<basename>` (1–3s) and only fall back to upstream. GitHub anonymous clone rate-limit (~60/hr) dies fast under sharding (7 shards × hundreds of clones).
4. **N_CTX cap.** Qwen 2.5 Coder 14B has `n_ctx_train=32768` (not YaRN-extended like 7B/32B). **Cap N_CTX ≤ 32K for 14B** — newer llama-cpp hard-errors above train ctx instead of warning.
5. **Per-shard GPU pinning.** Set `CUDA_VISIBLE_DEVICES=$SHARD_IDX` in each shard's per-instance env — the pathB child does not inherit `fim_daemon`'s `--gpu-index`, so all shards otherwise pile onto GPU 0 and OOM.
6. **Restart after every live fix.** A running daemon/shard froze `os.environ.copy()` at spawn — a live `pip install` or `.py` edit does **not** reach it. Kill + restart after any live fix. After pushing a `.py`, clear `__pycache__` (MFS mtime is stale, so new spawns load the old `.pyc`).
7. **Single-instance dry-run.** Run one instance end-to-end (parse → directive → inference → apply → test) before the full fan-out. A clean dry-run catches deploy blockers cheaply.
8. **Holdout integrity** (`CODE_RULES.md §9`). Assert `loop-set ∩ holdout = ∅` at cycle start; abort on violation. The 10-instance holdout is the only defense against overfitting the working set.

---

## 4. House-style rules (durable)

The behavioral conventions that have already been argued through. Deviate only with a stronger reason than convenience.

| Rule | Why |
|---|---|
| **Ship a `fired_on` marker with every feature** | If you can't grep proof it ran, it's silent dead code. Multiple shipped levers turned out inert for cycles. |
| **Scope in LOC/tokens/GPU-mem, not time** | Time estimates have been wrong every time; capability/mechanism framing is what's actionable. |
| **Honest-failure framing** | Exact `pass/fail/error` counts. Negative results are first-class evidence, not something to soften. |
| **No BREAKTHROUGH before ≥5/7 PASS + regression** | Premature wins poison the cycle record and the memory that reads it. |
| **Append-only result files, `CORRECTED:` markers** | Prevents accidental reinterpretation of results during analysis; git is the recovery path. |
| **Pre-flight before ≥100-inst cycles** | Every bench gotcha (inode, coverage, N_CTX, GPU pin, restart) has bitten us mid-run and cost money. |
| **Walkthrough before push/publish** | Surface files + validation + exact commands and get a greenlight before anything outward-facing. |
| **Push back on dependency-breaking sequence** | Flag it explicitly when step N depends on an un-done N−1; don't present it as a neutral choice. |
| **Facts over polish** | Don't refine on assumptions — run the thing and let the output decide. Verify-driven, not plan-driven. |
| **Memory discipline** | Save feedback / project / reference; never code patterns, file paths, or git-derivable facts. |
| **Respect locked invariants** | NodeKind/EdgeCategory/CellType u32 IDs, `::` qname separator, `.gmap` rebuild-whole-file — renumbering breaks persisted files. See `CLAUDE.md` + `CODE_RULES.md`. |
| **llama.cpp-only inference; no JSON injection when a graph-side channel exists** | `CODE_RULES.md §10`. Graph-derived text directives are the sanctioned channel. |

---

*Owners: glia engine + bench/{lens,latent}. New session-rhythm rules land here; architecture lands in `CLAUDE.md`; code conventions land in `CODE_RULES.md`.*
