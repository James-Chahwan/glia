# glia 0.5.7 — the goal layer, build targets, proof and the id allocator (scope seed)

**Born 2026-10-31**, split out of the 0.5.6 seed. James: *"Yeah that's probably better to make this 2 releases and then
let's update and complete 0.5.6"*. 0.5.6 finishes the started work; this release takes the new capabilities. It starts
after 0.5.6 ships.

## 1. Tracks moved from 0.5.4 (already specced)

The packets are in `dev-notes/leap-054-packets.json` with their corrections in `leap-054-corrections.json`; the track
lists are in `dev-notes/wave-runner/tracks_054.py`. Each needs the re-verify pass against the shipped 0.5.6 tree.

| track | what | packets | LOC at spec |
|---|---|---|---|
| T6 | goal-layer connector (§G) and the clarification queue (§C) | 21 | ~8,000 |
| T7 (part) | PR / issue text as intent (F.15): FC.19's measured experiment, then FC.20 `glia review --pr` only on PASS | 2 | ~840 |
| T9 | build-target graphs (bazel), the first non-code domain | 6 | ~2,180 |

They fill the 16 wave-0 slots reserved since 0.5.4 (`Reserved for 0.5.7 (track T6|T9)`): build-domain/src/lib.rs,
code-domain/src/goal.rs, engine/src/clarify/{answer,questions}.rs, engine/src/goal/{check,drift,extract,impact,join,resolve,why}.rs,
engine/src/targets.rs, parsers/build/bazel/src/lib.rs, py/src/{clarify,goal,targets}.rs, and the registry ids 0.5.4's F0.1
allocated for T6 (GOAL_UNIT 50, TRACES 38, UNIT 26).

## 2. From the Sibyl cohort follow-up (2026-10-26, option 1: "C + D go to 0.5.5", now 0.5.7)

- **C: per-repo calibrated inference as a repair loop**: detect -> hypothesise a mechanism from evidence (the overlay
  loop, an LLM fallback) -> verify by numbers (pairing rises, the drift replay) -> absorb (a per-repo record; promoted to
  an engine rule once it holds on several cohort repos). Record: `dev-notes/agent-ab-probe-0.5.4.md` §5.
- **D: calibrated confidence per row**, measured on the cohort ("crime coefficient").

## 3. Proof (the original 0.5.5 row)

Precision rate on real repos (T3's audit in 0.5.4 gives the baseline), agent before / after (0.5.4's harness and FH.13's
probes), big-repo per-phase timings, calibrated confidence, `decide/` a-e, `glia judge`. Candidate after it: a RunPod
rerun of the SWE-bench Lite / latent-injection arms (James 2026-10-23).

The agent before / after compares against `bench/agent-ab/results/0.5.4/baseline.json`; the edge precision against
FD.3c's baseline and 0.5.6's re-run.

**Section-level doc ties (parked from 0.5.6, James 2026-10-31 "okay fine").** 0.5.6 GP.9 measured file-level doc ties
(a doc file and the code files it co-changes with) as a one-hop bridge in ANS:impact and they FAILED
(`bench/rank-gate/doc-ties/report.md`: `[doc-ties] verdict=FAIL glia r10=0.2551`, floor 0.2594), so GP.10 / GP.11 were
not built and the rank gate's four FD.79 `accepted` glia rows stay. The retry: tie a doc SECTION (not the whole file) to
the code it changes with, which needs a history snapshot that records each commit's hunks, measured the same way against
a pass rule committed first; on PASS, build the bridge and empty `accepted`.

**Declared entries seed no entry flows (found by 0.5.6 GZ.1, 2026-11-01).** `entry_flows` seeds from
`CODE_PROFILE.tables.entry` (kinds, roles and names) only, while liveness also counts declared entries:
a node's ENTRYPOINT cell (DT.5l's `main_guard`, 0.5.6's `host:<row>` from resolver:host, GH.4 / GH.8 /
GH.9 / GH.10 / GH.11's host callbacks and script entries). So a host-started MODULE is live but has no
flow: on bench/substrate-gap/patterns/python/process_entry, `glia flows` lists only `GET /health`, not
worker.py's `loop -> process`. Seeding flows from ENTRYPOINT cells too changes `flows`, `find --flows`
and concept search's flow set, which GB.6's gate measured on the 0.5.6 tree; so it lands with a re-run of
the concept gate (`scripts/embedding-gate.sh concept-verdict`) under GB.6's committed rule.

**MCP servers as a host (reported by the repo-graph session, 2026-11-02, on glia-py 0.5.6).** A function
registered as an MCP tool by decorator is no entry: on repo-graph (repo_graph/server.py, `@mcp.tool()` on a
FastMCP / `MCPServer` instance), `glia impact repo_graph::server::get_graph --direction backward` marks all six
tool functions dead (the same on 0.5.5 and 0.5.6, so not a regression). It fits `code_domain::hosts` as GH.8's
Flask rows do (Shape::Decorator, Gate::Imports on `mcp` / `fastmcp`): `@<server>.tool()` / `.resource(<uri>)` /
`.prompt()` -> a host registration (`host_cmd:mcp:<name>` / `host_event:mcp:resource:<uri>` ..) HANDLED_BY the
function, which is then an entry; the TypeScript SDK's `server.tool(..)` / `registerTool` / `registerResource`
/ `registerPrompt` as the call form. Repro: a shallow clone of repo-graph, the query above; a
`patterns/python/host_command` fixture row and its TS twin.

## 4. Becoming PR-friendly (James 2026-10-31, after glia's first PR)

James: *"I just realized glia is never gonna be pr friendly"*, then *"we could just auto allocate ids for a bunch of
code but in 0.5.6 we are doing more graph work and stacking and domains soooo ... Maybe we will think if some way like
port protocol number system to domain or something else like a domain then allocator number idk ... But yeah we should
handle this at some point"*. `CONTRIBUTING.md` (0.5.5) states the current model: issues, fixtures, PRs read as designs
and rebuilt with credit, never an id in a PR. To design alongside 0.5.6's graph stacking and domains:

- **Id allocation by domain, like port / protocol numbers.** The u32 id spaces (node kinds, edge categories, cell
  types) split into blocks: a core block, one registered block per domain (code, build targets, goals, later visual
  domains), and an experimental / private range that never ships, so a branch or a local domain can mint ids without
  colliding and a release promotes them into the domain's block. An allocator (`glia ids alloc --domain <d>` or a
  build-time check) hands out the next number in a block and fails CI on an overlap; the header registries a `.gmap`
  already carries (LC.4) record which block each id came from. Old layouts keep decoding: allocated ids never move.
- **Contribution seams that stay stable across a leap**: a parser crate behind the `detect_language` / `parse_one_with`
  / build-group wiring, a fixture, a pattern cell; documented so a PR touching only a seam can merge.
- **Gates a contributor can run**: a fast tier (minutes) for PRs, the full rank / cohort gates at release only.

## 4b. Wave-runner: reuse unchanged gates (agreed 2026-11-01, built right after the 0.5.6 release)

James, after the 0.5.6 release close-out re-ran ~65 min for a baseline-only fix: *"Yeah add after the
release it's probably will happen if we need to run it particularly every couple of waves if a specific
change is required"*. `closeout.py` records, per gate, the inputs of its last green run (the code tree's
PARSER_STAMP / BUILD_STAMP and the tracked source digest for tests, fmt, clippy, rustdoc and the wheel;
that plus `bench/rank-gate/` for the rank gate; that plus `bench/cohort/results/baseline/` and the cohort
pins for the cohort gate; the fixture tree for run.py / matrix). A re-run whose inputs to a gate did not
move reuses its result and says so (`[closeout] reuse <gate> from <commit>`); a gate whose inputs moved,
or that failed, runs again. The release's last close-out still needs one full green set, built from
reused and fresh results over the same tree. A ratchet runs only from a fresh full run.

## 5. Distribution and later

- A Claude Code plugin packaging skills/, shaped by FH.13's agent-usage data (James 2026-10-24; ruled after 0.5.4).
- The Rust-native install (`cargo install` / crates.io, GitHub-release binaries) beyond the PyPI `glia-cli` wheels.
  James 2026-10-31, on why glia-cli is a PyPI package: *"okay sure go for it we will do the rust part later then"*:
  glia-cli ships on PyPI with 0.5.4; the Rust channel comes later.
- An image-embedding space for visual domains (2026-10-24; arrives with the first visual domain; VectorRow already fits).

## 6. Packets and the split (spec pass 2026-11-02 at c1a37f86)

James, 2026-11-02: *"All the specced work should come and spec everything but distribution for 0.5.7. That should land
smaller then 0.5.6 and we could make 0.5.7 2 releases due weekly cutoff reset"*. Seven read-only agents re-verified the
29 moved packets (all rewritten against the 0.5.6 tree, none already done) and specced the rest: 61 packets, ~26,100 LOC.
The split (James: *"Fine"*):

| release | ids | packets | LOC | file |
|---|---|---|---|---|
| 0.5.7 | H* | 27 | 10,360 | `leap-057-packets.json`, `leap-057-corrections.json`, `wave-runner/schedule_057.py` (W0-W9), `shared_brief_057.md` |
| 0.5.8 | J* | 35 | 15,570 | `leap-058-packets.json`, `leap-058-corrections.json`, `wave-runner/schedule_058.py` (W0-W15); its brief is written at its kickoff, after a re-verify against 0.5.7 |

- **0.5.7.** HP proof and process: HP.1 the close-out's gate reuse (§4b), wave 0 alone, with clippy as a recorded gate
  (James: *"Yeap sounds good"*; clean at c1a37f86, 0 warnings), HP.2 re-judging when only a baseline moved, HP.3 build
  timings, HP.4 the doc linker's DOCUMENTS|derived floor, HP.5 the precision run, HP.6 retiring the rank gate's four
  FD.79 accepted rows. HF entry flows from declared entries with the concept gate re-run (§3), MCP servers as a Python
  and a TypeScript host (§3), Rails nested resources (the chatwoot decline). HT build targets (T9) with the id blocks,
  `glia ids`, contribution seams and `scripts/pr-check.sh` (§4). HC calibrated confidence (§2 D): a model-written
  overlay stanza's pairings tier heuristic, the held-out audit panel, the run, per-row confidence only on PASS. HZ docs
  and handoffs.
- **0.5.8.** JG the goal layer, the clarification queue and PR intent (T6, FC.19 / FC.20), JR the repair loop (§2 C),
  JD `decide/` and `glia judge`, JP.1 the agent before / after, JZ docs and handoffs.
- **Id blocks** (James: *"5 [is] fine"*, *"lol code being 0 is such a funny happen stance"*): the high 16 bits of an id
  name its domain's block, 65,536 per block: 0 code (every existing id unchanged), 1 core, 2 build, 3 goal (reserved),
  0x8000-0xFFFE private. Nothing new on disk. GOAL_UNIT / TRACES / UNIT stay in block 0.
- **Doc ties** (James: *"go with recommend"*): the four FD.79 rows did not bind at the 0.5.6 release (glia ANS:impact
  r10 0.2640 vs 0.5.2's 0.2595; only `websocket/single/B0/mrr` was used), so HP.6 retires them; the section-level
  measurement is not built (probe: section ties reach 13 code files vs 18 for file ties, best case +0.0078 vs +0.0149
  r10).
- **Other rulings**: the goal graph is built for every repo (*"yeah recommended"*); glia never calls a language model in
  the repair loop (*"Sounds good"*); the agent before / after runs a pilot first, then James decides (*"Okay"*); Laya's
  own Python runs once in podman for the port check (*"Yes sounds fine"*). Every other spec question takes its default;
  all are listed with their answers in `leap-057-packets.json` `decisions`.
