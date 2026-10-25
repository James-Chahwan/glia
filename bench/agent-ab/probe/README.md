# The FE.4 probes (frozen)

These scripts are the probes of the FE.4 follow-up (2026-10-25 / 26) whose numbers are the
record in `dev-notes/agent-ab-probe-0.5.4.md`: the agent A/B triage, the gate that named a
client file on 0 of 13 drift tasks, the names prototype and the boundary health table. They are
kept as they were when those numbers were measured, so the record can be re-run as written; they
are no longer the maintained implementation.

The no-model probes moved into the outlier cohort (`bench/cohort/`, FH.1a / FH.1b), with their
measured definitions verbatim, over the cohort's 21 repos and its mined tasks instead of the
FE.4 eleven:

| script | what it measured | now |
|---|---|---|
| `mine.py` | the FE.1 v2 task rows (`tasks/<label>.jsonl`) | `cohort.py build-tasks` (ripple / symptom mining, FH.1a) |
| `drift.py` | the 13 drift tasks: a real cross-stack commit split at a synthetic start S (`drift-tasks/`, refs `refs/agent-ab/drift/<id>`) | `cohort.py build-tasks` / `starts` (the same 13 as `picks`, refs `refs/cohort/drift/<id>`, same trees: `starts --compare-agent-ab`) |
| `oracle.py` | locate (L) and rest (R) recall@10, glia against grep | `cohort.py run`: the `locate` and `rest` checks |
| `gate.py` | `contract-breaks` / `review --base HEAD~1` at S and C | `cohort.py run`: the `gate` and `clear` checks |
| `names.py` | removed names found across the boundary `diff-impact` reaches | `cohort.py run`: the `names` check |
| `health.py` | boundary health per repo at its head | `cohort.py run`: `health.jsonl` (plus the boundary node and pair counts, and the role rule: a library is never flagged on client reach) |
| `diag.py` | why a glia-arm agent session did or did not use glia (reads model transcripts) | not folded: it reads agent sessions, which the cohort runner never starts; the agent probes are FH.13's |

`cohort.py run` adds what the scripts left to a reader: every failed check is classified
(`glia_blind`, `glia_wrong`, `noisy`) into `bench/cohort/results/<release>/misses.jsonl`, and
`report` renders the per-repo and per-class tables. Run the cohort, not these scripts, to judge a
release (`bench/cohort/README.md`).

`drift-tasks/` stays: `bench/cohort/test_cohort.py` checks that the cohort's drift picks are its
13 commits. `config.json`, `tasks/` and `drift-repos.json` are the FE.4 agent runs' inputs
(`bench/agent-ab/run.py --config` / `--tasks-dir` / `--repos`).
