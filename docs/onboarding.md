# Onboarding — glia + scratch/lens

Sibling doc to neuropil's `ONBOARDING.md`. Same hire, second repo. Skim top to bottom, then come back to the sections that matter for what you're about to touch.

If you haven't read the neuropil onboarding yet, read that one first — it frames the two-repo split from the consumer side. This one is the substrate side.

---

## The 60-second pitch

**glia is a cross-language, cross-service code graph engine.** Rust workspace. You point it at one repo or many, it walks them with tree-sitter, runs ~30 framework extractors and 13 cross-graph resolvers, and emits a unified graph: every function/class/route/CLI command/DB table/IaC resource as a node, every call/handles-by/import/HTTP-stack/queue-flow/shared-schema as an edge. Output is a zero-copy `.gmap` binary, a dense-text projection for LLM context windows, JSON, or a pyo3 wheel.

**neuropil is the consumer.** Sibling repo. It path-depends on glia's crates (`repo-graph-{core,code-domain,graph,store,activation,projection-text,engine}`) and renders the graph as the 3D scene you've already toured. The MCP server at `github.com/James-Chahwan/repo-graph` wraps the same wheel and exposes it to LLM agents.

Together they answer: *"if a graph of the whole codebase exists as a first-class artefact, what do LLMs, IDEs, impact analyses, and observability surfaces look like when they read from it instead of grepping?"*

**Substrate is the moat.** Sourcegraph/ctags index single repos. Snyk scans dep lists. k9s browses k8s. None of them give you `route → handler → DB table → infra image → consumed-env-var` as one walkable graph. glia does. Everything else (the MCP, neuropil, the SWE-bench arm) is downstream.

What that looks like in numbers (45-repo cross-service eval, this release):

```
13,371 nodes / 14,105 intra-edges / 2,789 cross-edges in 3.1s
99-repo sweep median: 1.4s parse+resolve per repo
99-repo sweep max: elasticsearch (342k nodes / 336k edges) in 73s
```

Single laptop CPU. No daemon, no index server, no curation step.

Two threads of active work past the v0.4.x substrate:

- **v0.4.14 perf** — per-graph-area incremental rebuild (currently rebuild-whole-file every commit), iterator parallelisation (rayon over parse+extract+resolve), cleanup.
- **scratch/lens + scratch/latent** — the SWE-bench latent-injection arm. logit-lens crate (`scratch/lens/`), candle-fork model with `forward_input_embed` hook (`scratch/latent/`, excluded from default workspace), gold-aligned auto-driver pipeline. Single-instance SOLVE on marshmallow-1359 with a 7B Q4 model; generalising it is open research. This is the substrate underneath neuropil's deferred Inspect→LYR sub-mode.

Three angles that map to your background:

- **DevOps** — the cross-graph resolver layer is *the* DevOps surface here. HTTP / gRPC / Queue / SharedSchema / EventBus / WebSocket / DB / Cron / Config / IaC / Package — 13 of them, written as `CrossGraphResolver` impls. The first 6 are mature; **Cron and DB-NoSQL/Graph are the meatiest open ones**. IaC currently reads raw Dockerfile + k8s manifests; Kustomize/Helm rendering is explicit non-goal in v0.4.x but a natural v0.5+ ask. See `TODO.md` Day 1 for the locked list.
- **LLMs** — `projection-text/` is how a sub-graph becomes context for a 7B-or-larger model. `activation/` is Personalised PageRank (HippoRAG lineage) over the graph: seed nodes → PPR → top-K reachable. `scratch/lens/` is logit-lens over llama.cpp via cb_eval — captures residual streams per layer per ubatch. `scratch/latent/` is the parked candle arm where graph-derived pooled vectors get injected into the embedding stream. Your masters work plugs into the open research question at the end of this doc.
- **CAD / spatial** — glia itself is non-visual. Your CAD instinct shows up in the rkyv `.gmap` format (sharded by kind, owned-vs-archived discipline, zero-copy mmap), the locked NodeKind/EdgeCategory u32 ID registry, and the deterministic graph build (same input repo → same graph, no randomness). The visual layer is downstream in neuropil.

---

## The two-repo split (from this side)

```
~/Code/glia/                              ~/Code/neuropil/
  └ tree-sitter + extractors + resolvers    └ consumes via path-deps
  └ .gmap store (rkyv + mmap)               └ renders as 3D scene
  └ activation (PPR) + projection-text      └ chat panel + agent loop
  └ pyo3 wheel + CLI                        └ Monitor mode + probes
  └ scratch/{lens,latent} R&D               └ deferred Inspect→LYR consumer
```

**Rule:** treat them as separate Claude Code sessions. When work crosses repos, write a short spec in `~/.claude/plans/` and let the sibling session pick it up. We've learned the hard way that one agent juggling both gets confused.

The crate prefix `repo-graph-*` is intentional and locked — that's the PyPI/crates.io brand. The repo is named `glia`; the crates aren't. v0.5.0 may eventually unify; not in v0.4.x.

Only `py/` publishes to PyPI (as `repo-graph-py`). Everything else is internal workspace.

---

## Workspace layout

```
glia/
├── core/                  Node, Edge, QName, RepoId, NodeId. Domain-agnostic.
├── code-domain/           Code registries: 40 NodeKind IDs, 31 EdgeCategory IDs, CellType.
├── graph/                 Per-repo builder, universal resolver, all 13 cross-graph resolvers,
│                          MergedGraph, PPR activation glue.
├── store/                 .gmap binary container (rkyv + mmap, atomic write, sharded).
├── projection-text/       Dense sigil projection for LLM context. ~12 synth_*.rs bins.
├── activation/            Personalised PageRank, domain-agnostic. ActivationConfig is the API.
├── engine/                Orchestration glue. Used by py/ and cli/.
├── cli/                   `glia` binary (analyze / impact / merge / build / install-hooks).
├── py/                    pyo3 bindings. PUBLISHED as repo-graph-py wheel.
├── parsers/code/
│   ├── python/  go/  typescript/  rust/  java/  csharp/  ruby/  php/  swift/
│   ├── c_cpp/   scala/  clojure/  dart/  elixir/  solidity/  terraform/
│   ├── react/   angular/  vue/             — stacked on typescript
│   └── extractors/                          — cross-cutting: HTTP routes, gRPC, queues,
│                                              WebSocket, EventBus, GraphQL, CLI,
│                                              data_sources, data_entities, cron, config,
│                                              IaC, packages, ts_routes, react/angular/vue routes
├── scratch/
│   ├── lens/              logit-lens crate (cb_eval, residual capture, per-layer JSONL).
│   │   ├── src/           LensRuntime trait + Fake/LlamaCpp impls, jsonl.rs schema.
│   │   ├── cycle/         per-cycle results + append-only cycle_log.md / results_history.jsonl.
│   │   ├── manifests/     instance manifests incl. sacred holdout.json (NEVER touch).
│   │   └── scripts/       run_cycle_loop.sh + analyzers.
│   ├── latent/            candle-fork model + forward_input_embed hook. EXCLUDED from
│   │                      default workspace (cargo build skips candle download).
│   │                      Build with `cargo build -p repo-graph-latent`.
│   ├── 3d-viewer/         experimental 3D viewer (excluded).
│   └── tui-viewer/        experimental TUI (excluded).
├── tests/                 fixture-driven smoke tests: http_stack_smoke, go_smoke, ts_smoke, ...
├── dev-notes/
│   ├── glia-memory/       seed memory files for when this repo becomes its own auto-memory.
│   ├── graph_regen_diff_plan.md
│   ├── incremental_gmap_plan.md
│   ├── cycle_35_36_final_report.md          (recent SWE-bench cycle report)
│   ├── eval-substrate-2026-05-05*.md         (substrate eval)
│   └── ...
├── CLAUDE.md              architecture rules — load-bearing
├── CODE_RULES.md          operational conventions (synth_* bin pattern, kebab-case, append-only files)
├── TODO.md                v0.4.x ship plan (Day 1 substrate / Day 2 surface)
├── README.md              the public face
└── LICENSE                Glia Software License v0.1 (PolyForm Noncommercial + worker overlay)
```

Top-level rule: parsers extract, graph crate resolves. Parsers emit raw `ExtractedItems` with `UnresolvedRef`s; the graph builder walks the tree to turn those into concrete edges uniformly across languages. Locked at v0.4.3b.

---

## Vocabulary you'll bump into immediately

| Term | What it means |
|---|---|
| **QName** | Qualified name. `::`-separated, language-agnostic. `marshmallow::fields::List::_bind_to_schema`. Locked separator. |
| **NodeKind / EdgeCategory** | u32 IDs in `code-domain/`. Locked at v0.4.x. ROUTE=5, ENDPOINT=9, HANDLED_BY=9, CALLS=4, HTTP_CALLS=10, CLI_COMMAND=21. Renumbering breaks every persisted `.gmap`. |
| **RepoGraph / MergedGraph** | RepoGraph = one repo. MergedGraph = N repos joined by cross-graph resolvers. The CLI's `merge` builds the latter. |
| **CrossGraphResolver** | Trait every cross-service resolver implements. 13 of them today. `HttpStackResolver` is the canonical example to copy. |
| **ExtractedItems / UnresolvedRef** | What a parser hands to the graph builder. Parsers don't resolve names; they extract everything the AST gives + emit unresolved refs. The graph walker turns those into concrete edges. |
| **Universal resolver** | The shared walker in `graph/` that handles `SelfMethod` → enclosing CLASS/STRUCT, etc. Languages contribute via a reserved `extra_hook` seam when generic walking isn't enough. |
| **.gmap** | The on-disk format. rkyv zero-copy + mmap, sharded by kind. Write-once: rebuild-whole-file, no in-place mutation. Owned vs Archived types are the mental model. Lives at `<repo>/.glia/`. |
| **Cell** | A typed JSON payload hanging off a node. `CellType::POSITION = {"file","start_line","end_line"}`, `CellType::CODE = source text`. Same registry as NodeKind/EdgeCategory, locked IDs. |
| **Activation / PPR** | Personalised PageRank, damping = 0.5. `ActivationConfig` exposes direction + edge weights + node specificity as the three dials. Domain-agnostic. HippoRAG-shaped: seed → propagate → top-K. |
| **Projection (dense text)** | Sigil-based text rendering of a sub-graph. Prefix/default/module dedup, scope collapse. Designed to pack the most graph signal per LLM context token. |
| **Synth bin** | A `projection-text/src/bin/synth_*.rs` binary. Each one takes some input (issue text, traceback, test patch), resolves identifiers against the graph, and emits a markdown directive. Composed by `synth_directive.rs`. Pattern is locked — see `CODE_RULES.md §1`. |
| **Composer** | `synth_directive.rs`. Subprocess-spawns synth bins, scores their outputs, composes a final directive. Scoring is heuristic; not load-bearing. |
| **Tail-index** | The standard identifier → qname lookup. Build once per graph, group by last `::` segment, tie-break by shortest qname. `CODE_RULES.md §5`. |
| **cb_eval** | llama.cpp's per-tensor backend callback. Used in `scratch/lens/src/runtime.rs` to capture per-layer residuals during inference. Fires per *ubatch*, not per forward — accumulate by appending rows. Quirks list in `CODE_RULES.md §11`. |
| **LensStep** | The JSONL schema in `scratch/lens/src/jsonl.rs`. Per-layer top-K + gold token + gold rank + KL-vs-baseline. Mirrors neuropil's `RecordedFlow` envelope shape. |
| **Cycle** | One end-to-end SWE-bench loop pass. Driven by `scratch/lens/cycle/run_cycle_loop.sh <tag>`. Append-only result files; corrections land as new entries with `CORRECTED:` markers, never overwrites. |
| **Sacred holdout** | `scratch/lens/manifests/holdout.json`. 10 SWE-bench instances NEVER touched until v1.0 validation. Every cycle driver asserts loop-set ∩ holdout = ∅ at start. Violation = abort. |
| **Sentinel** | A SWE-bench instance the pipeline has solved before — kept in the loop to detect regressions. marshmallow-1359 is the canonical one. |
| **driver feature** | A Cargo feature gating clap/serde/regex/walkdir/parser-python so the library stays slim for the pyo3 wheel. Every `synth_*` bin sets `required-features = ["driver"]`. |

---

## The cross-graph resolver layer (your DevOps surface)

13 resolvers, each implements `CrossGraphResolver`. The shape: every resolver builds an index from the merged node set, then emits cross-repo edges. They run after all per-repo graphs are built.

What's shipped + maps to things you've shipped before:

| Familiar concept | Where it lives here |
|---|---|
| HTTP service-to-service map | `HttpStackResolver` — frontend `Endpoint` (axios/fetch/HttpClient call) ↔ backend `Route` (Flask/Express/Spring/Axum/…). Path + method match. |
| gRPC service mesh | `GrpcStackResolver` — client stub call ↔ proto service definition ↔ server impl. microservices-demo benchmark: 175 edges. |
| Pub/sub | `QueueStackResolver`, `EventBusResolver`, `WebSocketResolver` — producer ↔ consumer across Celery / BullMQ / Sidekiq / Oban / NATS / RabbitMQ / Kafka / raw Redis BLPOP. |
| Shared schema | `SharedSchemaResolver` — same type imported by 2+ services (e.g. a shared TS type, a Python dataclass shipped via internal package). |
| Database flow | `DbResolver` — SQL `Table` / `Column` nodes from migrations + ORM model classes, joined when N services touch the same table. NoSQL `Collection` + GraphDB `NodeLabel` exist as the umbrella but are thinner. **Real open work: deepen NoSQL + GraphDB tracks.** |
| Cross-service env vars | `ConfigResolver` — `os.environ` / `process.env` / `std::env::var` reads ↔ Dockerfile `ENV` / k8s `env`/`envFrom` / `.env` files. Same key consumed in N services = cross-edge. |
| Image / manifest references | `IacResolver` — Dockerfile `FROM`/`EXPOSE`/`CMD` ↔ k8s Deployment/Service/Ingress/ConfigMap/Secret ↔ docker-compose. **Kustomize templating + Helm rendering deliberately out of scope in v0.4.x.** |
| Dependency graph | `PackageResolver` — manifests (`package.json`, `go.mod`, `Cargo.toml`, `pyproject.toml`, …) + lockfiles. Cross-language reachability is the differentiator vs Endor/Snyk/Socket.dev — they stop at manifest match. |
| Cron / scheduled jobs | `CronResolver` — partially shipped. Handles crontab files, k8s CronJob manifests, GH Actions `schedule:`, Celery beat, Sidekiq-cron, node-cron, Quartz. **Corpus-sparse in current eval (0 edges); needs cron-heavy fixtures to validate.** |
| CLI invocation | `CliInvocationResolver` — invocation site ↔ command definition. Similar corpus-sparse state (0 edges). |
| Frontend route resolution | extractor-layer: `ts_routes` (Next.js Pages + App Router, SvelteKit, React Router, Angular Router, Vue Router). Feeds HttpStackResolver. |

Gaps that are explicitly open + ownable (not scoped out, just unbuilt):

- **DB resolver — NoSQL + GraphDB depth.** SQL track is mature (Django/SQLAlchemy/ActiveRecord/Eloquent/Sequelize/TypeORM/Prisma/GORM/sqlx/EF Core). MongoDB / DynamoDB / Firestore / CosmosDB are present as Collection nodes but the model-class → collection edge story is thinner. Neo4j Cypher MATCH/MERGE label refs land as nodes but no query parsing. Picking one of these to deepen is a clean, scoped piece of work.
- **Cron resolver — corpus + eval.** The code exists; it has zero fired edges in the current 99-repo sweep because the corpus is cron-light. The work is twofold: assemble a fixture of 3-5 cron-heavy repos (any Rails app with `whenever`, a k8s CronJob-heavy infra repo, a Sidekiq-cron consumer), validate firing, fix any extractor gaps that surface.
- **Org-internal-package routing.** Today when service A imports `@mycorp/shared-models`, glia creates a phantom external node. The real ask: detect that it's a sibling repo's published artefact and route to the actual definitions. Deferred to v0.5+ (depends on `glia merge` workflow), but worth understanding the shape now.
- **Effect classification.** Single static-reasoning pass tagging every node with `pure / read-state / write-state / network / fs / db / time / nondeterministic`. Compose along call edges. Optional v0.4.x deliverable, more likely v0.5. Visible win: `glia analyze` Mermaid diagrams gain edge annotations.

What's deliberately out of scope of v0.4.x (don't fight these unless you've read why):

- **No intra-procedural data-flow / taint analysis.** CodeQL territory. v0.5+ refinement.
- **No vulnerability matching against CVE feeds.** Snyk territory.
- **No source-level fix suggestions** — glia emits substrate; reasoning over it is downstream.
- **No Kustomize/Helm rendering.** Raw manifest only.
- **No daemon (`glia serve`).** Killed. CLI + post-commit hook is the shape.
- **No multi-repo manifest format / `glia discover`.** v0.4.x ships positional-args `glia merge` only. Workflow patterns get designed from real usage signal, not speculation.

---

## The LLM angle

Three threads to know about:

1. **Projection-text + synth bins.** `projection-text/` renders any sub-graph as compact sigil text for an LLM context window. The `synth_*` bins are domain-specific channels — `synth_traceback_target` resolves a traceback into "Edit ONE: `pkg::Class::method`" directives; `synth_test_expectation` does the same from a test patch; `synth_prose_mention` from natural-language identifiers; `synth_call_chain` walks call-edges from a seed. `synth_directive.rs` composes them. All follow the same locked shape — see `CODE_RULES.md §1, §6`.

2. **Activation as retrieval.** `activation/` is Personalised PageRank, damping 0.5, domain-agnostic. Code-domain wiring: edge weights + direction + node specificity in `graph/src/activation_glue.rs`. The HippoRAG lineage matters — PPR-over-structural-graph is meaningfully different from prose-extracted entity-relation triples. The downstream consumer in neuropil is "given a seed, light up the most-related nodes"; in the SWE-bench loop it's "given an issue, narrow to the K most-likely edit targets".

3. **scratch/lens + scratch/latent — the parked research arm.** This is the deep end and it's yours to engage with as much as you want.
   - **`scratch/lens/`** — logit-lens crate. Two `LensRuntime` impls: `FakeRuntime` (synthetic embeddings, end-to-end pipeline testing) and `LlamaCppRuntime` (real residual capture via `ggml_backend_sched_eval_callback`). The cb_eval quirks list (`CODE_RULES.md §11`) is non-obvious — per-ubatch accumulation, late-layer row-count variance, KV-cache clearing between passes, type-aware dequant for Q4_K_M. Read those rules before touching the runtime.
   - **`scratch/latent/`** — candle-forked qwen2 with `forward_input_embed` hook. The arm that injected graph-derived pooled vectors into the embedding stream. **Single-instance SOLVE on marshmallow-1359 with Qwen 2.5 Coder 7B Q4**, reproduces deterministically via the gold-aligned auto-driver. Excluded from default workspace; build explicitly with `cargo build -p repo-graph-latent`.
   - **Why parked:** the N=50 follow-up surfaced ~80% of apply-then-test failures were infra (pytest collection, import errors, wheel mismatches), not model output quality. Clean cross-instance results need apply/test-runner hardening. Bench inference subsequently moved from candle to llama.cpp (`scratch/latent/out/run_llama_pathB.py`, ~7× faster CPU decode + GBNF-constrained decoding). The candle fork stays for replay.
   - **The open research question (verbatim from `README.md`):** *"Given a graph + a problem + a query, what's the correct distillation over composition / sage-filtering / synthesised cells / pooled vectors that lets a 7B model do what a 70B model can do? There's a shape out there connecting static reasoning, query-specific context selection, and capability lifting. It hasn't fully connected yet."*

   This is where your masters work plugs in. The substrate ships independently; the reasoning layer above it is under design. v0.5+ will probably look very different from v0.4.13's latent-injection arm. "More vectors" isn't the answer; "smarter selection of what to feed where" is the hypothesis.

---

## The cycle loop (how SWE-bench work runs day-to-day)

`scratch/lens/cycle/run_cycle_loop.sh <cycle_tag> [model]` drives the 7-instance loop set.

What it does:
- For each instance in the loop manifest: `run_instance.py --instance-id … --split … --model … --tag cycle-<N>`
- Parses the F2P (fail-to-pass) field from the last line of `scratch/latent/out/instance_results.jsonl`
- Appends to `cycle/cycle-<N>-results.jsonl` AND `cycle/results_history.jsonl` (both append-only)
- Updates `cycle_log.md` via INSERT-AFTER-H1 summary block

Conventions that are load-bearing:
- **Append-only result files.** Corrections land as new entries with explicit `CORRECTED:` markers, never as edits to existing lines. Git history is the recovery mechanism.
- **Honest-failure framing.** Cycle entries report exact pass/fail/error counts without softening. ✓ `"pass=1 / fail=6 / error=0 / total wall=1328s"`. ✗ `"mostly working, needs tuning"`. No BREAKTHROUGH claims before PASS ≥ 5/7 on the working set.
- **Sacred holdout.** `scratch/lens/manifests/holdout.json` is 10 instances the loop **never** touches until v1.0 validation. Loop-set ∩ holdout = ∅ asserted at cycle start. Violation = abort. This is the only defense against overfitting to the 21-instance working set.
- **Tag convention.** `cycle-<N>` where N is the cycle number (`cycle-0.7`, `cycle-3.7`). Per-instance workdirs land at `scratch/latent/out/inst-<id>-<model>-cycle-<N>/`.

Inference stack rules (`CODE_RULES.md §10`):
- **llama.cpp only** in the inference loop. No candle. No Python.
- **Python is allowed** in eval harness (`run_instance.py`, `eval_specs.py`), the latent injection path (`run_llama_pathB.py`), and per-cycle drivers. Everywhere else Rust.
- **No JSON for graph→model injection** when there's a graph-side alternative. The latent JSON pool (`summaries-aplus.json`) is a Rust↔Python boundary to refactor away. Graph-derived text directives are the current sanctioned channel; per-token embed injection is the next.

---

## How to get running

```bash
# Both repos as siblings (you probably already have glia from the neuropil setup)
cd ~/Code
ls glia neuropil       # both should exist

# Toolchain — Rust 1.95+, edition 2024
cd ~/Code/glia
cargo build --workspace            # ~3-5 min cold. Skips scratch/latent (candle).
cargo build --release -p glia-cli  # release CLI

# Basic smoke
./target/release/glia analyze ~/Code/<some-repo>
./target/release/glia analyze ~/Code/<some-repo> --format mermaid > /tmp/graph.mmd
./target/release/glia impact ~/Code/<some-repo> 'pkg::Class::method' --direction both --depth 3
./target/release/glia merge ~/Code/svc-a ~/Code/svc-b ~/Code/svc-c --out /tmp/org.gmap

# Per-repo .gmap build (post-commit-hook flavour)
./target/release/glia build ~/Code/<some-repo>
ls ~/Code/<some-repo>/.glia/

# Opt-in git hooks (keeps .gmap fresh on every commit)
./target/release/glia install-hooks ~/Code/<some-repo>
# undo with --uninstall

# Python wheel (the published artefact)
cd py/
maturin develop --release          # builds the wheel into the active venv
python -c "import repo_graph; print(repo_graph.__version__)"

# Tests
cargo test --workspace             # workspace-wide; the tests/ dir has the fixture smokes
cargo test -p repo-graph-graph http_stack_smoke   # targeted

# Latent-injection arm (opt-in — pulls candle)
cargo build -p repo-graph-latent

# Cycle loop (the SWE-bench end-to-end)
cd scratch/lens
./cycle/run_cycle_loop.sh cycle-X.Y qwen-7b-coder
# results land in cycle/cycle-X.Y-results.jsonl
# summary appended to cycle/cycle_log.md
```

**First-day tour** (afternoon, no code changes):

1. `glia analyze ~/Code/<a-repo-you-know>` — read the Markdown summary. Get a feel for what counts as a Node and an Edge.
2. Same repo, `--format mermaid` — paste into a Mermaid renderer. Compare visually to your mental model of the codebase.
3. `glia impact <repo> '<a-function-qname-you-care-about>'` — both directions, depth 3. This is the "what calls X" / "what does X reach" answer.
4. `glia merge` across 2-3 sibling services you have lying around. Look for cross-edges in the output JSON. If you don't have siblings, clone `microservices-demo` + `bank-of-anthos` — the README numbers come from those.
5. Open `parsers/code/extractors/grpc.rs` and one of the simpler extractors (`extractors/cron.rs` or `extractors/cli.rs`). Get a feel for the extract-don't-resolve discipline.
6. Open `graph/src/resolvers/http_stack.rs` (or wherever `HttpStackResolver` lives) — read it end-to-end. This is the canonical CrossGraphResolver shape; every new resolver mirrors it.
7. Read `CODE_RULES.md` cover to cover. It's 192 lines and 100% load-bearing.
8. Skim `scratch/lens/cycle/cycle_log.md` from the bottom up — the recent cycle entries are the live state of the research arm.

---

## Starter tasks (graduated)

Pick one. Each ships end-to-end (parser/resolver/synth/runtime + tests + a real fixture). Don't worry about "is this idiomatic" yet — read the touched modules first, mirror the patterns you see, ask if anything looks weird.

### Task 1 — Tour + a free-fix (afternoon)

Goal: get the workspace green, navigate every crate, pick one small thing that bothers you.

Possible free-fixes (each is real and unowned):
- A typo / off-by-one in any README example.
- Look in `TODO.md` Day 2 for any item still unchecked that maps to <half-day work (e.g. `--features sweben` gating of synth_composition / synth_key_symbols if `cargo check` flags them).
- Pick any `// TODO:` comment in `graph/src/` younger than 6 weeks and do it.
- Run `cargo clippy --workspace -- -D warnings` and clear one diagnostic.

Deliverable: one commit with a clear before/after.

### Task 2 — Tweak an extractor + verify cross-edges fire (1-2 hours)

`parsers/code/extractors/queues.rs` (or whichever cross-cutting extractor module you prefer). Pick a framework that's listed in the README but feels sparse in the eval numbers (e.g. NATS, raw Redis BLPOP, Sidekiq).

Find a small open-source repo using it. Run `glia analyze <repo>` and `glia merge <producer> <consumer>`. Count the queue edges. If the count is off vs what you'd expect from grep, you found a gap. Add a pattern, regenerate, verify the edge count moves.

Deliverable: a commit + a one-line cycle-log-style note in `dev-notes/` saying "<extractor>: added pattern X, edges fired count Y → Z on fixture <name>".

This is a 'feel the loop' task — touches one file, one pattern, see the result as edge-count delta.

### Task 3 — Add a new fixture smoke (half-day)

The `tests/` dir has `http_stack_smoke`, `go_smoke`, `ts_smoke`, `py_smoke`. Pick a language or framework that's listed in the README but has no smoke test yet (probably Elixir/Phoenix, Swift, Clojure, or one of the Rust web framework siblings — Warp/Tide/Poem/Salvo).

The pattern: copy `tests/http_stack_smoke/` as scaffold. Add a fixture (a minimal repo with 2-3 routes / handlers / whatever the resolver expects). Write the test to assert specific edge counts / node existence. Wire into the workspace.

Deliverable: green smoke test, one commit. Bonus: surface a bug in the extractor while writing the fixture.

### Task 4 — Build a new cross-graph resolver end-to-end (1-2 weeks, the meaty starter)

Pick one:

- **Cron resolver, depth pass.** It exists, fires 0 edges in the current eval. Assemble a 3-5 repo cron-heavy fixture (Rails app with `whenever`, k8s CronJob infra repo, Sidekiq-cron consumer, Celery beat). Validate firing across that fixture. Fix gaps. End state: README cron numbers stop being 0.
- **DB resolver, NoSQL/GraphDB depth.** SQL track is mature; NoSQL is shallow (Collection nodes land but model-class → collection edges are thin). Pick MongoDB+Mongoose, Firestore+Firebase Admin SDK, or Neo4j+Cypher. Add the model-class detection, the edge emission, the cross-service join. Mongoose is probably the densest.
- **Effect classification reasoning trial.** Single static pass tagging every node with `pure / read-state / write-state / network / fs / db / time / nondeterministic`. Composed along call edges. Visible win: `glia analyze --format mermaid` gains edge annotations. Borderline v0.4.x / v0.5; ship it if diagrams visibly improve.

Reference impl to copy: `HttpStackResolver`. The shape is: index-build pass over MergedGraph → emit cross-edges. Pure function over the merged node set.

By the end you'll have touched: cross-graph resolver trait, MergedGraph internals, the universal resolver glue, `glia analyze` output formatting, a real eval fixture, and (if you go for effect classification) the activation / projection-text edge-annotation layer.

### Task 5 — The research-arm thinking task (longer arc)

This one's open-ended and plays to your masters.

Read, in order:
- `scratch/lens/README.md` (if it exists) + `scratch/lens/src/` end-to-end.
- `scratch/latent/` — the candle arm. `scratch/latent/out/run_llama_pathB.py` for the current bench-inference path.
- `dev-notes/cycle_35_36_final_report.md` — most recent cycle final report.
- `dev-notes/glia-memory/` — any file matching `latent`, `lens`, `composition`, `cell`, `sage`, `synth`.
- `CODE_RULES.md §10–13` (inference stack, lens runtime, JSONL schema, cycle harness).
- Your own auto-memory if you have access — recent cycle entries (`project_cycle_3_*`) and especially `project_lever_4_pivot_to_fim_gbnf` for context on why FIM + GBNF + iterative refinement replaced the "structured replace-block" direction.

Question to answer (over a week or two, no rush): **Given a graph + a problem + a query, what's the correct distillation over composition / sage-filtering / synthesised cells / pooled vectors that lets a 7B model do what a 70B model can do?**

This is the open research question. The substrate ships independently; this is the shape above it. Output is a short doc — `dev-notes/distillation-direction-<your-name>.md` — saying "here's what's been tried, here's what the dead-ends were, here's the smallest experiment that would tell us if direction X is the right one." We then talk before writing code.

You don't need to solve it. The doc is the deliverable. If the doc converges on "actually it's already pointed the right way, just needs Y", that's a perfect answer too.

---

## What NOT to touch yet

Standard guardrails, lifted from `CLAUDE.md` + `CODE_RULES.md`. Don't fight these unless you've read why:

- **NodeKind / EdgeCategory / CellType u32 IDs are locked.** Adding new kinds is fine; renumbering existing ones breaks every persisted `.gmap`. `code-domain/` is the source of truth.
- **QName separator is `::`.** Locked across all languages. Languages with their own native separators (Python's `.`, Go's `/`) get normalised to `::` at the extractor seam.
- **`.gmap` is rebuild-whole-file in v0.4.x.** No in-place mutation. Incremental rebuild is v0.4.14 work and it has its own design doc (`dev-notes/incremental_gmap_plan.md`). Don't reach for "just patch the file in place" — there's a reason the format is owned-vs-archived.
- **Parser-vs-graph split is locked at v0.4.3b.** Parsers extract, graph crate resolves. Don't add resolution logic inside a language parser. If the generic walker isn't enough, contribute via the `extra_hook` seam.
- **`scratch/lens/manifests/holdout.json`** — the 10-instance sacred holdout. Loop-set ∩ holdout = ∅ asserted at cycle start. Adding instances is fine; using them in the loop set is abort-level.
- **Append-only files.** `cycle_log.md`, `results_history.jsonl`, `marshmallow_log.md`, per-cycle results. Corrections = new entries with `CORRECTED:` markers. Never overwrite.
- **`scratch/latent/` is excluded from the default workspace** for a reason — pulls candle, multi-GB download. Build it explicitly when needed; don't add it to the default `cargo build`.
- **Only `py/` publishes to PyPI** (as `repo-graph-py`). Everything else is internal. Don't touch the `repo-graph-*` crate name prefix — that's the brand and it's locked.
- **No `unwrap()` / `panic!()`** in non-test code. Propagate via `?`.
- **clap arg names are kebab-case on the CLI** even when the Rust field is snake_case. `repo_canonical` → `--repo-canonical`. Subprocess invocations of synth bins MUST use the kebab form or clap exits with code 2. Burned us in cycle 0.6.
- **`required-features = ["driver"]`** on every `synth_*` `[[bin]]` entry — otherwise the pyo3 wheel build accidentally pulls clap/serde/regex/walkdir.

---

## How to get unstuck

- **Code-symbol questions** ("where is X defined?" / "what calls Y?"): use the `repo-graph` MCP tools on glia itself. Dogfooding works — glia indexes its own Rust. The CLAUDE.md "Repo-graph first" rule from neuropil applies here too. Grep is for string searches (TODO comments, error messages, log strings). For semantic searches, the MCP server is faster and more correct.
- **Architecture questions**: `CLAUDE.md` (architecture rules), `dev-notes/glia-memory/` (seed memory: format spec, rkyv design, kind/category IDs, treesitter quirks, cross-graph resolver backlog).
- **Daily-workflow questions**: `CODE_RULES.md`. 16 sections, ~190 lines, every rule has a real-incident pedigree.
- **Cycle / research-arm context**: `dev-notes/cycle_35_36_final_report.md` is the most recent end-of-cycle summary. `scratch/lens/cycle/cycle_log.md` is the running log; read bottom-up for live state.
- **Format / serialisation weirdness**: `dev-notes/glia-memory/reference_format_spec.md` + `reference_rkyv_design.md`. The owned-vs-archived discipline is non-obvious and the docs spell it out.
- **When a feature feels half-built**: it probably is. We have the same hard rule as neuropil — "wired or nothing". Substrate without consumer = scope dropout. Either finish the wire or revert it.
- **When a result looks too good**: check the sentinel + the holdout discipline first. The cycle log is honest about this; if you're seeing a number that contradicts a recent cycle entry, you've probably hit a fixture mismatch or an append-only file you accidentally rewrote.

---

## Closing note

glia is the older of the two repos and the calmer one. The substrate is mature; the research arm at the top of `scratch/` is where the live thinking happens. Some parts of the parsers/extractors layer are dense — that's because tree-sitter grammars are quirky and we've eaten the quirk in code so downstream code doesn't have to. The cycle loop conventions look heavy on first read; they exist because we burned cycles to learning each rule the hard way (the append-only one and the kebab-case one especially).

If neuropil is the "watch an LLM edit a real system" layer, glia is the "give the LLM a real map of the system before it edits" layer. Both have to be good for the whole thing to be good.

Welcome aboard. The fastest path in is: tour → pick a task → ask a lot of questions → ship something that moves a number in the cycle log or the README in week one.
