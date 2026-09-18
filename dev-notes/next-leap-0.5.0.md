# glia — the next leap (0.5.0): agreed scope

Status: **scope agreed 2026-09-18, not yet specced into packets.** This is the contract-break document promised on
2026-09-16, widened to the whole leap because James chose to ship the breaks and the next features as one release.
Companion: `dev-notes/handoff-2026-09-18-next-session.md` (state of the finished 19-wave programme).

## 0. Decisions (verbatim, 2026-09-16 → 2026-09-18)

| decision | James |
|---|---|
| contract breaks wait for one joint release | "also for all the contract changes we will break them next version in another session and mvoe repo-graph and glia together" |
| release the programme, then build the leap, ship breaks with it | "yeah so we take what we just did then what we just researched to do next then we can do the bump break release as we do the next leap okay ???" |
| 0.5.0 is preparation, not a second domain | "i think 0.5.0 is prep for cross domain to be honest" |
| fill the reserved cell types | "we should fill the reserved cells ?" → "lets do the fill" |
| waves A–G are the structure | "i like a-g of waves to work on for this leap" |
| the picked extras below | "these seem good to add" |
| cross-domain prep in this leap | "also do the cross-domain prep thats a great thing to add now" |
| first endorsed batch | "okay those all sound like reasonable inclusions" |
| nothing pushed until 0.5.0; the CLI line bug is fixed inside the leap | "the cli bug gets fixed in the leap" / "we aren't pushing whats commited locally till we wanna do the bump to 0.5.0 okay" |

Filters every item passed: graph-native assertions over search; FACT / DERIVED / HEURISTIC evidence tiers, no
unexplained scores; the security gates (no taint, no CVE-reachability, no mass-corpus crawler).

## 1. Sequence

**Nothing is pushed, tagged or published until the 0.5.0 bump** (James, 2026-09-18: "we aren't pushing whats commited
locally till we wanna do the bump to 0.5.0"). There is no 0.4.19. This supersedes §5.1 of the 2026-09-18 handoff.

1. **Keep building on local `main`.** The finished programme (142 commits since `origin/main` = v0.4.18) and every
   leap commit stay local.
2. **Build the leap** — waves A → G below, reusing `dev-notes/wave-runner/`.
3. **At the bump — release 0.5.0**, glia and repo-graph together (the crate/PyPI rename is a 0.5.0 gate per
   `CLAUDE.md`). James walks the commits first. Checklist:
   - Build neuropil (in-process Rust consumer): `GenerateResult` gained public `repo_labels` (`engine/src/build.rs:42`)
     and D adds more; D's `#[non_exhaustive]` settles it.
   - Confirm the wrapper decodes every new kind / category / cell id via `kind_names()` / `category_names()` /
     `cell_type_names()`; it still hardcodes cell ids 4–11, 16 and the entry kinds.
   - Release notes list every qname / id change: C# ROUTE qnames (A4.1), Terraform INFRA_RESOURCE (bbf4cfa), all of B,
     and the NodeId change.
   - Only then push → tag `v0.5.0` → PyPI (wheels build on `v*` tags) → the repo-graph pin moves.

Break classes: **content** (graph contents / qnames / ids change; API unchanged) · **format** (`.gmap` layout;
FORMAT_VERSION / MANIFEST_VERSION bump) · **API** (pyo3 / CLI / Rust signatures or value semantics) · **additive**.

## 2. Waves

### A. Extraction depth — Batch C + the programme's unowned findings (content)

Batch C is fully specced: 34 packets, ~4,470 LOC, in `dev-notes/wave-packets.json` (+ `packet-corrections.json`).
Run it by adding `C` to `KEEP_BATCHES` in `schedule.py`.

| area | packets |
|---|---|
| receiver types + heritage | A6.2a/b/c (C#, TS/Angular, Java field types — `this.svc.m()`), A6.3 TS, A6.4 Solidity, A6.5 Dart heritage via UnresolvedRef, A6.6 interface method table + method-level IMPLEMENTS (build-time) |
| tests + imports | A6.7 CamelCase TESTS (Java/Kotlin/C#/PHP/Swift/Scala), A6.8 tsconfig `paths` |
| DI | A7.1 TS family, A7.2 Java (Lombok/JSR-330/Dagger/Guice), A7.3 C# primary ctor + `[FromServices]`, A7.4 FastAPI `Depends`, A7.5 PHP, A7.8 INJECTS flip `live` |
| data layer | A13.1 canonical entity join key, A13.10–A13.17 eight ORMs, A13.2 Java entity prefix (qname), A13.4 CLI key normalisation (qname), A13.8 secrets + flags, A13.9 migrations + DDL |
| Kotlin | A14.1–A14.6 (tree-sitter-kotlin crate, calls, Spring, Ktor, Retrofit/Android) |

Not yet specced (need packets):

| item | evidence |
|---|---|
| Rust full-crate-path calls produce no CALLS | `repo_graph_engine::service_map(..)` from cli/py → engine API flagged dead |
| calls inside macro arguments not extracted | extractors wired via `run!` / `run_counted!` in `engine/src/extract.rs` under-report callers |
| enum variants + inline `mod x {}` are not nodes | `MatchTier::BaseFold`, `code_domain::endpoint` |
| constant table not consumed by the queue scanner | `const TOPIC = "orders"; send(TOPIC)` falls back to the framework tag |
| `glia arch` splits a one-file nested Gradle root into a service | `quokka_android/android/app` |
| frontend navigation edges: `router.navigate` / `routerLink` / href → frontend ROUTE | page flow + dead deep links (quokka found `/connect` by hand) |
| matrix `subproject` vocabulary predates PROJECT (kind 45) | three stale `·` cells; retarget `matrix_vocab.py`, re-author 3 probes |
| `AUTHORING.md` says single-repo builds never pair across services | measured false for HTTP / WS / GraphQL / gRPC / EVENT |
| Confluence `Config::base()` hardcodes `https://` | add a local stub seam so `docs sync`/`push` can be tested |

### B. Identity — every id and qname change, once (content)

neuropil keys persisted state by qname, so all of these land in one wave.

| item | evidence |
|---|---|
| path-independent RepoId → stable NodeIds | `engine/src/build.rs:83` builds RepoId from literal `file://{repo_path}`; `NodeId::from_parts` hashes it. Relative vs absolute path, each clone, each worktree = different ids. Required by E1 (graph delta) and F (persisted cells) |
| Java qnames double the class segment | `App::App::compute`, `UserController::UserController::getUser` |
| merge same-qname CLASS + SERVICE / COMPONENT at build time | readers patch it today via `pick_primary` (`graph/src/merged.rs`) |
| owner segment on ROUTE qnames | monorepo: two services serving one path collapse into one ROUTE (`graph/src/resolvers/http.rs` comment defers it) |
| leading-slash normalisation of route/endpoint qnames in every parser | only cs/java/ruby/php/python use `abs_path` |
| stable identity across file moves | qnames are path-derived; a move is delete + add, and F's persisted cells would orphan |
| Batch C qname packets land here | A13.2, A13.4, A13.10, A13.13 |

### C. Store format — one FORMAT_VERSION + MANIFEST_VERSION bump (format)

| item | evidence / why |
|---|---|
| edge cells — a cell slot on edges, like nodes | `Edge { from, to, category, confidence }` (`core/src/lib.rs:167`) carries nothing else |
| edge evidence: call-site position + emitting extractor/resolver | glia cannot say "A calls B at line 72"; every assertion in E cites it |
| self-describing header | `Header::for_code()` (`store/src/lib.rs:315`) writes empty kind/category/cell registries |
| domain-owned container sections | `Container` hardwires code-only `code_nav` / `symbols`; this is the 0.5.0 domain seam |
| A6.6 persisted — `interface_methods` on `SymbolTable` | parked since 2026-09-16; validated rkyv read fails on old files |
| persist `repo_labels`, `parse_errors`, `RepoGraph.properties` | `load_from_gmap` (`py/src/lib.rs:598`) comes back without them; `glia arch` loses repo names on the MCP warm path |
| `load_from_gmap` rebuilds a stale / old-format `.gmap` instead of raising | it takes no repo path today, so it cannot self-heal |
| one on-disk layout | `glia build` writes flat `<repo>/.glia/*.gmap` (no manifest, no cross_stack); pyo3 writes sharded `.ai/repo-graph/` (`DEFAULT_GMAP_SUBDIR`); `install-hooks` refreshes files the MCP never reads. Pick one directory with the rename |
| multi-repo workspace manifest + `glia merge` of pre-built `.gmap`s | merge without every source tree checked out; also how a future domain graph joins a code graph. Needs B (stable ids) + A6.6 persisted. Cross-repo node dedupe stays later |

### D. API contract + cross-domain prep (API)

| item | evidence / why |
|---|---|
| 1-based `line` everywhere — pyo3, `--json`, and the CLI tables | `locate_node` documents 0-based (`engine/src/answers.rs`); `nodes_json` already emits +1 (`py/src/lib.rs:135`); the five `format!("{f}:{l}")` sites in `cli/src/main.rs` (blast-radius, docs-for, contracts, trace, resolve) print the raw 0-based value, so every location is one line early. repo-graph's `server.py` `_eloc` prints it raw too and drops line 0 (`if line`) — its session fixes that side |
| pyo3 consolidation | JSON-string vs native returns; duplicate `resolve_signal`/`resolve`, `find_node`/`find_nodes_by_qname`; `generate` incremental=True vs `generate_many` False |
| graph logic out of the Python wrapper | traversal (bfs / predecessors / reachable_by; `neighbours` is outgoing-only, `py/src/lib.rs:178`), multi-path trace + flow building, ranked fuzzy find |
| multi-seed `blast_radius(qnames)` | diff_impact (E2) needs it; the wrapper unions in Python today |
| one entrypoint set + `entry_kinds()`; `live` on every record | engine `is_entrypoint` (`engine/src/answers.rs:35`) and the wrapper's hardcoded {5,11,13,15,17,19,21,37} disagree |
| interfaces query — "who implements X" | after A6.6; Go implicit interfaces are where search fails |
| absence answers — "no route serves POST /x" + that mechanism's blind-spot caveat | FACT-tier negative lookup |
| `#[non_exhaustive]` on public engine/graph result structs | every added field breaks neuropil's struct literals |
| delete dead `core::Flow` / `FlowKind` (`core/src/lib.rs:97`); move `project_name` (`:263`) to code-domain | core stays domain-free |
| activation hook traits — `SynthHook` / `FilterPredicate` / `RankingSignal` → one `ActivatedView`; synth bins folded; `driver` feature (`projection-text/Cargo.toml:22`) → `research` | James parked it for 0.5.0: "can i just leave it there to fix in 0.5.0" |
| pass composition — `post_passes` (`engine/src/passes.rs:11`) becomes a registry the domain fills | "reachableby, is a good thing to add. same with pass composition." |
| **domain profile** — one table per domain: entry kinds, blast carry edges (`blast_carry_edges`, `graph/src/blast.rs:41`), effect sinks, activation weights, passes, cell populators | extract only what code hardcodes today; no traits for imagined domains |
| generic algorithms live in domain-agnostic crates | graph delta, cycles, reachability in `graph` / `activation`, not `engine` |
| test-only toy second domain | proves the seam (header registries, container sections, profile, passes) without shipping a domain |
| rename | crates + PyPI package + MCP registry slug + `repo-graph` / `repo-graph-init` entrypoints + pyproject URLs |
| repo-graph side (its session) | MCP SDK 2.x port (pyproject pins `<2  # port before lifting`); P4 tool collapse onto these primitives |

### E. New answers — "what did my change do, and the evidence" (additive on top of C/D)

| # | item | depends on |
|---|---|---|
| E1 | graph delta — edges added / removed, HEAD vs working tree | B path-independent ids; incremental parse cache |
| E2 | diff_impact in one call | E1, D multi-seed blast_radius |
| E3 | tests to run for a change (reverse TESTS) + fill the **TEST** cell | A6.7 |
| E4 | effects(A) — downstream blast radius filtered to effect sinks; read vs write on data access (edge cell); function-level ACCESSES_DATA; config_flow as one more seed kind | C edge cells, D domain profile (sinks) |
| E5 | `why(edge)` — extractor + call site + confidence | C edge evidence |
| E6 | cycles (Tarjan SCC), cross-service event loops first | domain-agnostic crate |
| E7 | pattern conformance — one pattern (role chain to the data layer), experimental flag, counts not scores | E1, C evidence, A6.2 |
| E8 | `check_*` v1 — forbidden edges between two paths / projects + cycles → VIOLATION; rules stored as **CONSTRAINT** cells | F, E6 |
| E9 | SDD `spec_status` — implemented / declared_missing / undeclared (spec-kit, quokka `feature.yaml`) | contract files + DOCUMENTS (landed) |
| E10 | field-level contract diff — producer vs consumer fields (proto, Avro, OpenAPI, AsyncAPI) | schemas ingested waves 17–18; extends `glia contracts` |

### F. Everything that enters the graph from outside the source (additive + format via C)

| item | fills |
|---|---|
| cell write API through pyo3 / CLI, persisted across rebuilds (`store::upsert_cell`, `store/src/lib.rs:483`, has no caller today) | — |
| `.glia/overlay.toml` + `glia gaps` — the LLM overlay (repo-graph runs the agent, glia applies deterministically: ORIGIN `overlay:llm`, Weak, `--no-overlay`) | — |
| same file holds user config: skip dirs, roots, entrypoints | — |
| declared rules and invariants | **CONSTRAINT** |
| ADR doc sections that DOCUMENT a node + override entries | **DECISION** |
| agent reasoning notes per node ("so then we could just write in mempalace info if we wanted to by config ? or other types of things by mcp ?") | **CONV** |
| externally computed embeddings — glia stores bytes, never computes | **VECTOR** |
| git-history snapshot step (like `docs sync`, keeps builds deterministic) → blame / churn; **cochange-without-edge audit** feeds `coverage` | **ATTN** |
| test-report / CI-log snapshot (JUnit XML) mapped via `resolve`; **lcov overlay** in the same step | **FAIL** |

After A–G every registered cell type has an emitter (TEST in E3, the rest here).

### G. Perf + hygiene

| item | note |
|---|---|
| rayon over walk / parse / extract | "i wanna do a perf patch for per graph area rebuilds and iter parrellisation"; `byte_identical` must stay green |
| neuropil G8 branch-pair pre-commit hook | neuropil's UI already claims glia installs it |
| dogfood: quokka / lapse drop `generate-repo-map.py` ("~70% accurate") for glia output | real-repo validation |
| remove "Cross-language taint" from `README.md` (v0.5+ roadmap), `TODO.md:54`, `docs/onboarding.md:168` ("v0.5+ refinement") | contradicts the locked gate |
| reword `graph/src/resolvers/package.rs` header pitch ("differentiator vs Endor/Snyk/Socket.dev") | CVE-reachability-shaped |
| add `SECURITY.md` | defensive framing before a bigger release |
| clear stale `TODO.md` boxes; README v0.4.14 roadmap | most shipped |

## 3. Cross-domain prep at a glance

Self-describing header + domain-owned sections (C) · merge of pre-built graphs (C) · domain profile, pass
composition, activation hook traits, domain-free core (D) · generic algorithms in domain-agnostic crates (D/E) ·
test-only toy domain proving it (D). No non-code domain ships in 0.5.0.

## 4. Order inside the leap

A and B first (content) → C (format) → D (API) → E and F. Hard edges: E1 needs B's stable ids · E5 and E7 need C's
edge evidence · E3 needs A6.7 · E7 needs A6.2 · E8 needs F and E6 · C's `glia merge` of pre-built graphs needs B and
A6.6-persisted · F's persisted cells need B's move-stable identity. Only Batch C is specced; B–G need packets written
(same format as `wave-packets.json`) before `gen_wave.py` can run them.

## 5. Open

- **Engram contract v6** — does it join? There is no v6 doc; it is what Engram's v2–v5 docs (`/home/ivy/Code/Engram/docs/`)
  deferred. The contract is `engram-core` (`GMAP_FORMAT_VERSION = 5`), owned by Engram; `engram-export` is outside
  glia's workspace and unpublished, so it does not have to ship with 0.5.0.
  - glia-side pieces the leap already builds: stable identity across moves (B) → a move-stable `identity_hint`
    (today `<file>:<kind>:<ordinal>`, so a file move resets Engram's learned reward); graph delta (E1) + ParseCache
    `iter`/`diff` (A1.8) → `export --since <prior>` diff gmaps (v2 spec G16, deferred since v2).
  - engram-core contract changes (Engram's session): diff gmap `{added, removed, modified}` + `apply_diff`;
    DOCUMENTS as its own edge kind (today folded into plain association, `engram-export/src/lib.rs:174-187`); line
    anchors on doc Propositions (v5 §7, "defer to v6"); NatSpec `@param`/`@return`/`@dev` as edges (v5 §8, "defer to
    v6"); runtime / dev / peer import classification (v5 §8).
  - glia extraction, independent of the contract: Elixir `@doc` and Clojure docstrings (v4-landed).
  - Stale: Engram's `persist.rs` messages still say `glia export-engram`; the subcommand moved to the
    `glia-export-engram` binary (`engram-export/src/bin/`). Already done on the glia side: `.gitignore`-aware walk,
    intra-workspace import leak (A16.4), separate IMPLEMENTS.

## 6. Not in this leap

- **Later:** communities, duplicate-flow detection, dominators (needs middleware extraction + a security-gate ruling),
  hubs, RuntimeZone resolver, cross-repo node dedupe, Notion / wiki doc adapters, LSP.
- **Not doing:** error-handling / panic patterns, resource lifecycle (intra-procedural), articulation points,
  semantic search, runtime trace overlay, shared-`.gmap` multi-agent writes, dense-text semantic sigils.
- **Gated** (`project_glia_security_dual_use`): taint / value data-flow, CVE-reachability or package reachability joined
  to a vulnerability feed, SecurityZone resolver and the `^` security sigil, HTML sitemap-crawl doc adapter, git fetch
  or private-repo auth inside `glia merge`. TeamOwnership stays out ("everything but team ownership", 2026-04-17).
