# Edge-precision audit

Derived by `scripts/edge-audit.sh report` from the committed samples and verdicts; never edit by hand (`report --check` fails on any difference).

## Method

Strata are (edge category x evidence tier), the tier `glia why` prints. Each stratum's sample is the edges with the smallest xxh64 keys (seed `glia-edge-audit-0.5.4`), split across repos in proportion to their stratum sizes. p is correct / (correct + wrong) with a Wilson 95% interval; `unsure` verdicts are excluded and counted. Per category and per tier the estimate is stratified, weights N_h / N over the judged strata, with the finite population correction (a census stratum adds no variance); its 95% CI (`ci_method=stratified_wilson`) is the Wilson score over the effective sample size p (1 - p) / var, or, when var is 0 (every judged stratum at p 0 or 1), the strata's Wilson bounds weighted N_h / N (a census stratum's exact p_h). A stratum is under floor when p is below its tier's floor; insufficient when fewer than 10 rows are judged (a census excepted); noisy when unsure is over 10% of its verdicts. Floors: derived 0.9, fact 0.95, heuristic 0.7.

Panel `all` (HP.5): the fit and the held-out samples, drawn at different n, are post-stratified: a pooled stratum's p is the stratified estimate over its judged panel parts (weights N_hg over their N) with that estimate's interval, and a category's or tier's estimate is stratified over every judged (stratum, panel) part. A stratum held by one panel reads as that panel's.

## Agreement (blind re-judge)

A second judge re-judged each family's every id its first judge called `wrong` and as many of its `correct` ids, smallest id first (topped up to at least 20, `correct` first), without reading the first verdicts; kappa is over that verdict-stratified subset, not the family's verdict mix. Cohen's kappa is over the three verdicts, `unsure` its own category; it is undefined when both judges used one verdict only. A family below 0.6 is LOW AGREEMENT and its strata's precision is provisional. A disagreement is re-read at source and its adjudicated verdict replaces the first one; an unadjudicated one counts as `unsure`.

kappa: overall 1.000 (raw agreement 1.000, 103 ids); heldout 1.000, mechanism 1.000, structure undefined

| family | rule set | re-judged | missing | extra | agree | raw agreement | expected agreement | kappa | status |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| heldout | 56 | 56 | 0 | 0 | 56 | 1.000 | 0.500 | 1.000 | ok |
| mechanism | 20 | 20 | 0 | 0 | 20 | 1.000 | 0.625 | 1.000 | ok |
| structure | 20 | 27 | 0 | 7 | 27 | 1.000 | 1.000 | undefined | undefined |
| overall | 96 | 103 | 0 | 7 | 103 | 1.000 | 0.565 | 1.000 | ok |

disagreements 0, adjudicated 0, unadjudicated 0

Independence (`RULE-0.5.7.md`): of 103 re-judge pairs, 59 are self pairs (a re-judge name less one trailing `r` is the first judge's) and 44 independent; the independent pairs agree on 44 (expected agreement 0.799), kappa 1.000.

| re-judge | pairs | self pairs |
|---|---:|---:|
| FD.3c | 28 | 0 |
| GP.14r | 3 | 3 |
| HC.3r | 56 | 56 |
| HP.5x | 16 | 0 |

## Strata (pooled)

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 3575 | 15 | 15 | 12 | 3 | 0 | 0.700 | [0.398, 0.892] | 0.950 | under_floor |
| CALLS | derived | 6 | 6 | 6 | 6 | 0 | 0 | 1.000 | [0.610, 1.000] | 0.900 | ok |
| CALLS | fact | 196085 | 41 | 41 | 41 | 0 | 0 | 1.000 | [0.817, 1.000] | 0.950 | ok |
| CONTAINS | fact | 3891 | 23 | 23 | 22 | 1 | 0 | 0.932 | [0.700, 0.988] | 0.950 | under_floor |
| DEFINES | fact | 145911 | 21 | 21 | 21 | 0 | 0 | 1.000 | [0.729, 1.000] | 0.950 | ok |
| DEFINES_CONFIG | fact | 884 | 41 | 41 | 41 | 0 | 0 | 1.000 | [0.768, 1.000] | 0.950 | ok |
| DEPENDS_ON | fact | 3802 | 40 | 40 | 40 | 0 | 0 | 1.000 | [0.775, 1.000] | 0.950 | ok |
| DOCUMENTS | derived | 1173 | 40 | 40 | 35 | 5 | 0 | 0.942 | [0.880, 0.973] | 0.900 | ok |
| EVENT_FLOWS | derived | 14 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.900 | ok |
| EVENT_FLOWS | fact | 288 | 10 | 8 | 2 | 6 | 2 | 0.250 | [0.071, 0.591] | 0.950 | insufficient (under_floor, noisy) |
| EVENT_FLOWS | heuristic | 3 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.700 | ok |
| GRPC_CALLS | derived | 18 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.900 | ok |
| HANDLED_BY | derived | 63 | 40 | 40 | 40 | 0 | 0 | 1.000 | [0.855, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 4553 | 42 | 42 | 39 | 3 | 0 | 0.753 | [0.474, 0.912] | 0.950 | under_floor |
| HANDLED_BY | heuristic | 155 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.700 | ok |
| HAS_ATTRIBUTE | fact | 6428 | 23 | 23 | 22 | 1 | 0 | 0.947 | [0.759, 0.990] | 0.950 | under_floor |
| HTTP_CALLS | derived | 942 | 18 | 18 | 17 | 1 | 0 | 0.910 | [0.627, 0.984] | 0.900 | ok |
| IMPLEMENTS | derived | 584 | 40 | 40 | 40 | 0 | 0 | 1.000 | [0.813, 1.000] | 0.900 | ok |
| IMPLEMENTS | fact | 3706 | 41 | 41 | 41 | 0 | 0 | 1.000 | [0.763, 1.000] | 0.950 | ok |
| IMPLEMENTS | heuristic | 844 | 21 | 21 | 21 | 0 | 0 | 1.000 | [0.726, 1.000] | 0.700 | ok |
| IMPORTS | fact | 23450 | 41 | 41 | 41 | 0 | 0 | 1.000 | [0.771, 1.000] | 0.950 | ok |
| IMPORTS | heuristic | 14769 | 40 | 40 | 40 | 0 | 0 | 1.000 | [0.724, 1.000] | 0.700 | ok |
| INHERITS_FROM | fact | 2202 | 41 | 41 | 40 | 1 | 0 | 0.918 | [0.653, 0.985] | 0.950 | under_floor |
| INHERITS_FROM | heuristic | 1786 | 18 | 18 | 17 | 1 | 0 | 0.900 | [0.598, 0.982] | 0.700 | ok |
| INJECTS | fact | 3018 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.950 | ok |
| INJECTS | heuristic | 590 | 12 | 12 | 12 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.700 | ok |
| NAVIGATES_TO | derived | 40 | 10 | 10 | 2 | 8 | 0 | 0.200 | [0.057, 0.510] | 0.900 | under_floor |
| NAVIGATES_TO | fact | 24 | 10 | 10 | 9 | 1 | 0 | 0.900 | [0.596, 0.982] | 0.950 | under_floor |
| READS_CONFIG | fact | 777 | 40 | 40 | 40 | 0 | 0 | 1.000 | [0.753, 1.000] | 0.950 | ok |
| RETURNS_TYPE | fact | 458 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.950 | ok |
| RPC_CALLS | derived | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.900 | ok |
| RPC_CALLS | heuristic | 32 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.700 | ok |
| SAME_AS | derived | 240 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.900 | ok |
| SCHEDULES | fact | 24 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.734, 1.000] | 0.950 | ok |
| TESTS | derived | 5267 | 42 | 42 | 40 | 2 | 0 | 0.840 | [0.567, 0.955] | 0.900 | under_floor |
| TESTS | fact | 2328 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.950 | ok |
| USES | derived | 53 | 31 | 31 | 31 | 0 | 0 | 1.000 | [0.889, 1.000] | 0.900 | ok |
| USES | fact | 27281 | 41 | 41 | 41 | 0 | 0 | 1.000 | [0.793, 1.000] | 0.950 | ok |
| WS_CONNECTS | derived | 4 | 4 | 4 | 4 | 0 | 0 | 1.000 | [1.000, 1.000] | 0.900 | ok |

## Strata by panel

| category | tier | panel | N | judged | correct | p |
|---|---|---|---:|---:|---:|---:|
| ACCESSES_DATA | fact | fit | 5 | 5 | 5 | 1.000 |
| ACCESSES_DATA | fact | heldout | 3570 | 10 | 7 | 0.700 |
| CALLS | derived | heldout | 6 | 6 | 6 | 1.000 |
| CALLS | fact | fit | 110765 | 31 | 31 | 1.000 |
| CALLS | fact | heldout | 85320 | 10 | 10 | 1.000 |
| CONTAINS | fact | fit | 469 | 10 | 10 | 1.000 |
| CONTAINS | fact | heldout | 3422 | 13 | 12 | 0.923 |
| DEFINES | fact | fit | 52489 | 11 | 11 | 1.000 |
| DEFINES | fact | heldout | 93422 | 10 | 10 | 1.000 |
| DEFINES_CONFIG | fact | fit | 166 | 30 | 30 | 1.000 |
| DEFINES_CONFIG | fact | heldout | 718 | 11 | 11 | 1.000 |
| DEPENDS_ON | fact | fit | 1228 | 30 | 30 | 1.000 |
| DEPENDS_ON | fact | heldout | 2574 | 10 | 10 | 1.000 |
| DOCUMENTS | derived | fit | 409 | 30 | 25 | 0.833 |
| DOCUMENTS | derived | heldout | 764 | 10 | 10 | 1.000 |
| EVENT_FLOWS | derived | heldout | 14 | 10 | 10 | 1.000 |
| EVENT_FLOWS | fact | heldout | 288 | 8 | 2 | 0.250 |
| EVENT_FLOWS | heuristic | heldout | 3 | 3 | 3 | 1.000 |
| GRPC_CALLS | derived | fit | 18 | 18 | 18 | 1.000 |
| HANDLED_BY | derived | fit | 51 | 30 | 30 | 1.000 |
| HANDLED_BY | derived | heldout | 12 | 10 | 10 | 1.000 |
| HANDLED_BY | fact | fit | 59 | 30 | 30 | 1.000 |
| HANDLED_BY | fact | heldout | 4494 | 12 | 9 | 0.750 |
| HANDLED_BY | heuristic | heldout | 155 | 10 | 10 | 1.000 |
| HAS_ATTRIBUTE | fact | fit | 2374 | 11 | 11 | 1.000 |
| HAS_ATTRIBUTE | fact | heldout | 4054 | 12 | 11 | 0.917 |
| HTTP_CALLS | derived | fit | 7 | 7 | 7 | 1.000 |
| HTTP_CALLS | derived | heldout | 935 | 11 | 10 | 0.909 |
| IMPLEMENTS | derived | fit | 321 | 30 | 30 | 1.000 |
| IMPLEMENTS | derived | heldout | 263 | 10 | 10 | 1.000 |
| IMPLEMENTS | fact | fit | 561 | 30 | 30 | 1.000 |
| IMPLEMENTS | fact | heldout | 3145 | 11 | 11 | 1.000 |
| IMPLEMENTS | heuristic | fit | 11 | 11 | 11 | 1.000 |
| IMPLEMENTS | heuristic | heldout | 833 | 10 | 10 | 1.000 |
| IMPORTS | fact | fit | 6866 | 31 | 31 | 1.000 |
| IMPORTS | fact | heldout | 16584 | 10 | 10 | 1.000 |
| IMPORTS | heuristic | fit | 151 | 30 | 30 | 1.000 |
| IMPORTS | heuristic | heldout | 14618 | 10 | 10 | 1.000 |
| INHERITS_FROM | fact | fit | 219 | 30 | 30 | 1.000 |
| INHERITS_FROM | fact | heldout | 1983 | 11 | 10 | 0.909 |
| INHERITS_FROM | heuristic | fit | 8 | 8 | 8 | 1.000 |
| INHERITS_FROM | heuristic | heldout | 1778 | 10 | 9 | 0.900 |
| INJECTS | fact | heldout | 3018 | 10 | 10 | 1.000 |
| INJECTS | heuristic | fit | 2 | 2 | 2 | 1.000 |
| INJECTS | heuristic | heldout | 588 | 10 | 10 | 1.000 |
| NAVIGATES_TO | derived | heldout | 40 | 10 | 2 | 0.200 |
| NAVIGATES_TO | fact | heldout | 24 | 10 | 9 | 0.900 |
| READS_CONFIG | fact | fit | 144 | 30 | 30 | 1.000 |
| READS_CONFIG | fact | heldout | 633 | 10 | 10 | 1.000 |
| RETURNS_TYPE | fact | fit | 1 | 1 | 1 | 1.000 |
| RETURNS_TYPE | fact | heldout | 457 | 10 | 10 | 1.000 |
| RPC_CALLS | derived | heldout | 8 | 8 | 8 | 1.000 |
| RPC_CALLS | heuristic | heldout | 32 | 10 | 10 | 1.000 |
| SAME_AS | derived | heldout | 240 | 10 | 10 | 1.000 |
| SCHEDULES | fact | fit | 1 | 1 | 1 | 1.000 |
| SCHEDULES | fact | heldout | 23 | 10 | 10 | 1.000 |
| TESTS | derived | fit | 214 | 30 | 30 | 1.000 |
| TESTS | derived | heldout | 5053 | 12 | 10 | 0.833 |
| TESTS | fact | fit | 1 | 1 | 1 | 1.000 |
| TESTS | fact | heldout | 2327 | 10 | 10 | 1.000 |
| USES | derived | fit | 52 | 30 | 30 | 1.000 |
| USES | derived | heldout | 1 | 1 | 1 | 1.000 |
| USES | fact | fit | 11461 | 31 | 31 | 1.000 |
| USES | fact | heldout | 15820 | 10 | 10 | 1.000 |
| WS_CONNECTS | derived | fit | 2 | 2 | 2 | 1.000 |
| WS_CONNECTS | derived | heldout | 2 | 2 | 2 | 1.000 |

## Categories

| category | N | N judged strata | judged | p (stratified) | 95% CI |
|---|---:|---:|---:|---:|---|
| ACCESSES_DATA | 3575 | 3575 | 15 | 0.700 | [0.398, 0.892] |
| CALLS | 196091 | 196091 | 47 | 1.000 | [0.817, 1.000] |
| CONTAINS | 3891 | 3891 | 23 | 0.932 | [0.700, 0.988] |
| DEFINES | 145911 | 145911 | 21 | 1.000 | [0.729, 1.000] |
| DEFINES_CONFIG | 884 | 884 | 41 | 1.000 | [0.768, 1.000] |
| DEPENDS_ON | 3802 | 3802 | 40 | 1.000 | [0.775, 1.000] |
| DOCUMENTS | 1173 | 1173 | 40 | 0.942 | [0.880, 0.973] |
| EVENT_FLOWS | 305 | 305 | 21 | 0.292 | [0.104, 0.594] |
| GRPC_CALLS | 18 | 18 | 18 | 1.000 | [1.000, 1.000] |
| HANDLED_BY | 4771 | 4771 | 92 | 0.764 | [0.493, 0.915] |
| HAS_ATTRIBUTE | 6428 | 6428 | 23 | 0.947 | [0.759, 0.990] |
| HTTP_CALLS | 942 | 942 | 18 | 0.910 | [0.627, 0.984] |
| IMPLEMENTS | 5134 | 5134 | 102 | 1.000 | [0.763, 1.000] |
| IMPORTS | 38219 | 38219 | 81 | 1.000 | [0.753, 1.000] |
| INHERITS_FROM | 3988 | 3988 | 59 | 0.910 | [0.726, 0.975] |
| INJECTS | 3608 | 3608 | 22 | 1.000 | [0.723, 1.000] |
| NAVIGATES_TO | 64 | 64 | 20 | 0.463 | [0.327, 0.604] |
| READS_CONFIG | 777 | 777 | 40 | 1.000 | [0.753, 1.000] |
| RETURNS_TYPE | 458 | 458 | 11 | 1.000 | [0.723, 1.000] |
| RPC_CALLS | 40 | 40 | 18 | 1.000 | [0.778, 1.000] |
| SAME_AS | 240 | 240 | 10 | 1.000 | [0.723, 1.000] |
| SCHEDULES | 24 | 24 | 11 | 1.000 | [0.734, 1.000] |
| TESTS | 7595 | 7595 | 53 | 0.889 | [0.681, 0.968] |
| USES | 27334 | 27334 | 72 | 1.000 | [0.793, 1.000] |
| WS_CONNECTS | 4 | 4 | 4 | 1.000 | [1.000, 1.000] |

## Tiers

| tier | N | N judged strata | judged | p (stratified) | 95% CI |
|---|---:|---:|---:|---:|---|
| derived | 8412 | 8412 | 277 | 0.878 | [0.698, 0.957] |
| fact | 428685 | 428685 | 511 | 0.993 | [0.987, 0.996] |
| heuristic | 18179 | 18179 | 114 | 0.990 | [0.950, 0.998] |

## Under floor

- `ACCESSES_DATA|fact`
- `CONTAINS|fact`
- `EVENT_FLOWS|fact`
- `HANDLED_BY|fact`
- `HAS_ATTRIBUTE|fact`
- `INHERITS_FROM|fact`
- `NAVIGATES_TO|derived`
- `NAVIGATES_TO|fact`
- `TESTS|derived`

## Per repo (reported, not gated)

### chatwoot

edges 34628, unjudgeable 38, excluded 0; stratified p 0.971 [0.914, 0.991]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 1283 | 3 | 3 | 2 | 1 | 0 | 0.667 | [0.208, 0.939] | 0.950 | insufficient (under_floor) |
| CALLS | fact | 8159 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 412 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 108 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 17683 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 280 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 170 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | insufficient |
| EVENT_FLOWS | derived | 10 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | insufficient |
| HANDLED_BY | fact | 2 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HANDLED_BY | heuristic | 72 | 4 | 4 | 4 | 0 | 0 | 1.000 | [0.510, 1.000] | 0.700 | insufficient |
| HTTP_CALLS | derived | 291 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.900 | insufficient |
| IMPLEMENTS | fact | 82 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | fact | 1517 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 91 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | heuristic | 390 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.700 | insufficient |
| NAVIGATES_TO | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |
| NAVIGATES_TO | fact | 1 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.950 | under_floor |
| READS_CONFIG | fact | 285 | 4 | 4 | 4 | 0 | 0 | 1.000 | [0.510, 1.000] | 0.950 | insufficient |
| SAME_AS | derived | 2 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| SCHEDULES | fact | 16 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.950 | insufficient |
| TESTS | derived | 2207 | 4 | 4 | 3 | 1 | 0 | 0.750 | [0.301, 0.954] | 0.900 | insufficient (under_floor) |
| USES | fact | 802 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| WS_CONNECTS | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |

### gin-vue-admin

edges 9123, unjudgeable 11, excluded 0; stratified p 1.000 [0.248, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 221 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CALLS | fact | 2805 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 6 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 19 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 3020 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 263 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 269 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.900 | insufficient |
| EVENT_FLOWS | derived | 4 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.900 | insufficient |
| HANDLED_BY | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| HANDLED_BY | heuristic | 75 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.700 | insufficient |
| HTTP_CALLS | derived | 190 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | insufficient |
| IMPLEMENTS | derived | 263 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.900 | ok |
| IMPORTS | fact | 665 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 152 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| NAVIGATES_TO | fact | 2 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| READS_CONFIG | fact | 18 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| SAME_AS | derived | 238 | 9 | 9 | 9 | 0 | 0 | 1.000 | [0.701, 1.000] | 0.900 | insufficient |
| TESTS | derived | 92 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| USES | fact | 656 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### glia

edges 168997, unjudgeable 164, excluded skip=17674; stratified p 1.000 [0.999, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 95679 | 25 | 25 | 25 | 0 | 0 | 1.000 | [0.867, 1.000] | 0.950 | ok |
| CONTAINS | fact | 454 | 9 | 9 | 9 | 0 | 0 | 1.000 | [0.701, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 33278 | 6 | 6 | 6 | 0 | 0 | 1.000 | [0.610, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 326 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 382 | 28 | 28 | 23 | 5 | 0 | 0.821 | [0.644, 0.921] | 0.900 | under_floor |
| HANDLED_BY | derived | 23 | 14 | 14 | 14 | 0 | 0 | 1.000 | [0.785, 1.000] | 0.900 | ok |
| HAS_ATTRIBUTE | fact | 2113 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 491 | 26 | 26 | 26 | 0 | 0 | 1.000 | [0.871, 1.000] | 0.950 | ok |
| IMPORTS | fact | 5682 | 24 | 24 | 24 | 0 | 0 | 1.000 | [0.862, 1.000] | 0.950 | ok |
| IMPORTS | heuristic | 151 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 66 | 14 | 14 | 14 | 0 | 0 | 1.000 | [0.785, 1.000] | 0.950 | ok |
| RETURNS_TYPE | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 50 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | insufficient |
| TESTS | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| USES | derived | 52 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.900 | ok |
| USES | fact | 10518 | 27 | 27 | 27 | 0 | 0 | 1.000 | [0.875, 1.000] | 0.950 | ok |

### hoppscotch

edges 35428, unjudgeable 25, excluded 0; stratified p 0.996 [0.261, 0.999]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 146 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.950 | insufficient (under_floor) |
| CALLS | fact | 14817 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 91 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 20 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 7525 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 722 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 15 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| HANDLED_BY | derived | 3 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | insufficient |
| HANDLED_BY | fact | 97 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HANDLED_BY | heuristic | 8 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| HAS_ATTRIBUTE | fact | 330 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HTTP_CALLS | derived | 25 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| IMPLEMENTS | fact | 110 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 3 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| IMPORTS | fact | 4951 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 38 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | heuristic | 11 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| INJECTS | fact | 77 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INJECTS | heuristic | 84 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| NAVIGATES_TO | fact | 7 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| READS_CONFIG | fact | 97 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| SCHEDULES | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 134 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| USES | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |
| USES | fact | 3679 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |

### mealie

edges 32208, unjudgeable 13, excluded 0; stratified p 0.987 [0.933, 0.998]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 478 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CALLS | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |
| CALLS | fact | 8101 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 45 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 6736 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 105 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 32 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| HANDLED_BY | derived | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 289 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HAS_ATTRIBUTE | fact | 3660 | 9 | 9 | 8 | 1 | 0 | 0.889 | [0.565, 0.980] | 0.950 | insufficient (under_floor) |
| HTTP_CALLS | derived | 13 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| IMPLEMENTS | fact | 5 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | fact | 3567 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 230 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| INHERITS_FROM | fact | 665 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| INJECTS | fact | 61 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| READS_CONFIG | fact | 30 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| RETURNS_TYPE | fact | 457 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.950 | ok |
| SCHEDULES | fact | 4 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| TESTS | derived | 188 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| TESTS | fact | 2327 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.950 | ok |
| USES | fact | 3491 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |

### microservices-demo

edges 4883, unjudgeable 97, excluded 0; stratified p 1.000 [0.347, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 3 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | ok |
| CALLS | fact | 658 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 15 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 163 | 29 | 29 | 29 | 0 | 0 | 1.000 | [0.883, 1.000] | 0.950 | ok |
| DEFINES | fact | 2542 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 402 | 9 | 9 | 9 | 0 | 0 | 1.000 | [0.701, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |
| GRPC_CALLS | derived | 18 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.900 | ok |
| HANDLED_BY | derived | 28 | 16 | 16 | 16 | 0 | 0 | 1.000 | [0.806, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 36 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.950 | ok |
| HAS_ATTRIBUTE | fact | 33 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HTTP_CALLS | derived | 7 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | ok |
| IMPLEMENTS | derived | 318 | 29 | 29 | 29 | 0 | 0 | 1.000 | [0.883, 1.000] | 0.900 | ok |
| IMPLEMENTS | fact | 15 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| IMPORTS | fact | 25 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 8 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INJECTS | heuristic | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 73 | 15 | 15 | 15 | 0 | 0 | 1.000 | [0.796, 1.000] | 0.950 | ok |
| TESTS | derived | 39 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.900 | insufficient |
| USES | fact | 214 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### samples

edges 30752, unjudgeable 13, excluded 0; stratified p 1.000 [0.396, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | ok |
| CALLS | fact | 12665 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 3 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 15449 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 499 | 12 | 12 | 12 | 0 | 0 | 1.000 | [0.757, 1.000] | 0.950 | ok |
| DOCUMENTS | derived | 26 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| HANDLED_BY | fact | 2 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HAS_ATTRIBUTE | fact | 171 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 33 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 11 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.700 | ok |
| IMPORTS | fact | 1118 | 4 | 4 | 4 | 0 | 0 | 1.000 | [0.510, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 210 | 28 | 28 | 28 | 0 | 0 | 1.000 | [0.879, 1.000] | 0.950 | ok |
| READS_CONFIG | fact | 5 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| SCHEDULES | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 47 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | insufficient |
| USES | fact | 437 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### shell

edges 2500, unjudgeable 0, excluded 0; stratified p 1.000 [0.209, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 1311 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 846 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HAS_ATTRIBUTE | fact | 57 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 22 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | fact | 32 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| INHERITS_FROM | heuristic | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.700 | ok |
| USES | fact | 211 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### squidex

edges 63218, unjudgeable 33, excluded 0; stratified p 0.971 [0.949, 0.984]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 235 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CALLS | fact | 17742 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 2843 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 430 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 22801 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 465 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 14 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| HANDLED_BY | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 611 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.950 | insufficient (under_floor) |
| HTTP_CALLS | derived | 72 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.900 | insufficient (under_floor) |
| IMPLEMENTS | fact | 1876 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 594 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.700 | insufficient |
| IMPORTS | fact | 1838 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 1458 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| INHERITS_FROM | fact | 421 | 2 | 2 | 1 | 1 | 0 | 0.500 | [0.095, 0.905] | 0.950 | insufficient (under_floor) |
| INHERITS_FROM | heuristic | 875 | 4 | 4 | 3 | 1 | 0 | 0.750 | [0.301, 0.954] | 0.700 | insufficient |
| INJECTS | fact | 573 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INJECTS | heuristic | 228 | 4 | 4 | 4 | 0 | 0 | 1.000 | [0.510, 1.000] | 0.700 | insufficient |
| NAVIGATES_TO | derived | 39 | 9 | 9 | 1 | 8 | 0 | 0.111 | [0.020, 0.435] | 0.900 | insufficient (under_floor) |
| NAVIGATES_TO | fact | 14 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.950 | insufficient |
| READS_CONFIG | fact | 72 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| TESTS | derived | 1623 | 3 | 3 | 2 | 1 | 0 | 0.667 | [0.208, 0.939] | 0.900 | insufficient (under_floor) |
| USES | fact | 3868 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |

### tolgee

edges 81556, unjudgeable 34, excluded 0; stratified p 0.995 [0.979, 0.999]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 1207 | 3 | 3 | 2 | 1 | 0 | 0.667 | [0.208, 0.939] | 0.950 | insufficient (under_floor) |
| CALLS | fact | 27289 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 1 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.950 | under_floor |
| DEFINES_CONFIG | fact | 46 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 27707 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 306 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 135 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| HANDLED_BY | fact | 653 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HAS_ATTRIBUTE | fact | 42 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HTTP_CALLS | derived | 344 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.900 | insufficient |
| IMPLEMENTS | fact | 804 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 134 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| IMPORTS | fact | 2710 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 12930 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.700 | insufficient |
| INHERITS_FROM | fact | 566 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | heuristic | 326 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| INJECTS | fact | 2307 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.950 | insufficient |
| INJECTS | heuristic | 276 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.700 | insufficient |
| READS_CONFIG | fact | 42 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| SCHEDULES | fact | 2 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| TESTS | derived | 700 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| USES | fact | 1967 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| WS_CONNECTS | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |

### websocket

edges 1021, unjudgeable 0, excluded 0; stratified p 1.000 [0.261, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 452 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 374 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| HANDLED_BY | fact | 21 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.950 | ok |
| IMPLEMENTS | derived | 3 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| IMPORTS | fact | 9 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| TESTS | derived | 78 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.900 | ok |
| USES | fact | 81 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| WS_CONNECTS | derived | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | ok |

### zora-protocol

edges 23222, unjudgeable 17, excluded 0; stratified p 0.947 [0.874, 0.978]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | derived | 5 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.900 | ok |
| CALLS | fact | 6407 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 69 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 50 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 7950 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 433 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 129 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| EVENT_FLOWS | fact | 288 | 10 | 8 | 2 | 6 | 2 | 0.250 | [0.071, 0.591] | 0.950 | insufficient (under_floor, noisy) |
| EVENT_FLOWS | heuristic | 3 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.700 | ok |
| HANDLED_BY | derived | 5 | 4 | 4 | 4 | 0 | 0 | 1.000 | [0.510, 1.000] | 0.900 | insufficient |
| HANDLED_BY | fact | 2841 | 6 | 6 | 4 | 2 | 0 | 0.667 | [0.300, 0.903] | 0.950 | insufficient (under_floor) |
| HAS_ATTRIBUTE | fact | 22 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 268 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 102 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.700 | insufficient |
| IMPORTS | fact | 1336 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 50 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | heuristic | 176 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.700 | insufficient |
| READS_CONFIG | fact | 89 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| RPC_CALLS | derived | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.900 | ok |
| RPC_CALLS | heuristic | 32 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.700 | ok |
| TESTS | derived | 109 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| USES | fact | 1357 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
