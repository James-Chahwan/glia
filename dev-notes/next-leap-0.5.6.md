# glia 0.5.6 — finishing what is started: T4, T7's search and placement, T8, the host-to-plugin graph

**Renumbered 2026-10-31** (was the 0.5.5 seed). James: *"we could a small lua pr 0.5.5 then bump our 0.5.5 to 0.5.6"*:
0.5.5 is the Lua release (`dev-notes/next-leap-0.5.5.md`), the first community patch, from Brian Ealdwine's glia PR
#3. Every "0.5.5" below this note, and in the 0.5.4 records (packets, corrections, handoffs, reports), names this
plan, now 0.5.6. `--release 055` / id prefixes below become `056`.

**Born 2026-10-30.** James cut 0.5.4 to what was built plus two small tracks (*"Nah we can change to option 3 to round of
0.5.4 now right ?"*, superseding the 2026-10-24 ruling that 0.5.4 ships only after every track). 0.5.4 ships T0 T1 T2 T3
T5 T10 (+FD.31) T11 TS TH and FH.11c; the rest moved here.

**Split 2026-10-31** into two releases. James: *"Yeah that's probably better to make this 2 releases and then let's
update and complete 0.5.6"*. 0.5.6 finishes what is already started (the moved T4 / T7 / T8 work, the host-to-plugin
graph 0.5.5 promised, 0.5.4's follow-ups); 0.5.7 (`dev-notes/next-leap-0.5.7.md`) takes the new capabilities: the goal
layer and clarification queue (T6, with T7's PR-intent packets FC.19 / FC.20, which need it), build-target graphs (T9),
the proof row, calibrated confidence and the domain id allocator. The T6 / T9 slot files now read `Reserved for 0.5.7`.

## 1. Tracks moved from 0.5.4 (already specced)

The packets are in `dev-notes/leap-054-packets.json` with their corrections in `leap-054-corrections.json`; the track
lists are in `dev-notes/wave-runner/tracks_054.py`. They were specced against 2026-10-23's tree, so every one needs the
re-verify pass below before a wave.

| track | what | packets | LOC at spec |
|---|---|---|---|
| T4 | per-language precision and recall residuals (0.5.4 plan §D, §E, F.1) | 53 | ~15,870 |
| T7 (part) | concept search over flows (F.6, FC.15, FC.16, FB.9) and where new code goes (F.14, FC.21, FC.22) | 5 | ~1,830 |
| T8 | Stack Graphs-style incremental resolution, watch's body-only fast path, multi-repo watch, the watch memory budget (F.3) | 19 | ~7,080 |

Every prerequisite track these lean on shipped in 0.5.4 (T4 needs T2; T8 needs T2 and T5; FB.9 needs TS). T7's PR / issue
intent (FC.19, FC.20) depends on T6's goal graph (FC.17b, FC.10, FC.18) and moves to 0.5.7 with it. T6's and T9's wave-0
slots and registry ids (GOAL_UNIT 50, TRACES 38, UNIT 26) stay reserved, for 0.5.7.

Before a wave: the re-verify pass (`dev-notes/reverify_054.md`'s method) against the shipped `v0.5.5` tree, every packet
`unchanged` / `rewritten` / `not-needed (<commit>)` / `blocked`, then a new id prefix and a `--release 056` schedule.

## 2. The host-to-plugin graph (promised by 0.5.5)

0.5.5's §3 left these here, and the PR #3 reply promised the plugin families (Brian Ealdwine: *"Particularly glad about
the plugin families being generally useful, that required some thought"*); link the work on glia PR #3 when it starts.

- **One host-to-plugin mechanism**: PR #3's plugin families, manifests and LOADS, generalised (Neovim, Maki, VS Code
  commands, Obsidian plugins, Flask blueprints, webpack plugins, editor and game hosts), its family table as the first
  case. Any new id comes from `code-domain/src/lib.rs`'s next free (node 51 / edge 39 / cell 28), never PR #3's.
- **It fills the use-pattern axis**: Lua's host_command, host_event and config_table (`none`) and host_callback,
  embedded_script and script_entry (`partial`) are its targets; pattern rows for the other languages that are embedded or
  plugin-shaped.
- **The environment hosts, in the same table** (James 2026-10-31: *"0.5.6 they kinda go together as idea as current
  host for environment"*): hosts stack, each starting the next (orchestrator -> container -> process -> app framework ->
  plugin / route handler), so container and process starts are host rows too: Dockerfile `ENTRYPOINT` / `CMD`,
  docker-compose `command:` / `entrypoint:`, Kubernetes container `command` / `args`, systemd `ExecStart`, Procfile.
  glia already reads parts of this (Dockerfiles, k8s CronJobs, IaC, CLI entries); this joins them into one "who starts
  this" relation. Static only: glia reads what the files declare, never a running host. (The old RuntimeZone idea.)
- **Lua's remaining reach**: globals across files, `pcall` / `load` / `dofile` indirection, metatable `__call`
  constructors, OpenResty's nginx-side routing (the embedded_script pattern).

## 3. From 0.5.4's W20 follow-ups (James 2026-10-30: the precision extensions move here)

- Clients reached through class heritage: `this.get(url)` in a class extending an imported `APIService` / `HttpService` (the bulk of the remaining phantom_dropped: plane 43, chatwoot 128; FH.17b's handoff).
- TS type references imported from another file (`import { Dto } from './types'; http.get<Dto>()`) as USES through import bindings, and abstract method signatures' type names (FH.20's handoff).
- A NestJS verb decorator that shares its line with another decorator (`@UseGuards(X) @Get('a') foo()`) mints no ROUTE (FH.19's handoff).
- gin-vue-admin: 131 of 196 endpoints pair; the 65 unpaired look like Go router-group prefixes not folded into the route path (checked 2026-10-30).
- Dropped: a `nest_handler` rule name on Nest HANDLED_BY edges (cosmetic; the binding works through graph:refs).


## 4. From 0.5.4's W21 follow-ups and the 2026-10-30 build profile

- TS EVM reader (DT.10c / DT.10j's shape): a method-name wrapper read the way Go's DT.10f does (a string parameter handed to `getFunction(..)` or viem's `functionName`), and an imported TypeChain / ethers contract instance followed across files. These are the 3 TS wrapper cases and the ts x generated-sdk case in the Sibyl evm matrix (FH.11c's handoff; FH.23 fixed the stale-ABI pairing only).
- Build profile, the parts FH.21 leaves: on twenty, merge_parses + build_symbol_table + resolve_imports_ts stay serial (1.4 s); persist is serial (2 s). Profile one large Go and one large JVM repo the same way (per-second core samples + timestamped markers) before assuming they share twenty's shape.
- The full rank gate's glia snapshot (478 s) is rank_eval's history replay, not the build: profile it, then try spreading its PPR solves across the cores. James 2026-10-30 asked whether the GPU could help: not for the build (pointer-chasing, string lookups), possibly for batched PPR replays and for embeddings (already ruled GPU-default after 0.5.4), but only as an accelerator whose output is never the reference (float reduction order breaks the bit-for-bit rank pins).
- Org routing for the JVM (FA.22's follow-up): no `manifest_name` class forms for Maven / Gradle until a PROJECT records `groupId:artifactId` (project_roots::manifest_label reads a pom's artifactId only; Gradle roots fall back to the directory). Record `g:a`, then extend OrgPackageResolver's ecosystems.
- Under concurrent builds the TS-family language_build is the phase that grows (FH.22's measurement, 21 s alone -> 25-50 s with 4 builds at once), memory-bound rather than short of cores; FH.21 cut it to ~3 s on twenty, so re-measure before acting.
- Org routing residuals (FA.23): conditional npm `exports` objects, `./*` subpath patterns, `module` / `types` entries, and Rust `pub use` re-exports at a crate root are not routed.
- Edge audit, wrong verdicts in strata at or above floor (FD.3c, dev-notes/edge-precision-0.5.4.md): I1 IMPORTS tail_unique is kind- and stdlib-blind (Go stdlib, `java.`, C# `using static`; FD.13 covers PHP only); I2 rust_paths credits an inline `mod`'s `use` to the file MODULE; T1 TESTS name_match pairs a stem across packages, and a flutter drive host pairs its suite. R1 (IMPLEMENTS|fact under floor) rides with FD.11. Plus the report's own items: re-judge stratified by first verdict, and a category CI that is not [1, 1] at p = 1.
- gRPC proto parsing (FD.80's spec probe): a one-line service body (`service X { rpc Y(..) returns (..); }`) declares no rpcs at HEAD, so a single-file Go server's registration anchors on `main` (bisected: one-line body -> `main::main`, multi-line -> `main::server::SayHello`). FD.80's fixture uses multi-line services; the one-line body is its own fix.
- From 0.5.4's W25 (FD.73-FD.81): TOML dotted dependency tables (`[dependencies.serde]`) and target tables (`[target.'cfg(unix)'.dependencies]`) are never read (FD.77); gRPC impl recall gaps - a Python / Node impl class imported from another file, an in-repo constructor in another Go package (`handlers.NewOrderServer()`) read as foreign, an unread field / parameter argument binding only its base type's methods (FD.80); Go defined types of other shapes (`type ID string`, genproto `type Status int32`) mint no node, so two modules' copies differing only in them still pair as implicit IMPLEMENTS (FD.81).
- Docs as a second graph (James 2026-10-30, after FD.79: "it's more like the next release as we deal with more graphs and generality etc we should fix it"): on glia's own history 39% of 2-30-file commits change docs and code together (the five other rank-gate snapshots 0-10%), so a doc section's tie to code is real even where FD.79 now refuses the symbol link it borrowed (bench/rank-gate/gate.json `accepted` holds the glia drops FD.79 cost). Model that tie as its own evidence-carrying relation - a doc and the code it co-changes with, or a doc graph joined to the code graph - instead of DOCUMENTS edges bound by name, then retire the accepted rows.
- Wave agents never run the full rank or cohort gate (James 2026-10-30, at 0.5.4's finish: "everything will become part of 0.5.5"): measured agent time W21 109 / W22 285 / W23 60 / W24 15 / W25 367 min, W25's 86 min of it rank-gate runs in the shared tree, where a sibling's uncommitted edits were judged as FD.78's and FD.81 waited ~60 min on the gate lock; W26's FD.76 ran it again (14 min). The brief says: own tests, own fixtures and the debug CLI; the close-out owns the gates.
- Re-run the edge-precision audit (scripts/edge-audit.sh, FD.3c's method) after FD.73-FD.81 and FD.76: the eight strata under floor at FD.3c are unmeasured since their fixes (FD.76 expects DEFINES_CONFIG|fact to lose the 10 C1 rows; FD.78 shrinks samples' strata).
- Public-repo hygiene: engine/examples/rank_eval.rs names a function `kina_model` (a private repo's name, FZ.1's release check); rename it and re-take bench/edge-audit/samples/audit.jsonl, which pins it.

## 5. Small

- Close-out speed: per-step timestamps, then a faster rank gate (about 14 of a close-out's ~20 minutes).
- The DPG explainer's future directions (LLM uplift of new graph features, biology-inspired ideas; 2026-10-24): only in
  a chat paste so far; commit it as docs/dpg.md.

## 6. Packets (2026-10-31)

The re-verify and spec pass ran at 06012cc6: eight agents read the code and probed it with the release binary. They
re-checked the 77 moved packets (T4, T7's part, T8) and specced the new items: §2's plugin and environment hosts, and
§3-§5. The result:

- `dev-notes/leap-056-packets.json`: 100 packets, ~32,900 LOC. Each keeps `from` (its 0.5.4 id or spec-pass id) and
  `class`. They fall into six groups:
  - GA, watch: 20 packets. The body-only fast path is gated on GA.1, FA.1's measurement on twenty, photoprism, prefect
    and glia. Also multi-repo watch, the memory budget and the pooled Go call hook.
  - GB, concept search and placement: 5 packets.
  - GD, precision and recall fixes that move a graded result: 37 packets.
  - GH, the host graph: 20 packets, plugin hosts plus environment hosts and the `process_entry` column.
  - GP, build speed, rank-gate speed, docs as a second graph, the edge-audit re-run and hygiene: 16 packets.
  - GZ, the release docs and handoffs: 2 packets.
- `dev-notes/leap-056-corrections.json`: each packet's re-verify note, its carried 0.5.4 correction and its rulings.
- `dev-notes/precision-backlog.json`: 35 real defects with no example yet, ~9,600 LOC. A release pulls one in once an
  example turns up.
- FA.24 was dropped: an update's clone is 4-5% of its time, against FA.24's own 20% bar.

Schedule `dev-notes/wave-runner/schedule_056.py` (`--release 056`, ids G*, no wave 0, no tracks): 13 waves. Brief
`shared_brief_056.md`.

Rulings (James 2026-10-31):
- Docs graph: measure first with the pass rule committed before the run (*"Is fine sounds good"*).
- A URL may be folded from a string literal handed to a constructor (*"Sounds fine"*).
- Multi-repo watch stays (*"yeah it needs to be here it's a real world limitations sometimes"*: the test repos are
  monorepos only because they were built here).
- A separate `process_entry` pattern column (*"yeap"*).
- No new registry ids, with the host table as data (*"Why a table ? But sounds fine"*, then *"Okay as long as it's not a
  crutch"*: a row never mints an edge by name; the parser must read the registration in the AST, gated on the host
  being present).
- Shell entry scripts are read for their start / `exec` line only (*"sounds fine"*).
- The listed defaults hold (*"Everything else sounds fine"*):
  - wave agents never run the full gates;
  - FA.1 runs on the cohort repos;
  - FA.24 is dropped;
  - a fix keeps its whole bundle;
  - JVM projects keep their label and gain a coordinate field;
  - Neovim keymaps are not commands;
  - the semantic flow tier is fused only.
