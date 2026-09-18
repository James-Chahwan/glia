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
| Engram's v6 needs ride the leap (not import classification); repo-graph and Engram then update to the bump | "okay add the things that engram needs in v6 to the leap bump then we can hand off to repo-graph and endgram to update to glia bump which is much more simpler yuh and yeah leave out import classification" |
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
| Elixir `@doc` and Clojure docstrings | Engram v4-landed: the comment walk returns None for these; needs per-parser body extraction |
| Solidity NatSpec `@param` / `@return` / `@dev` as structured doc data | Engram v5 §8 "defer to v6"; today collapsed into one DOC cell |
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

### G. Perf, hygiene, Engram v6 and the handoffs

| item | note |
|---|---|
| rayon over walk / parse / extract | "i wanna do a perf patch for per graph area rebuilds and iter parrellisation"; `byte_identical` must stay green |
| neuropil G8 branch-pair pre-commit hook | neuropil's UI already claims glia installs it |
| dogfood: quokka / lapse drop `generate-repo-map.py` ("~70% accurate") for glia output | real-repo validation |
| remove "Cross-language taint" from `README.md` (v0.5+ roadmap), `TODO.md:54`, `docs/onboarding.md:168` ("v0.5+ refinement") | contradicts the locked gate |
| reword `graph/src/resolvers/package.rs` header pitch ("differentiator vs Endor/Snyk/Socket.dev") | CVE-reachability-shaped |
| add `SECURITY.md` | defensive framing before a bigger release |
| clear stale `TODO.md` boxes; README v0.4.14 roadmap | most shipped |

**Engram v6 (glia side) and the two handoffs.** Engram's contract is single-version and bumps in lockstep; a
re-export is the upgrade path (Engram memory `engram-single-version-gmap`). Following the v2 precedent, glia makes
the `engram-core` change, updates `engram-export`, and writes the landed doc; Engram's session applies it.

| item | glia side |
|---|---|
| `engram-core` `GMAP_FORMAT_VERSION` 5 → 6 | the contract diff below, in `/home/ivy/Code/Engram/crates/engram-core` (Engram is **not a git repo** — back it up before editing) |
| incremental export: `glia-export-engram --since <prior>` → diff gmap `{added, removed, modified}` | built on E1 graph delta + ParseCache `iter`/`diff` (A1.8); Engram adds `apply_diff` (asked since v2 G16: "full re-seed loses all Path-B learning") |
| move-stable `identity_hint` | today `<file>:<kind>:<ordinal>` — a file move resets Engram's learned reward; built on B's move-stable identity |
| line numbers on spans + line anchors on doc Propositions | the exporter already reads POSITION rows and converts them to bytes (`engram-export/src/lib.rs` "span story"); carry the rows (v5 consumer pass: "line numbers are a v6 follow-up"; v5 §7 "defer to v6") |
| DOCUMENTS as its own `EdgeKind` | today folded into plain association (`engram-export/src/lib.rs:174-187`) |
| NatSpec tags as edges | maps A's NatSpec extraction (v5 §8 "defer to v6") |
| rename safety | `engram-export` imports `repo_graph_*` by crate name and is excluded from the workspace — `cargo test --workspace` cannot see the 0.5.0 rename break it; build it separately with Engram as a sibling |
| handoff: `Engram/docs/glia-v6-landed.md` | the shape Engram's memory says worked: TL;DR · contract diff · mechanical compile fixes (file:line) · meaningful read-side work · re-test commands · done vs pending. Also fixes the stale `glia export-engram` wording it quotes (`persist.rs`, `ROADMAP.md`) |
| handoff: repo-graph | one doc for the repo-graph session: every API change in D with old → new, the new primitives, the MCP SDK 2.x port, the P4 tool collapse, the `_eloc` line fix |

Not in v6: runtime / dev / peer import classification.

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

Specced — see section 7 for the packet counts, the schedule, and the decisions that gate specific waves (7.1).

## 6. Not in this leap

- **Later:** `.graphqls` routing, Go struct-held routers, ws / graphql / grpc client host narrowing, communities, duplicate-flow detection, dominators (needs middleware extraction + a security-gate ruling),
  hubs, RuntimeZone resolver, cross-repo node dedupe, Notion / wiki doc adapters, LSP, per-graph-area rebuilds
  (revisit if a big repo is slow after LG.1).
- **Not doing:** error-handling / panic patterns, resource lifecycle (intra-procedural), articulation points,
  semantic search, runtime trace overlay, shared-`.gmap` multi-agent writes, dense-text semantic sigils.
- **Gated** (`project_glia_security_dual_use`): taint / value data-flow, CVE-reachability or package reachability joined
  to a vulnerability feed, SecurityZone resolver and the `^` security sigil, HTML sitemap-crawl doc adapter, git fetch
  or private-repo auth inside `glia merge`. TeamOwnership stays out ("everything but team ownership", 2026-04-17).

## 7. Specced (2026-09-19) — 229 packets, W0 + 39 waves

Spec run `wf_65413aa0-59f`: 11 spec → adversarial-verify pipelines, 2 Batch C re-verifiers, 1 integration critic
(25 agents, 0 errors). Every packet has symbol-verified anchors, exhaustive files_touched, a gate with a pre-fix
baseline measured on HEAD, a fired_on marker and a declared breaking block.

| phase | packets | LOC |
|---|---|---|
| L0 — wave 0: id allocation + god-file splits (engine / graph facades, py, cli, engine build.rs) | 6 | 1,480 |
| A — extraction depth (incl. 13 programme leftovers and run 2's 15 fixes) | 59 | 15,100 |
| Batch C — re-verified at HEAD: 9 still valid, 25 respecced, 0 obsolete | 34 | 5,045 |
| B — identity (incl. run 2's LB.7–LB.9) | 16 | 4,760 |
| C — store format | 16 | 4,600 |
| D — API + cross-domain prep | 29 | 8,680 |
| E — new answers | 22 | 6,065 |
| F — outside inputs + reserved cells | 21 | 6,665 |
| G — perf, hygiene, Engram v6, handoffs | 26 | 6,455 |
| **total** | **229** | **~58,850** |

Declared breaks across the packets: api_signature 31, cli_output 27, cell_value 15, edge_removal 13, format 11,
qname_shape 10, node_id 10, out_of_repo 20 (96 packets break nothing).

**Schedule** (`python3 dev-notes/wave-runner/leap_schedule.py`): W0 runs the six splits one at a time
(L0.1 → L0.3 → L0.2 → L0.6 → L0.4 → L0.5; graph before engine, then py and cli), then W1–W39 file-disjoint. Dependency
depth 28. Without the wave-0 splits the same packets need 63 waves. The critical path is the store/format spine
(LC.2 → LC.3a → LC.3d → LC.3b → LD.13 → LD.14a → LD.15 → LD.6 → LD.4 → LG.3 → LD.11 rename) followed by the Engram
tail (LG.7 → LG.10 → LG.8 → LG.9 → LG.12 → LG.8a → LG.11 → LG.14), which edits the same two engram-export files and can
run as one serial workflow. LC.2 (core::Edge) and LC.3b run alone in their waves.

**Registry ids** (L0.1, compacted so each ALL table stays contiguous): edge_category 35 NAVIGATES_TO, 36 CO_CHANGES;
cell_type 19 DOC_TAGS, 20 ROLE, 21 EVIDENCE, 22 SCHEMA_FIELDS, 23 COVERAGE, 24 ENTRYPOINT, 25 ACCESS_MODE. No new node
kinds (enum variants reuse ATTRIBUTE, inline mods reuse PACKAGE).

**Files.** `dev-notes/leap-packets.json` (packets, Batch C rows, dependency patch, split remaps, exclusive list,
orchestrator edits, the integration report) · `dev-notes/leap-corrections.json` (61 packets; overrides the spec) ·
`dev-notes/wave-runner/leap_schedule.py` · `shared_brief_leap.md` (the breaking-release brief) · `gen_wave.py --leap N`
· `closeout.py --leap N <run-id>`.

**Running a wave** (not started — spec and plan only): `python3 dev-notes/wave-runner/leap_schedule.py --verify`, then
`python3 dev-notes/wave-runner/gen_wave.py --leap N <scratch>/leap-wN.js`, run it as a Workflow, then
`python3 dev-notes/wave-runner/closeout.py --leap N <run-id>`. Wave 0 and single-packet waves render as sequential scripts.
The baseline (`baseline.json`, 1,056 tests / 140 fixtures / matrix 146 of 480) was measured after programme wave 19;
no code has changed since.

### 7.1 Decisions (James, 2026-09-19)

1. **Engram edits** (LG.7, LG.14): approved — *"Yeah sure I'll do it before that wave atleast"*. Engram is now a git
   repo (baseline commit 8566a7b, 2026-09-19); the packets commit their Engram changes there.
2. **pyo3 auto-persist**: dropped — *"Oh okay that's best to fix to place we git ignore and don't double write"*.
   `generate()` only builds; `save_to` / `save_to_default` are the one writer (LD.2). LC.9's layout dir
   `<repo>/.glia/graph/` writes its own `.gitignore` (`*`) so it never shows in any repo's `git status`; checked-in
   inputs (`.glia/overlay.toml`, `.glia/cells.jsonl`) stay outside it. The wrapper already saves explicitly, so it now
   writes once instead of twice.
3. **Verifier scope additions** (LB.4c `page:` qnames, LA.6e `glia pages`, LG.3d overlay data_entity wrapper, LD.4a
   cross_service by project, LD.6 wider entry set): kept — *"Makes sense then"*.
4. **Per graph area rebuilds**: not in 0.5.0 — *"Cool no worries"*. Full builds take 8.2 s (glia) and 2.8 s
   (quokka-stack) with the debug binary; revisit only if a big repo is slow after LG.1.
5. **GLIA_NO_PERSIST purge gap**: closed by LD.2 — *"Do it then"*. `generate(incremental=False)` reads, writes and
   purges nothing; `purge_parse_cache()` is explicit.
6. **LG.8** diffs exported gmaps, with a dependency on LE.1: confirmed — *"Yeah sure sounds reasonable?"*.

### 7.2 Run 2 — the unowned findings, specced

Workflow `wf_5cd00d1d-1ff` (6 agents, 0 errors) turned run 1's 14 unowned findings into 22 packets (~5.5k LOC), merged
into `leap-packets.json` with dependencies wired from the verifiers' notes; the schedule stays W0 + 39 waves.

- **Identity (B):** LB.7a–d stop the doubled class segment in Scala, PHP (directory scope), Swift and namespace-less C#
  (namespace scope); LB.8 gives queue / WebSocket / GraphQL / gRPC nodes an owner segment in monorepos (LB.4a's
  mechanism); LB.9a/b give same-stem files in different languages their own MODULE identity.
- **Precision / crash (A):** LA.25a/b the UTF-8 panic (the real site is `data_entities.rs` `scan_cypher_labels`, plus
  the same class in the Rust parser) and `[parse]` lines so a caught panic is never silent; LA.26 gates the whole GraphQL
  operation-needle table; LA.27 SDL only from SDL; LA.28 the Cypher needle; LA.29 event emitters need a real bus.
- **Coverage (A):** LA.30a–c Java / TS enums (the generic resolver treats ENUM like CLASS, so LA.3 now follows LA.30a);
  LA.31 tRPC POSITION; LA.32a gin ROUTE method + POSITION; LA.33 kafkajs-style callback consumers; LA.34 Dart bare
  self-calls; LA.35a/b Rust typed receivers (the LA.1b caveat row is narrowed to what stays blind, not deleted).

### 7.3 Run 2 decisions — all taken as recommended (James, 2026-09-19: *"yeah go with your calls and run 3 also git init engram i guess because cbf"*)

1. Solidity keeps `contracts::Token::Token::transfer`: a source unit is Solidity's namespace (solc names it
   `contracts/Token.sol:Token`, and two files may both declare `contract Token`). **Kept.**
2. C++ doubles too (`src::Widget::Widget::run`), and a `.h` / `.cpp` pair shares one MODULE. Needs its own design
   (namespace scope, header/impl split). **Specced in run 3.**
3. PHP goes to directory scope (Laravel / Symfony put everything in `App`), file-scoped C# to namespace scope (.NET
   namespaces are project-named). **Confirmed.**
4. LB.8 covers queue / ws / graphql / grpc; tRPC (RPC_PROCEDURE / RPC_CALL) and the in-process event bus collapse the
   same way — one table row and one resolver line each. **Specced in run 3 (LB.8b).**
5. With LB.9b a file's identity depends on its siblings (adding `api/user.ts` renames `api/user.py`'s symbols); LB.6's
   move-stable record is unchanged by it. **Accepted.**
6. Run-2 scope beyond the item text: LA.25b, LA.25a's CLI `[parse]` lines, LA.26 gating the whole needle table, LA.29's
   `.subscribe(` / typed-publish duplicate / AWS SDK `.send(new ...)` cases. **Accepted** (each is a measured false
   positive or a silent failure).
7. LA.27 recognises embedded SDL only in `typeDefs` / `type_defs` variables. **Accepted**, with a coverage caveat row (LA.27 correction).
8. Go ROUTE identity is path-only (`route:/activity/:id` is one node for GET, PATCH and DELETE, so a DELETE client
   traces into the GET handler); `ts_routes` has the same shape. Identity-shaped (the verifier cut it as LA.32b).
   **Specced in run 3.**
9. LA.35b narrows LA.1b's Rust caveat row to what stays blind instead of deleting it. **Kept narrowed.**
10. LA.33 keeps HANDLED_BY on the subscribing setup function and adds the edge to the bound callback. **Kept both.**

### 7.4 Found by run 2 — identity and correctness go to run 3, coverage waits for after 0.5.0

Identity-shaped (B, if done): contract-op DOC_SECTION qnames `contract::<stem>::<op>` drop the directory, so two
services' `openapi.yaml` ops collide; same-group same-stem files (`Widget.h` + `Widget.cpp`, `util.js` + `util.ts`,
`core.clj` + `core.cljs`, `Foo.java` + `Foo.kt`) still share a MODULE after LB.9b; two C# `file class X` in one block
namespace share a node.

Correctness: Swift `navigation_suffix` keeps its leading dot, so zero Swift self-calls resolve; Dart top-level
function bodies are never walked (no top-level Dart function has an outgoing call); GraphQL decorator nouns
(`@Query(`, `@Resolver(`, `ObjectType`, strawberry) match anywhere, comments included (every NestJS repo, glia's own
build); eventbus event names can span newlines; `data_entities.rs` scanners mint entities from their own source
(`mongoose.model(`, `.collection(`); DOM / Redux verbs (`.on(`, `.dispatch(`, `.trigger(`) mint event nodes; a C#
`using` inside a block namespace and a PHP `use` inside a braced namespace never resolve (0 IMPORTS); LA.4's post-cache
queue re-emit loses LE.4c's owner edges.

Coverage (after 0.5.0): `.graphqls` files (Spring for GraphQL / gqlgen default) are never routed; Go struct-held
routers (`s.router.GET`) produce no ROUTE; no host narrowing for ws / graphql / grpc clients across owners (LB.4b is
HTTP only).

### 7.5 Spec-run hygiene

The spec agents were read-only on every repo, but three of them called the wheel's `generate()` on other repos, which
wrote untracked, regenerable `.ai/repo-graph/` caches into quokka-stack (rewrote existing shards) and neuropil (a new
untracked `.ai/` directory). No tracked file in any repo changed; quokka's tracked `.ai/repo-graph` deletions predate
the run. The leap brief now forbids the wheel on other repos.
