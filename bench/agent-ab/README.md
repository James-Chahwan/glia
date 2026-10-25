# Agent before / after

Every ranking result before 0.5.4 is one proxy: "predict the commit's other files", which
partly favours history signals by construction. This harness measures what glia's answers do
for an agent instead: a Claude Code session is given a held-out commit's task on a public repo,
works in a checkout of the commit's parent, and is scored against what the commit actually
changed. It is measurement tooling (stdlib Python, hermetic `python3 test_*.py` tests, like
`bench/substrate-gap/`), not graph tooling.

The harness lands in four parts: FE.1 the task set (this directory's `repos.json`, `tasks.py`,
`tasks/`), FE.2 the session driver (`run.py`), FE.3 the scorer (`score.py`), FE.4 the measured
run and the committed report.

## What it measures

Per session: whether the agent changed the files the commit changed (file recall, precision,
solved), how close its edits sit to the commit's hunks (changed-line overlap, in parent
coordinates), how many files it opened before the first true one, tokens, cost, turns and wall
time. The ground truth for each task is in its row (`truth`, below); FE.3 defines the metrics.

## Arms

FE.2's driver runs every task under three arms, the same prompt text for each:

- `none`: no glia binary and no glia skill (the control);
- `glia-0.5.2`: the 0.5.2 release binary and its `skills/glia/SKILL.md`;
- `glia-0.5.3`: the 0.5.3 binary (the ranking release) and its skill.

In `config.json` and every output the arms are `none`, `g052` and `g053`. Each glia arm's
snapshot steps (history sync, calibrate) run before the agent's clock starts.

## Tasks

`repos.json` lists six public repos, each pinned by a 40-hex `end_rev` (the upstream default
branch tip when the task set was built, `git ls-remote <url> HEAD`):

| label | lang | tuned | gate_rev |
|---|---|---|---|
| shell (pop-os/shell) | TypeScript | yes | the rank gate's pinned rev |
| websocket (gorilla/websocket) | Go | yes | the rank gate's pinned rev |
| samples (flutter/samples) | Dart | yes | the rank gate's pinned rev |
| commons-text (apache/commons-text) | Java | no | null |
| microservices-demo (GoogleCloudPlatform/microservices-demo) | polyglot | no | null |
| click (pallets/click) | Python | no | null |

`tuned` repos are the rank gate's public OSS snapshots (`bench/rank-gate/snapshots.json`; their
`gate_rev` is that file's `rev`, matched by name); 0.5.3's rankings were tuned and gated on their
history. The three untuned repos were in no ranking round and no gate, so FE.3 reports the
0.5.3-vs-0.5.2 line on them beside the pooled one. apache/commons-io and apache/commons-lang are
not eligible as untuned repos: both were round 3's repos and are rank-gate snapshots. Every row
must be `public: true` with an https url; `tasks.py` refuses anything else. No private repo,
ever: this repo is public.

glia itself is excluded: its post-0.5.2 commit subjects carry packet ids whose plan file in the
tree names every changed file, and its commits are 300-450 LOC packets, too large for one agent
session. glia stays in the rank gate.

Selection, over `git log --first-parent <end_rev>`, newest first; the newest `--n` (10) eligible
commits per repo are taken. A commit C with parent P = C^1 is a task when:

1. no author or committer name / email matches `[bot]`, dependabot, renovate or github-actions;
2. its subject, after stripping one conventional-commit prefix (`^[a-z]+(\([^)]*\))?!?: `), has
   at least 4 words and does not start with bump, release, merge, revert, update / upgrade
   dependenc, format, lint, typo, changelog, doc(s) or a version number (case-insensitive);
3. `git diff -M P C` changes 3..=8 non-test code files (extension in `CODE_EXTS`), at least one
   of them modified or renamed (not only added / deleted); the added + deleted lines over all
   code files (test code included) are 5..=400; renames are at most half of the changed files;
   neither the prompt subject nor a fetched issue's title names a changed code file (its stem,
   4+ characters, as a word, case-insensitive). The task is structure-shaped: the first pilot
   (2026-10-25, then 1..=6 files) drew 36 one-file tasks of 60, each a grep away, and no glia
   arm session called glia once;
4. a test file is one with a `test` / `tests` / `__tests__` / `testdata` directory component or
   a basename matching `TEST_RES` (`_test.go`, `test_*.py`, `*_test.py`, `Test*.java`,
   `*Test.java`, `_test.dart`, `*.spec|test.[jt]sx?`);
5. a first-parent merge whose message is GitHub's `Merge pull request #<n> from <branch>` with a
   non-empty title line is one task over `M^1..M` (the PR's whole change, rules 1-4 applied to
   it; a bot branch name is rule 1): prompt subject = the title line, `pr: <n>` recorded. The PR
   body is never fetched (it states the solution) and the merge's `#<n>` is never an issue.

Issue text is a snapshot step at task-build time, never during a run: when the subject or body
names `#<n>` (a `fixes|closes|resolves #<n>` first, at most three numbers tried) and the repo is
on github.com, `gh api repos/<o>/<r>/issues/<n>` is read and kept only when it is an issue (not
a PR) with a non-empty title; the body is cut at 4,000 characters on a line boundary. No `gh` or
no network keeps `issue: null`, `issue_fetch: "unavailable"`. The commit body is never prompt
text. Every stored subject and issue text passes through `redact()`, a Python mirror of
`code_domain::snapshots::redact_untrusted` (the Rust function is the reference); author
identities are never stored.

Each repo yielded the full 10 rows at the pinned `end_rev`s.

A row (`tasks/<label>.jsonl`, one JSON object per line, keys sorted, newest first):

```
{id: "<label>-<C[:12]>", repo, commit, parent, author_time, subject, prompt_subject,
 pr: <n>|null, issue: {number, title, body}|null, issue_fetch: "ok"|"none"|"unavailable",
 after_gate_rev: bool|null,
 truth: {code: [{path, status (A|M|D|R), old_path|null, hunks: [[start, len], ..]}],
         tests: [paths], other: [paths]},
 stats: {code_files, code_lines, test_files}}
```

`hunks` are `@@ -s,l` pairs of `git diff -U0` in PARENT coordinates (`l = 0` is a pure
insertion after line `s`; an added file is `[[0, 0]]`). `after_gate_rev` is true when C is a
strict descendant of `gate_rev` (unseen by 0.5.3's tuning), null without a gate. `code_lines`
counts the non-test code files only.

## Running

```
cd bench/agent-ab
python3 tasks.py build [--repo <label>] [--n 10] [--pin]   # --pin re-pins every end_rev first
python3 tasks.py check [--repo <label>]                     # re-derive, exit 1 on any drift
python3 test_tasks.py
```

Bare clones live in `${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab}/<label>.git`, fetched only
when `end_rev` / `gate_rev` do not resolve. `GLIA_AGENT_AB_OFFLINE=1` makes no clone, fetch or
`gh` call; `check` never calls `gh` (it carries the committed `issue` / `issue_fetch` over), so
it runs offline once the clones exist. Each command prints, per repo,
`[agent-ab] tasks repo=<label> rows=<n> issues=<k> after_gate=<g> sha256=<12 hex>` and then
`[agent-ab] tasks total=<n> repos=<r>`. The scorer's commands are under Scoring below.

### The session driver (`run.py`, FE.2)

```
python3 run.py binaries                  # each glia arm's release binary + skill, once
python3 run.py isolation                 # one cheap session per arm; must print three `ok`
python3 run.py pilot [--run-id <id>]     # first task of each repo x every arm x 1 rep
python3 run.py run --run-id <id> [--keep] [--max-usd <usd>]   # the matrix; resumes
python3 run.py status [--run-id <id>]
python3 test_run.py                      # hermetic: fake `claude` and `glia` on PATH
```

Pinned in `config.json`: the model, `claude --version` (run.py refuses a mismatch), runs (3),
max turns (40), the per-session timeout (1200 s), the budget (`max_usd`), the denied tools
(Agent, Task, WebFetch, WebSearch, NotebookEdit), `GLIA_THREADS` (4) and each glia arm's `rev`
(`v0.5.2`, `v0.5.3`; a rev `git rev-parse --verify <rev>^{commit}` cannot resolve is refused).
`prompt.txt` is the one prompt template (`{repo}` = the GitHub `owner/name`, `{task}` = the
row's `prompt_subject`, plus `Issue #<n>: <title>` and its body when `issue_fetch` is `ok`),
identical for every arm. A run records the sha256 of the config (all but `max_usd`), the prompt
and every task file, the Claude Code version, the model and each arm's binary in
`runs/<id>/meta.json`, and a resume with any of them changed is refused.

Everything lives under `${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab}` (DIR), never in the glia
tree:

- `binaries` builds `cargo build --release -p glia-cli` in a detached worktree of this repo at
  each arm's rev (`DIR/glia@<sha12>`, `CARGO_TARGET_DIR=DIR/target@<sha12>`), keeps only
  `DIR/bin/<arm>/glia` and `meta.json` {rev, sha, version} (the whole `glia --version` line,
  build stamp included), deletes the target dir and the worktree, and writes the rev's
  `skills/glia/SKILL.md` to `DIR/skills/<arm>/glia/SKILL.md`. A binary whose meta sha matches
  is not rebuilt. Two arms reporting one build stamp are refused.
  `[agent-ab] binary arm=<a> sha=<12> version=<v>`.
- Each session works in its own checkout, `DIR/work/<run>/<task>/<arm>/<rep>/`: `git init`, a
  fetch of the task's PARENT alone from the bare clone (`uploadpack.allowAnySHA1InWant`), `main`
  at it, `.glia/` and `.claude/` in `.git/info/exclude`, no remote. The run aborts if the task
  commit is in the copy. The copy is deleted after the session's row unless `--keep`.
- A glia arm's prep runs before the agent's clock, timed as `prep_ms`: `glia history sync`,
  `glia build`, and for g053 (`"calibrate": true`) `glia history calibrate`, then the arm's
  skill to `<copy>/.claude/skills/glia/SKILL.md`. A failing step is `prep_failed` (no session).
  Sync runs before build so the layout the agent's first answer reads already ingested the
  history snapshot (the spec listed build first).
- The environment is the caller's, identical for every arm but PATH's first entry: a fresh
  `CLAUDE_CONFIG_DIR=DIR/claude-config/<run>` (no user CLAUDE.md, skills, plugins, hooks or
  memory), the caller's `CLAUDECODE` / `CLAUDE_CODE_*` session variables and
  `GLIA_RANK_PRIVATE` removed, `GLIA_THREADS` from the config, and PATH = (`DIR/bin/<arm>` for
  a glia arm) + `DIR/bin/deny` (shims `glia`, `mcp-repo-graph`, `repo-graph` that print
  `<name>: command not found` and exit 127) + the caller's PATH. Auth comes only from
  `CLAUDE_CODE_OAUTH_TOKEN` (`claude setup-token`, subscription; ruling FE-3): run.py refuses to
  start without it, and refuses when `ANTHROPIC_API_KEY` (metered) is set at all; it records
  `auth=oauth_token`, never the value.
- The session: `claude -p <prompt> --model <m> --output-format stream-json --verbose
  --mcp-config empty-mcp.json --strict-mcp-config --setting-sources project --permission-mode
  bypassPermissions --max-turns <n> [--settings {"enabledPlugins": {<each disabled_plugins>: false}}]
  --disallowedTools <each>` (the disabled plugins are Claude Code's own built-ins, which load under
  any config dir and which the isolation check refuses), cwd the copy, the stream in
  `DIR/sessions/<run>/<task>/<arm>/<rep>.jsonl`, killed after the timeout (`timeout`). A result
  whose error reads as a rate limit sleeps until the stream's `resetsAt`, else 900 s (at most
  6 h), the checkout reset, up to 5 attempts; then the run stops, resumable (exit 3).
- Isolation, on every session from its `system`/`init` event: `mcp_servers` empty, no `mcp__`
  or denied tool in `tools`, `plugins` empty, and in `skills` (the field Claude Code 2.1.310
  emits: the user-invocable skill names) `glia` in a glia arm, no `glia*` skill in the control,
  no plugin-namespaced (`a:b`) skill and none of the caller's `~/.claude/skills` names. Then the
  transcript: the control running `glia` / `mcp-repo-graph` / `repo-graph` (bare or by path) or
  installing glia-cli / glia-py / mcp-repo-graph, or a glia arm running another glia binary (by
  path, `uvx`, `cargo run`), is `contaminated`; any arm running `git clone|fetch|pull|ls-remote`,
  `git remote add`, `gh`, `curl`, `wget`, WebFetch or WebSearch is `network`. Both are excluded
  from scoring and reported. `run.py isolation` runs one session per arm (`--max-turns 1`,
  `Reply with the word ok.`) and prints `[agent-ab] isolation arm=<a> mcp=<n> skills=<list>
  ok|FAIL`; `pilot` and `run` refuse to start without a passing isolation for the pinned Claude
  Code version, model and binaries.
- One row per session in `DIR/runs/<run>/sessions.jsonl` (a resume skips keys already there):
  run_id, task_id, repo, arm, rep, model (init event), claude_version, glia_sha, prep_ms,
  terminated (`end|max_turns|timeout|error|prep_failed|contaminated|network`), violations,
  attempts, auth, num_turns, duration_ms, wall_ms, usage (input, cache creation, cache read,
  output tokens), total_cost_usd, tool_calls, glia_calls [{subcommand, flags}], files_opened
  (repo-relative, first-open order: Read paths and `cat|head|tail|less|bat|sed -n` file
  arguments inside the copy, never under `.glia/` / `.claude/`), grep_calls (Grep + Bash
  `rg` / `grep` / `git grep`), patch {path, sha256, bytes} (`git add -A -N . && git diff
  --no-color -U0 HEAD`, in `DIR/patches/<run>/<task>.<arm>.<rep>.diff`).
  `[agent-ab] session task=<id> arm=<a> rep=<r> turns=<t> usd=<c> opened=<n> glia_calls=<g>
  terminated=<why>`.
- Order: rep, then task in file order, then the arms rotated by the task's index (none, g052,
  g053 / g052, g053, none / ..). Budget: before each session, when the run's spent
  `total_cost_usd` plus the median session so far would pass `max_usd`, the run stops
  (`[agent-ab] budget stop spent=<usd>`), resumable with a raised `--max-usd`.
- `pilot` runs the first task of each repo under every arm once, then prints
  `[agent-ab] pilot sessions=<n> usd=<sum> median_usd=<m> est_full_usd=<m x tasks x arms x runs>
  est_hours=<median (wall + prep) x sessions> <usage>` and stops, where `<usage>` is the
  subscription's window share read from `~/.claude/rate-limits.json` (`GLIA_RATE_LIMITS`
  overrides the path) before and after: `five_hour_pct=<a>-><b> seven_day_pct=<a>-><b>
  est_full_seven_day_pct=<7-day delta / sessions x full sessions> est_full_weeks=<that / 100>`.
  The statusline rewrites that file only while an interactive session renders, so keep one open
  during the pilot; a file not refreshed since the pilot began reads `usage=unknown
  reason=rate_limits_not_refreshed` (`reason=no_rate_limits_file` when absent). usd is
  `total_cost_usd`, the list-price equivalent, not a charge; the full run is `run.py run --run-id
  <the pilot's id>` (the pilot's sessions count as rep 1), started only after James approves the
  estimate.

`--permission-mode bypassPermissions` lets the agent run any shell command. The checkout is a
disposable copy with no remote, but start a run only from an account with nothing sensitive in
reach of a shell.

### Scoring (`score.py`, FE.3)

```
python3 score.py score <run_id> [--out-md <f>] [--out-json <f>] [--out-sessions <f>]
python3 score.py score --from-metrics <sessions metrics jsonl> [--out-md <f>] [--out-json <f>]
python3 score.py compare <old baseline.json> <new baseline.json>
python3 test_score.py                    # hermetic: a canned run under a temp DIR
```

`score <run_id>` reads `DIR/runs/<id>/meta.json` and `sessions.jsonl`, each session's patch
under `DIR/patches/<id>/` (its sha256 checked against the row) and the task rows in `tasks/`
(refused when a task file's sha256 is not the run's). It prints
`[agent-ab] score sessions=<n> excluded=<k> tasks=<t>`.

Per session, with T the truth's code paths (a rename's new path, a deletion's old path) and E
the non-test code files the patch touches (`tasks.py`'s `CODE_EXTS` / `TEST_RES`; never under
`.glia/` or `.claude/`; a path is its new name, a deleted file's its old one):

- `file_recall` = |E n T| / |T|; `file_precision` = |E n T| / |E| (0 when E is empty);
- `solved` = T is a subset of E and |E \ T| <= max(2, |T|);
- `line_overlap` = over the truth hunks of M files, the fraction whose parent range
  [s, s + max(l, 1)) meets an agent hunk's parent range (`@@ -s,l`) on that file widened by 3
  lines each side; null when the task has no M file;
- `first_truth_open` = the 1-based index in `files_opened` of the first T path (a rename's old
  name counts too: the agent works at the parent), null when none; `opened` = len(files_opened);
- `tokens_in` = input + cache creation + cache read; `tokens_out`; `usd` = total_cost_usd;
  `turns`; `wall_s`; `glia_calls` (reported, never judged).

Sessions terminated `contaminated`, `network`, `prep_failed` or `error` are excluded and
counted by reason; `timeout` and `max_turns` are scored on the patch the agent left. A task's
metric is the mean over its scored reps (`solved` a rate, nulls skipped); an arm's is the mean
over tasks, each task weight 1. Pairs `g052-none`, `g053-none`, `g053-g052`: the paired per-task
difference of task means, with a task-cluster bootstrap 95% percentile interval (2,000
resamples of tasks with replacement, `random.Random(20261023)`), pooled, tuned / untuned, each
after_gate_rev true / false when both sides hold >= 5 tasks (never on this task set: every
tuned row is after its gate and every untuned row is null, so tuned / untuned carries that
split), and per repo. Verdict: `better` / `worse` when the interval
excludes 0 in the metric's good direction (higher: solved, file_recall, file_precision,
line_overlap; lower: opened, first_truth_open, tokens_in, tokens_out, usd, turns, wall_s), else
`n.s.`; a single shared task gives no interval and is `n.s.`.

The reading rule, fixed before any number: an arm HELPS an agent when, pooled, `solved` or
`file_recall` is `better` and no quality metric (`solved`, `file_recall`, `line_overlap`) is
`worse`, or when every quality metric is `n.s.` and `tokens_in` or `usd` is `better`; it HURTS
when a quality metric is `worse`; otherwise NO MEASURED EFFECT. `g053-g052` (0.5.3's rankings
against 0.5.2's, the line that informs 0.5.4's ranking work) is read pooled and on the untuned
subset; where they disagree the report says the pooled effect rests on the tuned repos.
File-level success against one commit's diff undercounts a valid fix elsewhere; `line_overlap`
and `file_recall` show partial credit. No test is executed.

`--out-md` writes the report (header with the run's fingerprints and exclusions, the rule, the
outcome per pair, an arm x metric table and a pair table per subset, cost, glia usage: the
share of glia-arm sessions with a glia call and the subcommand histogram). `--out-json` writes
`baseline.json`: `{schema: 1, run_id, fingerprints: {model, claude_version, glia: {arm: sha},
tasks: {file: sha256}, prompt_sha256, config_sha256}, per_task: [{task_id, arm, n, metrics}],
pooled: {arm: metrics}, pairs: {pair: {metric: {diff, lo, hi, verdict}}}, pairs_by_subset}`.
`--out-sessions` writes the per-session metric rows: one `kind: meta` line (the fingerprints
and the run's date), then one `kind: session` row per session sorted by (task_id, arm, rep)
with its terminated reason, metrics, glia subcommands and patch sha256: no transcript, patch
text or path outside the task repo. `--from-metrics` scores from that file alone, so a committed
metrics file reproduces the committed baseline byte for byte. Both outputs are deterministic
(rows sorted, floats at 3 decimals in the report and 6 in the JSON; the date is the run
meta's mtime).

`compare old new` refuses (exit 2, naming the field) unless `fingerprints.tasks` and
`fingerprints.prompt_sha256` match, then prints, per arm in both and per judged metric measured
on a shared task, new - old over the shared tasks with the same bootstrap:
`[agent-ab] compare arm=<a> metric=<m> diff=<d> ci=[lo,hi] verdict=<v>`. This is the call
0.5.5's proof makes against `results/0.5.4/baseline.json`.

## Cost

From repo-graph's bench (Sonnet 5 orientation sessions cost $0.09-0.68 metered) scaled to edit
tasks: roughly $0.5-2 per session on Opus 5.5. The full matrix is 60 tasks x 3 arms x 3 runs =
540 sessions, about $300-1,000 metered (Sonnet 5.5 about half); `max_usd` (400) stops it
cleanly, resumable. On a Claude subscription (`apiKeySource: none`, `CLAUDE_CODE_OAUTH_TOKEN`)
there is no metered charge (`total_cost_usd` is still reported and still counts against
`max_usd`), but about 540 x 3-5 min of sessions against the 5-hour and weekly limits: check
`~/.claude/rate-limits.json` and `dev-notes/wave-runner/usage_gate.py` before starting. The
pilot (18 sessions) measures the real figure first.

## Reproducibility

Pinned: the repos (`end_rev`), the selection rules (`tasks.py`), the issue text (committed, never
refetched), the ground truth (re-derived byte for byte by `check`), each arm's glia binary and
skill (FE.2). Not pinnable: the model's sampling, so FE.2 runs every (task, arm) several times
and FE.3 reports task-cluster bootstrap intervals rather than single numbers. Upstream history
can be rewritten: a bare clone keeps `end_rev` once fetched, and `check` fails loudly when it
cannot resolve it.

## The 0.5.5 baseline

FE.4 runs the 0.5.4 matrix (run id `0.5.4`: the pilot's sessions are rep 1, then `run.py run
--run-id 0.5.4`) and commits it scored, metrics only (no transcripts or patches, which quote
third-party source; those stay under DIR):

- `results/0.5.4/sessions.jsonl`: FE.3's `--out-sessions` rows, the input that reproduces the rest;
- `results/0.5.4/baseline.json`: the 0.5.5 contract;
- `dev-notes/agent-ab-0.5.4.md`: the report, with a short Reading section under it.

They are written by

```
python3 score.py score 0.5.4 --out-md ../../dev-notes/agent-ab-0.5.4.md \
    --out-json results/0.5.4/baseline.json --out-sessions results/0.5.4/sessions.jsonl
```

and any machine re-derives the baseline from the committed metrics alone:

```
python3 score.py score --from-metrics results/0.5.4/sessions.jsonl --out-json /tmp/b.json
cmp /tmp/b.json results/0.5.4/baseline.json
```

0.5.5's proof runs the same tasks and prompt (`compare` refuses on a changed task file or
prompt), scores its own run to `results/0.5.5/baseline.json`, and compares:

```
python3 score.py compare results/0.5.4/baseline.json results/0.5.5/baseline.json
```

one `[agent-ab] compare arm=<a> metric=<m> diff=<d> ci=[lo,hi] verdict=<v>` line per arm and
judged metric (new - old over the shared tasks, the task-cluster bootstrap).

The run is subscription-authenticated (`CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`,
ruling FE-3) and usage-gated: check `dev-notes/wave-runner/usage_gate.py` before the pilot and
before each resume of the full run; the full run starts only after James approves the pilot's
estimate.
