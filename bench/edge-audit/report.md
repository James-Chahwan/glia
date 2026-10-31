# Edge-precision audit

Derived by `scripts/edge-audit.sh report` from the committed samples and verdicts; never edit by hand (`report --check` fails on any difference).

## Method

Strata are (edge category x evidence tier), the tier `glia why` prints. Each stratum's sample is the edges with the smallest xxh64 keys (seed `glia-edge-audit-0.5.4`), split across repos in proportion to their stratum sizes. p is correct / (correct + wrong) with a Wilson 95% interval; `unsure` verdicts are excluded and counted. Per category and per tier the estimate is stratified, weights N_h / N over the judged strata, with the finite population correction (a census stratum adds no variance); its 95% CI (`ci_method=stratified_wilson`) is the Wilson score over the effective sample size p (1 - p) / var, or, when var is 0 (every judged stratum at p 0 or 1), the strata's Wilson bounds weighted N_h / N (a census stratum's exact p_h). A stratum is under floor when p is below its tier's floor; insufficient when fewer than 10 rows are judged (a census excepted); noisy when unsure is over 10% of its verdicts. Floors: derived 0.9, fact 0.95, heuristic 0.7.

## Agreement (blind re-judge)

A second judge re-judged each family's every id its first judge called `wrong` and as many of its `correct` ids, smallest id first (topped up to at least 20, `correct` first), without reading the first verdicts; kappa is over that verdict-stratified subset, not the family's verdict mix. Cohen's kappa is over the three verdicts, `unsure` its own category; it is undefined when both judges used one verdict only. A family below 0.6 is LOW AGREEMENT and its strata's precision is provisional. A disagreement is re-read at source and its adjudicated verdict replaces the first one; an unadjudicated one counts as `unsure`.

kappa: overall 1.000 (raw agreement 1.000, 54 ids); mechanism 1.000, structure 1.000

| family | rule set | re-judged | missing | extra | agree | raw agreement | expected agreement | kappa | status |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| mechanism | 20 | 21 | 0 | 1 | 21 | 1.000 | 0.528 | 1.000 | ok |
| structure | 20 | 33 | 0 | 13 | 33 | 1.000 | 0.835 | 1.000 | ok |
| overall | 40 | 54 | 0 | 14 | 54 | 1.000 | 0.676 | 1.000 | ok |

disagreements 0, adjudicated 0, unadjudicated 0

## Strata (pooled)

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 5 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.950 | ok |
| CALLS | fact | 103531 | 31 | 31 | 31 | 0 | 0 | 1.000 | [0.890, 1.000] | 0.950 | ok |
| CONTAINS | fact | 443 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.950 | ok |
| DEFINES | fact | 50145 | 12 | 12 | 12 | 0 | 0 | 1.000 | [0.757, 1.000] | 0.950 | ok |
| DEFINES_CONFIG | fact | 166 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.950 | ok |
| DEPENDS_ON | fact | 1228 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.950 | ok |
| DOCUMENTS | derived | 418 | 31 | 31 | 23 | 8 | 0 | 0.742 | [0.568, 0.863] | 0.900 | under_floor |
| GRPC_CALLS | derived | 18 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.900 | ok |
| HANDLED_BY | derived | 40 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 59 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.950 | ok |
| HAS_ATTRIBUTE | fact | 2184 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.950 | ok |
| HTTP_CALLS | derived | 7 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | ok |
| IMPLEMENTS | derived | 321 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.900 | ok |
| IMPLEMENTS | fact | 540 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.950 | ok |
| IMPLEMENTS | heuristic | 14 | 14 | 14 | 14 | 0 | 0 | 1.000 | [0.785, 1.000] | 0.700 | ok |
| IMPORTS | fact | 6556 | 31 | 31 | 31 | 0 | 0 | 1.000 | [0.890, 1.000] | 0.950 | ok |
| IMPORTS | heuristic | 160 | 30 | 30 | 27 | 3 | 0 | 0.900 | [0.744, 0.965] | 0.700 | ok |
| INHERITS_FROM | fact | 194 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.950 | ok |
| INHERITS_FROM | heuristic | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.700 | ok |
| INJECTS | heuristic | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 145 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.950 | ok |
| RETURNS_TYPE | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| SCHEDULES | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 214 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.900 | ok |
| TESTS | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| USES | fact | 10303 | 31 | 31 | 31 | 0 | 0 | 1.000 | [0.890, 1.000] | 0.950 | ok |
| WS_CONNECTS | derived | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | ok |

## Categories

| category | N | N judged strata | judged | p (stratified) | 95% CI |
|---|---:|---:|---:|---:|---|
| ACCESSES_DATA | 5 | 5 | 5 | 1.000 | [1.000, 1.000] |
| CALLS | 103531 | 103531 | 31 | 1.000 | [0.890, 1.000] |
| CONTAINS | 443 | 443 | 10 | 1.000 | [0.723, 1.000] |
| DEFINES | 50145 | 50145 | 12 | 1.000 | [0.757, 1.000] |
| DEFINES_CONFIG | 166 | 166 | 30 | 1.000 | [0.886, 1.000] |
| DEPENDS_ON | 1228 | 1228 | 30 | 1.000 | [0.886, 1.000] |
| DOCUMENTS | 418 | 418 | 31 | 0.742 | [0.575, 0.860] |
| GRPC_CALLS | 18 | 18 | 18 | 1.000 | [1.000, 1.000] |
| HANDLED_BY | 99 | 99 | 60 | 1.000 | [0.886, 1.000] |
| HAS_ATTRIBUTE | 2184 | 2184 | 11 | 1.000 | [0.741, 1.000] |
| HTTP_CALLS | 7 | 7 | 7 | 1.000 | [1.000, 1.000] |
| IMPLEMENTS | 875 | 875 | 74 | 1.000 | [0.888, 1.000] |
| IMPORTS | 6716 | 6716 | 61 | 0.998 | [0.994, 0.999] |
| INHERITS_FROM | 202 | 202 | 38 | 1.000 | [0.891, 1.000] |
| INJECTS | 2 | 2 | 2 | 1.000 | [1.000, 1.000] |
| READS_CONFIG | 145 | 145 | 30 | 1.000 | [0.886, 1.000] |
| RETURNS_TYPE | 1 | 1 | 1 | 1.000 | [1.000, 1.000] |
| SCHEDULES | 1 | 1 | 1 | 1.000 | [1.000, 1.000] |
| TESTS | 215 | 215 | 31 | 1.000 | [0.887, 1.000] |
| USES | 10303 | 10303 | 31 | 1.000 | [0.890, 1.000] |
| WS_CONNECTS | 2 | 2 | 2 | 1.000 | [1.000, 1.000] |

## Tiers

| tier | N | N judged strata | judged | p (stratified) | 95% CI |
|---|---:|---:|---:|---:|---|
| derived | 1020 | 1020 | 148 | 0.894 | [0.818, 0.941] |
| fact | 175502 | 175502 | 314 | 1.000 | [0.850, 1.000] |
| heuristic | 184 | 184 | 54 | 0.913 | [0.792, 0.967] |

## Under floor

- `DOCUMENTS|derived`

## Per repo (reported, not gated)

### glia

edges 156224, unjudgeable 136, excluded org_packages=5 skip=15731; stratified p 0.999 [0.999, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 88821 | 25 | 25 | 25 | 0 | 0 | 1.000 | [0.867, 1.000] | 0.950 | ok |
| CONTAINS | fact | 428 | 9 | 9 | 9 | 0 | 0 | 1.000 | [0.701, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 30947 | 6 | 6 | 6 | 0 | 0 | 1.000 | [0.610, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 326 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 388 | 27 | 27 | 20 | 7 | 0 | 0.741 | [0.553, 0.868] | 0.900 | under_floor |
| HANDLED_BY | derived | 23 | 17 | 17 | 17 | 0 | 0 | 1.000 | [0.816, 1.000] | 0.900 | ok |
| HAS_ATTRIBUTE | fact | 1922 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 473 | 26 | 26 | 26 | 0 | 0 | 1.000 | [0.871, 1.000] | 0.950 | ok |
| IMPORTS | fact | 5421 | 24 | 24 | 24 | 0 | 0 | 1.000 | [0.862, 1.000] | 0.950 | ok |
| IMPORTS | heuristic | 148 | 27 | 27 | 27 | 0 | 0 | 1.000 | [0.875, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 67 | 14 | 14 | 14 | 0 | 0 | 1.000 | [0.785, 1.000] | 0.950 | ok |
| RETURNS_TYPE | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 49 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | insufficient |
| TESTS | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| USES | fact | 9506 | 27 | 27 | 27 | 0 | 0 | 1.000 | [0.875, 1.000] | 0.950 | ok |

### microservices-demo

edges 4858, unjudgeable 97, excluded 0; stratified p 0.999 [0.346, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 3 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | ok |
| CALLS | fact | 658 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 15 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 163 | 29 | 29 | 29 | 0 | 0 | 1.000 | [0.883, 1.000] | 0.950 | ok |
| DEFINES | fact | 2530 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 402 | 9 | 9 | 9 | 0 | 0 | 1.000 | [0.701, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | ok |
| GRPC_CALLS | derived | 18 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.900 | ok |
| HANDLED_BY | derived | 17 | 13 | 13 | 13 | 0 | 0 | 1.000 | [0.772, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 36 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.950 | ok |
| HAS_ATTRIBUTE | fact | 34 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HTTP_CALLS | derived | 7 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | ok |
| IMPLEMENTS | derived | 318 | 29 | 29 | 29 | 0 | 0 | 1.000 | [0.883, 1.000] | 0.900 | ok |
| IMPLEMENTS | fact | 12 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 3 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.700 | ok |
| IMPORTS | fact | 16 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 6 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.700 | insufficient (under_floor) |
| INHERITS_FROM | fact | 8 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INJECTS | heuristic | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 73 | 15 | 15 | 15 | 0 | 0 | 1.000 | [0.796, 1.000] | 0.950 | ok |
| TESTS | derived | 39 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.900 | insufficient |
| USES | fact | 214 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### samples

edges 30291, unjudgeable 12, excluded 0; stratified p 1.000 [0.445, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | ok |
| CALLS | fact | 12291 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 3 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 15448 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 499 | 12 | 12 | 12 | 0 | 0 | 1.000 | [0.757, 1.000] | 0.950 | ok |
| DOCUMENTS | derived | 28 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | insufficient |
| HANDLED_BY | fact | 2 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HAS_ATTRIBUTE | fact | 171 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 33 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 11 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.700 | ok |
| IMPORTS | fact | 1078 | 4 | 4 | 4 | 0 | 0 | 1.000 | [0.510, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 2 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.700 | insufficient (under_floor) |
| INHERITS_FROM | fact | 185 | 28 | 28 | 28 | 0 | 0 | 1.000 | [0.879, 1.000] | 0.950 | ok |
| READS_CONFIG | fact | 5 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| SCHEDULES | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 48 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | insufficient |
| USES | fact | 412 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### shell

edges 2377, unjudgeable 0, excluded 0; stratified p 1.000 [0.209, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 1309 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 846 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 1 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.900 | under_floor |
| HAS_ATTRIBUTE | fact | 57 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 22 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | fact | 32 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| INHERITS_FROM | heuristic | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.700 | ok |
| USES | fact | 89 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### websocket

edges 1026, unjudgeable 0, excluded 0; stratified p 0.996 [0.260, 0.999]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 452 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 374 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| HANDLED_BY | fact | 21 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.950 | ok |
| IMPLEMENTS | derived | 3 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| IMPORTS | fact | 9 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 4 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.700 | insufficient (under_floor) |
| TESTS | derived | 78 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.900 | ok |
| USES | fact | 82 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| WS_CONNECTS | derived | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | ok |
