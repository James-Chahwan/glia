# glia v0.4.x substrate eval

**Repos scanned:** 45  
**Successful:** 45  
**Total nodes (per-repo):** 13,417  
**Total edges (per-repo):** 14,103  
**Per-repo elapsed:** 1.5s
**Merged elapsed:** 1.6s
**Merged cross-edges:** 2,613

## Aggregate node-kind counts (per-repo summed)

| Node kind | Count |
|---|---|
| METHOD | 3440 |
| MODULE | 2467 |
| FUNCTION | 1596 |
| PACKAGE_DEP | 1566 |
| CLASS | 950 |
| ROUTE | 603 |
| DATA_ENTITY | 583 |
| INTERFACE | 500 |
| CONFIG_KEY | 334 |
| STRUCT | 308 |
| ATTRIBUTE | 216 |
| COMPONENT | 190 |
| INFRA_RESOURCE | 131 |
| PACKAGE | 112 |
| GRPC_CLIENT | 74 |
| SERVICE | 66 |
| EVENT_HANDLER | 57 |
| ENDPOINT | 33 |
| GRPC_SERVICE | 31 |
| QUEUE_PRODUCER | 30 |
| EVENT_EMITTER | 27 |
| CLI_INVOCATION | 20 |
| DATABASE | 16 |
| GRAPHQL_OPERATION | 12 |
| CACHE | 10 |
| GRAPHQL_RESOLVER | 8 |
| ENUM | 8 |
| WS_CLIENT | 8 |
| DIRECTIVE | 6 |
| HOOK | 3 |
| COMPOSABLE | 3 |
| CRON_JOB | 2 |
| EMAIL_SERVICE | 2 |
| WS_HANDLER | 2 |
| PIPE | 1 |
| GUARD | 1 |
| QUEUE_CONSUMER | 1 |

## Aggregate edge-category counts (per-repo summed)

| Edge category | Count |
|---|---|
| DEFINES | 6976 |
| DEPENDS_ON | 3217 |
| CALLS | 1439 |
| ACCESSES_DATA | 1035 |
| READS_CONFIG | 280 |
| HAS_ATTRIBUTE | 216 |
| CONTAINS | 208 |
| HANDLED_BY | 193 |
| DEFINES_CONFIG | 164 |
| TESTS | 163 |
| IMPORTS | 128 |
| INFRA_REFERENCES | 55 |
| EVENT_FLOWS | 10 |
| INHERITS_FROM | 9 |
| USES | 4 |
| WS_CONNECTS | 4 |
| SCHEDULES | 2 |

## Cross-graph edges from merged resolver pass

| Resolver edge category | Count |
|---|---|
| SHARES_DEPENDENCY | 1021 |
| SHARES_DATA_ENTITY | 691 |
| SHARES_CONFIG | 370 |
| SHARES_INFRA_REF | 280 |
| SHARES_SCHEMA | 140 |
| HTTP_CALLS | 66 |
| EVENT_FLOWS | 25 |
| WS_CONNECTS | 16 |
| GRAPHQL_CALLS | 4 |

## v0.4.x new resolver coverage

| Resolver | Substrate node kind | Nodes emitted | Cross-edges (merged) |
|---|---|---|---|
| DbResolver | DATA_ENTITY | 583 | 691 |
| CronResolver | CRON_JOB | 2 | 0 |
| ConfigResolver | CONFIG_KEY | 334 | 370 |
| IacResolver | INFRA_RESOURCE | 131 | 280 |
| PackageResolver | PACKAGE_DEP | 1,566 | 1,021 |

## Per-repo

| Repo | Nodes | Edges | Cross-edges | Elapsed (ms) |
|---|---|---|---|---|
| nestjs | 1,252 | 2,033 | 41 | 66 |
| javalin | 1,072 | 659 | 0 | 196 |
| react-cra | 1,060 | 1,030 | 69 | 139 |
| aspnetcore-clean | 742 | 632 | 2 | 80 |
| axum | 604 | 849 | 0 | 45 |
| frontend | 572 | 787 | 2 | 41 |
| chi | 562 | 682 | 19 | 72 |
| flask | 529 | 759 | 0 | 24 |
| productcatalogservice | 502 | 530 | 1 | 30 |
| checkoutservice | 494 | 661 | 1 | 32 |
| shippingservice | 471 | 496 | 0 | 28 |
| symfony-demo | 463 | 488 | 1 | 36 |
| hono | 433 | 377 | 2 | 54 |
| hapi | 423 | 212 | 4 | 230 |
| angular-realworld | 342 | 277 | 9 | 65 |
| nextjs | 321 | 327 | 0 | 28 |
| spring | 318 | 303 | 0 | 23 |
| fastify | 308 | 336 | 1 | 65 |
| rails-sample | 289 | 236 | 10 | 17 |
| accounts | 250 | 400 | 6 | 14 |
| vue-create | 243 | 144 | 7 | 39 |
| ledger | 226 | 231 | 0 | 22 |
| express | 195 | 119 | 1 | 28 |
| emailservice | 182 | 183 | 0 | 8 |
| frontend | 171 | 174 | 0 | 9 |
| laravel | 170 | 145 | 0 | 11 |
| recommendationservice | 170 | 165 | 0 | 8 |
| slim | 146 | 123 | 0 | 8 |
| django | 109 | 104 | 0 | 3 |
| koa | 109 | 52 | 1 | 11 |
| shoppingassistantservice | 105 | 102 | 0 | 1 |
| ledgermonolith | 77 | 70 | 0 | 7 |
| cartservice | 68 | 64 | 0 | 9 |
| loadgenerator | 64 | 62 | 0 | 1 |
| loadgenerator | 63 | 63 | 0 | 0 |
| sveltekit-realworld | 56 | 41 | 0 | 4 |
| paymentservice | 51 | 40 | 0 | 2 |
| result | 47 | 31 | 0 | 71 |
| currencyservice | 46 | 36 | 0 | 2 |
| adservice | 40 | 29 | 0 | 2 |
| sinatra | 24 | 12 | 0 | 8 |
| vote | 18 | 17 | 0 | 0 |
| components | 13 | 7 | 0 | 0 |
| worker | 13 | 12 | 0 | 1 |
| seed-data | 4 | 3 | 0 | 0 |

