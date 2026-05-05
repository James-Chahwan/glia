# glia v0.4.x substrate eval

**Repos scanned:** 45  
**Successful:** 45  
**Total nodes (per-repo):** 13,372  
**Total edges (per-repo):** 14,105  
**Per-repo elapsed:** 1.5s
**Merged elapsed:** 1.6s
**Merged cross-edges:** 2,789

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
| SERVICE | 66 |
| EVENT_HANDLER | 57 |
| GRPC_CLIENT | 57 |
| ENDPOINT | 33 |
| GRPC_SERVICE | 31 |
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
| QUEUE_CONSUMER | 2 |
| PIPE | 1 |
| GUARD | 1 |
| QUEUE_PRODUCER | 1 |

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
| GRPC_CALLS | 2 |

## Cross-graph edges from merged resolver pass

| Resolver edge category | Count |
|---|---|
| SHARES_DEPENDENCY | 1021 |
| SHARES_DATA_ENTITY | 691 |
| SHARES_CONFIG | 370 |
| SHARES_INFRA_REF | 280 |
| GRPC_CALLS | 175 |
| SHARES_SCHEMA | 140 |
| HTTP_CALLS | 66 |
| EVENT_FLOWS | 25 |
| WS_CONNECTS | 16 |
| GRAPHQL_CALLS | 4 |
| QUEUE_FLOWS | 1 |

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
| nestjs | 1,250 | 2,033 | 41 | 67 |
| javalin | 1,070 | 659 | 0 | 197 |
| react-cra | 1,059 | 1,030 | 69 | 137 |
| aspnetcore-clean | 737 | 632 | 2 | 79 |
| axum | 601 | 849 | 0 | 44 |
| frontend | 571 | 787 | 2 | 42 |
| chi | 562 | 682 | 19 | 72 |
| flask | 528 | 759 | 0 | 25 |
| productcatalogservice | 501 | 530 | 1 | 29 |
| checkoutservice | 493 | 661 | 1 | 32 |
| shippingservice | 471 | 496 | 0 | 30 |
| symfony-demo | 462 | 488 | 1 | 37 |
| hono | 431 | 377 | 2 | 54 |
| hapi | 422 | 212 | 4 | 231 |
| angular-realworld | 341 | 277 | 9 | 64 |
| nextjs | 320 | 327 | 0 | 28 |
| spring | 318 | 303 | 0 | 23 |
| fastify | 307 | 336 | 1 | 65 |
| rails-sample | 289 | 236 | 10 | 17 |
| accounts | 250 | 400 | 6 | 14 |
| vue-create | 243 | 144 | 7 | 40 |
| ledger | 226 | 231 | 0 | 23 |
| emailservice | 182 | 183 | 0 | 8 |
| express | 178 | 119 | 1 | 28 |
| frontend | 171 | 174 | 0 | 8 |
| laravel | 170 | 145 | 0 | 11 |
| recommendationservice | 169 | 165 | 0 | 8 |
| slim | 146 | 123 | 0 | 8 |
| django | 109 | 104 | 0 | 3 |
| koa | 107 | 52 | 1 | 11 |
| shoppingassistantservice | 105 | 102 | 0 | 1 |
| ledgermonolith | 77 | 70 | 0 | 7 |
| cartservice | 67 | 65 | 1 | 9 |
| loadgenerator | 64 | 62 | 0 | 1 |
| loadgenerator | 63 | 63 | 0 | 0 |
| sveltekit-realworld | 56 | 41 | 0 | 4 |
| paymentservice | 51 | 40 | 0 | 2 |
| currencyservice | 46 | 36 | 0 | 1 |
| result | 46 | 31 | 0 | 71 |
| adservice | 39 | 30 | 1 | 2 |
| sinatra | 24 | 12 | 0 | 8 |
| vote | 19 | 17 | 0 | 0 |
| worker | 14 | 12 | 0 | 1 |
| components | 13 | 7 | 0 | 0 |
| seed-data | 4 | 3 | 0 | 0 |

