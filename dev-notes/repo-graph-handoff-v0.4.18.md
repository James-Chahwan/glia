# Reverse handoff — glia → repo-graph (v0.4.18, 2026-07-08)

This is the mirror of `handoff-v6.md`. That doc took repo-graph's strategy INTO
glia. This one takes what glia SHIPPED back out to the repo-graph MCP wrapper.
Read it in the repo-graph session; act on repo-graph only (glia is done + released).

**glia is now at v0.4.18** (tag pushed, wheel builds as `repo-graph-py 0.4.18`).
Everything below is available once PyPI has 0.4.18 and repo-graph pins it.

> **Successor note (2026-09-16).** This is a point-in-time handoff and is kept as
> written — §0–§6 still describe repo-graph's work, which is unchanged. But glia did
> **not** stay still: the 2026-09-15 review
> (`dev-notes/review-2026-09-15-coverage-and-issues.md`) opened a multi-wave extraction
> programme (`dev-notes/wave-plan-2026-09-16.md`). Re-check **§7 "What is NOT done"**
> against the current git log before relying on it — items there may have shipped.

---

## 0. First, the mechanical chores (from the older backlog, still open)

- **Remove the stale vendored `rust/` fork** in repo-graph and depend on the
  published wheel instead. The fork has been drifting for several releases.
- **Pin `repo-graph-py >= 0.4.18`.** (Was blocked on the wheel publishing —
  v0.4.17 silently no-op'd on PyPI because `py/pyproject.toml` had drifted a
  release behind the workspace version; 0.4.18 fixes that in lockstep.)

---

## 1. What glia shipped this cycle (the release contents)

- **Tier-4 external doc ingestion** — Confluence/Notion/wiki pages → graph.
  `glia docs sync --space <KEY>` fetches to a local snapshot; the build ingests
  it deterministically → `DOC_SPACE`(44) + `DOC_SECTION`(42) + `DOCUMENTS`(6)
  edges. `glia docs push` writes pages back. (Confluence adapter live; Notion/
  wiki are the same seam.)
- **v6 P1 — substrate completeness: blind 48 → 0.** Every (framework × edge)
  cell in `bench/substrate-gap` extracts. The big flips: Dart/Go/Java/Swift/
  Python client HTTP → ENDPOINT→HTTP_CALLS; Angular/Java/C# constructor DI →
  INJECTS; TS/JS/Angular/React/Vue imports → real IMPORTS edges (were cells only).
- **v6 P2 — coverage / blind-spot signaling** (see §3).
- **v6 P3 — answer-shaped primitives** (see §2). Plus liveness (`live` flag) and
  multi-repo CLI.

---

## 2. The new engine surface repo-graph can now call (via `repo_graph_py`)

These are the P3 primitives — the answer-shaping the handoff asked glia to own so
the wrapper goes thin. All return a **JSON string** (array). On `PyGraph` from
`generate()` / `generate_many()`:

| method | signature | returns (per record) |
|---|---|---|
| `blast_radius` | `(qname, direction="both", depth=4, top_k=None, live_only=False)` | `{id, qname, name, kind, reason, depth, score, live, file, line}` |
| `cross_stack_trace` | `(feature, depth=6)` | `{depth, mechanism, cross_service, from_qname, to_qname, to_kind, to_file, to_line}` |
| `resolve` | `(text, kind="auto", top_k=None)` | `{id, qname, name, kind, score, file, line}` |
| `governing_docs` | `(qname)` | `{id, qname, name, kind, score, file, line}` (DOC_SECTIONs) |
| `coverage` | `()` | `{language, edge_category, note, verify, edges_found}` |

Semantics:
- **`blast_radius`** — complete, deduped, PPR-ranked, LOCATED closure. `reason` =
  the edge category that first reached the node ("why it's in scope"). Structural
  `imports`/`contains`/`defines` are EXCLUDED (no fan-out noise — the old `impact`
  problem). `live` = reachable from an entrypoint (route/handler/main/test/
  component); `live_only=True` drops likely-dead. `direction` ∈ forward
  (affects) / backward (affected-by) / both.
- **`cross_stack_trace`** — the ORDERED path across services with `mechanism`
  labels (CALLS/HTTP_CALLS/QUEUE_FLOWS/…) and `cross_service` per hop. Use for
  "how does this feature flow end to end".
- **`resolve`** — stacktrace/diff/test signal → ranked located nodes (feed
  straight to the agent, or use as the seed for `blast_radius`).
- **`governing_docs`** — "what are the rules for X?" — the doc sections that
  DOCUMENTS a symbol. (Equivalent to `blast_radius(x, backward)` filtered to
  DOCUMENTS; named for convenience.)
- **`coverage`** — see §3.

CLI equivalents (all take `--with <repo>` for cross-service, `--json`):
`glia blast-radius`, `glia trace`, `glia resolve`, `glia coverage`,
`glia docs-for`. Engine free functions if you bind more: `blast_radius_by_qname`,
`cross_stack_trace`, `resolve_signal_located`, `coverage_report`,
`governing_docs`, `entrypoint_reachable`, `locate_node`.

---

## 3. P2 coverage — surface it so graph+grep is DELIBERATE

`coverage()` returns, for the languages actually in the repo, the known
extraction caveats + how many edges of each flagged category exist. The universal
ones ("static analysis can't see dynamic dispatch / string-built URLs") plus
per-language (python urllib, TS custom HTTP wrappers, dart non-dio, Flask typed
converters). `edges_found = 0` on a flagged category = extra reason to grep.

**repo-graph action:** thread this into tool responses (or a dedicated tool) so
the agent knows WHERE the graph is blind and falls back to grep there — instead
of trusting a silent absence. This is the whole "graph+grep is reliable only if
the agent knows the blind spots" thesis, made mechanical.

---

## 4. P4 — the tool collapse (this is the repo-graph session's main job)

The handoff's P4: **collapse ~13 MCP tools → ~4**, powered by the now-complete
substrate + the P3 primitives. The primitives above are the enabler; the collapse
itself lives here. Recommended target surface (OPEN — refine as you build):

| tool | subsumes | powered by |
|---|---|---|
| **`orient`** | `status`, `dense_text` | existing status + dense_text |
| **`blast_radius`** | `find`, `impact`, `activate`, `neighbours`, most `read×N` | `PyGraph.blast_radius` (+ `live`, `coverage` ride along) |
| **`trace`** | `flow`, `trace`, cross-stack | `PyGraph.cross_stack_trace` |
| **`resolve`** | `locate`, stacktrace/diff entry | `PyGraph.resolve` → seeds `blast_radius` |
| **`read`** | `read` | existing (batch node source) |

`governing_docs` and `coverage` are probably response FIELDS / sub-modes rather
than top-level tools (keep the surface small — fewer schemas = less fixed
per-turn MCP tax, the P4 point). The exact 4-vs-5 and what folds into what is the
open design decision.

---

## 5. What in the wrapper is now SUPERSEDED (go thin)

The handoff said these repo-graph features "stay until the engine primitives (P3)
supersede them." They now do:

- **`path:line` locating** → every P3 record carries `file`/`line` from the
  engine (`locate_node`). Drop the wrapper's path-guess-by-extension.
- **`⊘` dead-code marker** → `blast_radius` records carry `live: bool` from the
  engine's `entrypoint_reachable`. Use that instead of the wrapper heuristic
  (which false-flagged DI before INJECTS existed — now fixed).
- **ranked `impact`** → `blast_radius` is PPR-ranked in-engine.
- **batch `read` / `node_cells` surfacing** → still wrapper-side, keep.
- **`--setting-sources project` isolation, recall/precision grader** → keep
  (test harness, not superseded).

Rule going forward (from `feedback_cross_benefit_belongs_in_glia`): anything of
cross-benefit belongs in glia; the wrapper is MCP transport/glue + presentation.
If you find the wrapper re-deriving something glia has, push it into glia instead.

---

## 6. Gotchas / lessons carried over

- **Version lockstep:** the wheel version is `py/pyproject.toml`, NOT Cargo.
  Bump it AND `[workspace.package].version` together or the publish no-ops on an
  already-existing PyPI version (this is what stalled 0.4.17 → PyPI).
- **Stale-wheel trap (local dev):** `maturin build` silently reuses a stale
  `.so`. Before trusting a rebuilt wheel: `cargo clean -p repo-graph-py
  -p repo-graph-engine` first, and verify `.so` mtime > source mtime.
- **Determinism:** the build is byte-identical; network doc-sync writes a local
  snapshot that the build reads (never the network in the build path). Keep any
  new network I/O out of the deterministic path.

---

## 7. What is NOT done (open)

- **P4 itself** — the tool collapse (this doc's §4). glia can't do it; it's here.
- **Liveness is best-effort** — `entrypoint_reachable` uses a generous entrypoint
  set (routes/handlers/main/test/components) to avoid false-dead. If you see a
  live node flagged `live:false`, the entrypoint set is missing a root type —
  extend `is_entrypoint` in glia (engine).
- **Notion / wiki doc adapters** — the Confluence seam is proven; the others
  reuse it, not yet built.
- **cross_stack_trace ranking** — returns BFS order, not ranked/multiple-paths;
  fine for v1.
