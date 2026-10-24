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

Each glia arm's snapshot steps (history sync, calibrate) run before the agent's clock starts.

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
3. `git diff -M P C` changes 1..=6 non-test code files (extension in `CODE_EXTS`), at least one
   of them modified or renamed (not only added / deleted); the added + deleted lines over all
   code files (test code included) are 5..=300; renames are at most half of the changed files;
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
`[agent-ab] tasks total=<n> repos=<r>`. The session driver (`run.py`, FE.2) and the scorer
(`score.py`, FE.3) document their own commands here when they land.

## Cost

The sessions run headless (`claude -p`) under a subscription, so no metered dollars, but each
one spends rate-limit budget. FE.2 records each session's cost from Claude Code's result JSON;
FE.4's pilot (18 sessions) extrapolates the full run's cost and hours and stops for James's
approval before the full run starts.

## Reproducibility

Pinned: the repos (`end_rev`), the selection rules (`tasks.py`), the issue text (committed, never
refetched), the ground truth (re-derived byte for byte by `check`), each arm's glia binary and
skill (FE.2). Not pinnable: the model's sampling, so FE.2 runs every (task, arm) several times
and FE.3 reports task-cluster bootstrap intervals rather than single numbers. Upstream history
can be rewritten: a bare clone keeps `end_rev` once fetched, and `check` fails loudly when it
cannot resolve it.

## The 0.5.5 baseline

FE.4 commits the scored 0.5.4 run (metrics only: no transcripts or patches, which quote
third-party source) as `results/0.5.4/baseline.json`; 0.5.5's proof runs the same tasks and
compares against it with FE.3's `score.py compare`.
