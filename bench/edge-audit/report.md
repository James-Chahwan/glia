# Edge-precision audit

Derived by `scripts/edge-audit.sh report` from the committed samples and verdicts; never edit by hand (`report --check` fails on any difference).

## Method

Strata are (edge category x evidence tier), the tier `glia why` prints. Each stratum's sample is the edges with the smallest xxh64 keys (seed `glia-edge-audit-0.5.4`), split across repos in proportion to their stratum sizes. p is correct / (correct + wrong) with a Wilson 95% interval; `unsure` verdicts are excluded and counted. Per category and per tier the estimate is stratified, weights N_h / N over the judged strata, with the finite population correction (a census stratum adds no variance). A stratum is under floor when p is below its tier's floor; insufficient when fewer than 10 rows are judged (a census excepted); noisy when unsure is over 10% of its verdicts. Floors: derived 0.9, fact 0.95, heuristic 0.7.

## Agreement (blind re-judge)

A second judge re-judged each family's ids ending in hex `0` or its first 10% by id (whichever is larger, at least 20) without reading the first verdicts. Cohen's kappa is over the three verdicts, `unsure` its own category; it is undefined when both judges used one verdict only. A family below 0.6 is LOW AGREEMENT and its strata's precision is provisional. A disagreement is re-read at source and its adjudicated verdict replaces the first one; an unadjudicated one counts as `unsure`.

kappa: overall 1.000 (raw agreement 1.000, 51 ids); mechanism 1.000, structure undefined

| family | rule set | re-judged | missing | extra | agree | raw agreement | expected agreement | kappa | status |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| mechanism | 21 | 21 | 0 | 0 | 21 | 1.000 | 0.556 | 1.000 | ok |
| structure | 30 | 30 | 0 | 0 | 30 | 1.000 | 1.000 | undefined | undefined |
| overall | 51 | 51 | 0 | 0 | 51 | 1.000 | 0.763 | 1.000 | ok |

disagreements 0, adjudicated 0, unadjudicated 0

## Strata (pooled)

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 15 | 15 | 15 | 7 | 8 | 0 | 0.467 | [0.248, 0.699] | 0.950 | under_floor |
| CALLS | fact | 96729 | 32 | 32 | 32 | 0 | 0 | 1.000 | [0.893, 1.000] | 0.950 | ok |
| CONTAINS | fact | 397 | 10 | 10 | 10 | 0 | 0 | 1.000 | [0.723, 1.000] | 0.950 | ok |
| DEFINES | fact | 49393 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.950 | ok |
| DEFINES_CONFIG | fact | 239 | 30 | 30 | 20 | 10 | 0 | 0.667 | [0.488, 0.808] | 0.950 | under_floor |
| DEPENDS_ON | fact | 1316 | 30 | 30 | 28 | 2 | 0 | 0.933 | [0.787, 0.982] | 0.950 | under_floor |
| DOCUMENTS | derived | 893 | 31 | 31 | 12 | 19 | 0 | 0.387 | [0.237, 0.562] | 0.900 | under_floor |
| GRPC_CALLS | derived | 18 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.900 | ok |
| HANDLED_BY | derived | 17 | 17 | 17 | 17 | 0 | 0 | 1.000 | [0.816, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 50 | 30 | 30 | 25 | 5 | 0 | 0.833 | [0.664, 0.927] | 0.950 | under_floor |
| HAS_ATTRIBUTE | fact | 2032 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.950 | ok |
| IMPLEMENTS | derived | 489 | 30 | 30 | 24 | 6 | 0 | 0.800 | [0.627, 0.905] | 0.900 | under_floor |
| IMPLEMENTS | fact | 205 | 30 | 30 | 28 | 2 | 0 | 0.933 | [0.787, 0.982] | 0.950 | under_floor |
| IMPLEMENTS | heuristic | 11 | 11 | 11 | 11 | 0 | 0 | 1.000 | [0.741, 1.000] | 0.700 | ok |
| IMPORTS | fact | 5624 | 31 | 31 | 30 | 1 | 0 | 0.968 | [0.838, 0.994] | 0.950 | ok |
| IMPORTS | heuristic | 160 | 30 | 30 | 27 | 3 | 0 | 0.900 | [0.744, 0.965] | 0.700 | ok |
| INHERITS_FROM | fact | 196 | 30 | 30 | 30 | 0 | 0 | 1.000 | [0.886, 1.000] | 0.950 | ok |
| INHERITS_FROM | heuristic | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.700 | ok |
| INJECTS | heuristic | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 162 | 30 | 30 | 23 | 7 | 0 | 0.767 | [0.591, 0.882] | 0.950 | under_floor |
| RETURNS_TYPE | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| SCHEDULES | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 225 | 30 | 30 | 28 | 2 | 0 | 0.933 | [0.787, 0.982] | 0.900 | ok |
| TESTS | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| USES | fact | 9275 | 31 | 31 | 31 | 0 | 0 | 1.000 | [0.890, 1.000] | 0.950 | ok |
| WS_CONNECTS | derived | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | ok |

## Categories

| category | N | N judged strata | judged | p (stratified) | 95% CI |
|---|---:|---:|---:|---:|---|
| ACCESSES_DATA | 15 | 15 | 15 | 0.467 | [0.467, 0.467] |
| CALLS | 96729 | 96729 | 32 | 1.000 | [1.000, 1.000] |
| CONTAINS | 397 | 397 | 10 | 1.000 | [1.000, 1.000] |
| DEFINES | 49393 | 49393 | 11 | 1.000 | [1.000, 1.000] |
| DEFINES_CONFIG | 239 | 239 | 30 | 0.667 | [0.509, 0.824] |
| DEPENDS_ON | 1316 | 1316 | 30 | 0.933 | [0.845, 1.000] |
| DOCUMENTS | 893 | 893 | 31 | 0.387 | [0.219, 0.556] |
| GRPC_CALLS | 18 | 18 | 18 | 1.000 | [1.000, 1.000] |
| HANDLED_BY | 67 | 67 | 47 | 0.876 | [0.813, 0.939] |
| HAS_ATTRIBUTE | 2032 | 2032 | 11 | 1.000 | [1.000, 1.000] |
| IMPLEMENTS | 705 | 705 | 71 | 0.842 | [0.743, 0.941] |
| IMPORTS | 5784 | 5784 | 61 | 0.966 | [0.905, 1.000] |
| INHERITS_FROM | 204 | 204 | 38 | 1.000 | [1.000, 1.000] |
| INJECTS | 2 | 2 | 2 | 1.000 | [1.000, 1.000] |
| READS_CONFIG | 162 | 162 | 30 | 0.767 | [0.630, 0.903] |
| RETURNS_TYPE | 1 | 1 | 1 | 1.000 | [1.000, 1.000] |
| SCHEDULES | 1 | 1 | 1 | 1.000 | [1.000, 1.000] |
| TESTS | 226 | 226 | 31 | 0.934 | [0.851, 1.000] |
| USES | 9275 | 9275 | 31 | 1.000 | [1.000, 1.000] |
| WS_CONNECTS | 2 | 2 | 2 | 1.000 | [1.000, 1.000] |

## Tiers

| tier | N | N judged strata | judged | p (stratified) | 95% CI |
|---|---:|---:|---:|---:|---|
| derived | 1644 | 1644 | 128 | 0.599 | [0.497, 0.700] |
| fact | 165636 | 165636 | 324 | 0.998 | [0.995, 1.000] |
| heuristic | 181 | 181 | 51 | 0.912 | [0.826, 0.997] |

## Under floor

- `ACCESSES_DATA|fact`
- `DEFINES_CONFIG|fact`
- `DEPENDS_ON|fact`
- `DOCUMENTS|derived`
- `HANDLED_BY|fact`
- `IMPLEMENTS|derived`
- `IMPLEMENTS|fact`
- `READS_CONFIG|fact`

## Per repo (reported, not gated)

### glia

edges 141737, unjudgeable 98, excluded skip=14385; stratified p 0.994 [0.990, 0.997]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 3 | 3 | 3 | 0 | 3 | 0 | 0.000 | [0.000, 0.561] | 0.950 | under_floor |
| CALLS | fact | 81008 | 25 | 25 | 25 | 0 | 0 | 1.000 | [0.867, 1.000] | 0.950 | ok |
| CONTAINS | fact | 386 | 9 | 9 | 9 | 0 | 0 | 1.000 | [0.701, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 28189 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 329 | 7 | 7 | 5 | 2 | 0 | 0.714 | [0.359, 0.918] | 0.950 | insufficient (under_floor) |
| DOCUMENTS | derived | 828 | 27 | 27 | 12 | 15 | 0 | 0.444 | [0.276, 0.627] | 0.900 | under_floor |
| HAS_ATTRIBUTE | fact | 1692 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 132 | 19 | 19 | 17 | 2 | 0 | 0.895 | [0.686, 0.971] | 0.950 | under_floor |
| IMPORTS | fact | 4280 | 22 | 22 | 21 | 1 | 0 | 0.955 | [0.782, 0.992] | 0.950 | ok |
| IMPORTS | heuristic | 148 | 27 | 27 | 27 | 0 | 0 | 1.000 | [0.875, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 85 | 16 | 16 | 9 | 7 | 0 | 0.562 | [0.332, 0.769] | 0.950 | under_floor |
| RETURNS_TYPE | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 49 | 7 | 7 | 7 | 0 | 0 | 1.000 | [0.646, 1.000] | 0.900 | insufficient |
| TESTS | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| USES | fact | 8414 | 27 | 27 | 27 | 0 | 0 | 1.000 | [0.875, 1.000] | 0.950 | ok |

### microservices-demo

edges 5001, unjudgeable 89, excluded 0; stratified p 0.975 [0.960, 0.990]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 3 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | ok |
| CALLS | fact | 657 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| CONTAINS | fact | 11 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 163 | 20 | 20 | 20 | 0 | 0 | 1.000 | [0.839, 1.000] | 0.950 | ok |
| DEFINES | fact | 2521 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 402 | 9 | 9 | 9 | 0 | 0 | 1.000 | [0.701, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 3 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.900 | insufficient (under_floor) |
| GRPC_CALLS | derived | 18 | 18 | 18 | 18 | 0 | 0 | 1.000 | [0.824, 1.000] | 0.900 | ok |
| HANDLED_BY | derived | 17 | 17 | 17 | 17 | 0 | 0 | 1.000 | [0.816, 1.000] | 0.900 | ok |
| HANDLED_BY | fact | 27 | 16 | 16 | 11 | 5 | 0 | 0.688 | [0.444, 0.858] | 0.950 | under_floor |
| HAS_ATTRIBUTE | fact | 34 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | derived | 486 | 29 | 29 | 23 | 6 | 0 | 0.793 | [0.616, 0.901] | 0.900 | under_floor |
| IMPLEMENTS | fact | 12 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 3 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.700 | ok |
| IMPORTS | fact | 16 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 6 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.700 | insufficient (under_floor) |
| INHERITS_FROM | fact | 8 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INJECTS | heuristic | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.700 | ok |
| READS_CONFIG | fact | 73 | 13 | 13 | 13 | 0 | 0 | 1.000 | [0.772, 1.000] | 0.950 | ok |
| TESTS | derived | 39 | 5 | 5 | 5 | 0 | 0 | 1.000 | [0.566, 1.000] | 0.900 | insufficient |
| USES | fact | 225 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### samples

edges 33937, unjudgeable 12, excluded 0; stratified p 0.996 [0.995, 0.996]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| ACCESSES_DATA | fact | 9 | 9 | 9 | 4 | 5 | 0 | 0.444 | [0.189, 0.733] | 0.950 | under_floor |
| CALLS | fact | 13334 | 4 | 4 | 4 | 0 | 0 | 1.000 | [0.510, 1.000] | 0.950 | insufficient |
| DEFINES_CONFIG | fact | 76 | 10 | 10 | 0 | 10 | 0 | 0.000 | [0.000, 0.278] | 0.950 | under_floor |
| DEFINES | fact | 17480 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 584 | 13 | 13 | 13 | 0 | 0 | 1.000 | [0.772, 1.000] | 0.950 | ok |
| DOCUMENTS | derived | 60 | 2 | 2 | 0 | 2 | 0 | 0.000 | [0.000, 0.658] | 0.900 | insufficient (under_floor) |
| HANDLED_BY | fact | 2 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| HAS_ATTRIBUTE | fact | 249 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 42 | 6 | 6 | 6 | 0 | 0 | 1.000 | [0.610, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | heuristic | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.700 | ok |
| IMPORTS | fact | 1287 | 6 | 6 | 6 | 0 | 0 | 1.000 | [0.610, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 2 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.700 | insufficient (under_floor) |
| INHERITS_FROM | fact | 187 | 28 | 28 | 28 | 0 | 0 | 1.000 | [0.879, 1.000] | 0.950 | ok |
| READS_CONFIG | fact | 4 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| SCHEDULES | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| TESTS | derived | 61 | 8 | 8 | 7 | 1 | 0 | 0.875 | [0.529, 0.978] | 0.900 | insufficient (under_floor) |
| USES | fact | 465 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### shell

edges 2327, unjudgeable 0, excluded 0; stratified p 0.999 [0.999, 0.999]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 1278 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 829 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DOCUMENTS | derived | 2 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.900 | insufficient (under_floor) |
| HAS_ATTRIBUTE | fact | 57 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPLEMENTS | fact | 19 | 3 | 3 | 3 | 0 | 0 | 1.000 | [0.439, 1.000] | 0.950 | insufficient |
| IMPORTS | fact | 32 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| INHERITS_FROM | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| INHERITS_FROM | heuristic | 8 | 8 | 8 | 8 | 0 | 0 | 1.000 | [0.676, 1.000] | 0.700 | ok |
| USES | fact | 89 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |

### websocket

edges 1024, unjudgeable 0, excluded 0; stratified p 0.989 [0.976, 1.000]

| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|
| CALLS | fact | 452 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEFINES | fact | 374 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| DEPENDS_ON | fact | 1 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | ok |
| HANDLED_BY | fact | 21 | 13 | 13 | 13 | 0 | 0 | 1.000 | [0.772, 1.000] | 0.950 | ok |
| IMPLEMENTS | derived | 3 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.900 | insufficient |
| IMPORTS | fact | 9 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| IMPORTS | heuristic | 4 | 1 | 1 | 0 | 1 | 0 | 0.000 | [0.000, 0.793] | 0.700 | insufficient (under_floor) |
| TESTS | derived | 76 | 10 | 10 | 9 | 1 | 0 | 0.900 | [0.596, 0.982] | 0.900 | ok |
| USES | fact | 82 | 1 | 1 | 1 | 0 | 0 | 1.000 | [0.206, 1.000] | 0.950 | insufficient |
| WS_CONNECTS | derived | 2 | 2 | 2 | 2 | 0 | 0 | 1.000 | [0.342, 1.000] | 0.900 | ok |
