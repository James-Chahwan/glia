# glia 0.5.4 - the post-0.5.3 re-verify pass

0.5.4 was specced (2026-10-23) against a moving HEAD (37fb9384 .. 42215a67) while 0.5.3's waves landed (W0-W3 of 16 at integration). Seed §4: *"After 0.5.3's last close-out, a re-verify pass (one agent per group) checks every packet against the landed 0.5.3 tree and rewrites what moved; only then do 0.5.4's waves start."* This file is that pass's procedure. It edits only the scratchpad / plan copies of leap-054-packets.json and leap-054-corrections.json, never code.

## 0. Preconditions (stop if any fails)

1. 0.5.3's last close-out has run: `python3 dev-notes/wave-runner/schedule_053.py --verify` prints VERIFIED with every wave LANDED, and the close-out log lists no unrouted followup (0.5.3 finishes its own followups).
2. EZ.1 / EZ.2 have landed (CLAUDE.md and the 0.5.3 handoffs describe the tree you verify against). The version bump to 0.5.3 may or may not have happened; it changes nothing here except `cargo pkgid` strings in F0.2's acceptance.
3. Record `BASE=$(git rev-parse HEAD)`; every rewrite cites it.

## 1. Inputs per group agent

- its packets from leap-054-packets.json (`jq '.packets[] | select(.id | startswith("FA."))'`), its group's `<G>.verify.md` notes (scratchpad), and the integration_report's text_patches / deps_patch for its ids;
- `git log --oneline 37fb9384..$BASE` and, per E* packet it leans on (table below), that packet's landed commit(s): `git log --grep '<E-id>' --oneline`;
- leap-053-corrections.json (rulings and HANDOFFs that changed E* packets after spec: e.g. EA-1 history window 2000, EG-1/EG-2 Holm, INT-1 six gate repos, EB-5 markdown tracker globs, EC-2 strict BUILD_STAMP, EA-5+EC-7 SELh0 default).

One agent per group (FA, FB, FC, FD, FE) plus one for F0 / FZ. The F0 agent goes first (its outputs - next free ids, the slot list, the lock state - are inputs to the others).

## 2. What to diff (per packet)

For every packet, mechanically then by reading:

1. **Paths.** Every `files_touched` entry exists at BASE or is created by a named F*/E* packet (`python3 dev-notes/wave-runner/validate_packets.py packets <batch file> <G>` prints no WARN). A file 0.5.3 moved or split (e.g. engine/src/review.rs, find.rs, pack.rs, watch.rs, rank_eval.rs, py/src/*.rs) is re-read.
2. **Symbols.** Every backticked identifier in entry_points / implementation / acceptance resolves at BASE: `git grep -n -w '<sym>'` (or the repo-graph MCP after a refresh). For E* APIs, the name, signature and module path are taken from the LANDED code, not from leap-053-packets.json text (the 0.5.3 integrator and its corrections renamed several: e.g. EB.3's `candidates(merged, repo)` became `Candidates::of(&FileIndex, repo)`, EB.3 no longer makes `module_files` pub(crate)).
3. **E* behaviour assumptions.** Each row of the table below names what the 0.5.4 packet assumes; confirm it from the landed commit body (declared breaks, measured numbers, followups). In particular: the rank gate's commands and repo set (six repos after INT-1), the `[rank-gate] check: ok` line form, `baseline-add` accepting only new rankers, gate.json shape keys (`single/k2/k3/half/find`), the history snapshot version (2) and messages.jsonl fields (`at`, `br`), IdVocab / IdsConfig / TrackerDecl names, the calibration record layout and `select()` / `log_selection()` call shape, RankCache holders (PyGraph, WatchSession, CLI), FileIndex as the one file space.
4. **Counts and pins.** Literal counts quoted in acceptance (test counts, `[passes]` counts, gaps CATEGORIES, `grep -c '^pub mod '`, registry rows, py `mod` lines) are re-read at BASE; integrator text patches already made the cross-group ones HEAD-relative - keep them relative.
5. **Done already?** A packet (or a row of an FD packet) whose behaviour 0.5.3 already shipped is marked `not-needed` with the commit (FD's PROBE clause), never re-implemented.
6. **Surfaces.** Every cli/surface/<cmd>.txt, py/api_surface/<m>.txt, README `## CLI` line and skill file a packet regenerates still exists under that name at BASE (0.5.3 regenerated find / review / blast / pack / diff_impact / snapshots / watch).
7. **Gate wiring.** Any packet whose acceptance runs `bash scripts/rank-gate.sh <cmd>` uses a subcommand that exists in the landed script; a packet adding a gated ranker uses `baseline-add` only for a NEW ranker (FB.7c's verify note) and never edits gate.json's 0.5.3 `items`.
8. **Ordering.** If an E* change created a new file overlap between two F packets with no dependency, either add a `depends_on` (deps_patch) or note that file overlap serialises them; rerun `python3 schedule_054.py --file <copy>` and paste the new wave table.

Each agent returns, per packet: `unchanged` | `rewritten` (with a unified diff of the JSON fields) | `not-needed (<commit>)` | `blocked (<reason>)`, and a list of new open questions. Rewrites go into the packet file (spec text) or, when they only add a fact, into leap-054-corrections.json as `RE-VERIFY (<BASE>): ...` entries, which override the packet where they disagree.

## 3. Hot spots (known moving targets at integration time)

- **engine/src/watch.rs** (EA.7 RankCache holder, EC.1 calibration exclusions): FA.4 FA.8 FA.9 FA.12 FA.24 FB.3 FC.4 FD.66 FD.67 - re-read WatchSession's fields and WatchPhases before FA.8 (per-repo state split).
- **engine/src/review.rs / find.rs / pack.rs** (EA.5 EA.6 EB.7 EB.8 EC.8 EC.9 EC.11 EC.13 EG.7): FA.20 FB.7a FB.8 FB.11 FB.12b FB.13c FC.10 FC.16 FC.20 FC.22 FC.24 FD.7c FD.28 FD.69 - landed field names (`files`, `selection`, `intent`, `title`) and section order.
- **engine/src/intent/** (EB.3-EB.6): FC.2 FC.3a FC.3b FC.5a - `DefRow`, `IdDefs::namespaces`, `Namespaces.def`, `read_defs`'s signature (FC.3a splits it into `defs_from_texts`).
- **engine/examples/rank_eval.rs** (EG.1 EG.2 EG.6 EG.7 EA.9 EB.9): FB.6 FB.7c FC.19 FC.20 FC.21 FC.22 FA.1 (surface_replay is its own example) - flag names (`--sem`, `--pr-intent`, `--place`) must not collide with landed flags.
- **snapshots/src/lib.rs** (EB.1): FA.15 FB.13a FC.17a - module list and the history writer's helpers they reuse.
- **scripts/rank-gate.sh, bench/rank-gate/** (EG.3, EG.6, EG.7 and corrections EG-1 / EG-2 / INT-1): every packet naming the gate.
- **CLAUDE.md / README / skills** (EZ.1): FZ.1 appends, FE.7 counts EXAMPLED as "HEAD length + 6".
- **py/src/graph.rs** (EA.7 RankCache field): FB.5c, FA.19, FC.7, FC.14 - PyGraph fields their methods read.

## 4. Packets leaning on 0.5.3 (E* ids and symbols named in the packet text)

| packet | E* ids cited | 0.5.3 symbols / surfaces named |
|---|---|---|
| F0.3 | E0.2 | calibrate |
| F0.5 | EG.3 | rank-gate.sh |
| FA.2 | EG.3 | rank-gate.sh |
| FA.4 | EA.7 EC.1 |  |
| FA.8 | EA.7 EC.1 | RankCache |
| FA.9 | EA.7 | RankCache |
| FA.15 | EB.1 | select( |
| FA.16 |  | select( |
| FA.20 | EA.5 EB.7 EC.11 EG.7 | rank-gate.sh |
| FA.22 | EG.3 | rank-gate.sh |
| FA.23 |  | rank-gate.sh |
| FB.3 | EA.7 EC.1 | calibration-snapshot |
| FB.6 | EA.9 EB.2 EB.3 EB.9 EG.1 EG.2 EG.3 EG.6 EG.7 | algo::fuse, rank-gate.sh |
| FB.7a | EB.8 EC.13 | rank-gate.sh, ANS: |
| FB.7b | EA.7 EB.8 | RankCache |
| FB.7c | EG.1 EG.3 EG.7 | rank-gate.sh, gate.json, baseline-add, ratchet, ANS: |
| FB.8 | EA.6 EC.9 | calibrate, rank-gate.sh, ANS: |
| FB.9 |  | rank-gate.sh |
| FB.11 | EA.6 EC.9 | rank-gate.sh, ANS: |
| FB.12a | EB.4 |  |
| FB.12b | EA.5 EB.7 EC.11 EC.12 EG.7 | rank-gate.sh |
| FB.13a | EB.1 | messages.jsonl |
| FB.13b | EB.1 EC.6 | calibrate, history calibrate |
| FB.13c | EB.8 EC.13 |  |
| FB.13d | EG.4 |  |
| FC.1 | EB.5a |  |
| FC.2 | EB.4 | IdVocab, IdsConfig |
| FC.3a | EB.4 EB.5a EB.5b | IdVocab, IdsConfig, markdown_globs, DefRow, IdDefs, Namespaces |
| FC.3b | EB.4 EB.5b EB.5c EB.6 | IdVocab, DefRow, IdDefs, Namespaces |
| FC.4 | EB.4 EB.5b EB.5c | IdVocab |
| FC.5a | EA.3 EB.3 EB.5c | FileIndex, Candidates::of |
| FC.5b | EA.3 EA.4 EB.3 EB.4 EB.5b EB.8 EG.3 | FileIndex, rank_files, rank-gate.sh, find_files |
| FC.8 | EB.3 EB.5b |  |
| FC.10 | EB.4 EB.5b EB.7 | IdVocab, review --title, title_source |
| FC.11 | EG.3 | rank-gate.sh |
| FC.15 | EG.3 | rank-gate.sh |
| FC.16 | EB.8 |  |
| FC.17a | EB.1 | redact_untrusted |
| FC.17b | EB.1 |  |
| FC.18 |  | IdVocab |
| FC.19 | EB.6 EC.12 EG.3 EG.7 | rank-gate.sh, SELt2 |
| FC.20 | EB.7 EC.11 EC.12 EG.1 EG.7 | rank-gate.sh, gate.json, baseline-add, ANS:, title_source |
| FC.21 | EC.3 EC.4 EG.3 | calibrate, rank-gate.sh |
| FC.22 | EA.3 EA.7 EB.3 EB.8 EC.11 EG.1 | RankCache, CochangeIndex, TextIndex, calibrate, rank-gate.sh, gate.json, baseline-add, ANS:, find_files |
| FC.23 | EB.1 EC.10 | read_history_messages, HistoryMessage, messages.jsonl |
| FC.24 | EB.4 EG.6 | Ranking, rank-gate.sh, ANS: |
| FD.2 | EG.3 | rank-gate.sh |
| FD.4a | EG.3 | rank-gate.sh |
| FD.4b | EG.3 | rank-gate.sh |
| FD.4c | EG.3 | rank-gate.sh |
| FD.4d | EG.3 | rank-gate.sh, ratchet |
| FD.5a | EG.3 | rank-gate.sh |
| FD.5b | EG.3 | rank-gate.sh |
| FD.6 | EG.3 | rank-gate.sh |
| FD.7c | EA.5 EA.6 EB.7 EG.3 | rank-gate.sh |
| FD.8 | EG.3 | rank-gate.sh |
| FD.9 | EG.3 | rank-gate.sh |
| FD.10a | EG.3 | rank-gate.sh |
| FD.10b | EG.3 | rank-gate.sh |
| FD.11 | EG.3 | rank-gate.sh |
| FD.12 | EG.3 | rank-gate.sh |
| FD.13 | EG.3 | rank-gate.sh |
| FD.14 | EG.3 | rank-gate.sh |
| FD.15 | EG.3 | rank-gate.sh |
| FD.16 | EG.3 | rank-gate.sh |
| FD.17 | EG.3 | rank-gate.sh |
| FD.18 | EG.3 | rank-gate.sh |
| FD.19 | EG.3 | rank-gate.sh |
| FD.20 | EG.3 | rank-gate.sh |
| FD.21 | EG.3 | rank-gate.sh |
| FD.22 | EA.3 EG.3 | rank-gate.sh |
| FD.25 | EG.3 | rank-gate.sh |
| FD.27 | EG.3 | rank-gate.sh |
| FD.28 | EA.5 EC.8 EG.3 | rank-gate.sh |
| FD.29 | EG.3 | rank-gate.sh |
| FD.30 | EG.3 | rank-gate.sh |
| FD.31 | EG.3 | rank-gate.sh |
| FD.32 | EG.3 | rank-gate.sh |
| FD.33 | EG.3 | rank-gate.sh |
| FD.35 | EG.3 | rank-gate.sh |
| FD.36 | EG.3 | rank-gate.sh |
| FD.37 | EG.3 | rank-gate.sh |
| FD.38 | EG.3 | rank-gate.sh |
| FD.39 | EG.3 | rank-gate.sh |
| FD.40 | EG.3 | rank-gate.sh |
| FD.41 | EG.3 | rank-gate.sh |
| FD.42 | EG.3 | rank-gate.sh |
| FD.43 | EG.3 | rank-gate.sh |
| FD.44 | EG.3 | rank-gate.sh |
| FD.45 | EG.3 | rank-gate.sh |
| FD.46 | EG.3 | rank-gate.sh |
| FD.47 | EG.3 | rank-gate.sh |
| FD.48 | EG.3 | rank-gate.sh |
| FD.49 | EG.3 | rank-gate.sh |
| FD.50 | EG.3 | rank-gate.sh |
| FD.51 | EG.3 | rank-gate.sh |
| FD.52 | EG.3 | rank-gate.sh |
| FD.53 | EG.3 | rank-gate.sh |
| FD.54 | EG.3 | rank-gate.sh |
| FD.55 | EG.3 | rank-gate.sh |
| FD.56 | EG.3 | rank-gate.sh |
| FD.57 | EG.3 | rank-gate.sh |
| FD.58 | EG.3 | rank-gate.sh |
| FD.59 | EG.3 | rank-gate.sh |
| FD.60 | EG.3 | rank-gate.sh |
| FD.61 | EG.3 | rank-gate.sh |
| FD.62 | EG.3 | rank-gate.sh |
| FD.63 | EG.3 | rank-gate.sh |
| FD.64 | EG.3 | rank-gate.sh |
| FD.65 | EG.3 | rank-gate.sh |
| FD.66 | EG.3 | rank-gate.sh |
| FD.67 |  | rank-gate.sh |
| FD.71 |  | rank-gate.sh |
| FE.1 | EG.3 |  |
| FE.2 | EB.1 EC.1 EC.11 EC.13 EC.6 EC.8 EC.9 EZ.1 EZ.2 | calibrate, review --title, history calibrate, calibration-snapshot |
| FE.4 | EZ.2 | review --title |
| FE.5 | EG.5 |  |
| FE.6 | EG.5 |  |
| FE.7 | EB.7 EB.8 EC.6 EZ.1 | calibrate, review --title, history calibrate |
| FE.8 | EA.3 EB.2 EC.13 EC.8 EG.3 | calibrate |
| FZ.1 | EZ.1 | rank-gate.sh |
| FZ.2 | EZ.2 |  |

The table is generated (integrator) from the packet JSON; a packet absent from it names no 0.5.3 API and needs only checks 1, 2, 4 and 6.

## 5. After the pass

1. Validate each batch (`validate_packets.py packets <batch> <G>` for F0 FA FB FC FD FE FZ: all `OK`).
2. Rerun `python3 schedule_054.py --file leap-054-packets.json` and replace integration_report.waves with its output (`--json`).
3. Fold any rulings James has given since 2026-10-23 into `rulings` and the matching corrections (the 0.5.3 form: `RULING (overrides the packet where they disagree): <id>: James <date> (<quote>): <decision>`), and move answered questions from open_questions to open_questions_resolved; check `rulings_needed_before_wave` - a wave whose ruling is missing does not start.
4. The 0.5.4 plan commit (pre_w0_plan_commit), then W0.
