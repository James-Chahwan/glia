# glia 0.5.5 — proof, plus the tracks moved out of 0.5.4 (scope seed)

**Born 2026-10-30.** James cut 0.5.4 to what was built plus two small tracks (*"Nah we can change to option 3 to round of
0.5.4 now right ?"*, superseding the 2026-10-24 ruling that 0.5.4 ships only after every track). 0.5.4 ships T0 T1 T2 T3
T5 T10 (+FD.31) T11 TS TH and FH.11c. Everything below moves here.

## 1. Tracks moved from 0.5.4 (already specced)

The packets are in `dev-notes/leap-054-packets.json` with their corrections in `leap-054-corrections.json`; the track
lists are in `dev-notes/wave-runner/tracks_054.py`. They were specced against 2026-10-23's tree, so every one needs the
re-verify pass (`dev-notes/reverify_054.md`'s method) against the shipped 0.5.4 before a wave, and a new id prefix and
`--release 055` schedule.

| track | what | packets | LOC at spec |
|---|---|---|---|
| T4 | per-language precision and recall residuals (0.5.4 plan §D, §E, F.1) | 53 | ~15,870 |
| T6 | goal-layer connector (§G) and the clarification queue (§C) | 21 | ~8,000 |
| T7 | concept search over flows (F.6), PR / issue text as intent (F.15), where new code goes (F.14) | 7 | ~2,670 |
| T8 | Stack Graphs-style incremental resolution, watch's body-only fast path, multi-repo watch, the watch memory budget (F.3) | 19 | ~7,080 |
| T9 | build-target graphs (bazel), the first non-code domain | 6 | ~2,180 |

The moved tracks' empty wave-0 slots ship in 0.5.4 untouched, reserved here (James 2026-10-30: kept rather than removed,
since 0.5.5 starts straight away): build-domain/src/lib.rs, code-domain/src/goal.rs, engine/src/clarify/{answer,questions}.rs,
engine/src/goal/{check,drift,extract,impact,join,resolve,why}.rs, engine/src/targets.rs, parsers/build/bazel/src/lib.rs,
py/src/{clarify,goal,targets}.rs. The registry ids 0.5.4's F0.1 allocated for T6 (GOAL_UNIT 50, TRACES 38, UNIT 26) stay
reserved too. 0.5.5's packets fill these slots; no new wave-0 is needed for them.

## 2. From the Sibyl cohort follow-up (2026-10-26, option 1: "C + D go to 0.5.5")

- **C: per-repo calibrated inference as a repair loop**: detect -> hypothesise a mechanism from evidence (the overlay
  loop, an LLM fallback) -> verify by numbers (pairing rises, the drift replay) -> absorb (a per-repo record; promoted to
  an engine rule once it holds on several cohort repos). Record: `dev-notes/agent-ab-probe-0.5.4.md` §5.
- **D: calibrated confidence per row**, measured on the cohort ("crime coefficient").

## 3. Proof (the original 0.5.5 row)

Precision rate on real repos (T3's audit in 0.5.4 gives the baseline), agent before / after (0.5.4's harness and FH.13's
probes), big-repo per-phase timings, calibrated confidence, `decide/` a-e, `glia judge`. Candidate after it: a RunPod
rerun of the SWE-bench Lite / latent-injection arms (James 2026-10-23).

## 4. Raised in conversation, not yet specced

- A Claude Code plugin packaging skills/, shaped by FH.13's agent-usage data (James 2026-10-24; ruled after 0.5.4).
- Native GitHub-release binaries / multi-platform install beyond the PyPI `glia-cli` wheels (2026-10-24; undecided).
- An image-embedding space for visual domains (2026-10-24; arrives with the first visual domain; VectorRow already fits).
- The DPG explainer's future directions (LLM uplift of new graph features, biology-inspired ideas; 2026-10-24): only in
  a chat paste so far; commit it as docs/dpg.md.
- Close-out speed: per-step timestamps, then a faster rank gate (about 14 of a close-out's ~20 minutes).

## 5. From 0.5.4's W20 follow-ups (James 2026-10-30: the precision extensions move here)

- Clients reached through class heritage: `this.get(url)` in a class extending an imported `APIService` / `HttpService` (the bulk of the remaining phantom_dropped: plane 43, chatwoot 128; FH.17b's handoff).
- TS type references imported from another file (`import { Dto } from './types'; http.get<Dto>()`) as USES through import bindings, and abstract method signatures' type names (FH.20's handoff).
- A NestJS verb decorator that shares its line with another decorator (`@UseGuards(X) @Get('a') foo()`) mints no ROUTE (FH.19's handoff).
- gin-vue-admin: 131 of 196 endpoints pair; the 65 unpaired look like Go router-group prefixes not folded into the route path (checked 2026-10-30).
- Dropped: a `nest_handler` rule name on Nest HANDLED_BY edges (cosmetic; the binding works through graph:refs).

## 6. From 0.5.4's W21 follow-ups and the 2026-10-30 build profile

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
