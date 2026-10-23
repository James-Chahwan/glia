# The rank gate (0.5.3, EG.3)

`engine/examples/rank_eval.rs` run as a release check on fixed public snapshots. A ranking change that
lowers recall@10 or MRR of a gated ranker fails the wave close-out
(`dev-notes/next-leap-0.5.3.md`, "Gate").

## What it measures

- **Snapshots** (`snapshots.json`): six public repositories pinned to a full 40-hex commit. glia at
  `v0.5.2` (`9837b1ff`, run with `--pre glia`, so its IDR reads only the public dev-notes at each
  parent revision), pop-os/shell `9616931a` (759 non-merge commits), gorilla/websocket `dcea2f08`
  (285), flutter/samples `291ba126` (1626) - round 1's repos at round 1's revisions - and, by ruling
  INT-1 (James 2026-10-23), apache/commons-lang `cee7cc6b` and apache/commons-io `11f0abe7`, round
  3's ticket-id repos. Every row is `"public": true` with an `https://` url; the script refuses any
  other row (`[rank-gate] refused <name>: not a public https snapshot`, exit 2). No private repository
  ever enters the gate, and the script unsets `GLIA_RANK_PRIVATE` and never passes the private-config
  switch, so only the public modes run.
- **One run per snapshot**, the same flags for every row:
  `--round1 on --tune on --pick base,d85 --round2 on --round3 on --r3-perq on --ans on
  --max-commits 300 --max-seeds 5 --boot 1000` (plus the row's own `flags`). `--pick base,d85` is round
  1's harness line: without it PT* is not the tuned PPR the reports measured.
- **Gated rankers** (`gate.json`, EG.2's format), per query shape:
  - `single`: ANS:impact, ANS:pack, P1, PT*, B0, H10[PT*], H9s50, SELt2, SELs0, ANS:impact@sel,
    ANS:pack@sel, ANS:impact.files
  - `k2` / `k3` / `half` (the review shapes): ANS:impact, ANS:pack, P1, SELt2, SELs0, ANS:impact@sel,
    ANS:pack@sel, ANS:impact.files
  - `find`: ANS:find, T1s, SELf2t

  Every name was checked present in every snapshot's run. No Leiden-pooled ranker is gated: those
  move with the checkout path (round 2's note). The ANS rankers are the engine's own answers
  (`diff_impact`, `pack`, `find`; EG.1); the others are the harness references they are compared to.
  By ruling EA-1 the ANS rankers read the same history window the shipped answers read.

## The ANS rankers

Each is the engine's own answer, its node rows rolled up to files by first appearance
(`rows_to_keys`), run on the snapshot copy (no `.glia/`) with an in-memory history:

- **ANS:impact / ANS:pack** (EG.1, EA.9): `diff_impact_from_diff` over the seed files and `pack_ids`
  over their nodes, default options, the RankCache given test commit c's prefix co-change (training
  commits strictly older than c), so the default ranking H10 reads C0. Item 2 (co-change first).
- **ANS:impact@sel / ANS:pack@sel** (EG.6): the same two answers with `select = false` and the ranking
  forced to the engine's own calibrated pick at c. Once per run, `calibrate::calibrate_snapshot`
  (EC.5, group `structure`) replays a history snapshot built in memory from rank_eval's commit list:
  the newest 2000 non-merge commits (`history sync`'s window, ruling EA-1), candidate paths with no
  rename fold, messages through `clean_message`; then a query of shape s at c answers with SELh0's
  `CommitScore.pick` for (s, c) (H10, H10[PT*] or H10[B0]), over the same prefix co-change as
  ANS:impact (EA.9's, from every older commit: on commons-io and commons-lang, the two snapshots with
  more than 2000 commits, that C0 sees more history than a 2000-commit sync would). Items 1 (PT* / sum pooling reaches an
  answer only through this pick) and 10 (structure-only selection). Fired_on, once per run:
  `[rank_eval] ans calibration sets=<n> test_commits=<t> ms=<ms>`.
- **ANS:impact.files** (EG.6): the default diff impact's file roll-up (`BlastRadius::files`, EA.5,
  `top_files = usize::MAX`, rank 1 first) instead of its rows: EG.1's contract that an answer's
  file-level roll-up gets a ranker reading it. ANS:impact keeps reading rows, so its 0.5.2 column
  stays comparable. Item 1.

**Why @sel is leak-free.** The pick at c is made by EC.3's online rule from test commits strictly
older than c (unit-tested in `calibrate::select`); the C0 an H10* pick reads is the prefix of training
commits strictly older than c (EA.9); and the replay scores every query before its own commit is
observed (EC.2's forward pass). A test commit with no `CommitScore` of its shape (outside the window,
or no replay query of that shape) takes the pick the rule makes from the strictly older commits: the
next newer `CommitScore`'s pick, or after the newest one the argmax of the row's summed recall@10.
`rank_eval --ans-leak on` drops the newest test commit's files from the in-memory snapshot; none of
that commit's ANS:*@sel values may move (EG.6 checked it on websocket: 20 values, 0 moved).

Measured at EG.6 (`paired` commit-cluster bootstrap, B = 1000, test commits resampled), ANS:impact@sel
minus ANS:impact, recall@10 [95% CI]: glia single +0.0007 [-0.0006, +0.0029], k2 +0.0020 [0, +0.0059],
k3 +0.0028 [0, +0.0067], half +0.0023 [0, +0.0069]; shell single +0.0008 [-0.0015, +0.0035], k2 -0.0018
[-0.0054, 0]; websocket single -0.0022 [-0.0128, +0.0053]; samples single +0.0020 [+0.0001, +0.0045], k2
-0.0020 [-0.0095, +0.0028], k3 -0.0005 [-0.0091, +0.0065]; commons-lang single -0.0006 [-0.0015, 0];
commons-io single +0.0031 [-0.0004, +0.0074], k2 -0.0010 [-0.0224, +0.0199], k3 +0.0037 [-0.0033,
+0.0116], half +0.0016 [-0.0043, +0.0084]; every other (snapshot, shape) row exactly 0. No row's CI lies
wholly below 0. The in-memory calibration costs one `calibrate_snapshot` per run (on the loaded
second check: glia 28.5 s, samples 22.7 s, commons-lang 2.6 s, the rest under 2 s) and the ANS calls two more answers per seeded
query; a whole `check` took 456 s with rank_eval built (760 s on a loaded machine), still far below
the close-out's 7200 s.

The @sel rankers and ANS:impact.files entered the gate with `baseline-add`: their 0.5.2 column is
their own first value, so the ratchet protects them from EG.6 on, not against 0.5.2.
- **Metrics**: recall@10 and MRR, per query, for every gated (shape, ranker).

## The fail rule

`check` runs `rank_eval --gate-check` (EG.2) once per (snapshot, baseline), against **both**
`baseline/0.5.2/` (the release's starting point: no 0.5.3 change may end below it) and
`baseline/current/` (the ratchet: a later packet may not undo an earlier gain), by ruling EG-1.
Each run is its own family: diff = new - base per row with the commit-cluster bootstrap (B = 1000),
and Holm's step-down over the rows whose values changed decides `drop` / `gain`, one-sided per
direction, family alpha 0.05 (ruling EG-2). This is Holm, not a per-row CI rule: with one changed row
the threshold is p <= 0.05. An unchanged row diffs exactly 0 and is `same`.

The last line of `check` is exactly one of:

    [rank-gate] check: ok - <rows> rows, <gains> gains, 0 drops on <n> snapshots
    [rank-gate] check: FAILED (<k> drops: <label>/<shape>/<ranker>/<metric>, ..)      exit 1
    [rank-gate] check: FAILED (drift <label>/<shape>, ..)                             exit 2
    [rank-gate] check: FAILED (<fetch / build / run / missing ..>)                    exit 2

`rows` counts every judged row of every (snapshot, baseline) check; `gains` and the drop list are
distinct `<label>/<shape>/<ranker>/<metric>` rows over both baselines. Drift means a shape's queries
or commit clusters moved: only a declared re-baseline fixes it. The harness is deterministic, so a
rerun never clears a drop.

## Running it

    bash scripts/rank-gate.sh fetch          # bare clones + git-archive copies (cached after the first)
    bash scripts/rank-gate.sh run            # build rank_eval (release) and run + trim every snapshot
    bash scripts/rank-gate.sh check          # fetch + run + judge; the close-out gates on its last line
    bash scripts/rank-gate.sh coverage       # every scope item measured or exempt

Everything is written under `GLIA_RANK_GATE_DIR` (default `~/.cache/glia-rank-gate`): `<name>.git`
(bare clones), `<name>@<rev12>/` (the copies; the directory name is part of the build identity, never
rename it), and `run/` (runs, trims, verdicts, logs, `last-check.json`). Nothing lands in the repo.
rank_eval is built in `CARGO_TARGET_DIR` (default `target/`). The fetch is shared by every agent: never
delete it. A second fetch performs no network operation (`[rank-gate] fetch <name>: cached`).

Cost at EG.3 (16 threads, wall seconds of `run`, rank_eval build excluded): glia 203 s (1253 queries), samples 42 s (1111), commons-lang
24 s (863), commons-io 18 s (937), shell 15 s (641), websocket 6 s (278); a whole `check` with rank_eval
already built took 5 min 13 s, and the first release build of rank_eval about 1 min 45 s more. The
close-out's timeout is 7200 s (twice the measured wall is far below it; 7200 is the floor).

## Baselines

`baseline/<release>/<label>.json` are `rank_eval --gate-trim` outputs: per shape the query `ids`,
`commit_of` and each gated ranker's per-query values. Size at EG.3: `du -sh bench/rank-gate/baseline` = 4.0M (2.0M per
release directory, six snapshots; glia 538K, samples 488K, commons-io 417K, commons-lang 370K, shell
193K, websocket 76K).

- `baseline/0.5.2/` was captured by EG.3 with `bash scripts/rank-gate.sh baseline 0.5.2` before any
  ranking packet landed (the engine still ranked as 0.5.2). It never moves except by a declared
  re-baseline.
- `baseline/current/` started equal to it. The close-out's `ratchet` copies the last green check's
  trims over it, only those whose values changed (`glia_head` alone never rewrites one), and refuses
  unless the last check was ok and ran at HEAD.
- `baseline <release>` (a re-baseline) may be run only by the integrator or by a packet that declares
  it. Never edit these files by hand.
- `baseline-add`: a packet that adds a gated ranker (EG.6, EG.7) edits `gate.json`, runs `check`
  (the new ranker reports `new`, unjudged), then `baseline-add`, which merges only the new columns
  from that check's trims into both baselines (it adds columns, never rewrites one, and refuses when
  the query ids differ).

## Coverage

`gate.json` `items` maps every scope item of `dev-notes/next-leap-0.5.3.md` (1-11) to the gated ANS
ranker(s) that measure it in an answer, or to `{"exempt": <why>}` (items 5 and 9). The packet that
ports an item into an answer lists its ranker there (items 1, 2, 10: EG.6; items 3, 4, 6, 7, 8, 11:
EG.7). `coverage` ends `[rank-gate] coverage: ok - <m> items measured, <e> exempt` or
`[rank-gate] coverage: FAILED (unmeasured items: <list>)` (exit 1); the release's last close-out
requires ok.

## The close-out

`dev-notes/wave-runner/closeout.py` runs `check` after the engram-export check in every wave of a
release that has `bench/rank-gate/gate.json`, unless no packet commit of the wave touched `core/`,
`code-domain/`, `graph/`, `parsers/`, `engine/`, `activation/`, `bench/rank-gate/` or
`scripts/rank-gate.sh` (`[rank-gate] check: skipped - wave touched no ranked path`). A timeout is a
failed gate (`rank gate (timeout)`). When every gate is clean it runs `ratchet` and commits
`bench/rank-gate/baseline/current`. The release's last close-out always checks, then runs `coverage`.
