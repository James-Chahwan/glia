# Calibrated confidence: the held-out calibration (HC.2 / HC.3)

Rule: `dev-notes/confidence-gate-0.5.7.md` (committed before any held-out id was judged). Fit: the 0.5.6 audit, 27 strata, 516 judged. Held-out: chatwoot, gin-vue-admin, hoppscotch, mealie, squidex, tolgee, zora-protocol (38 strata, 363 sampled at n 10 per stratum, 361 judged, 2 unsure, 0 pending). Seed `glia-edge-audit-0.5.4`.

```
[edge-audit] calibration strata=25 fit_n=496 heldout_n=263 covered=25/25 cal_error=0.0218
CONFIDENCE: R1=yes (25/25) R2=yes (0.0218) R3=yes (25) -> PASS
```

K = 25 strata with >= 10 held-out judged ids and a fit p. R1: the held-out Wilson upper bound reaches the fit's Wilson lower bound on 25/25 (needs >= 9/10). R2: the held-out-weighted mean |p_fit - p_held| is 0.0218 (needs <= 0.05). R3: |K| = 25 (needs >= 15).

## Strata

| stratum | N held-out | fit k/n | fit p [lo, hi] | held-out k/n | held-out p [lo, hi] | in K | R1 holds | abs diff | w |
|---|---:|---:|---|---:|---|---|---|---:|---:|
| ACCESSES_DATA\|fact | 3570 | 5/5 | 1.000 [0.566, 1.000] | 7/10 | 0.700 [0.397, 0.892] | yes | yes | 0.300 | 0.014 |
| CALLS\|derived | 6 | 0/0 | - [-, -] | 6/6 | 1.000 [0.610, 1.000] | no | - | - | - |
| CALLS\|fact | 85320 | 31/31 | 1.000 [0.890, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.324 |
| CONTAINS\|fact | 3422 | 10/10 | 1.000 [0.723, 1.000] | 12/13 | 0.923 [0.667, 0.986] | yes | yes | 0.077 | 0.013 |
| DEFINES_CONFIG\|fact | 718 | 30/30 | 1.000 [0.886, 1.000] | 11/11 | 1.000 [0.741, 1.000] | yes | yes | 0.000 | 0.003 |
| DEFINES\|fact | 93422 | 12/12 | 1.000 [0.757, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.355 |
| DEPENDS_ON\|fact | 2574 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.010 |
| DOCUMENTS\|derived | 764 | 23/31 | 0.742 [0.568, 0.863] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.258 | 0.003 |
| EVENT_FLOWS\|derived | 14 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| EVENT_FLOWS\|fact | 288 | 0/0 | - [-, -] | 2/8 | 0.250 [0.071, 0.591] | no | - | - | - |
| EVENT_FLOWS\|heuristic | 3 | 0/0 | - [-, -] | 3/3 | 1.000 [0.439, 1.000] | no | - | - | - |
| GRPC_CALLS\|derived | 0 | 18/18 | 1.000 [0.824, 1.000] | 0/0 | - [-, -] | no | - | - | - |
| HANDLED_BY\|derived | 12 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.000 |
| HANDLED_BY\|fact | 4494 | 30/30 | 1.000 [0.886, 1.000] | 9/12 | 0.750 [0.468, 0.911] | yes | yes | 0.250 | 0.017 |
| HANDLED_BY\|heuristic | 155 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| HAS_ATTRIBUTE\|fact | 4054 | 11/11 | 1.000 [0.741, 1.000] | 11/12 | 0.917 [0.646, 0.985] | yes | yes | 0.083 | 0.015 |
| HTTP_CALLS\|derived | 935 | 7/7 | 1.000 [0.646, 1.000] | 10/11 | 0.909 [0.623, 0.984] | yes | yes | 0.091 | 0.004 |
| IMPLEMENTS\|derived | 263 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.001 |
| IMPLEMENTS\|fact | 3145 | 30/30 | 1.000 [0.886, 1.000] | 11/11 | 1.000 [0.741, 1.000] | yes | yes | 0.000 | 0.012 |
| IMPLEMENTS\|heuristic | 833 | 14/14 | 1.000 [0.785, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.003 |
| IMPORTS\|fact | 16584 | 31/31 | 1.000 [0.890, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.063 |
| IMPORTS\|heuristic | 14618 | 27/30 | 0.900 [0.744, 0.965] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.100 | 0.056 |
| INHERITS_FROM\|fact | 1983 | 30/30 | 1.000 [0.886, 1.000] | 10/11 | 0.909 [0.623, 0.984] | yes | yes | 0.091 | 0.007 |
| INHERITS_FROM\|heuristic | 1778 | 8/8 | 1.000 [0.676, 1.000] | 9/10 | 0.900 [0.596, 0.982] | yes | yes | 0.100 | 0.007 |
| INJECTS\|fact | 3018 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| INJECTS\|heuristic | 588 | 2/2 | 1.000 [0.342, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.002 |
| NAVIGATES_TO\|derived | 40 | 0/0 | - [-, -] | 2/10 | 0.200 [0.057, 0.510] | no | - | - | - |
| NAVIGATES_TO\|fact | 24 | 0/0 | - [-, -] | 9/10 | 0.900 [0.596, 0.982] | no | - | - | - |
| READS_CONFIG\|fact | 633 | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.002 |
| RETURNS_TYPE\|fact | 457 | 1/1 | 1.000 [0.206, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.002 |
| RPC_CALLS\|derived | 8 | 0/0 | - [-, -] | 8/8 | 1.000 [0.676, 1.000] | no | - | - | - |
| RPC_CALLS\|heuristic | 32 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| SAME_AS\|derived | 240 | 0/0 | - [-, -] | 10/10 | 1.000 [0.723, 1.000] | no | - | - | - |
| SCHEDULES\|fact | 23 | 1/1 | 1.000 [0.206, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.000 |
| TESTS\|derived | 5053 | 30/30 | 1.000 [0.886, 1.000] | 10/12 | 0.833 [0.552, 0.953] | yes | yes | 0.167 | 0.019 |
| TESTS\|fact | 2327 | 1/1 | 1.000 [0.206, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.009 |
| USES\|derived | 1 | 0/0 | - [-, -] | 1/1 | 1.000 [0.206, 1.000] | no | - | - | - |
| USES\|fact | 15820 | 31/31 | 1.000 [0.890, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | yes | 0.000 | 0.060 |
| WS_CONNECTS\|derived | 2 | 2/2 | 1.000 [0.342, 1.000] | 2/2 | 1.000 [0.342, 1.000] | no | - | - | - |

Not transferred (R1 fails): none.
Largest gaps in K: ACCESSES_DATA|fact, DOCUMENTS|derived, HANDLED_BY|fact, TESTS|derived, IMPORTS|heuristic.
Held-out strata under their tier floor: ACCESSES_DATA|fact, CONTAINS|fact, EVENT_FLOWS|fact, HANDLED_BY|fact, HAS_ATTRIBUTE|fact, INHERITS_FROM|fact, NAVIGATES_TO|derived, NAVIGATES_TO|fact, TESTS|derived.

## Per tier (information)

| tier | fit n | fit p [lo, hi] | held-out n | held-out p [lo, hi] |
|---|---:|---|---:|---|
| derived | 148 | 0.894 [0.818, 0.941] | 100 | 0.869 [0.664, 0.957] |
| fact | 314 | 1.000 [0.850, 1.000] | 198 | 0.987 [0.978, 0.992] |
| heuristic | 54 | 0.913 [0.792, 0.967] | 63 | 0.990 [0.950, 0.998] |

## Per held-out repo (information)

| repo | edges | unjudgeable | sampled | judged | wrong | unsure | p [lo, hi] |
|---|---:|---:|---:|---:|---:|---:|---|
| chatwoot | 34628 | 38 | 50 | 50 | 3 | 0 | 0.971 [0.914, 0.991] |
| gin-vue-admin | 9123 | 11 | 46 | 46 | 0 | 0 | 1.000 [0.248, 1.000] |
| hoppscotch | 35428 | 25 | 33 | 33 | 1 | 0 | 0.996 [0.261, 0.999] |
| mealie | 32208 | 13 | 53 | 53 | 1 | 0 | 0.987 [0.933, 0.998] |
| squidex | 63218 | 33 | 69 | 69 | 13 | 0 | 0.971 [0.949, 0.984] |
| tolgee | 81556 | 34 | 50 | 50 | 2 | 0 | 0.995 [0.979, 0.999] |
| zora-protocol | 23222 | 17 | 62 | 60 | 8 | 2 | 0.947 [0.874, 0.978] |

## Sub-strata by emitter / rule (information: both sides n >= 10)

| sub-stratum | fit k/n | fit p [lo, hi] | held-out k/n | held-out p [lo, hi] | R1 holds | abs diff |
|---|---:|---|---:|---|---|---:|
| DEPENDS_ON\|fact\|extractor:manifest/- | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.000 |
| IMPLEMENTS\|heuristic\|graph:refs/global_unique | 14/14 | 1.000 [0.785, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.000 |
| IMPORTS\|heuristic\|graph:imports/tail_unique | 27/30 | 0.900 [0.744, 0.965] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.100 |
| READS_CONFIG\|fact\|extractor:config/- | 30/30 | 1.000 [0.886, 1.000] | 10/10 | 1.000 [0.723, 1.000] | yes | 0.000 |
| TESTS\|derived\|pass:tests/name_match | 18/18 | 1.000 [0.824, 1.000] | 8/10 | 0.800 [0.490, 0.943] | yes | 0.200 |

## Held-out re-judge agreement (information)

Overall: 1.000 (56 re-judged of 56 expected, 0 missing); disagreements 0, adjudicated 0, unadjudicated 0.
- heldout: 1.000 (56 re-judged of 56 expected)

## Name-drift answer precision (information: the cohort's names probe)

Flagged files that are truth files / flagged files, over the drift tasks whose start ran the names probe.

| repo | held-out | tasks | flagged | truth | p [lo, hi] |
|---|---|---:|---:|---:|---|
| chatwoot | yes | 4 | 111 | 0 | 0.000 [0.000, 0.034] |
| documenso | no | 4 | 2 | 1 | 0.500 [0.095, 0.905] |
| gin-vue-admin | yes | 4 | 0 | 0 | - [-, -] |
| hoppscotch | yes | 4 | 105 | 2 | 0.019 [0.005, 0.067] |
| immich | no | 6 | 292 | 2 | 0.007 [0.002, 0.025] |
| mealie | yes | 4 | 78 | 0 | 0.000 [0.000, 0.047] |
| medusa | no | 4 | 144 | 0 | 0.000 [0.000, 0.026] |
| microservices-demo | no | 4 | 86 | 3 | 0.035 [0.012, 0.098] |
| photoprism | no | 4 | 10 | 0 | 0.000 [0.000, 0.278] |
| plane | no | 4 | 20 | 0 | 0.000 [0.000, 0.161] |
| prefect | no | 4 | 0 | 0 | - [-, -] |
| samples | no | 3 | 231 | 0 | 0.000 [0.000, 0.016] |
| scaffold-eth-2 | no | 4 | 17 | 3 | 0.176 [0.062, 0.410] |
| squidex | yes | 4 | 373 | 0 | 0.000 [0.000, 0.010] |
| tolgee | yes | 4 | 0 | 0 | - [-, -] |
| twenty | no | 4 | 17 | 0 | 0.000 [0.000, 0.184] |
| zora-protocol | yes | 4 | 38 | 1 | 0.026 [0.005, 0.135] |
| all | no | 69 | 1524 | 12 | 0.008 [0.004, 0.014] |
