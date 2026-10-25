# Agent A/B probes and the Sibyl cohort: the record (0.5.4, 2026-10-25/26)

What the FE.4 agent before / after work found, in order, and why 0.5.4 gained the FH
batch (the Sibyl cohort follow-up, James 2026-10-26, option 1). Harness: `bench/agent-ab/`
(run.py, score.py, tasks.py); probes: `bench/agent-ab/probe/` (mine, drift, diag, oracle,
gate, names, health). Model Opus 5.5 through Claude Code 2.1.310, subscription only (ruling
FE-3); arms `none`, `g052` (v0.5.2), `g053` (v0.5.3); glia arms get the skill; 1 rep unless
said. Raw sessions live under `~/.cache/glia-agent-ab/` (never in the tree).

## 1. The pilots

| run | tasks | glia calls | outcome (none / g052 / g053) | tokens in |
|---|---|---|---|---|
| 0.5.4-pilot1 | FE.1 v1: 1..6 code files, subject prompt; 36 of 60 one-file | **0 of 12 sessions** | identical | equal |
| 0.5.4 (pilot 2) | FE.1 v2: 3..8 code files, subject names no changed file, glia arms told to start with glia | 11 of 12 sessions, ~1 call each | file recall 0.60 / 0.60 / 0.64; solved 0.17 each | 101k / 221k / 284k |

Isolation needed two fixes first: Claude Code's built-in plugins load under any config dir
(now `disabled_plugins` through `--settings`), and the newer agent / outward tools are
disallowed (Workflow, SendMessage, RemoteTrigger, PushNotification, DesignSync, cron,
worktree).

## 2. The shape probe and the drift probe

| shape | tasks | glia calls / session | outcome | verdict |
|---|---|---|---|---|
| one-file commits | 6 | 0 | identical | dead: nothing structural to ask |
| 3+ files, glia prompt | 6 | ~1 | recall .60 / .60 / .64 | weak; 2-3x tokens |
| plain (control) | 3 | ~1.5 | recall .50 all | no signal |
| ripple | 2 | ~2 | recall .42 all | no signal |
| cross-language | 2 | ~1 | recall .14 all | too hard for every arm |
| symptom (issue only) | 2 | ~1 | recall .75 all; overlap .38 / .38 / .75 | the only hint |
| drift, immich | 6 | 0-2 | 1 won, 1 lost | no signal |
| drift, photoprism | 3 | ~0 | all arms solve all | too easy (`git show HEAD` names the fields) |
| drift, twenty | 4 | ~0.5 | identical | no signal |

A drift task (`probe/drift.py`) splits a real commit that changed a server and its clients:
the checkout starts at the server half (refs/agent-ab/drift/<id>, parent = the real
parent), the truth is the authors' client half.

## 3. Why: the diagnostics

`probe/diag.py` over 117 glia-arm sessions: on the small repos 29 of 40 sessions had a glia
answer name a truth file FIRST, and the outcome still tied (grep reached it a turn later).
On the drift repos 14 of 25 loaded the skill and never ran glia; 8 ran glia and got nothing
useful. The skill added a median 16,081 tokens of context per load (glia's own answers: a
median 1,904 bytes per session). The skill is now a 9.6 KB router with `reference.md` /
`examples.md` on demand and a "before you finish a change" step (c5d8110e).

## 4. glia alone, no model

- **Gate** (`probe/gate.py`, `contract-breaks` / `review --base HEAD~1` at the broken state
  S and the fixed state C, 13 drift tasks): a client file named **0 of 13**; 0 breaking on
  12 of 13; photoprism fd8448 reported 125 rows (48 distinct changes, one shared error
  schema repeated per operation) at S and the same 125 at C; `review` named a client file
  on 2 of 13.
- **Names prototype** (`probe/names.py`): the boundary itself is missing. On immich
  `diff-impact` reached 600 files and none was the client truth file.
- **The immich chain**, each a general mechanism, not a style:
  1. `@Controller(RouteKey.User)` + `@Put('me')` mints ROUTE `PUT /me`: an enum-member
     prefix is not folded (FH.3).
  2. The generated SDK's `oazapfts.fetchJson("/users/me", { method: "PUT" })` mints no
     ENDPOINT (FH.4).
  3. `web/` imports `@immich/sdk`, a pnpm `workspace:*` package: unresolved (FH.5).
  4. 415 `.svelte` files are not parsed (FH.6).
  Also a junk `endpoint:GET:/data:image/svg+xml;base64,..` from a spec file (FH.4), and
  immich's Zod contracts (`z.object({..}).meta({ id: 'SearchAssetResponseDto' })`) are no
  node at all (FH.7 is the style-agnostic answer).
- **Boundary health** (`probe/health.py`, HTTP only, g053):

| repo | role | routes | client endpoints | HTTP pairs | client reach | dead | unparsed | flags |
|---|---|---|---|---|---|---|---|---|
| immich | app | 436 | 14 | 5 | 0.21 | 3,997 | .svelte 415, .mdx 12 | no client endpoints, SFC, workspace imports |
| twenty | app | 111 | 375 | 29 | 0.08 | 7,463 | .mdx 3,029 | clients unpaired, workspace imports (10 of 42) |
| photoprism | app | 235 | 46 | 16 | 0.35 | 1,156 | - | - |
| gin-vue-admin | app | 210 | 1 | 0 | 0.00 | 1,243 | - | routes unreached; client calls are `service({ url, method })` (FH.4) |
| samples | app | 23 | 10 | 1 | 0.10 | 1,046 | - | clients unpaired |
| microservices-demo | services | 2 | 7 | 1 | 0.14 | 78 | - | its one pair is WRONG: loadgenerator `GET /` -> cartservice (gRPC) (FH.14) |
| websocket, commons-text | library | 8, 1 | 0 | 0 | - | 27, 657 | - | false flags: libraries (FH.2 role) |
| scaffold-eth-2 | app | 5 | 0 | 0 | - | 9 | - | the client reads contracts through hooks (no HTTP) |
| shell, click | library | 0 | 0 | 0 | - | 191, 285 | - | - |

## 5. What it means, and what 0.5.4 does about it

On repos an agent can grep, glia's locate answers are right and change nothing for a
frontier model: locate-assist is obsolete there (James). glia's value is the four things an
agent cannot get file by file: facts across repos, exhaustive checks, gates, cost at scale;
and on real full-stack repos glia could do none of them, because the boundary chain broke at
mechanisms our fixtures never chained, and it said nothing about being blind. Our matrix
read 346 full over hand-made per-mechanism fixtures shaped by our own projects.

The FH batch (dev-notes/leap-054-packets.json, track TH): FH.1 the outlier cohort, FH.2
boundary health as an answer, FH.3-FH.6 and FH.14 the chain fixes, FH.7 style-agnostic
drift, FH.8 compact answers, FH.9 answers that say they are blind, FH.10-FH.11 the Sibyl
matrix (real-repo grades, chain cells, a generated pairwise half), FH.12 the cohort gate in
every close-out, FH.13 the agent probes on the shapes that matter (verifier role,
multi-repo, big repo) with the slim skill. 0.5.5 takes the learning half: per-repo
calibrated inference that hypothesises and verifies missing mechanisms (detect, hypothesise,
verify, absorb) and a calibrated confidence per row measured on the cohort.
