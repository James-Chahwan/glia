# Cohort results: baseline

glia `glia 0.5.3 (build 0.5.3+p6ebc23fbd28cc0fa)`; 21 repos, 200 tasks; misses 331 (glia_blind 165, glia_wrong 144, noisy 22); errors 0. Written by `python3 bench/cohort/cohort.py run --release baseline` (no model); `check` re-derives it.

## Boundary health per repo

| repo | role | routes | endpoints | HTTP pairs | client reach | route reach | dead | boundary nodes | unparsed | blind |
|---|---|---|---|---|---|---|---|---|---|---|
| shell | library | 0 | 0 | 0 | - | - | 191 | - | - | - |
| websocket | library | 8 | 0 | 0 | - | 0.125 | 27 | WS_CLIENT 1 | - | - |
| samples | app | 23 | 10 | 1 | 0.1 | 0.783 | 1046 | ENDPOINT 10, EVENT_EMITTER 1 | - | clients_unpaired |
| commons-text | library | 0 | 0 | 0 | - | - | 657 | - | - | - |
| microservices-demo | service-set | 2 | 7 | 0 | 0.0 | 0.0 | 78 | ENDPOINT 7, GRPC_CLIENT 19 | - | routes_unreached, clients_unpaired |
| click | library | 0 | 0 | 0 | - | - | 285 | - | - | - |
| immich | app | 430 | 300 | 291 | 0.97 | 0.958 | 4518 | ENDPOINT 300, GRPC_CLIENT 5, WS_CLIENT 3 | .mdx 12, .svelte 415 | unparsed_sfc |
| twenty | app | 203 | 507 | 51 | 0.101 | 0.35 | 7386 | ENDPOINT 507, EVENT_EMITTER 2, GRAPHQL_OPERATION 875, GRPC_CLIENT 1, QUEUE_PRODUCER 5 | .mdx 3029 | clients_unpaired, unparsed_sfc |
| photoprism | app | 235 | 46 | 16 | 0.348 | 0.357 | 1156 | ENDPOINT 46, EVENT_EMITTER 3, QUEUE_PRODUCER 12, WS_CLIENT 1 | - | - |
| gin-vue-admin | app | 210 | 198 | 131 | 0.662 | 0.657 | 1248 | ENDPOINT 198, EVENT_EMITTER 4 | - | - |
| scaffold-eth-2 | app | 5 | 0 | 0 | - | 1.0 | 9 | EVENT_EMITTER 1 | - | no_client_endpoints |
| documenso | app | 44 | 36 | 0 | 0.0 | 0.091 | 767 | ENDPOINT 36, GRAPHQL_OPERATION 1, GRPC_CLIENT 1, QUEUE_PRODUCER 1, RPC_CALL 183 | .mdx 143 | routes_unreached, clients_unpaired, unparsed_sfc |
| medusa | app | 334 | 829 | 5 | 0.006 | 0.976 | 4012 | ENDPOINT 829, EVENT_EMITTER 5, QUEUE_PRODUCER 5 | .mdx 1033 | clients_unpaired, unparsed_sfc |
| mealie | app | 208 | 34 | 3 | 0.088 | 0.034 | 1785 | ENDPOINT 34 | - | routes_unreached, clients_unpaired |
| hoppscotch | app | 59 | 79 | 15 | 0.19 | 0.305 | 1884 | ENDPOINT 79, EVENT_EMITTER 1, GRAPHQL_OPERATION 3, QUEUE_PRODUCER 2, WS_CLIENT 1 | .svelte 2 | clients_unpaired, unparsed_sfc |
| tolgee | app | 595 | 366 | 340 | 0.91 | 0.56 | 3320 | ENDPOINT 366, WS_CLIENT 2 | .mdx 16 | unparsed_sfc |
| prefect | app | 183 | 417 | 22 | 0.041 | 0.169 | 6740 | ENDPOINT 417, EVENT_EMITTER 9, GRAPHQL_OPERATION 1, GRPC_CLIENT 3, QUEUE_PRODUCER 31 | .mdx 961 | routes_unreached, clients_unpaired, unparsed_sfc |
| zora-protocol | app | 0 | 21 | 0 | 0.0 | - | 656 | ENDPOINT 21, EVENT_EMITTER 204, GRAPHQL_OPERATION 2, RPC_CALL 67 | .mdx 114 | clients_unpaired, unparsed_sfc |
| chatwoot | app | 292 | 38 | 2 | 0.053 | 0.243 | 13851 | ENDPOINT 38, EVENT_EMITTER 39, QUEUE_PRODUCER 1, WS_CLIENT 1 | - | clients_unpaired |
| squidex | app | 307 | 89 | 72 | 0.775 | 0.489 | 6187 | ENDPOINT 89, GRAPHQL_OPERATION 2, QUEUE_PRODUCER 2 | - | - |
| plane | app | 445 | 49 | 0 | 0.0 | 0.0 | 4463 | ENDPOINT 49, QUEUE_PRODUCER 33 | - | routes_unreached, clients_unpaired |

## Probes per repo

Each cell is passed / judged (a check that does not apply is not judged).

| repo | tasks | locate | rest | gate | clear | names | blind | wrong | noisy |
|---|---|---|---|---|---|---|---|---|---|
| shell | 8 | 8/8 | 7/8 | - | - | - | 0 | 0 | 1 |
| websocket | 5 | 5/5 | 1/4 | - | - | - | 0 | 0 | 3 |
| samples | 11 | 4/11 | 5/11 | 0/3 | - | 0/3 | 10 | 10 | 0 |
| commons-text | 4 | 4/4 | 4/4 | - | - | - | 0 | 0 | 0 |
| microservices-demo | 10 | 9/9 | 9/9 | 0/4 | - | 3/4 | 2 | 5 | 0 |
| click | 8 | 6/8 | 7/8 | - | - | - | 0 | 2 | 1 |
| immich | 14 | 5/14 | 6/14 | 0/6 | - | 1/6 | 18 | 10 | 1 |
| twenty | 12 | 4/12 | 4/12 | 0/4 | - | 0/4 | 16 | 8 | 2 |
| photoprism | 8 | 4/8 | 4/8 | 0/4 | 0/1 | 0/4 | 8 | 9 | 0 |
| gin-vue-admin | 8 | 2/8 | 4/7 | 0/4 | - | 0/4 | 4 | 13 | 0 |
| scaffold-eth-2 | 9 | 7/8 | 6/8 | 0/3 | - | 2/3 | 3 | 5 | 0 |
| documenso | 12 | 4/12 | 5/12 | 0/4 | - | 1/4 | 7 | 15 | 3 |
| medusa | 12 | 2/12 | 2/12 | 0/4 | - | 0/4 | 19 | 9 | 2 |
| mealie | 8 | 3/7 | 3/7 | 0/3 | - | 0/3 | 11 | 4 | 1 |
| hoppscotch | 12 | 8/12 | 6/12 | 0/4 | - | 1/4 | 13 | 5 | 1 |
| tolgee | 12 | 7/12 | 9/12 | 0/4 | - | 0/4 | 11 | 5 | 1 |
| prefect | 8 | 3/8 | 3/8 | 0/4 | - | 0/4 | 8 | 11 | 2 |
| zora-protocol | 8 | 6/8 | 7/8 | 0/4 | - | 1/4 | 2 | 9 | 1 |
| chatwoot | 12 | 7/12 | 7/12 | 0/4 | - | 0/4 | 14 | 5 | 0 |
| squidex | 10 | 3/10 | 3/10 | 0/4 | - | 0/4 | 10 | 10 | 2 |
| plane | 9 | 4/9 | 5/9 | 0/4 | - | 0/4 | 9 | 9 | 1 |

## The drift gate

contract-breaks at S named a client truth file on 0 of 69 drift tasks; review on 22.

| task | S breaking | rows | distinct | client named | review named | C breaking | names: reached / flagged truth |
|---|---|---|---|---|---|---|---|
| samples-90cd385ef475 | 0 | 0 | 0 | 0 | 0 | 0 | 4 / 0 of 4 |
| samples-9fef1332f3c7 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| samples-effe13307c41 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| microservices-demo-1c8abe204b55 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 1 of 1 |
| microservices-demo-4d8848bc5eeb | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 1 of 1 |
| microservices-demo-577eeb38c8c2 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 1 of 1 |
| microservices-demo-65b5da1a9b3e | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| immich-3f6a07120318 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| immich-47c5a3dbf6d3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| immich-7b51c50a96cf | 0 | 0 | 0 | 0 | 2 | 0 | 0 / 0 of 2 |
| immich-8b3d6b320bf7 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| immich-8d377bfef5a0 | 0 | 0 | 0 | 0 | 1 | 0 | 2 / 2 of 4 |
| immich-c7f6197e7556 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| twenty-09bf84ea7096 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 3 |
| twenty-5f842479ec45 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 3 |
| twenty-65354f9e1d04 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| twenty-b682ae535e4b | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| photoprism-2268b313c29e | 0 | 0 | 0 | 0 | 0 | 0 | 1 / 0 of 2 |
| photoprism-b0c23316cc72 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| photoprism-c901cd78a200 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 2 |
| photoprism-fd8448d6dee1 | 125 | 125 | 48 | 0 | 0 | 125 | 0 / 0 of 2 |
| gin-vue-admin-09603c436ef2 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| gin-vue-admin-24215f8b287b | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| gin-vue-admin-abe85cf6e7d3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| gin-vue-admin-f6fa4c30640c | 0 | 0 | 0 | 0 | 0 | 0 | 1 / 0 of 3 |
| scaffold-eth-2-19f0ea1379f6 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| scaffold-eth-2-4fa0a912d273 | 0 | 0 | 0 | 0 | 0 | 0 | 1 / 0 of 1 |
| scaffold-eth-2-87557d93c8c5 | 0 | 0 | 0 | 0 | 0 | 0 | 3 / 2 of 3 |
| scaffold-eth-2-c43f51c36ea0 | 0 | 0 | 0 | 0 | 0 | 0 | 1 / 1 of 1 |
| documenso-0f29ba6bb6fd | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| documenso-a1d4bec1430a | 0 | 0 | 0 | 0 | 0 | 0 | 1 / 1 of 2 |
| documenso-d448e3f8f6a3 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| documenso-e658cc581878 | 0 | 0 | 0 | 0 | 2 | 0 | 2 / 0 of 4 |
| medusa-25d81462d4ba | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| medusa-735231340d13 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| medusa-7c3b0339379e | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| medusa-879fb81983bc | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 3 |
| mealie-31ef8e581a2b | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| mealie-6fb78ba46840 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 3 |
| mealie-8b53861e17a0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| mealie-f4b4f87ec44a | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| hoppscotch-3836d70e710e | 0 | 0 | 0 | 0 | 2 | 0 | 2 / 2 of 8 |
| hoppscotch-465fa980850a | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 4 |
| hoppscotch-60c607c185d4 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| hoppscotch-bfc00f899ce8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| tolgee-347cc5cdd81a | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| tolgee-614b1b4eb7ac | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| tolgee-95b0f1c6c2e3 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| tolgee-f5f294880bbd | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| prefect-5e6a3eea1ba1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| prefect-65ea05bef8d9 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| prefect-d49fdd0d0901 | 0 | 0 | 0 | 0 | 2 | 0 | 2 / 0 of 3 |
| prefect-d95a511f081d | 0 | 0 | 0 | 0 | 0 | 0 | 1 / 0 of 7 |
| zora-protocol-0c0d8e85e185 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| zora-protocol-278d7705e7ec | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 1 |
| zora-protocol-4af9553f9861 | 0 | 0 | 0 | 0 | 2 | 0 | 2 / 0 of 3 |
| zora-protocol-959a950ee4ac | 0 | 0 | 0 | 0 | 2 | 0 | 2 / 1 of 9 |
| chatwoot-5c2b3f78bd3f | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| chatwoot-7fe41fe03731 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| chatwoot-98a9ed710ad7 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| chatwoot-f0039f26682e | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| squidex-0a22ecffbd8d | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |
| squidex-57aa395ff626 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| squidex-65b6184b2e45 | 0 | 0 | 0 | 0 | 0 | 0 | 1 / 0 of 1 |
| squidex-6937c0fbb13a | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 2 |
| plane-4fc79a2d7e93 | 0 | 0 | 0 | 0 | 1 | 0 | 1 / 0 of 2 |
| plane-85c65cbd14ca | 0 | 0 | 0 | 0 | 1 | 0 | 3 / 0 of 6 |
| plane-b73d6344adee | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 3 |
| plane-c2b0ecd424eb | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 of 1 |

## Misses by class

### glia_blind (165)

| repo | task | check | why |
|---|---|---|---|
| chatwoot | - | health | clients_unpaired |
| chatwoot | chatwoot-5c2b3f78bd3f | rest | app/javascript/dashboard/routes/dashboard/settings/account/components/AutoResolve.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-5c2b3f78bd3f | gate | app/javascript/dashboard/routes/dashboard/settings/account/components/AutoResolve.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-5c2b3f78bd3f | names | app/javascript/dashboard/routes/dashboard/settings/account/components/AutoResolve.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-75cdd57ab401 | rest | app/javascript/dashboard/routes/dashboard/conversation/ContactPanel.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-7fe41fe03731 | gate | app/javascript/dashboard/components-next/HelpCenter/Pages/ArticleEditorPage/ArticleEditor.vue: no path (no boundary node in the file); app/javascript/dashboard/routes/dashboard/helpcenter/pages/PortalsArticlesNewPage.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-7fe41fe03731 | names | app/javascript/dashboard/components-next/HelpCenter/Pages/ArticleEditorPage/ArticleEditor.vue: no path (no boundary node in the file); app/javascript/dashboard/routes/dashboard/helpcenter/pages/PortalsArticlesNewPage.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-98a9ed710ad7 | rest | app/javascript/dashboard/routes/dashboard/settings/inbox/components/BotConfiguration.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-98a9ed710ad7 | gate | app/javascript/dashboard/routes/dashboard/settings/inbox/components/BotConfiguration.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-98a9ed710ad7 | names | app/javascript/dashboard/routes/dashboard/settings/inbox/components/BotConfiguration.vue: no path (no boundary node in the file) |
| chatwoot | chatwoot-abf371d2c080 | rest | spec/controllers/widgets_controller_spec.rb: no path (no boundary node in the file) |
| chatwoot | chatwoot-b69fccf1e61d | rest | app/javascript/dashboard/components-next/AssignmentPolicy/AgentCapacityPolicyCard/AgentCapacityPolicyCard.vue: no path (no boundary node in the file); spec/enterprise/controllers/api/v1/accounts/agent_capacity_policies/users_controller_spec.rb: no path (no boundary node in the file); spec/requests/devise_overrides/confirmations_controller_spec.rb: no path (no boundary node in the file); spec/requests/devise_overrides/passwords_controller_spec.rb: no path (no boundary node in the file) |
| chatwoot | chatwoot-f0039f26682e | gate | app/javascript/portal/portalHelpers.js: no path (no boundary node in the file) |
| chatwoot | chatwoot-f0039f26682e | names | app/javascript/portal/portalHelpers.js: no path (no boundary node in the file) |
| documenso | - | health | clients_unpaired |
| documenso | - | health | routes_unreached |
| documenso | - | health | unparsed_sfc |
| documenso | documenso-341481d6dba8 | rest | apps/web/src/app/(signing)/sign/[token]/complete/page.tsx: no path (no boundary node in the file); apps/web/src/app/(signing)/sign/[token]/page.tsx: no path (no boundary node in the file) |
| documenso | documenso-861225b7c440 | rest | packages/trpc/server/template-router/router.ts: no path (no boundary node in the file) |
| documenso | documenso-a1d4bec1430a | rest | apps/remix/app/components/dialogs/envelope-item-edit-dialog.tsx: unpaired RPC_CALL; apps/remix/app/utils/toast-error-messages.ts: no path (no boundary node in the file) |
| documenso | documenso-e658cc581878 | rest | apps/remix/app/components/tables/documents-table-status-filter.tsx: no path (no boundary node in the file); apps/remix/app/components/tables/inbox-table.tsx: unpaired RPC_CALL; apps/remix/app/routes/_authenticated+/inbox.tsx: no path (no boundary node in the file) |
| gin-vue-admin | gin-vue-admin-abe85cf6e7d3 | rest | web/src/view/systemTools/autoCode/index.vue: no path (no boundary node in the file) |
| gin-vue-admin | gin-vue-admin-abe85cf6e7d3 | gate | web/src/view/systemTools/autoCode/index.vue: no path (no boundary node in the file) |
| gin-vue-admin | gin-vue-admin-abe85cf6e7d3 | names | web/src/view/systemTools/autoCode/index.vue: no path (no boundary node in the file) |
| gin-vue-admin | gin-vue-admin-f6fa4c30640c | rest | web/src/components/chooseImg/index.vue: no path (no boundary node in the file); web/src/view/example/upload/upload.vue: no path (no boundary node in the file); web/src/view/systemTools/system/system.vue: no path (no boundary node in the file) |
| hoppscotch | - | health | clients_unpaired |
| hoppscotch | - | health | unparsed_sfc |
| hoppscotch | hoppscotch-465fa980850a | rest | packages/hoppscotch-common/src/platform/std/kernel-interceptors/agent/index.ts: no path (paired HTTP_CALLS); packages/hoppscotch-common/src/platform/std/kernel-interceptors/native/index.ts: no path (no boundary node in the file); packages/hoppscotch-common/src/platform/std/kernel-interceptors/proxy/index.ts: no path (no boundary node in the file); packages/hoppscotch-selfhost-web/src/platform/auth/desktop/index.ts: no path (no boundary node in the file) |
| hoppscotch | hoppscotch-465fa980850a | gate | packages/hoppscotch-common/src/platform/std/kernel-interceptors/agent/index.ts: no path (paired HTTP_CALLS); packages/hoppscotch-common/src/platform/std/kernel-interceptors/native/index.ts: no path (no boundary node in the file); packages/hoppscotch-common/src/platform/std/kernel-interceptors/proxy/index.ts: no path (no boundary node in the file); packages/hoppscotch-selfhost-web/src/platform/auth/desktop/index.ts: no path (no boundary node in the file) |
| hoppscotch | hoppscotch-465fa980850a | names | packages/hoppscotch-common/src/platform/std/kernel-interceptors/agent/index.ts: no path (paired HTTP_CALLS); packages/hoppscotch-common/src/platform/std/kernel-interceptors/native/index.ts: no path (no boundary node in the file); packages/hoppscotch-common/src/platform/std/kernel-interceptors/proxy/index.ts: no path (no boundary node in the file); packages/hoppscotch-selfhost-web/src/platform/auth/desktop/index.ts: no path (no boundary node in the file) |
| hoppscotch | hoppscotch-60c607c185d4 | rest | packages/hoppscotch-selfhost-web/src/pages/device-login.vue: no path (paired HTTP_CALLS) |
| hoppscotch | hoppscotch-6e7d28db7b8f | rest | packages/hoppscotch-app/src/components/realtime/Log.vue: no path (no boundary node in the file); packages/hoppscotch-app/src/components/smart/Tabs.vue: no path (no boundary node in the file); packages/hoppscotch-app/src/pages/realtime.vue: no path (no boundary node in the file) |
| hoppscotch | hoppscotch-77a561b58153 | rest | packages/hoppscotch-app/helpers/backend/GQLClient.ts: unpaired ENDPOINT; packages/hoppscotch-app/helpers/teams/TeamCollectionAdapter.ts: no path (no boundary node in the file) |
| hoppscotch | hoppscotch-bfc00f899ce8 | locate | every truth file unparsed |
| hoppscotch | hoppscotch-bfc00f899ce8 | rest | packages/hoppscotch-selfhost-web/prod_run.mjs: unparsed; packages/hoppscotch-sh-admin/prod_run.mjs: unparsed |
| hoppscotch | hoppscotch-bfc00f899ce8 | gate | packages/hoppscotch-selfhost-web/prod_run.mjs: unparsed; packages/hoppscotch-sh-admin/prod_run.mjs: unparsed |
| hoppscotch | hoppscotch-bfc00f899ce8 | names | packages/hoppscotch-selfhost-web/prod_run.mjs: unparsed; packages/hoppscotch-sh-admin/prod_run.mjs: unparsed |
| hoppscotch | hoppscotch-eb2cc58dcab6 | rest | packages/hoppscotch-desktop/src-tauri/src/lib.rs: no path (no boundary node in the file) |
| immich | - | health | unparsed_sfc |
| immich | immich-3f6a07120318 | rest | web/src/routes/auth/change-password/+page.svelte: no path (no boundary node in the file) |
| immich | immich-3f6a07120318 | gate | web/src/routes/auth/change-password/+page.svelte: no path (no boundary node in the file) |
| immich | immich-3f6a07120318 | names | web/src/routes/auth/change-password/+page.svelte: no path (no boundary node in the file) |
| immich | immich-47c5a3dbf6d3 | gate | web/src/lib/services/api-key.service.ts: no path (no boundary node in the file) |
| immich | immich-47c5a3dbf6d3 | names | web/src/lib/services/api-key.service.ts: no path (no boundary node in the file) |
| immich | immich-7b51c50a96cf | rest | web/src/lib/modals/PersonMergeSuggestionModal.svelte: no path (no boundary node in the file); web/src/routes/(user)/people/[personId]/[[photos=photos]]/[[assetId=id]]/MergeFaceSelector.svelte: no path (no boundary node in the file) |
| immich | immich-7b51c50a96cf | gate | web/src/lib/modals/PersonMergeSuggestionModal.svelte: no path (no boundary node in the file); web/src/routes/(user)/people/[personId]/[[photos=photos]]/[[assetId=id]]/MergeFaceSelector.svelte: no path (no boundary node in the file) |
| immich | immich-7b51c50a96cf | names | web/src/lib/modals/PersonMergeSuggestionModal.svelte: no path (no boundary node in the file); web/src/routes/(user)/people/[personId]/[[photos=photos]]/[[assetId=id]]/MergeFaceSelector.svelte: no path (no boundary node in the file) |
| immich | immich-8b3d6b320bf7 | rest | mobile/lib/domain/services/search.service.dart: no path (no boundary node in the file); mobile/lib/infrastructure/repositories/search_api.repository.dart: no path (no boundary node in the file) |
| immich | immich-8b3d6b320bf7 | gate | mobile/lib/domain/services/search.service.dart: no path (no boundary node in the file); mobile/lib/infrastructure/repositories/search_api.repository.dart: no path (no boundary node in the file) |
| immich | immich-8b3d6b320bf7 | names | mobile/lib/domain/services/search.service.dart: no path (no boundary node in the file); mobile/lib/infrastructure/repositories/search_api.repository.dart: no path (no boundary node in the file) |
| immich | immich-95c1f0efeb19 | rest | open-api/typescript-sdk/src/fetch-client.ts: no path (paired HTTP_CALLS); server/src/dtos/database-backup.dto.ts: no path (no boundary node in the file); server/src/services/database-backup.service.ts: no path (no boundary node in the file); web/src/lib/components/maintenance/MaintenanceBackupEntry.svelte: no path (no boundary node in the file); web/src/lib/components/maintenance/MaintenanceBackupsList.svelte: no path (no boundary node in the file) |
| immich | immich-b08a8fb56c17 | rest | mobile/android/app/src/main/kotlin/app/alextran/immich/NativeImage.kt: no path (no boundary node in the file); native/crates/immich_core/src/lib.rs: no path (no boundary node in the file); native/crates/immich_core_ffi/src/bitmap.rs: no path (no boundary node in the file); native/crates/immich_core_ffi/src/lib.rs: no path (no boundary node in the file); native/immich_native_core/hook/build.dart: no path (no boundary node in the file) |
| immich | immich-c7f6197e7556 | rest | web/src/routes/(user)/user-settings/OauthSettings.svelte: no path (no boundary node in the file); web/src/routes/admin/system-settings/AuthSettings.svelte: no path (no boundary node in the file) |
| immich | immich-c7f6197e7556 | gate | web/src/routes/(user)/user-settings/OauthSettings.svelte: no path (no boundary node in the file); web/src/routes/admin/system-settings/AuthSettings.svelte: no path (no boundary node in the file) |
| immich | immich-c7f6197e7556 | names | web/src/routes/(user)/user-settings/OauthSettings.svelte: no path (no boundary node in the file); web/src/routes/admin/system-settings/AuthSettings.svelte: no path (no boundary node in the file) |
| immich | immich-cd47ee30676e | rest | server/src/dtos/auth.dto.ts: no path (no boundary node in the file); server/src/services/download.service.ts: no path (no boundary node in the file) |
| mealie | - | health | clients_unpaired |
| mealie | - | health | routes_unreached |
| mealie | mealie-04a1a797e60e | rest | frontend/app/middleware/admin-only.ts: no path (no boundary node in the file); frontend/app/middleware/advanced-only.ts: no path (no boundary node in the file); frontend/app/middleware/can-manage-household-only.ts: no path (no boundary node in the file); frontend/app/middleware/can-manage-only.ts: no path (no boundary node in the file); frontend/app/middleware/can-organize-only.ts: no path (no boundary node in the file); frontend/app/middleware/group-only.ts: no path (no boundary node in the file) |
| mealie | mealie-31ef8e581a2b | gate | frontend/app/components/Domain/Group/GroupAIProviderDialog.vue: no path (no boundary node in the file) |
| mealie | mealie-31ef8e581a2b | names | frontend/app/components/Domain/Group/GroupAIProviderDialog.vue: no path (no boundary node in the file) |
| mealie | mealie-6fb78ba46840 | rest | frontend/app/components/Layout/LayoutParts/AppHeader.vue: no path (no boundary node in the file); frontend/app/components/global/AppLogo.vue: no path (no boundary node in the file); frontend/app/plugins/app-info.client.ts: no path (no boundary node in the file) |
| mealie | mealie-6fb78ba46840 | gate | frontend/app/components/Layout/LayoutParts/AppHeader.vue: no path (no boundary node in the file); frontend/app/components/global/AppLogo.vue: no path (no boundary node in the file); frontend/app/plugins/app-info.client.ts: no path (no boundary node in the file) |
| mealie | mealie-6fb78ba46840 | names | frontend/app/components/Layout/LayoutParts/AppHeader.vue: no path (no boundary node in the file); frontend/app/components/global/AppLogo.vue: no path (no boundary node in the file); frontend/app/plugins/app-info.client.ts: no path (no boundary node in the file) |
| mealie | mealie-8b53861e17a0 | rest | frontend/app/pages/g/[groupSlug]/r/create/html.vue: no path (no boundary node in the file) |
| mealie | mealie-8b53861e17a0 | gate | frontend/app/pages/g/[groupSlug]/r/create/html.vue: no path (no boundary node in the file) |
| mealie | mealie-8b53861e17a0 | names | frontend/app/pages/g/[groupSlug]/r/create/html.vue: no path (no boundary node in the file) |
| medusa | - | health | clients_unpaired |
| medusa | - | health | unparsed_sfc |
| medusa | medusa-1aa0331a9b77 | locate | every truth file unparsed |
| medusa | medusa-1aa0331a9b77 | rest | scripts/deps-update/lib.mjs: unparsed; scripts/deps-update/report.mjs: unparsed |
| medusa | medusa-25d81462d4ba | rest | packages/core/js-sdk/src/admin/claim.ts: no path (no boundary node in the file) |
| medusa | medusa-25d81462d4ba | gate | packages/core/js-sdk/src/admin/claim.ts: no path (no boundary node in the file) |
| medusa | medusa-25d81462d4ba | names | packages/core/js-sdk/src/admin/claim.ts: no path (no boundary node in the file) |
| medusa | medusa-72ad0d0ed4f9 | rest | packages/admin/dashboard/src/routes/regions/region-edit/region-edit.tsx: no path (no boundary node in the file) |
| medusa | medusa-735231340d13 | rest | packages/admin/dashboard/src/routes/products/product-options-manage/components/product-options-manage-form/product-options-manage-form.tsx: no path (no boundary node in the file) |
| medusa | medusa-735231340d13 | gate | packages/admin/dashboard/src/routes/products/product-options-manage/components/product-options-manage-form/product-options-manage-form.tsx: no path (no boundary node in the file) |
| medusa | medusa-735231340d13 | names | packages/admin/dashboard/src/routes/products/product-options-manage/components/product-options-manage-form/product-options-manage-form.tsx: no path (no boundary node in the file) |
| medusa | medusa-7c3b0339379e | rest | packages/core/js-sdk/src/store/index.ts: no path (no boundary node in the file) |
| medusa | medusa-7c3b0339379e | gate | packages/core/js-sdk/src/store/index.ts: no path (no boundary node in the file) |
| medusa | medusa-7c3b0339379e | names | packages/core/js-sdk/src/store/index.ts: no path (no boundary node in the file) |
| medusa | medusa-879fb81983bc | rest | packages/admin/dashboard/src/routes/orders/order-create-claim/components/claim-create-form/claim-inbound-item.tsx: no path (no boundary node in the file); packages/admin/dashboard/src/routes/orders/order-create-exchange/components/exchange-create-form/exchange-inbound-item.tsx: no path (no boundary node in the file); packages/admin/dashboard/src/routes/orders/order-create-return/components/return-create-form/return-item.tsx: no path (no boundary node in the file) |
| medusa | medusa-879fb81983bc | gate | packages/admin/dashboard/src/routes/orders/order-create-claim/components/claim-create-form/claim-inbound-item.tsx: no path (no boundary node in the file); packages/admin/dashboard/src/routes/orders/order-create-exchange/components/exchange-create-form/exchange-inbound-item.tsx: no path (no boundary node in the file); packages/admin/dashboard/src/routes/orders/order-create-return/components/return-create-form/return-item.tsx: no path (no boundary node in the file) |
| medusa | medusa-879fb81983bc | names | packages/admin/dashboard/src/routes/orders/order-create-claim/components/claim-create-form/claim-inbound-item.tsx: no path (no boundary node in the file); packages/admin/dashboard/src/routes/orders/order-create-exchange/components/exchange-create-form/exchange-inbound-item.tsx: no path (no boundary node in the file); packages/admin/dashboard/src/routes/orders/order-create-return/components/return-create-form/return-item.tsx: no path (no boundary node in the file) |
| medusa | medusa-ae63a6d62993 | rest | integration-tests/http/src/api/middlewares.ts: no path (no boundary node in the file); packages/core/framework/src/http/middlewares/index.ts: no path (no boundary node in the file); packages/medusa/src/api/store/search/route.ts: no path (no boundary node in the file) |
| medusa | medusa-f373c17dd60c | rest | packages/core/core-flows/src/draft-order/workflows/update-draft-order-action-shipping-method.ts: no path (no boundary node in the file); packages/core/js-sdk/src/admin/user.ts: unpaired ENDPOINT; packages/core/utils/src/core-flows/events.ts: no path (no boundary node in the file); packages/medusa/src/api/admin/users/[id]/auth-providers/route.ts: no path (no boundary node in the file); packages/medusa/src/api/admin/users/[id]/reset-password/route.ts: no path (no boundary node in the file) |
| microservices-demo | - | health | clients_unpaired |
| microservices-demo | - | health | routes_unreached |
| photoprism | photoprism-2268b313c29e | rest | frontend/src/common/export-download.js: unpaired ENDPOINT; frontend/src/common/export-media.js: no path (no boundary node in the file) |
| photoprism | photoprism-b0c23316cc72 | rest | frontend/src/component/people/edit/dialog.vue: no path (no boundary node in the file); frontend/src/model/subject.js: no path (no boundary node in the file) |
| photoprism | photoprism-b0c23316cc72 | gate | frontend/src/component/people/edit/dialog.vue: no path (no boundary node in the file); frontend/src/model/subject.js: no path (no boundary node in the file) |
| photoprism | photoprism-b0c23316cc72 | names | frontend/src/component/people/edit/dialog.vue: no path (no boundary node in the file); frontend/src/model/subject.js: no path (no boundary node in the file) |
| photoprism | photoprism-c901cd78a200 | rest | frontend/src/component/photo/edit/dialog.vue: no path (no boundary node in the file); frontend/src/model/photo.js: no path (paired HTTP_CALLS) |
| photoprism | photoprism-fd375b3ca7dd | rest | internal/entity/auth_user.go: no path (no boundary node in the file); internal/entity/auth_user_add.go: no path (no boundary node in the file) |
| photoprism | photoprism-fd8448d6dee1 | gate | frontend/src/common/api.js: no path (paired QUEUE_FLOWS); frontend/src/component/notify.vue: no path (no boundary node in the file) |
| photoprism | photoprism-fd8448d6dee1 | names | frontend/src/common/api.js: no path (paired QUEUE_FLOWS); frontend/src/component/notify.vue: no path (no boundary node in the file) |
| plane | - | health | clients_unpaired |
| plane | - | health | routes_unreached |
| plane | plane-85c65cbd14ca | rest | apps/web/app/layout.tsx: no path (no boundary node in the file); apps/web/core/services/api.service.ts: unpaired ENDPOINT; packages/services/src/api.service.ts: unpaired ENDPOINT; packages/services/src/index.ts: no path (no boundary node in the file) |
| plane | plane-b73d6344adee | rest | apps/admin/vite.config.ts: no path (no boundary node in the file); apps/space/vite.config.ts: no path (no boundary node in the file); apps/web/vite.config.ts: no path (no boundary node in the file) |
| plane | plane-b73d6344adee | gate | apps/admin/vite.config.ts: no path (no boundary node in the file); apps/space/vite.config.ts: no path (no boundary node in the file); apps/web/vite.config.ts: no path (no boundary node in the file) |
| plane | plane-b73d6344adee | names | apps/admin/vite.config.ts: no path (no boundary node in the file); apps/space/vite.config.ts: no path (no boundary node in the file); apps/web/vite.config.ts: no path (no boundary node in the file) |
| plane | plane-c2b0ecd424eb | rest | apps/web/core/lib/wrappers/authentication-wrapper.tsx: no path (no boundary node in the file) |
| plane | plane-c2b0ecd424eb | gate | apps/web/core/lib/wrappers/authentication-wrapper.tsx: no path (no boundary node in the file) |
| plane | plane-c2b0ecd424eb | names | apps/web/core/lib/wrappers/authentication-wrapper.tsx: no path (no boundary node in the file) |
| prefect | - | health | clients_unpaired |
| prefect | - | health | routes_unreached |
| prefect | - | health | unparsed_sfc |
| prefect | prefect-49e37224ab17 | rest | src/prefect/_internal/server_maintenance.py: no path (no boundary node in the file); src/prefect/runner/server.py: no path (no boundary node in the file) |
| prefect | prefect-5e6a3eea1ba1 | gate | ui-v2/src/components/automations/action-details/action-details.stories.tsx: no path (no boundary node in the file); ui-v2/src/components/automations/action-details/action-details.tsx: no path (no boundary node in the file) |
| prefect | prefect-5e6a3eea1ba1 | names | ui-v2/src/components/automations/action-details/action-details.stories.tsx: no path (no boundary node in the file); ui-v2/src/components/automations/action-details/action-details.tsx: no path (no boundary node in the file) |
| prefect | prefect-ce5d6e413f93 | rest | src/prefect/server/models/block_documents.py: no path (no boundary node in the file); src/prefect/server/models/block_schemas.py: no path (no boundary node in the file); src/prefect/server/models/storage_defaults.py: no path (no boundary node in the file) |
| prefect | prefect-d95a511f081d | rest | ui-v2/src/api/admin/admin.ts: no path (no boundary node in the file); ui-v2/src/api/admin/index.ts: no path (no boundary node in the file); ui-v2/src/api/block-documents/block-documents.ts: no path (no boundary node in the file); ui-v2/src/components/blocks/blocks-page.tsx: no path (no boundary node in the file); ui-v2/src/routes/blocks/index.tsx: no path (no boundary node in the file) |
| samples | - | health | clients_unpaired |
| samples | samples-5f4c7956953f | rest | compass_app/app/lib/routing/router.dart: no path (no boundary node in the file); compass_app/app/lib/ui/home/widgets/home_screen.dart: no path (no boundary node in the file); compass_app/app/lib/ui/home/widgets/home_title.dart: no path (no boundary node in the file) |
| samples | samples-7e12f75defd8 | rest | compass_app/app/lib/data/services/shared_preferences_service.dart: no path (no boundary node in the file) |
| samples | samples-90cd385ef475 | rest | compass_app/app/lib/config/assets.dart: no path (no boundary node in the file); compass_app/app/lib/routing/routes.dart: no path (no boundary node in the file); compass_app/app/lib/ui/core/themes/colors.dart: no path (no boundary node in the file); compass_app/app/lib/ui/core/themes/theme.dart: no path (no boundary node in the file) |
| samples | samples-9fef1332f3c7 | rest | compass_app/app/lib/data/repositories/booking/booking_repository_local.dart: no path (no boundary node in the file) |
| samples | samples-9fef1332f3c7 | gate | compass_app/app/lib/data/repositories/booking/booking_repository_local.dart: no path (no boundary node in the file) |
| samples | samples-9fef1332f3c7 | names | compass_app/app/lib/data/repositories/booking/booking_repository_local.dart: no path (no boundary node in the file) |
| samples | samples-effe13307c41 | rest | compass_app/app/lib/routing/router.dart: no path (no boundary node in the file) |
| samples | samples-effe13307c41 | gate | compass_app/app/lib/routing/router.dart: no path (no boundary node in the file) |
| samples | samples-effe13307c41 | names | compass_app/app/lib/routing/router.dart: no path (no boundary node in the file) |
| scaffold-eth-2 | - | health | no_client_endpoints |
| scaffold-eth-2 | scaffold-eth-2-4b225311b8da | rest | packages/nextjs/app/blockexplorer/_components/TransactionHash.tsx: no path (no boundary node in the file); packages/nextjs/app/debug/_components/contract/TxReceipt.tsx: no path (no boundary node in the file); packages/nextjs/components/scaffold-eth/Address/AddressCopyIcon.tsx: no path (no boundary node in the file); packages/nextjs/hooks/scaffold-eth/index.ts: no path (no boundary node in the file); packages/nextjs/types/react-copy-to-clipboard/react-copy-to-clipboard.d.ts: no path (no boundary node in the file) |
| scaffold-eth-2 | scaffold-eth-2-87557d93c8c5 | rest | packages/nextjs/hooks/scaffold-eth/useFetchBlocks.ts: no path (no boundary node in the file); packages/nextjs/next-env.d.ts: no path (no boundary node in the file); packages/nextjs/utils/scaffold-eth/contract.ts: no path (no boundary node in the file) |
| squidex | squidex-0a22ecffbd8d | rest | frontend/src/app/shared/components/search/search-form.component.ts: no path (paired HTTP_CALLS) |
| squidex | squidex-0a22ecffbd8d | gate | frontend/src/app/shared/components/search/search-form.component.ts: no path (paired HTTP_CALLS) |
| squidex | squidex-0a22ecffbd8d | names | frontend/src/app/shared/components/search/search-form.component.ts: no path (paired HTTP_CALLS) |
| squidex | squidex-4e3d950885c2 | rest | backend/src/Squidex.Infrastructure.MongoDb/MongoDb/MongoExtensions.cs: no path (no boundary node in the file); backend/src/Squidex.Infrastructure/Orleans/LoggingFilter.cs: no path (no boundary node in the file) |
| squidex | squidex-57aa395ff626 | gate | frontend/src/app/framework/utils/types.ts: no path (no boundary node in the file); frontend/src/app/shared/state/schemas.forms.ts: no path (no boundary node in the file) |
| squidex | squidex-57aa395ff626 | names | frontend/src/app/framework/utils/types.ts: no path (no boundary node in the file); frontend/src/app/shared/state/schemas.forms.ts: no path (no boundary node in the file) |
| squidex | squidex-65b6184b2e45 | rest | frontend/src/app/shared/model/custom.ts: no path (no boundary node in the file) |
| squidex | squidex-6937c0fbb13a | rest | frontend/src/app/shared/components/search/search-form.component.ts: no path (paired HTTP_CALLS); frontend/src/app/shared/state/queries.ts: no path (no boundary node in the file) |
| squidex | squidex-6937c0fbb13a | gate | frontend/src/app/shared/components/search/search-form.component.ts: no path (paired HTTP_CALLS); frontend/src/app/shared/state/queries.ts: no path (no boundary node in the file) |
| squidex | squidex-6937c0fbb13a | names | frontend/src/app/shared/components/search/search-form.component.ts: no path (paired HTTP_CALLS); frontend/src/app/shared/state/queries.ts: no path (no boundary node in the file) |
| tolgee | - | health | unparsed_sfc |
| tolgee | tolgee-347cc5cdd81a | gate | webapp/src/component/layout/TopBanner/useAnnouncement.tsx: no path (no boundary node in the file); webapp/src/constants/docLinks.ts: no path (no boundary node in the file) |
| tolgee | tolgee-347cc5cdd81a | names | webapp/src/component/layout/TopBanner/useAnnouncement.tsx: no path (no boundary node in the file); webapp/src/constants/docLinks.ts: no path (no boundary node in the file) |
| tolgee | tolgee-614b1b4eb7ac | rest | webapp/src/views/projects/translations/TranslationHeader/search/SearchSyntaxHelp.tsx: no path (no boundary node in the file) |
| tolgee | tolgee-614b1b4eb7ac | gate | webapp/src/views/projects/translations/TranslationHeader/search/SearchSyntaxHelp.tsx: no path (no boundary node in the file) |
| tolgee | tolgee-614b1b4eb7ac | names | webapp/src/views/projects/translations/TranslationHeader/search/SearchSyntaxHelp.tsx: no path (no boundary node in the file) |
| tolgee | tolgee-95b0f1c6c2e3 | gate | webapp/src/component/layout/TopBanner/useAnnouncement.tsx: no path (no boundary node in the file); webapp/src/constants/docLinks.ts: no path (no boundary node in the file) |
| tolgee | tolgee-95b0f1c6c2e3 | names | webapp/src/component/layout/TopBanner/useAnnouncement.tsx: no path (no boundary node in the file); webapp/src/constants/docLinks.ts: no path (no boundary node in the file) |
| tolgee | tolgee-ce078f0fbb87 | rest | backend/data/src/main/kotlin/io/tolgee/development/testDataBuilder/data/SlackTestData.kt: no path (no boundary node in the file) |
| tolgee | tolgee-f5f294880bbd | gate | webapp/src/translationTools/useErrorTranslation.ts: no path (no boundary node in the file) |
| tolgee | tolgee-f5f294880bbd | names | webapp/src/translationTools/useErrorTranslation.ts: no path (no boundary node in the file) |
| twenty | - | health | clients_unpaired |
| twenty | - | health | unparsed_sfc |
| twenty | twenty-09bf84ea7096 | gate | packages/twenty-front/src/modules/settings/roles/role-settings/components/SettingsRoleSettings.tsx: no path (no boundary node in the file); packages/twenty-front/src/modules/settings/roles/role-settings/components/stories/SettingsRoleSettings.stories.tsx: no path (no boundary node in the file); packages/twenty-front/src/modules/settings/roles/role/components/SettingsRoleDraftSyncEffect.tsx: no path (no boundary node in the file) |
| twenty | twenty-09bf84ea7096 | names | packages/twenty-front/src/modules/settings/roles/role-settings/components/SettingsRoleSettings.tsx: no path (no boundary node in the file); packages/twenty-front/src/modules/settings/roles/role-settings/components/stories/SettingsRoleSettings.stories.tsx: no path (no boundary node in the file); packages/twenty-front/src/modules/settings/roles/role/components/SettingsRoleDraftSyncEffect.tsx: no path (no boundary node in the file) |
| twenty | twenty-5f842479ec45 | rest | packages/twenty-front/src/modules/activities/emails/hooks/useEmailComposerState.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/activities/emails/hooks/useSendEmail.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/side-panel/pages/compose-email/components/SidePanelComposeEmailPage.tsx: no path (no boundary node in the file) |
| twenty | twenty-5f842479ec45 | gate | packages/twenty-front/src/modules/activities/emails/hooks/useEmailComposerState.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/activities/emails/hooks/useSendEmail.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/side-panel/pages/compose-email/components/SidePanelComposeEmailPage.tsx: no path (no boundary node in the file) |
| twenty | twenty-5f842479ec45 | names | packages/twenty-front/src/modules/activities/emails/hooks/useEmailComposerState.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/activities/emails/hooks/useSendEmail.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/side-panel/pages/compose-email/components/SidePanelComposeEmailPage.tsx: no path (no boundary node in the file) |
| twenty | twenty-65354f9e1d04 | rest | packages/twenty-front/src/modules/error-handler/components/ErrorMessageEffect.tsx: no path (no boundary node in the file) |
| twenty | twenty-65354f9e1d04 | gate | packages/twenty-front/src/modules/error-handler/components/ErrorMessageEffect.tsx: no path (no boundary node in the file) |
| twenty | twenty-65354f9e1d04 | names | packages/twenty-front/src/modules/error-handler/components/ErrorMessageEffect.tsx: no path (no boundary node in the file) |
| twenty | twenty-a055a5b5f3ca | rest | packages/twenty-server/src/engine/core-modules/impersonation/services/impersonation-authorization.service.ts: no path (no boundary node in the file); packages/twenty-server/src/engine/guards/admin-panel-guard.ts: no path (no boundary node in the file); packages/twenty-server/src/engine/guards/admin-panel-or-impersonate.guard.ts: no path (no boundary node in the file); packages/twenty-server/src/engine/guards/server-level-impersonate.guard.ts: no path (no boundary node in the file) |
| twenty | twenty-b682ae535e4b | rest | packages/twenty-front/src/modules/apollo/services/ApolloFactory.ts: no path (no boundary node in the file) |
| twenty | twenty-b682ae535e4b | gate | packages/twenty-front/src/modules/apollo/services/ApolloFactory.ts: no path (no boundary node in the file) |
| twenty | twenty-b682ae535e4b | names | packages/twenty-front/src/modules/apollo/services/ApolloFactory.ts: no path (no boundary node in the file) |
| twenty | twenty-d2aab27b3411 | rest | packages/twenty-front/src/modules/object-record/record-picker/single-record-picker/components/SingleRecordPickerLoadingEffect.tsx: no path (no boundary node in the file); packages/twenty-front/src/modules/object-record/record-picker/single-record-picker/components/SingleRecordPickerMenuItems.tsx: no path (no boundary node in the file) |
| twenty | twenty-f08ce7111b25 | rest | packages/twenty-front/src/modules/workflow/workflow-steps/workflow-actions/form-action/types/WorkflowFormActionField.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/workflow/workflow-variables/types/StepOutputSchemaV2.ts: no path (no boundary node in the file); packages/twenty-front/src/modules/workflow/workflow-variables/utils/generate/generateFormOutputSchema.ts: no path (no boundary node in the file); packages/twenty-shared/src/workflow/index.ts: no path (no boundary node in the file); packages/twenty-shared/src/workflow/workflow-schema/index.ts: no path (no boundary node in the file); packages/twenty-shared/src/workflow/workflow-schema/types/OutputSchema.ts: no path (no boundary node in the file); packages/twenty-shared/src/workflow/workflow-schema/utils/search-variable-in-output-schema.ts: no path (no boundary node in the file) |
| zora-protocol | - | health | clients_unpaired |
| zora-protocol | - | health | unparsed_sfc |

### glia_wrong (144)

| repo | task | check | why |
|---|---|---|---|
| chatwoot | chatwoot-75cdd57ab401 | locate | no truth file in find's top 10 (first at >20) |
| chatwoot | chatwoot-98a9ed710ad7 | locate | no truth file in find's top 10 (first at >20) |
| chatwoot | chatwoot-abf371d2c080 | locate | no truth file in find's top 10 (first at >20) |
| chatwoot | chatwoot-b69fccf1e61d | locate | no truth file in find's top 10 (first at >20) |
| chatwoot | chatwoot-f0039f26682e | locate | no truth file in find's top 10 (first at >20) |
| click | click-269a4ae575f7 | locate | no truth file in find's top 10 (first at 15) |
| click | click-f1c2ce9a7407 | locate | no truth file in find's top 10 (first at >20) |
| documenso | documenso-0f29ba6bb6fd | gate | 0 breaking on a breaking change |
| documenso | documenso-0f29ba6bb6fd | names | a client truth file is reached and holds no removed name |
| documenso | documenso-20ae849a1aa2 | locate | no truth file in find's top 10 (first at >20) |
| documenso | documenso-4b72e7d5466c | locate | no truth file in find's top 10 (first at >20) |
| documenso | documenso-61967b22c1bb | locate | no truth file in find's top 10 (first at 17) |
| documenso | documenso-779de01fe8fb | locate | no truth file in find's top 10 (first at >20) |
| documenso | documenso-861225b7c440 | locate | no truth file in find's top 10 (first at >20) |
| documenso | documenso-a1d4bec1430a | locate | no truth file in find's top 10 (first at >20) |
| documenso | documenso-a1d4bec1430a | gate | 0 breaking on a breaking change |
| documenso | documenso-d448e3f8f6a3 | locate | no truth file in find's top 10 (first at 20) |
| documenso | documenso-d448e3f8f6a3 | gate | 0 breaking on a breaking change |
| documenso | documenso-d448e3f8f6a3 | names | a client truth file is reached and holds no removed name |
| documenso | documenso-e658cc581878 | locate | no truth file in find's top 10 (first at >20) |
| documenso | documenso-e658cc581878 | gate | 0 breaking on a breaking change |
| documenso | documenso-e658cc581878 | names | a client truth file is reached and holds no removed name |
| gin-vue-admin | gin-vue-admin-09603c436ef2 | locate | no truth file in find's top 10 (first at 16) |
| gin-vue-admin | gin-vue-admin-09603c436ef2 | gate | 0 breaking on a breaking change |
| gin-vue-admin | gin-vue-admin-09603c436ef2 | names | a client truth file is reached and holds no removed name |
| gin-vue-admin | gin-vue-admin-24215f8b287b | locate | no truth file in find's top 10 (first at 15) |
| gin-vue-admin | gin-vue-admin-24215f8b287b | gate | 0 breaking on a breaking change |
| gin-vue-admin | gin-vue-admin-24215f8b287b | names | a client truth file is reached and holds no removed name |
| gin-vue-admin | gin-vue-admin-2daa65f00d71 | locate | no truth file in find's top 10 (first at >20) |
| gin-vue-admin | gin-vue-admin-97e7be9d3c57 | locate | no truth file in find's top 10 (first at >20) |
| gin-vue-admin | gin-vue-admin-abe85cf6e7d3 | locate | no truth file in find's top 10 (first at >20) |
| gin-vue-admin | gin-vue-admin-ca7d06780ff5 | rest | 1 of 6 rest files in 15 rows, first ranked file at 20 |
| gin-vue-admin | gin-vue-admin-f6fa4c30640c | locate | no truth file in find's top 10 (first at 15) |
| gin-vue-admin | gin-vue-admin-f6fa4c30640c | gate | 0 breaking on a breaking change |
| gin-vue-admin | gin-vue-admin-f6fa4c30640c | names | a client truth file is reached and holds no removed name |
| hoppscotch | hoppscotch-13b46d52f232 | locate | no truth file in find's top 10 (first at >20) |
| hoppscotch | hoppscotch-3836d70e710e | gate | 0 breaking on a breaking change |
| hoppscotch | hoppscotch-60c607c185d4 | locate | no truth file in find's top 10 (first at >20) |
| hoppscotch | hoppscotch-60c607c185d4 | gate | 0 breaking on a breaking change |
| hoppscotch | hoppscotch-6e7d28db7b8f | locate | no truth file in find's top 10 (first at 11) |
| immich | immich-3f6a07120318 | locate | no truth file in find's top 10 (first at >20) |
| immich | immich-457da17c5d02 | locate | no truth file in find's top 10 (first at >20) |
| immich | immich-616839e8d43a | locate | no truth file in find's top 10 (first at 11) |
| immich | immich-7b51c50a96cf | locate | no truth file in find's top 10 (first at >20) |
| immich | immich-8b3d6b320bf7 | locate | no truth file in find's top 10 (first at >20) |
| immich | immich-8d377bfef5a0 | gate | 0 breaking on a breaking change |
| immich | immich-95c1f0efeb19 | locate | no truth file in find's top 10 (first at >20) |
| immich | immich-9ee412110f02 | locate | no truth file in find's top 10 (first at >20) |
| immich | immich-c7f6197e7556 | locate | no truth file in find's top 10 (first at 12) |
| immich | immich-cd47ee30676e | locate | no truth file in find's top 10 (first at >20) |
| mealie | mealie-04a1a797e60e | locate | no truth file in find's top 10 (first at >20) |
| mealie | mealie-4ab0d7305645 | locate | no truth file in find's top 10 (first at >20) |
| mealie | mealie-6fb78ba46840 | locate | no truth file in find's top 10 (first at >20) |
| mealie | mealie-8b53861e17a0 | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-25d81462d4ba | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-6adae2ca2e31 | locate | no truth file in find's top 10 (first at 16) |
| medusa | medusa-735231340d13 | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-7c3b0339379e | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-879fb81983bc | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-ae63a6d62993 | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-aff9d3e847f2 | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-d249d29ddef7 | locate | no truth file in find's top 10 (first at >20) |
| medusa | medusa-f8dce55556a1 | locate | no truth file in find's top 10 (first at >20) |
| microservices-demo | microservices-demo-1c8abe204b55 | gate | 0 breaking on a breaking change |
| microservices-demo | microservices-demo-4d8848bc5eeb | gate | 0 breaking on a breaking change |
| microservices-demo | microservices-demo-577eeb38c8c2 | gate | 0 breaking on a breaking change |
| microservices-demo | microservices-demo-65b5da1a9b3e | gate | 0 breaking on a breaking change |
| microservices-demo | microservices-demo-65b5da1a9b3e | names | a client truth file is reached and holds no removed name |
| photoprism | photoprism-2268b313c29e | locate | no truth file in find's top 10 (first at 16) |
| photoprism | photoprism-2268b313c29e | gate | 0 breaking on a breaking change |
| photoprism | photoprism-2268b313c29e | names | a client truth file is reached and holds no removed name |
| photoprism | photoprism-b0c23316cc72 | locate | no truth file in find's top 10 (first at >20) |
| photoprism | photoprism-c901cd78a200 | locate | no truth file in find's top 10 (first at >20) |
| photoprism | photoprism-c901cd78a200 | gate | 0 breaking on a breaking change |
| photoprism | photoprism-c901cd78a200 | names | a client truth file is reached and holds no removed name |
| photoprism | photoprism-fd8448d6dee1 | locate | no truth file in find's top 10 (first at 14) |
| photoprism | photoprism-fd8448d6dee1 | clear | breaking 125 at S, 125 at C after the authors' fix |
| plane | plane-1cf91d99790b | locate | no truth file in find's top 10 (first at >20) |
| plane | plane-4fc79a2d7e93 | locate | no truth file in find's top 10 (first at >20) |
| plane | plane-4fc79a2d7e93 | gate | 0 breaking on a breaking change |
| plane | plane-4fc79a2d7e93 | names | a client truth file is reached and holds no removed name |
| plane | plane-85c65cbd14ca | locate | no truth file in find's top 10 (first at >20) |
| plane | plane-85c65cbd14ca | gate | 0 breaking on a breaking change |
| plane | plane-85c65cbd14ca | names | a client truth file is reached and holds no removed name |
| plane | plane-a06d10046df7 | locate | no truth file in find's top 10 (first at >20) |
| plane | plane-c2b0ecd424eb | locate | no truth file in find's top 10 (first at >20) |
| prefect | prefect-2ec5061c1f8c | locate | no truth file in find's top 10 (first at >20) |
| prefect | prefect-5e6a3eea1ba1 | locate | no truth file in find's top 10 (first at >20) |
| prefect | prefect-65ea05bef8d9 | locate | no truth file in find's top 10 (first at >20) |
| prefect | prefect-65ea05bef8d9 | gate | 0 breaking on a breaking change |
| prefect | prefect-65ea05bef8d9 | names | a client truth file is reached and holds no removed name |
| prefect | prefect-d49fdd0d0901 | locate | no truth file in find's top 10 (first at >20) |
| prefect | prefect-d49fdd0d0901 | gate | 0 breaking on a breaking change |
| prefect | prefect-d49fdd0d0901 | names | a client truth file is reached and holds no removed name |
| prefect | prefect-d95a511f081d | locate | no truth file in find's top 10 (first at >20) |
| prefect | prefect-d95a511f081d | gate | 0 breaking on a breaking change |
| prefect | prefect-d95a511f081d | names | a client truth file is reached and holds no removed name |
| samples | samples-57ecb5f7ba97 | locate | no truth file in find's top 10 (first at >20) |
| samples | samples-57ecb5f7ba97 | rest | 9 of 10 rest files in 89 rows, first ranked file at >20 |
| samples | samples-5f4c7956953f | locate | no truth file in find's top 10 (first at >20) |
| samples | samples-649e0e4db96a | locate | no truth file in find's top 10 (first at >20) |
| samples | samples-7e12f75defd8 | locate | no truth file in find's top 10 (first at 15) |
| samples | samples-90cd385ef475 | locate | no truth file in find's top 10 (first at >20) |
| samples | samples-90cd385ef475 | gate | 0 breaking on a breaking change |
| samples | samples-90cd385ef475 | names | a client truth file is reached and holds no removed name |
| samples | samples-9fef1332f3c7 | locate | no truth file in find's top 10 (first at >20) |
| samples | samples-effe13307c41 | locate | no truth file in find's top 10 (first at >20) |
| scaffold-eth-2 | scaffold-eth-2-4fa0a912d273 | gate | 0 breaking on a breaking change |
| scaffold-eth-2 | scaffold-eth-2-4fa0a912d273 | names | a client truth file is reached and holds no removed name |
| scaffold-eth-2 | scaffold-eth-2-87557d93c8c5 | gate | 0 breaking on a breaking change |
| scaffold-eth-2 | scaffold-eth-2-ae1338f18077 | locate | no truth file in find's top 10 (first at 18) |
| scaffold-eth-2 | scaffold-eth-2-c43f51c36ea0 | gate | 0 breaking on a breaking change |
| squidex | squidex-0a22ecffbd8d | locate | no truth file in find's top 10 (first at >20) |
| squidex | squidex-37c046c9894f | locate | no truth file in find's top 10 (first at >20) |
| squidex | squidex-4e3d950885c2 | locate | no truth file in find's top 10 (first at >20) |
| squidex | squidex-57aa395ff626 | locate | no truth file in find's top 10 (first at >20) |
| squidex | squidex-57aa395ff626 | rest | 1 of 2 rest files in 0 rows, first ranked file at 13 |
| squidex | squidex-65b6184b2e45 | locate | no truth file in find's top 10 (first at >20) |
| squidex | squidex-65b6184b2e45 | gate | 0 breaking on a breaking change |
| squidex | squidex-d778c17bcea1 | locate | no truth file in find's top 10 (first at >20) |
| squidex | squidex-fcb339271455 | locate | no truth file in find's top 10 (first at >20) |
| squidex | squidex-fcb339271455 | rest | 1 of 9 rest files in 49 rows, first ranked file at 11 |
| tolgee | tolgee-3f361208b8ac | locate | no truth file in find's top 10 (first at >20) |
| tolgee | tolgee-614b1b4eb7ac | locate | no truth file in find's top 10 (first at >20) |
| tolgee | tolgee-8c6e11d68345 | locate | no truth file in find's top 10 (first at >20) |
| tolgee | tolgee-91534b2ad1d7 | locate | no truth file in find's top 10 (first at 19) |
| tolgee | tolgee-96d51124c96c | locate | no truth file in find's top 10 (first at >20) |
| twenty | twenty-09bf84ea7096 | locate | no truth file in find's top 10 (first at >20) |
| twenty | twenty-5f842479ec45 | locate | no truth file in find's top 10 (first at >20) |
| twenty | twenty-65354f9e1d04 | locate | no truth file in find's top 10 (first at >20) |
| twenty | twenty-a055a5b5f3ca | locate | no truth file in find's top 10 (first at >20) |
| twenty | twenty-b682ae535e4b | locate | no truth file in find's top 10 (first at >20) |
| twenty | twenty-d2aab27b3411 | locate | no truth file in find's top 10 (first at >20) |
| twenty | twenty-e1b4d607303c | locate | no truth file in find's top 10 (first at 14) |
| twenty | twenty-f08ce7111b25 | locate | no truth file in find's top 10 (first at >20) |
| zora-protocol | zora-protocol-0c0d8e85e185 | gate | 0 breaking on a breaking change |
| zora-protocol | zora-protocol-0c0d8e85e185 | names | a client truth file is reached and holds no removed name |
| zora-protocol | zora-protocol-278d7705e7ec | gate | 0 breaking on a breaking change |
| zora-protocol | zora-protocol-278d7705e7ec | names | a client truth file is reached and holds no removed name |
| zora-protocol | zora-protocol-4af9553f9861 | locate | no truth file in find's top 10 (first at 16) |
| zora-protocol | zora-protocol-4af9553f9861 | gate | 0 breaking on a breaking change |
| zora-protocol | zora-protocol-4af9553f9861 | names | a client truth file is reached and holds no removed name |
| zora-protocol | zora-protocol-959a950ee4ac | locate | no truth file in find's top 10 (first at >20) |
| zora-protocol | zora-protocol-959a950ee4ac | gate | 0 breaking on a breaking change |

### noisy (22)

| repo | task | check | why |
|---|---|---|---|
| click | click-269a4ae575f7 | rest | 3 of 3 rest files in 64 rows, first ranked file at >20 |
| documenso | documenso-20ae849a1aa2 | rest | 8 of 9 rest files in 200 rows, first ranked file at >20 |
| documenso | documenso-779de01fe8fb | rest | 1 of 11 rest files in 200 rows, first ranked file at >20 |
| documenso | documenso-d448e3f8f6a3 | rest | 1 of 1 rest files in 200 rows, first ranked file at >20 |
| hoppscotch | hoppscotch-60c607c185d4 | names | 13 files flagged for 1 truth files |
| immich | immich-9ee412110f02 | rest | 1 of 1 rest files in 200 rows, first ranked file at >20 |
| mealie | mealie-a0bdca405f95 | rest | 1 of 5 rest files in 200 rows, first ranked file at >20 |
| medusa | medusa-6adae2ca2e31 | rest | 1 of 2 rest files in 63 rows, first ranked file at 14 |
| medusa | medusa-f8dce55556a1 | rest | 2 of 6 rest files in 200 rows, first ranked file at >20 |
| plane | plane-011328c79300 | rest | 3 of 3 rest files in 189 rows, first ranked file at >20 |
| prefect | prefect-2ec5061c1f8c | rest | 4 of 4 rest files in 200 rows, first ranked file at 11 |
| prefect | prefect-d49fdd0d0901 | rest | 1 of 3 rest files in 200 rows, first ranked file at 14 |
| shell | shell-29869118d1b8 | rest | 1 of 2 rest files in 200 rows, first ranked file at 12 |
| squidex | squidex-65b6184b2e45 | names | 14 files flagged for 1 truth files |
| squidex | squidex-d778c17bcea1 | rest | 2 of 10 rest files in 200 rows, first ranked file at >20 |
| tolgee | tolgee-91534b2ad1d7 | rest | 2 of 7 rest files in 200 rows, first ranked file at >20 |
| twenty | twenty-6c071357d103 | rest | 2 of 3 rest files in 200 rows, first ranked file at >20 |
| twenty | twenty-e1b4d607303c | rest | 3 of 8 rest files in 200 rows, first ranked file at 13 |
| websocket | websocket-1d5465562bd1 | rest | 1 of 2 rest files in 197 rows, first ranked file at >20 |
| websocket | websocket-682b25fffcaa | rest | 1 of 2 rest files in 200 rows, first ranked file at >20 |
| websocket | websocket-bcef8431c980 | rest | 1 of 4 rest files in 200 rows, first ranked file at >20 |
| zora-protocol | zora-protocol-959a950ee4ac | rest | 4 of 9 rest files in 200 rows, first ranked file at >20 |
