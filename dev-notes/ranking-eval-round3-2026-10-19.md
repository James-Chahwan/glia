# Ranking experiments, round 3: id history, text gating, history across a monorepo import, BFS in selection, and the review / find shapes (2026-10-19)

James, 2026-10-19: *"Okay yeah run round 3 to hopefully dust out any final cobwebs"*.

Harness: `engine/examples/rank_eval.rs --round3 on` (with `--round1 on --round2 on`; Rust only; git through `std::process`; no
Cargo edits). Datasets, revisions, queries, metrics, leak rules and the paired commit-cluster bootstrap are round 1's.
Runs: round 2's invocations plus `--round3 on`, on round 2's 10 repos and two new open-source repos with ticket
ids, apache/commons-lang at `cee7cc6b` and apache/commons-io at `11f0abe7` (full-history bare clones, git-archive copies).

Reproduction: on all 10 round-2 repos, every round-2 row of the fresh run equals round 2's `final/*.json` (0 differing of
21,276-30,342 values per repo: top-level metrics, every round-1 subset metric, round-1 contrasts, paired rows, SEL picks).
The copies were restored at round 2's own checkout paths, so the 10 Leiden-pooled rows also reproduce this time. The round-3
bank recomputes 34 earlier rankers per query to check its own code: 0 mismatching queries on every repo. Zero build warnings.

Private repos (Kina, quokka-stack, quokka_web, turps, Engram, slurpngulp) are reported as aggregate metrics only. Kina's
per-commit IDH detail is kept outside this repo.

## Verdicts

- **A. Id history (IDH) is a real signal that adds nothing to the fused rankings.**
  - IDH: the subject's ids -> earlier training commits whose subjects named the same id -> their files (newest weighs 1,
    each older 0.7x, summed). Ids: round 2's scoped keys (Kina), packet ids (glia), ticket ids `[A-Z][A-Z0-9]+-N` and `#N`
    (a `#N` carries only once an earlier subject named it). Control: each key swapped for another key seen by then.
  - On the ticket-id repos it is real: on the queries it reaches, IDH - IDH~ +0.688 [+0.565, +0.807] (commons-lang, 112
    queries) and +0.405 [+0.288, +0.538] (commons-io, 133). IDH's top 10 holds a changed file for 38 of 42 and 34 of 43
    reached commits (controls 7 and 7).
  - It adds nothing over the subject's BM25: LX[IDH,H9s50] - H9s50 +0.000 [+0.000, +0.000] (lang) and -0.002 [-0.005,
    +0.001] (io) on all queries. Attribution: the BM25 keeps the ticket number as a token. Dropping digit tokens costs lang
    -0.143 [-0.252, -0.055] on IDH's queries, and IDH first over the digit-free text recovers exactly that +0.143.
  - Kina: IDH reaches 8 of 121 id subjects, all already reached by IDR; it routes 0 of the 34 that IDR leaves without a
    file (the scoped ids almost never recur in an earlier subject). LX[IDRH,H9s50] - LX[IDR,H9s50] -0.001 [-0.003, +0.000].
  - glia: IDH dilutes IDR where both fire: -0.107 [-0.209, -0.029] on IDH's 69 queries (-0.006 on all).
  - No transfer shape changes this (A rows of the T table: 0 repos above 0 in any shape).
- **B. Per-query text gating carries no information beyond the on-rate.**
  - Four confidence features of the subject's BM25 match (top-score margin, informative matched terms with idf >= 3,
    top-file peakedness, top score) x four text / no-text pairs; the threshold is picked online from earlier test commits.
  - flutter/samples: the gate learns to turn text off (on-rate 0.12-0.18) and lifts the always-on H9s50 by +0.058 [+0.032,
    +0.088]. A random gate at the same rate does as well: +0.008 [-0.005, +0.021]. It is back to P1: -0.003 [-0.015, +0.008].
  - quokka_web: the realistic blend is not hurt (H9s50 - P1 +0.006 [-0.046, +0.066]); round 2's -0.177 is text alone (T1),
    which no answer uses. Gates change nothing there.
  - Repos where text helps: gates cost up to -0.012 (Engram, slurpngulp, Gmg); no gate beats its control on any repo.
  - Bucket selection (confidence tertiles, online) SELb - SELt2 lies between -0.026 and +0.015, CI off 0 only on Engram
    (-0.008 [-0.015, -0.001]) and turps (+0.007 [+0.000, +0.015]).
- **C. quokka-stack's pre-merge history is real signal, but no lift.**
  - Mapping, verified by blob match at the import commits: turps -> its subdirectory, 87 / 87 files (its subtree-split commit is in
    the local turps clone); quokka_web -> its subdirectory, 125 / 134 paths, 88 blobs (the local clone stops two days before
    the split); quokka_app -> its subdirectory, 73 / 114 paths. 313 ancestor commits, 285 of them before quokka-stack's
    first; 84 eligible ancestor commits warm online selection up and are never scored.
  - Co-change gains coverage: C0 +0.096 [+0.015, +0.175], C0r +0.149 [+0.086, +0.221]. The true map beats the shuffled
    map on co-change-first rankers (H10 +0.101 [+0.015, +0.201]; H18 +0.069 [+0.025, +0.132]).
  - But against no ancestry the rankings fall: H10 -0.071 [-0.140, -0.004], H10[PT*] -0.072 [-0.132, -0.018], SELt2
    -0.050 [-0.123, +0.016], SELt -0.095 [-0.168, -0.028]. The best is H18 +0.034 [-0.021, +0.091]. Old co-change
    (turps from 2023) outranks the graph. So it is not a `history sync` option yet.
- **D. SELt3 (SELt2 + B0, H10[B0], H18s[B0]) is a no-op for the text answer; structure-only selection fixes BFS for the
  text-free one.**
  - SELt3 - SELt2 within ±0.004 recall@10 on all 12 repos (MRR +0.030 [+0.003, +0.059] on commons-io); never below P1
    (CI above 0 on 10 of 12; quokka-stack +0.050 and quokka_web +0.060 n.s.), never below SELt2.
  - On shell and slurpngulp the selected text ranker is already far above BFS: SELt3 - B0 +0.124 [+0.093, +0.154] and
    +0.161 [+0.113, +0.209].
  - For the answers without text (impact, blast radius), the online pick of {P1, PT*, B0} (SELs0) is never below P1 in
    the single-seed shape and is above it on 7 of 12. It beats PT* on shell +0.071 [+0.051, +0.092], slurpngulp +0.033
    [+0.006, +0.061], commons-lang +0.092 [+0.064, +0.119] and commons-io +0.037 [+0.012, +0.061]. In the review shapes it
    is mostly n.s., with one loss: glia k2 -0.019 [-0.033, -0.007].
- **T. Transfer: the 0.5.3 mechanisms mostly hold in the review shape; co-change-first and PPR do not belong in find.**
  - Review: seed 2, 3 or half of a commit's changed files and score the rest, on 12 repos. Selection SELt2 - P1 has CI
    above 0 on 9 / 6 / 6 repos (k2 / k3 / half) and below on none. IDR first holds on every id repo in every shape (glia
    +0.528 to +0.569, Kina +0.082 to +0.101). Co-change first (H10 - P1): 5 / 4 / 4 above, 0 / 0 / 1 below. The intent
    blend (H9s50 - P1): 5 / 3 / 4 above, 1 below (flutter/samples, -0.087 to -0.128). PPR d85 + sum pooling: mostly n.s.
  - Find: no seed; the subject's BM25 over history, PPR and co-change seeded from its top 10 files, IDR / IDH, the work
    neighbourhood. IDR first holds (glia +0.603, Kina +0.105). Selection over find's fusions plus the text ranking
    (SELf2t) - T1s: CI above 0 on 6 of 12, below on none.
  - Co-change first in find is below PPR on 6 of 12 (Engram -0.099, Kina -0.063, commons -0.045 / -0.053, glia -0.040,
    shell -0.035). PPR seeded from the text's top files is below the text alone on 4 of 12 and above on 1.
  - The work-neighbourhood blend (ALL5x) is the best fixed find fusion on 7 of 12 (over T1s: quokka_web +0.171, turps
    +0.152, quokka-stack +0.095, commons-lang +0.046).
  - Commons-lang / commons-io, untuned, reproduce the round-1 battery: H10 - P1 +0.175 / +0.107; H18s[PT*] - P1 +0.200 /
    +0.117; SELt2 - P1 +0.197 / +0.114; H9s50 = H9a50 (the subject keeps the whole optimistic gain); W5 - Wr5 +0.197 /
    +0.183; M - M~ +0.107 / +0.098.

## What remains low or negative

- quokka-stack and quokka_web: SELt3 - P1 +0.050 / +0.060, not significant (110 / 83 queries); the pre-merge history did
  not lift quokka-stack (C).
- flutter/samples: text hurts in every seeded shape (single -0.062, review -0.087 to -0.128). Selection covers it (SELt2 +0.068
  over P1); no confidence feature predicts it per query.
- PPR d85 + sum pooling (item 1) on unseen repos: commons-lang -0.035 [-0.052, -0.019], commons-io +0.019 [+0.008,
  +0.031]; BFS beats both on both. Only per-repo selection covers it.
- PT* alone still trails BFS on shell (-0.073), slurpngulp (-0.034), commons-lang (-0.093) and commons-io (-0.039). SELs0
  covers the single-seed answer; in review shapes BFS from several seeds collapses (Kina k3 -0.187).
- Find: co-change first (6 of 12 below), PPR from the text's top files (4 of 12 below), and SELf2 without the text
  ranking (slurpngulp -0.033 [-0.060, -0.009]; with the text ranking added, SELf2t, -0.011 [-0.028, +0.000]).
- IDH added to IDR on glia: -0.107 on its 69 queries.
- Kina: 34 of 121 id subjects still reach no file.
- Not retested: the law layer at its control, W5 at chance on glia, edge weights, cold start, the role-restricted hops.

## Readiness

- **Items 1-5 (agreed): ready, with scope notes.**
  - Co-change first (2) is for impact / review / pack, as scoped; it must not go into `find`.
  - The intent blend (3) holds in review where text helps, and selection must keep the text-free fallback for repos like
    flutter/samples.
  - PPR d85 (1) is not a win on every repo (commons-lang -0.035), so it should reach the answers through selection (4), not
    as a fixed default alone.
- **Items 6-8 (proposed in round 2): ready, unchanged.** IDR first holds in every shape tested (review k2 / k3 / half and
  find) and is a no-op where subjects carry no id. Selection v2 holds in review (none below P1).
- **Item 9:** not retested.
- **IDH, confidence gating, ancestor history, SELt3's BFS candidates: not ready.** Each sits at its control or adds nothing
  over what the agreed items already do.

## Small / medium wins for 0.5.3

- (small) Structure-only selection for the text-free answers (SELs0: item 4's online pick over {P1, PT*, B0} for impact
  / blast radius). Single seed: never below P1, CI above 0 on 7 of 12, and it beats PT* on the four repos where BFS
  wins. Calibrate it on the answer's own shape; in review it is n.s. with one loss (glia k2 -0.019).
- (small) Find's selection set (item 4 for `find`): the find-shape fusions plus the text ranking itself (SELf2t), IDR
  first. CI above 0 over the text alone on 6 of 12, none below; the work-neighbourhood blend ALL5x is its most frequent pick on 7
  of 12 repos. Leave co-change-first and PPR-from-text out of find's fixed default.
- (small, a note on item 3) Keep digit-bearing tokens in the intent BM25: on ticket-id repos the number token is what
  routes an id to its history (+0.143 on commons-lang's id queries).
- Item 6 needs no change from A: IDH adds nothing over IDR + BM25 and dilutes IDR on glia.

## Caveats

1. The two ticket-id repos are one project family (Apache Commons, Java). Their revisions (end-2018 / end-2019) were chosen
   for id density, which was checked before any ranking ran.
2. SELs0, SELh0 and SELf2t were added during round 3: SELs0 / SELh0 after the first batch showed PT* below BFS again,
   SELf2t after SELf2 fell below the text alone on slurpngulp. They are a-priori sets (the shape's own base rankers), but
   their addition was post-hoc. SELt3x (A / B winners) was not run: neither A nor B produced a winner.
3. C: the warm-up commits and the ancestor co-change were not separated; the fixed rankers (no warm-up) already show no
   lift. quokka_web's local history stops two days before its subtree split.
4. B's thresholds are online per repo; the controls match each gate's on-rate over the run, not per commit.
5. Multiple comparisons: 412-424 rankers per repo in the single-seed shape, 99 per review shape, 31 in find.
6. Transfer shapes are small on small repos (quokka-stack k2 13 queries, k3 / half 10).


Tables: `dev-notes/ranking-eval-round3-2026-10-19-tables.md`. Every weak spot across the six reports, with its status per round: `dev-notes/ranking-scoreboard.md`.
