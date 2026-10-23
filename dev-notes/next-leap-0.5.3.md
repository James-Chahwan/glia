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

All six are produced by `engine/examples/rank_eval.rs`. Ground truth is held-out git commits: seed one changed file and
score the commit's other files. Private repos are reported as aggregate numbers only.

- `dev-notes/ranking-eval-2026-10-18.md`: 132 rankers on glia, Kina and quokka-stack.
- `dev-notes/ranking-eval-vertical-2026-10-18.md`: the upper layers.
- `dev-notes/keys-eval-glia-2026-10-18.md`: resolved cross-layer keys.
- `dev-notes/ranking-eval-round1-2026-10-18.md`: realistic intent and milestones, on 9 repos, 6 of them untuned.
- `dev-notes/ranking-eval-round2-2026-10-18.md`: typed id routing, resolver variants and selection v2, on 10 repos.
- `dev-notes/ranking-eval-round3-2026-10-19.md`: id history, per-query text gating, history across a monorepo import, BFS in
  selection, and the review / find shapes, on 12 repos (two new Apache Commons repos with ticket ids).
- `dev-notes/ranking-scoreboard.md`: every weak spot across the six reports, with its status per round.

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

**From round 2** (`dev-notes/ranking-eval-round2-2026-10-18.md`): these wins join (James confirmed 2026-10-23, *"yes to all"*).

| # | mechanism | measured | effort | where |
|---|---|---|---|---|
| 6 | IDR, typed id routing: a subject's ids -> the scoped-id resolver -> the defining task / requirement row -> the files it names (pending / done state), plus the code citing the id, read at the parent revision | Kina 0.595 recall@10 vs 0.531 for the optimistic full-message blend (160% of its gain); glia 0.920 | medium | `review` (the PR title), `find` (the query) |
| 7 | typed links first: the lexicographic fusion LX[IDR, ...] | beats min-max blending IDR on glia | small | the fusion in 3 / 6 |
| 8 | item 4's candidate set gains the 4 id-routed fusions (SELt v2) | never below PPR or the optimistic blend on 10 repos; a no-op where subjects carry no id | small | item 4 |
| 9 | bare-id namespacing in the key resolver: a findings series stays local, a restated series merges, label rows don't define, siblings share, the default is global | link precision 0.667 -> 0.963; no ranking effect | small | the scoped-id resolver of 6 |

Measured as losses, so they stay out: mention roles as a hop restriction (roles stay for ownership, where they help); text as
a ranker behind typed links; an extra hop through a defining row's mentions. The instrument-generic law diff is a small
win for the law layer, which predicts nothing, so it goes to 0.5.4 with the goal-layer connector.

**From round 3** (`dev-notes/ranking-eval-round3-2026-10-19.md`). It reproduces round 2 exactly (0 differing values on 10
repos). Items 6-9 hold in every shape tested and need no change. Scope notes for the agreed items:
- **Item 1:** PPR d0.85 is not a win on every new repo (commons-lang -0.035 recall@10), so it reaches the answers through
  item 4's selection, not as a fixed default alone.
- **Item 2:** co-change first stays out of `find`; seeded from the text's top files it is below PPR on 6 of 12 repos.
- **Item 3:** the intent BM25 keeps digit tokens. On ticket-id repos the number is what routes an id to its history (+0.143
  on commons-lang's id queries).
- **Items 4 and 8** hold in the review shape (2, 3 or half of the changed files as seeds): CI above 0 on 9 / 6 / 6 of 12
  repos, below on none.

Two more small items join (confirmed 2026-10-23):

| # | mechanism | measured | effort | where |
|---|---|---|---|---|
| 10 | structure-only selection for the answers with no text: item 4's online pick over {PPR, PPR d0.85 + sum, BFS}, calibrated on the answer's own shape | single seed: never below PPR, CI above 0 on 7 of 12; beats tuned PPR on the 4 repos where BFS wins (shell +0.071, commons-lang +0.092); in the review shape mostly n.s., one loss (glia, 2 seeds, -0.019) | small | `blast_radius`, `impact`, `pack` |
| 11 | find's selection set: find's fusions plus the text ranking itself, typed links first | above the text ranking alone on 6 of 12 repos, below on none; the work-neighbourhood blend is its most frequent pick (7 of 12) | small | `find` |

Measured and kept out: id history, an id -> earlier commits naming it. It is real on ticket-id repos (+0.688 over its
control), but the subject's BM25 already matches the ticket number, and it dilutes IDR on glia (-0.107 where both fire).
Also kept out:
- per-query text gating: no confidence feature beats a random gate at the same rate;
- quokka-stack's pre-merge history: the co-change is real, but it outranks the graph (H10 -0.071);
- BFS candidates in the text answers' selection: within ±0.004 on all 12 repos.

**Small hygiene (confirmed 2026-10-23).**
- **Remove engram-export's DE.5 side file** (`--ppr-candidates`, `.ppr.jsonl`, `ppr_side.rs`, exit 8; 0.5.4 seed §F item
  7). Engram asked for it gone "before 0.5.3 ships".
- **Fix the wheels workflow:** bump the Node 20 actions and pin the runner image (0.5.4 seed §E). Needed before 0.5.3's
  wheels build. Rehearsed 2026-10-23 on draft PR #1 (`ci/wheels-node24` at 6b03413d): run 39946342215 green, 6 builds
  plus sdist, publish skipped, no Node 20 deprecation in the logs, runner images ubuntu-24.04 / macos-14-arm64 as pinned.

## Consumers

- **repo-graph** (just on 0.5.2): rankings change order, not shape. A 0.5.2 -> 0.5.3 handoff.
- **Engram:** reads no rankings; the DE.5 removal if it lands here.
- **neuropil:** skipped 0.5.2 (James 2026-10-18); 0.5.3's release docs write it a 0.5.1 -> 0.5.3 handoff carrying
  `dev-notes/neuropil-handoff-0.5.2.md` forward.

## Order

1. Rounds 2 and 3 finished; items 6-11 and the hygiene confirmed (James 2026-10-23). Also confirmed: the public harness
   and reports drop the Kina domain specifics; they move to `~/Code/kina-extras/` and the harness reads them from there.
2. The spec run (2026-10-23, four groups, spec + adversarial verify each, one integrator): 43 packets, ~13,640 LOC,
   16 waves (W0-W15) in `dev-notes/leap-053-packets.json` (its `rulings`: James's 23 rulings), corrections
   `dev-notes/leap-053-corrections.json`, schedule `dev-notes/wave-runner/schedule_053.py` (runner `--release 053`, ids E*).
   Larger than the seed's estimate: exact harness parity per port, the id-routing machinery, and gate coverage of every item.
3. Waves with close-outs, `rank_eval` as a gate.
4. The release docs and handoffs.

Pace: the 7-day usage window was 81% on 2026-10-18 and resets Friday 2026-10-23 00:00, so the waves run after the reset.
