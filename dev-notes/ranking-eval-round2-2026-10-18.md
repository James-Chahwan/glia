# Ranking experiments, round 2: typed id routing, resolver weak spots, a second law event, quokka_web, selection v2 (2026-10-18)

James, 2026-10-18: *"No keep testing and experimenting to most of low or negatives go away"*; *"the round 2 can finish and see
if theres any smalls or mediums to do in this 0.5.3"*.

Harness: `engine/examples/rank_eval.rs --round2 on` (plus `--law-event tr`; Rust only). Datasets, revisions, queries, metrics,
controls and the paired commit-cluster bootstrap are round 1's. Every round-1 ranker reproduces exactly on all nine round-1
repos (0 rows differ). The keyed run's 429 rankers also reproduce, except the 10 Leiden-pooled ones, which move with the
checkout path as documented. Private repos (Kina, quokka-stack, quokka_web, turps, Engram, slurpngulp): aggregate metrics only.
Kina's per-commit detail and its law-event tables are kept outside this repo.

## Verdicts

- **A. Kina's gap is closed, and reversed, by routing the subject's ids as typed links (IDR).**
  - IDR's path, at the parent revision: subject id -> the scoped-id resolver -> the defining task / requirement row -> the
    files that row names (pending 1.0, done 0.5), plus the code citing the id.
  - An ambiguous id is resolved by a cascade, in order: a spec written next to it, the one scope whose row is still open,
    the scope an earlier commit resolved to, then the newest spec.
  - Kina:
    - LX[IDR,H9s50] ranks typed links first, then the subject's BM25 + PPR. Recall@10 0.595, against 0.531 for the
      optimistic full-message H9a50: +0.064 [+0.032, +0.097]. That is 160% of the optimistic gain; round 1's H9s50 kept 71%.
    - On the 497 queries whose subject names an id: +0.115 [+0.070, +0.159].
  - glia (a subject's packet id -> its `files_touched`, pending = no LANDED wave): IDR alone reaches 0.920 recall@10 (P1
    0.294, H9a50 0.404). This measures the agent-wave process, in which packets list their files.
- **B1. Namespacing reused bare ids fixes precision, not ranking.**
  - Bare ids that more than one series reuses are separated by their defining page and heading:
    - a findings series is local to its spec;
    - a restated series merges into one namespace;
    - a label row is not a definition;
    - ids on the same line share a series;
    - an unqualified mention takes the one global series.
  - Hand audit of 54 links: namespaced 52/54 (0.963), base 36/54 (0.667).
  - Ranking moves by 0.000-0.001.
- **B2. Mention roles help ownership and state, and hurt as a hop restriction.**
  - Role-aware routing beats the no-roles control: Kina -0.035 [-0.053, -0.018] without roles; glia -0.215 [-0.244, -0.189].
  - Implements-only hops in the law chains cut R1m by 0.160 and 0.428. The units citing law mention the ids that reach code;
    they don't implement them.
  - One extra hop through a defining row's mentions costs IDR 0.023 on Kina.
- **B3. Text as a ranker behind typed links is not a lever.** It never adds a candidate:
  - the requirement layer VR1h: -0.031 [-0.063, -0.002];
  - the hybrid HR25: -0.011;
  - over IDR: Kina +0.001, glia -0.013 [-0.023, -0.005].
- **C. A second law event (Kina; one instrument amended, one section changed and one added).**
  - The section diff matches the instrument's own amendment-history endnote.
  - This time the docs cite both changed and unchanged sections: 1 of 9 cited sections changed (11%), against 78% in the
    first event. So changedness can select.
  - The decision rows (R5) separate the two. 8 files are reached only from the changed section (precision 0.875 on a proxy
    truth) and 3 only from the unchanged ones (0.667).
  - The permuted-map share is at its floor (about 1/9) with one changed section, so the event is untestable rather than
    failed. No commit implements this amendment, so the truth is a proxy.
- **D. quokka_web (42 commits, 83 queries).**
  - Text hurts: T1 -0.177 [-0.314, -0.057] against P1.
  - The work neighbourhood is real: W5 beats its random-window control by +0.133 [+0.035, +0.243], and HWa25[W5] beats P1 by
    +0.090 [+0.013, +0.172].
  - quokka-stack's own history is truncated by its monorepo merge; turps (round 1) is quokka's backend history.
- **E. SELt v2: SELt's 10 fixed candidates plus 4 id-routed fusions, chosen online per repo, leak-free.**
  - No repo is below P1 or below the optimistic H9a50.
  - Above P1 on 10 of 10 repos, with the CI above 0 on 8 (not the two quokka repos).
  - At or above H9a50 on 10 of 10, with the CI above 0 on 4.
  - Never below round 1's SELt, and identical where subjects carry no id.

## What remains low or negative

- quokka-stack and quokka_web: positive, not significant (110 and 83 queries).
- flutter/samples: text still hurts (H9s50 - P1 -0.062); only selection covers it.
- The law layer in the ranking replay (VR3h) is still at its permuted control.
- Restricting hops by role: the law R1m chain -0.160 / -0.428; VR3h -0.019.
- PT* below BFS on shell (-0.073) and slurpngulp (-0.034), and W5 at chance on glia. Both are unchanged from round 1.
- IDR coverage on Kina: 121 of 199 test-commit subjects name an id, and 87 of those reach a file. A plan id whose tracker
  row names no files routes nowhere.

## Wins for the 0.5.3 ranking release

| mechanism | measured | effort |
|---|---|---|
| IDR: route a PR / commit subject's typed ids through the goal layer at the parent revision in `review` / `find`. The defining rows' names-file edges carry pending / done state, plus the code citing the id; typed links rank first. | Kina +0.064 over the optimistic blend; glia 0.920 | medium |
| SELt v2's candidate set: SELt plus the 4 id-routed fusions | never below P1 or H9a50 on 10 repos; a no-op where no ids exist | small |
| typed links first: the lexicographic fusion LX[IDR, ...] | beats min-max blending IDR on glia | small |
| bare-id namespacing in the key resolver | precision 0.667 -> 0.963; no ranking effect | small |
| an instrument-generic law diff: page-furniture handling and an amendment-history cross-check | the second event's diff matches its endnote | small |

## Caveats

1. glia's IDR measures the agent-wave process. Kina's measures spec-kit task rows that name files. Neither is a general property
   of repos.
2. The namespace rules were refined while reading this population. The 54-link audit was drawn after the rules were fixed,
   from the same population.
3. The law event is one small event: one amended section, judged against a proxy truth.
4. About 340-350 rankers per repo are compared.
5. Every layer is read at the parent. Kina's 36 wiki-era test commits read wiki revisions no later than their own author
   times (0 of 36 later).

## Tables

### Repos

| repo | candidate files | non-merge commits | test commits | queries | chance r@10 | subjects naming an id (reaching a file) |
|---|---|---|---|---|---|---|
| glia | 2549 | 1176 | 300 | 1253 | 0.004 | 295 (295) |
| Kina (private) | 543 | 291 | 199 | 764 | 0.018 | 121 (87) |
| quokka-stack (private) | 206 | 137 | 40 | 110 | 0.049 | 6 (6) |
| quokka_web (private) | 63 | 42 | 22 | 83 | 0.161 | 0 (0) |
| pop-os/shell | 61 | 759 | 216 | 641 | 0.167 | 2 (0) |
| gorilla/websocket | 35 | 285 | 107 | 278 | 0.294 | 0 (0) |
| flutter/samples | 1314 | 1626 | 298 | 1111 | 0.008 | 0 (0) |
| Engram (private) | 101 | 384 | 95 | 364 | 0.100 | 0 (0) |
| turps (private) | 55 | 232 | 45 | 153 | 0.185 | 0 (0) |
| slurpngulp (private) | 89 | 57 | 50 | 216 | 0.114 | 0 (0) |

### A. Id routing (recall@10 / MRR; share = the share of the optimistic H9a50's gain over P1)

ALL = every query; ID = queries whose subject names an id; REACH = queries whose IDR reaches a file.

| ranker | Kina ALL | share | Kina ID | Kina REACH | glia ALL | share |
|---|---|---|---|---|---|---|
| P1 | 0.426 / 0.476 | 0% | 0.460 / 0.550 | 0.435 / 0.549 | 0.294 / 0.416 | 0% |
| H9a50[P1] | 0.531 / 0.567 | 100% | 0.533 / 0.611 | 0.497 / 0.587 | 0.404 / 0.570 | 100% |
| H9s50 | 0.501 / 0.540 | 71% | 0.504 / 0.594 | 0.469 / 0.562 | 0.397 / 0.567 | 94% |
| H18s[PT*] | 0.514 / 0.576 | 84% | 0.522 / 0.611 | 0.481 / 0.585 | 0.403 / 0.580 | 99% |
| SELt | 0.507 / 0.563 | 77% | 0.512 / 0.594 | 0.470 / 0.570 | 0.406 / 0.525 | 102% |
| IDR | 0.266 / 0.403 | -151% | 0.387 / 0.592 | 0.496 / 0.764 | 0.920 / 0.974 | 567% |
| LX[IDR,P1] | 0.526 / 0.603 | 95% | 0.614 / 0.745 | 0.651 / 0.822 | 0.925 / 0.978 | 572% |
| LX[IDR,H9s50] | 0.595 / 0.655 | 160% | 0.648 / 0.772 | 0.671 / 0.811 | 0.934 / 0.986 | 580% |
| LX[IDR,C0,P1] | 0.570 / 0.669 | 136% | 0.637 / 0.767 | 0.668 / 0.813 | 0.930 / 0.983 | 576% |
| LX[IDR,H18s[PT*]] | 0.605 / 0.681 | 169% | 0.662 / 0.772 | 0.677 / 0.812 | 0.934 / 0.986 | 580% |
| H9si (min-max blend) | 0.592 / 0.637 | 158% | 0.644 / 0.744 | 0.665 / 0.771 | 0.881 / 0.914 | 532% |
| SELt2 | 0.599 / 0.673 | 164% | 0.653 / 0.762 | 0.670 / 0.801 | 0.931 / 0.983 | 577% |

Mechanism contrasts (recall@10 on ID queries, a - b, 95% CI):

| a | b | Kina | glia |
|---|---|---|---|
| IDRn (names-file) | IDRc (citing code only) | +0.244 [+0.189, +0.298] | +0.835 [+0.809, +0.862] |
| LX[IDRh,H9s50] (one extra hop) | LX[IDR,H9s50] | -0.023 [-0.038, -0.010] | -0.003 [-0.007, -0.000] |
| LX[IDR~roles,H9s50] (no roles) | LX[IDR,H9s50] | -0.035 [-0.053, -0.018] | -0.215 [-0.244, -0.189] |
| LX[IDRt,H9s50] (text ranker) | LX[IDR,H9s50] | +0.000 [+0.000, +0.000] | -0.013 [-0.021, -0.005] |

### E. SELt v2 on every repo (recall@10)

| repo | P1 | H9a50 (opt.) | SELt | SELt2 | SELt2 - P1 | SELt2 - H9a50 | SELt2 - SELt | last pick |
|---|---|---|---|---|---|---|---|---|
| glia | 0.294 | 0.404 | 0.406 | 0.931 | +0.637 [+0.608, +0.665] | +0.527 [+0.498, +0.558] | +0.524 [+0.493, +0.555] | LX[IDR,H18s[PT*]] |
| Kina | 0.426 | 0.531 | 0.507 | 0.599 | +0.174 [+0.138, +0.211] | +0.068 [+0.036, +0.101] | +0.092 [+0.065, +0.120] | LX[IDR,H18s[PT*]] |
| quokka-stack | 0.615 | 0.625 | 0.642 | 0.665 | +0.050 [-0.026, +0.125] | +0.040 [-0.032, +0.106] | +0.022 [-0.026, +0.073] | LX[IDR,C0,PT*] |
| quokka_web | 0.399 | 0.404 | 0.458 | 0.458 | +0.060 [-0.025, +0.144] | +0.054 [-0.011, +0.117] | +0.000 | HTsc[W5] |
| pop-os/shell | 0.618 | 0.801 | 0.803 | 0.803 | +0.184 [+0.152, +0.217] | +0.002 [-0.016, +0.020] | +0.000 | ALL5 |
| gorilla/websocket | 0.673 | 0.800 | 0.812 | 0.812 | +0.139 [+0.079, +0.200] | +0.012 [-0.023, +0.047] | +0.000 | H18s[PT*] |
| flutter/samples | 0.447 | 0.372 | 0.514 | 0.514 | +0.068 [+0.046, +0.092] | +0.142 [+0.113, +0.176] | +0.000 | H10[PT*] |
| Engram | 0.646 | 0.754 | 0.770 | 0.770 | +0.125 [+0.095, +0.156] | +0.016 [-0.000, +0.033] | +0.000 | ALL5 |
| turps | 0.485 | 0.578 | 0.664 | 0.664 | +0.179 [+0.100, +0.275] | +0.086 [+0.026, +0.157] | +0.000 | H18s[PT*] |
| slurpngulp | 0.620 | 0.817 | 0.839 | 0.839 | +0.218 [+0.158, +0.275] | +0.022 [-0.001, +0.044] | +0.000 | HTsd[W5] |

### B. Resolver variants in Kina's keyed vertical replay (recall@10, variant - base, 95% CI)

| layer | base | +ns - base | +roles - base | +trank - base | +text (run 4) - base |
|---|---|---|---|---|---|
| VR1h | 0.342 | +0.000 [-0.000, +0.001] | +0.024 [+0.009, +0.040] | -0.031 [-0.063, -0.002] | +0.013 [+0.001, +0.025] |
| HR25[VR1h] | 0.547 | -0.000 [-0.002, +0.001] | -0.006 [-0.013, -0.001] | -0.011 [-0.021, -0.002] | -0.004 [-0.009, +0.001] |
| VR2u | 0.327 | -0.001 [-0.002, +0.000] | +0.000 [-0.006, +0.008] | -0.019 [-0.049, +0.007] | -0.002 [-0.013, +0.010] |
| HR25[VR2u] | 0.541 | -0.000 [-0.001, -0.000] | -0.003 [-0.006, -0.001] | -0.001 [-0.011, +0.009] | -0.008 [-0.015, -0.000] |
| VR3h | 0.136 | +0.000 [-0.002, +0.003] | -0.019 [-0.033, -0.004] | - | +0.002 [-0.005, +0.010] |
| HR25[VR3h] | 0.528 | -0.001 [-0.003, -0.000] | -0.000 [-0.010, +0.010] | - | -0.007 [-0.013, -0.003] |

### D. quokka_web (recall@10 / MRR, 83 queries)

| contrast | recall@10 | MRR |
|---|---|---|
| PT* - P1 | +0.004 [-0.030, +0.025] | +0.069 [-0.005, +0.136] |
| H10 - P1 | +0.010 [-0.049, +0.075] | +0.036 [-0.038, +0.100] |
| T1 - P1 | -0.177 [-0.314, -0.057] | -0.107 [-0.250, +0.041] |
| H18s[PT*] - P1 | +0.023 [-0.044, +0.094] | +0.068 [-0.011, +0.146] |
| W5 - Wr5 | +0.133 [+0.035, +0.243] | +0.165 [-0.041, +0.344] |
| HWa25[W5] - P1 | +0.090 [+0.013, +0.172] | +0.008 [-0.016, +0.032] |
| SELt2 - P1 | +0.060 [-0.025, +0.144] | +0.048 [-0.002, +0.107] |
