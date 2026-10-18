# glia 0.5.3 — ranking (scope seed)

James, 2026-10-18, after the ranking experiments: *"okay better plan 0.5.3 becomes this [the ranking table] because it's a
small lay up step and has the biggest generalised wins for 3 smalls and 2 mediums and the round 2 can finish and see if theres
any smalls or mediums to do in this 0.5.3 and what was the 0.5.3 becomes 0.5.4 just a rename same stuff just moved to next
version"*.

The old 0.5.3 seed (scale, the Engram sidecars, the goal-layer connector) is now `dev-notes/next-leap-0.5.4.md`, unchanged in
scope. This release is small: the ranking mechanisms the 2026-10-18 experiments measured and generalised, each behind a
measured gate. A normal 0.5.2 -> 0.5.3 release with its own handoffs. The 0.5.3 rule still holds (*"For 0.5.3 all follow
must be completed within it"*): follow-up waves until a close-out lists none.

## Evidence

All four are produced by `engine/examples/rank_eval.rs`. Ground truth is held-out git commits: seed one changed file and
score the commit's other files. Private repos are reported as aggregate numbers only.

- `dev-notes/ranking-eval-2026-10-18.md`: 132 rankers on glia, Kina and quokka-stack.
- `dev-notes/ranking-eval-vertical-2026-10-18.md`: the upper layers.
- `dev-notes/keys-eval-glia-2026-10-18.md`: resolved cross-layer keys.
- `dev-notes/ranking-eval-round1-2026-10-18.md`: realistic intent and milestones, on 9 repos, 6 of them untuned.

## Scope

| # | mechanism | measured | effort | where |
|---|---|---|---|---|
| 1 | PPR damping 0.85 + sum pooling of node scores per file | small gains on 6 of 7 unseen repos; sum pooling > max on every repo | small: config | `CODE_TABLES.activation_config` defaults, the file roll-up in blast radius / pack |
| 2 | co-change as the tie-break (H10: co-change, then PPR) in impact / review / pack | about +0.10 MRR on every repo | small: co-change is already stored by `glia history sync` (CO_CHANGES) | `blast_radius`, `diff_impact`, `review`, `pack` |
| 3 | an intent blend for review / find (H9s: minmax(BM25 of the PR title / query over past commit subjects -> the files they touched) blended with PPR) | the subject alone keeps 89-106% of the optimistic gain on 6 repos (Kina 71%) | medium: `history sync` also keeps commit subjects and file lists | `review` (the PR title / commit subject), `find` (the query text) |
| 4 | per-repo calibrated selection: pick the best fixed fusion from the repo's own earlier commits (leak-free, online) | beats PPR on 9 of 9 repos (CI above 0 on 8), and removed the remaining negatives | medium: a replay over the repo's history in the snapshot step, persisted as a small calibration record | a new step beside `history sync`, read by the answers |
| 5 | keep the PPR index between calls | about 3x faster, bit-identical scores | small | the activation plan / adjacency cache |

**Gate.** `rank_eval` runs as a release check on fixed repo snapshots (glia plus the open-source shell / websocket /
samples). A ranking change that lowers recall@10 or MRR beyond its CI fails the close-out.

**From round 2** (pending): any small or medium mechanism it measures as a win joins here. Candidates are:
- Kina's subject task-id routing (id -> task row -> the files it names);
- role-aware mention joining;
- text as a ranker behind typed links.

**Proposed small hygiene, for James to confirm.**
- **Remove engram-export's DE.5 side file** (`--ppr-candidates`, `.ppr.jsonl`, `ppr_side.rs`, exit 8; 0.5.4 seed §F item
  7). Engram asked for it gone "before 0.5.3 ships".
- **Fix the wheels workflow:** bump the Node 20 actions and pin the runner image (0.5.4 seed §E). Needed before 0.5.3's
  wheels build.

## Consumers

- **repo-graph** (just on 0.5.2): rankings change order, not shape. A 0.5.2 -> 0.5.3 handoff.
- **Engram:** reads no rankings; the DE.5 removal if it lands here.
- **neuropil:** skipped 0.5.2 (James 2026-10-18); 0.5.3's release docs write it a 0.5.1 -> 0.5.3 handoff carrying
  `dev-notes/neuropil-handoff-0.5.2.md` forward.

## Order

1. Round 2 finishes; its small / medium wins are added above.
2. A small spec run (spec + adversarial verify per mechanism).
3. Waves with close-outs, `rank_eval` as a gate.
4. The release docs and handoffs.

Pace: the 7-day usage window was 81% on 2026-10-18 and resets Friday 2026-10-23 00:00, so the waves run after the reset.
