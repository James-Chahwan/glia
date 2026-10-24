# The use signal: a registered-test proposal (joint with repo-graph)

**Status: proposal; owner repo-graph.** Written in glia 0.5.4 (packet FE.8). glia builds nothing for
it. Registration = a dated commit of this file plus repo-graph's acknowledgement; James decides
whether to send it. `[use-signal]`

Source: `dev-notes/next-leap-0.5.4.md` §F, after item 15 ("glia builds nothing for it until
repo-graph logs usage and a test is registered"), and the registered-test rule from Engram's DE.5
outcome (§0, 2026-10-18: register a test with a reason first, then ask).

## 1. Question

Do the answer rows an agent opened or acted on predict what later work in the same repo touches,
beyond what commit history already gives (co-change C0, 0.5.3 EA.3) and beyond the 0.5.3
calibrated rankings (the per-repo pick, EC.8 / EC.13)? If not, there is no use signal worth a
glia feature, and this file closes it.

## 2. What repo-graph would log

repo-graph's MCP server, opt-in (off by default), local only, never uploaded.

Per tool call:
`{ts, session: sha256(session id), tool, query: sha256(query text), glia_version,
rows: [{qname, file, rank}]}`.

Per later agent action in the same session:
`{ts, session, action: opened_file | edited_file | read_node, file, qname?}`, joined to the
earliest earlier row that named the same file (or qname): `{row_rank, row_tool, row_call_ts}`.
An action no returned row named is logged with `row: null` (it counts for the held-out truth,
not for U).

Retention and redaction: no source text, no query text (hash only), no author or user identity;
files are repo-relative paths; qnames are glia qnames as returned. A per-repo log, rotated by the
user. Where it lives: repo-graph's own data dir, never `.glia/`, so no glia format, snapshot or
build input changes.

## 3. The registered test (fixed before any data is read)

Units: sessions with >= 1 edit, per repo, in time order.

Signal: `U(file)` = recency-weighted count of "opened or edited after being returned" events over
the repo's EARLIER sessions only (weight `0.5 ^ (age_days / 30)`; the half-life is fixed here,
never tuned). Leak rule: a held-out session reads U built strictly from sessions that ended
before it started, and C0 / the calibration record from commits before its start.

Task: rank the files a held-out session edits, given its first answer's seed files (the distinct
files of the rows of its first glia call; a session whose first call returned no row is
excluded and counted).

Rankers, each over the same candidate set (0.5.3's FileIndex for the repo at that time):
- `CAL`: the 0.5.3 calibrated pick for the seeds' shape (EC.8; EC.13 for a find-shaped call),
  read through `rank::rank_files` with the repo's calibration record; fallback when none, named.
- `C0`: co-change alone (EA.3's `c0`).
- `U`: the use signal alone, ties by (repo label, path).
- `U+CAL`: min-max normalise each, mean, through `algo::fuse` (EB.2's `minmax` + `mean`).
- Control `U~`: the same per-file counts, permuted among the files of each directory (seeded,
  one permutation per repo, seed written here: 20261024). It keeps U's popularity and directory
  shape and destroys its file identity.

Metrics: recall@10 (the session's edited files) and MRR (first edited file), paired by session.
CI: the task-cluster percentile bootstrap of `bench/agent-ab/score.py` (`bootstrap`, its SEED and
RESAMPLES), one cluster per session, 95%.

Pass rule (both parts, recall@10 the primary metric, MRR reported beside it):
1. `U+CAL - CAL > 0` with the CI excluding 0 on at least 2 of 3 repos; and
2. `U - U~ > 0` with the CI excluding 0 pooled (the signal is the files, not popularity).

Anything else is FAIL. No rerun with another half-life, permutation or ranker set to change the
verdict.

Minimum data: 200 sessions with edits per repo, over >= 3 repos. Repos are public, or the report
is aggregate-only (counts and metric deltas; no path, qname or query appears in it). Below the
minimum the verdict is "insufficient data", not FAIL, and logging continues.

The report: one table per repo plus pooled (ranker, n, recall@10, MRR, diff vs CAL, CI), the
U vs U~ line, the excluded-session counts, and one verdict line `PASS` / `FAIL` /
`insufficient data`, committed by repo-graph before glia reads it. The scored rows can later be
read by FE.3's `--compare` like any arm.

## 4. What glia would build if it passes (a later leap; nothing in 0.5.4)

- A `use` snapshot step beside `history sync` (`glia use sync <repo> <log>`), never inside a
  build, writing a redacted per-file count table under `.glia/`; a build never reads it.
- One fusion candidate (`U+CAL`'s form) in the calibration bank, selected per repo by the 0.5.3
  replay like every other candidate, and judged by the rank gate (EG.3,
  `bash scripts/rank-gate.sh check`). It ranks; it never promotes, hides or creates a row, and
  never seeds blast radius.

## 5. What it costs, and what it does not do

Costs repo-graph: one logging hook in its MCP server (tool calls and the read / edit actions it
already sees), the opt-in flag, the join, and the scoring script that calls FE.3's bootstrap.
Costs glia: nothing until a PASS report exists.

It does not: send telemetry off the machine; log source, query or commit text; change any
ranking, answer or default before the test passes; touch a glia format or snapshot.

## 6. Status

Proposal; owner repo-graph. Registered when this file's dated commit is acknowledged by
repo-graph's session. Sending it is James's call (four repos, four sessions; glia acts on glia
only).
