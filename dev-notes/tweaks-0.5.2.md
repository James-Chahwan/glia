# glia 0.5.2 tweaks - the here-and-now gaps (scope, before the release)

James, 2026-10-16, after the Kina and quokka-stack demos of the four bets: *"seems like we are picking up gaps to fix like
the models between front and back and the nats queue grpc gql but thats good / we need to handle this model gap / lets
do this as 0.5.2 tweaks because its the here and now issues maybe check for some more types like this"*.

Measured on `git archive` copies of Kina `8ac238c` and quokka-stack `a77d4cb` with glia `c4987609` (`glia gaps --top-k
2000`, `glia query`, `glia check`, `glia identity`, pyo3 `WatchSession`). These become packets `DT.<n>` in
`dev-notes/leap-052-packets.json`, depending on DZ.1 / DZ.2 so they schedule as waves W15+ (the landed W0-W14 do not
move), with one closing docs packet. 0.5.2 is bumped only after them and after James's own testing.

## Gaps of this type

| # | gap | where it showed | measured |
|---|---|---|---|
| T1 | **Client <-> server contract models are not identified** (James: "we need to handle this model gap") | Kina `frontend/.../core/models` `export interface Offer` vs `backend/internal/models/models.go` `type Offer struct` (json tags); quokka web / Android models vs turps | calls pair (Kina 108 HTTP_CALLS) but no model link; quokka's 39 contract_op classes never include a client |
| T2 | **NATS mints no queue node** for dynamic or config-templated subjects | quokka `turps/Services/chat`: `conn.Publish(notifSubject(userID), ..)` on a `*nats.Conn` parameter; JetStream subjects from `config.Appconf.NATS.JetStream.Publish.SubjectTemplate` | 0 QUEUE_PRODUCER / QUEUE_CONSUMER, while the QUEUE_FLOWS caveat says such topics are kept as an unresolved tag |
| T3 | **A WebSocket upgraded inside an HTTP route is not paired** | quokka `chat_ws_controller.go:381` gorilla `Upgrade` inside gin `GET /api/protected/chat/ws`; web `new WebSocket(url)` | `ws:ws` handler and `ws_client:ws` client, 0 WS_CONNECTS; the route stays an unpaired_route |
| T4 | **A trailing query interpolation defeats HTTP pairing** | Kina `smr.service.ts:91` `` `${this.base}/smr${q}` `` vs `GET /admin/smr` | unpaired_endpoint + unpaired_route; already a `suspected_edge` |
| T5 | **dead_symbol is mostly framework noise in UI code** | Kina 429 / 517 dead are Angular METHODs; quokka 307 Angular + 334 Dart METHODs + 44 Dart CLASSes | sample: methods bound only from `.html` / inline templates, lifecycle hooks and constructors, Flutter `build` / `createState` / `dispose` and widget constructors used in build trees, and generated protobuf (`quokka_chat.pb.dart`, ORIGIN generated) |
| T6 | **Test-file routes count as product routes** | Kina `accesslog_test.go` `GET /panic`, `POST /api/auth/password/reset/:token`; `apperr_test.go` `GET /x`; `refusal_audit_test.go` `POST /trades`; `clientip_test.go`, `requestid_test.go` | 7 of Kina's 12 unpaired_route rows and 2 of 3 unhandled_route rows |
| T7 | **A forbid_edge between directories of one project never anchors** | `from = "backend/internal/handlers"`, `to = "backend/internal/repository"` | `[declared] orphaned constraint ... (no PROJECT)` on stderr only; the rule silently leaves `glia check` |
| T8 | **`glia check` repeats every caveat under every violation; GQL answers are unlocated** | Kina check: three HANDLED_BY paragraphs per row; `glia query --gql` rows print `at = -` while the same Datalog query locates them | output only |
| T9 | **pyo3 `neighbours(categories=..)` takes ids only** | `["CALLS"]` -> `TypeError: ... 'str' object cannot be interpreted as an integer` | API only |
| T10 | **Smart contracts are not modelled across the stack** | Kina `contracts/contracts/EscrowManager.sol` called from the frontend (ethers, 7+ files) and the backend (`ethclient`, `signer.go`, `indexer.go`) | no edge joins a call to the Solidity function; 26 of Kina's dead METHODs are Solidity functions |

Checked and fine: gRPC (quokka's Android client and turps' WS-to-gRPC bridge both pair to `grpc:chat.QuokkaChatService`).
Not testable here: GraphQL (neither repo uses it).

## Real findings in the apps (not glia gaps; reported to James)

- Kina's frontend calls 12 endpoints the backend does not register: `POST /api/user/email/change`, `/phone/change`,
  `/phone/verify`, `/2fa/enrol`, `/2fa/verify`, `DELETE /api/user/2fa`, `POST /api/trades/:id/dispute/evidence`,
  `/dispute/escalate`, `/rating`, `GET /api/trades/:id/receipt.pdf`, `GET /api/rates/reference`, `GET /api/public/stats`
  (`GET /admin/reports/metrics` is documented in the frontend as not existing yet).
- Kina's backend routes `GET /admin/rates`, `POST /admin/rates/manual`, `GET /api/user/trading-eligibility`,
  `GET /api/user/wallet` have no frontend caller.
- quokka's Android app sends `PUT /api/protected/settings/account`; turps registers only `DELETE`.
- Kina: 8 handler methods call repository methods directly (the layering rule in the demo).
