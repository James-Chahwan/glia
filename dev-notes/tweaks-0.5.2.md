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

| T11 | **A GraphQL operation named unlike its root field does not pair** | examples-glia `query StockList { products { .. } }` vs `Query.products`; `mutation Restock { restock .. }` pairs only because the names agree | 1 of 2 GRAPHQL_CALLS |

James, 2026-10-16: *"lets consider this pre 0.5.3 work and part of 0.5.2"* - T10 is in this batch. The test bed for every
tweak is `~/Code/examples-glia` (expected answers and baseline: `dev-notes/examples-glia-expected.md`), plus the Kina and
quokka-stack shapes above, each turned into a substrate-gap fixture.

| T12 | **One hint format between glia and Engram** | Engram's first real correction row (Kina `entryGuard`): the gmap's `identity_hint` is `frontend/src/app/app.routes.ts:3:0`, the cell store's `Identity::hint` is `v1\|3\|app.routes.ts\|9bdbdf56d843b8d0\|entryGuard` | Engram cannot match a row's stored hint to its facts; `glia cell set` takes no hint |

T12 (James, 2026-10-16: *"yeah sure lets get everyone aligned"*): the `v1|..` form is the shared key; glia-export-engram
writes each fact's v1 hint into a digest-bound `<out>.hints.jsonl` (rows `{key, hint}`, a header with `gmap_digest`, as
`.ppr.jsonl`; no GMAP contract change), and `glia cell set --hint` / pyo3 `write_cell` / `PyGraph.set_cell` `hint=` take
that form, validated, with a reported target when a fresh layout's binding disagrees.

| T13 | **TS re-export barrels are not followed** | quokka-stack web: 32 files import from `@app/core` through two barrel levels (`core/index.ts` `export * from './models'` -> `models/index.ts` `export * from './user.model'`); Kina `import { WalletActionSheet } from '../../wallet'` | after DT.1b (W15) the import reaches the barrel's index module, but `export * from` / `export { X } from` emit no import, so the imported names bind nothing |

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

## Who does what, in order (the one plan every session follows)

1. **glia (this session): DONE 2026-10-17** (W15-W26, 57 DT packets, close-out `ac5d2d5b`: 6217 tests, 414 fixtures
   clean, matrix 346 full / 0 partial / 0 none). The tweak packets T1-T13 ran as 0.5.2 waves W15+ with the usual close-outs, then a closing
   docs packet that also updates the three DZ.2 handoffs with every new break. 0.5.2 is then code-complete again.
2. **James:** tests 0.5.2 (Kina, quokka-stack, examples-glia); any tweak he finds lands on main first.
3. **DONE: the Kina + quokka-stack demos re-ran** on the tweaked glia (James, 2026-10-17: *"okay after this we should
   test kina and quokka-stack again and hopefully we can rul the 0.5.3 answer any questions then begin build
   overnight"*); their dead_symbol false positives became the close-out (ruling 7, DT.5h-DT.5l and DT.14, W27-W31).
4. **DONE: Engram's registered replays** at the local tag `0.5.2-complete` (`196728e3`, 2026-10-18), built from source
   (flat-ppr vs flat-fs, X1 off / on, the `since` pairing totals; the corrections writer passes `--hint`). They stand
   for the release: no source file changes between the tag and `v0.5.2`. Results: `dev-notes/next-leap-0.5.3.md` §E.

The 2026-10-17 plan (*"can't we just go 0.5.1-> 0.5.3 release"*, *"yeah sounds fine"*: no 0.5.2 release, one 0.5.1 ->
0.5.3 bump, one combined handoff per consumer, repo-graph held until then) is **reversed**. James, 2026-10-18: *"maybe we
just do the 0.5.2 and slowly work on 0.5.3 over the next week"*, then "I prep, you push" and "repo-graph upgrades to 0.5.2
now". The new order:

5. **Release 0.5.2 now.** glia prepares the release commit: the bump 0.5.1 -> 0.5.2 (workspace, `py/pyproject.toml`,
   `engram-export/Cargo.toml`), the docs saying released, the gates, the wheel and README numbers re-measured. James
   pushes it, tags `v0.5.2` and lets `wheels-py.yml` publish glia-py 0.5.2 (2026-10-18).
6. **repo-graph upgrades to 0.5.2 now** from `dev-notes/repo-graph-handoff-0.5.2.md` (`glia-py==0.5.2`; no longer held).
7. **neuropil** applies `dev-notes/neuropil-handoff-0.5.2.md` after the publish (`=0.5.2` on all six crates).
8. **Engram:** its replays at `196728e3` stand (step 4); it pins the exporter at `v0.5.2`
   (`dev-notes/engram-handoff-0.5.2.md`).
9. **glia:** 0.5.3 is paced over the week, a normal 0.5.2 -> 0.5.3 release: the spec run from
   `dev-notes/next-leap-0.5.3.md` (open questions to James), its waves, follow-up waves until no followup is left, then
   its own release with its own 0.5.2 -> 0.5.3 handoffs.

## Spec run and rulings (2026-10-16)

Specced by `wf_ba54b0e2-6de` (T1-T11: five clusters, each a spec agent and an adversarial verifier) and `wf_cfa53bab-d42`
(T12): 52 packets plus the docs close-out DT.13 = 53 packets, about 20,300 LOC, scheduled as W15-W25 in
`dev-notes/leap-052-packets.json` (W0-W14 unchanged, `schedule_052.py --verify` green). Each verifier's smaller open
questions take that verifier's recommended default; anything deferred is routed by the close-outs (0.5.3 seed).

James's rulings (the five the spec run raised, then later ones):
1. An orphaned / rejected stanza is not an error by default (*"how can we do better it seems odd to break with an
   error"*): `glia check` lists it under "Rules that check nothing" with the reason and a suggested fix, non-blocking;
   `--strict` / `strict=True` makes it exit 2 (correction on DT.7b).
2. Drop the name-only pairing of Solidity events in EventBusResolver (*"yeah sure"*; correction on DT.10b).
3. Contract-model classes collapse in answers (*"i guess so that makes sense"*; DT.1h).
4. JSON-tag renames in `contract-breaks` / review: IN (James, *"yes ?"*, 2026-10-17), advisory by default - an inferred
   client <-> server model break shows in review / contract-breaks but blocks only for a FACT pairing or on opt-in; one
   packets DT.1o (the engine judge) and DT.1p (contract-breaks / review / CLI / pyo3), specced by `wf_c85572dc-7b3`,
   scheduled W24 / W25, so DT.13 moved to W26.
5. Config-templated subjects: noted for later (*"is it nessacary at some point and we should not it for later"*; the
   0.5.3 seed's value-flow item).
6. TS re-export barrels: IN (James, 2026-10-17: *"yeah we should handle the barrel export its such a common web developer
   trope"*): one packet DT.1q (T13), specced by its own spec + verify run.
7. dead_symbol false positives found by the re-demo: IN as 0.5.2 close-out (James, 2026-10-17: *"lets fix the false
   positives that counts as closeout i think"*). Kina's remaining 52 rows hold about 8 (Go `init()`, Go `Error()` through
   the built-in `error` interface, interface methods called through a range variable, the Transloco loader's
   `getTranslation`); every remaining row on Kina and quokka-stack is audited first (176 rows: 103 false, 73 truly dead), then each false-positive mechanism
   is specced as a DT.5 packet (spec + adversarial verify) and run as W27+, and `0.5.2-complete` moves to the new end
   (it did: `196728e3`, where Engram replayed).
