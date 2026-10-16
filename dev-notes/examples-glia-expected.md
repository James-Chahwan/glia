# examples-glia - the expected answers (ground truth for the 0.5.2 tweaks)

`~/Code/examples-glia` (commit `fbfb6c0`, James 2026-10-16: *"go make a simple project in code for examples-glia and add
graphql frontend project super simpler but how i did with frontend and backends so it's not obvious because im kinda just
know the models are the same in my head read the connection alot of devs probably have this pattern ... alot of projects
are gonna come in this way"*). Kept out of that repo so glia cannot read its own answers there (the doc linker would ingest
them). Projects: `web` (Angular storefront, REST + WebSocket + ethers), `admin` (Apollo GraphQL console), `api` (Go: gin
REST under an `/api` group, gqlgen GraphQL, gorilla WebSocket inside a gin route, NATS), `contracts` (Solidity escrow).

## Expected links

**Contract models (T1; no OpenAPI, no shared schema: the models agree only by convention).**

| client model | server model | joined through | expected drift |
|---|---|---|---|
| `web/.../models/order.ts` `Order` | `api/internal/models/order.go` `Order` | `GET /api/orders`, `GET /api/orders/:id`, `POST /api/orders`, `PATCH /api/orders/:id/status` responses (`http.get<Order[]>`, `c.JSON(.., o)`) | **`customerName` vs `json:"customer_name"`: the client never receives the name (a real bug)**; `discountCode?` the server never sends; `InternalNote` is `json:"-"` (no drift) |
| `OrderLine` | `OrderLine` | nested in both | none |
| `NewOrder` | `PlaceOrderRequest` | `POST /api/orders` request body (`http.post<Order>(.., order)`, `c.ShouldBindJSON(&req)`) | `discountCode?` sent by the client, dropped by the server |
| `{ status }` literal | `StatusChange` | `PATCH /api/orders/:id/status` body | none |
| `web/.../models/product.ts` `Product` | `CatalogueItem` (names differ) | `GET /api/products` (`http.get<Product[]>`, `c.JSON(.., items)`) | none (`id`, `name`, `priceCents`, `inStock`) |
| `admin/src/types.ts` `StockItem` | GraphQL `type Product` (`api/graph/schema.graphqls`) and gqlgen `model.Product` | `query StockList { products { .. } }` | **`sku` requested by the query but absent from the schema (the query fails at run time)** |

**Channels.**

| channel | client side | server side | tweak |
|---|---|---|---|
| HTTP | `OrderService.list` `` `${this.base}${query}` `` | `GET /api/orders` | T4 (trailing query interpolation) |
| HTTP | `get`, `place`, `cancel`, `ProductService.catalogue` | the other four routes | pair at baseline |
| GraphQL | `query StockList` (root field `products`) | `Query.products` resolver | **T11 (op name differs from its root field)** |
| GraphQL | `mutation Restock` | `Mutation.restock` | pairs at baseline |
| GraphQL transport | `ApolloClient({ uri: '.../graphql' })` | `POST /graphql` | the route should not read as unpaired |
| WebSocket | `` new WebSocket(`${environment.liveUrl}/live/orders`) `` | gin `GET /live/orders` -> `Hub.Serve` -> `upgrader.Upgrade` | T3 |
| NATS | `PublishShipment`: `conn.Publish(shipmentSubject(id), ..)`, `shipmentSubject` returns `"shop.shipments." + orderID` | `conn.Subscribe("shop.shipments.*", ..)` | T2 (subject from a function; wildcard consumer) |
| NATS | `OrderPlaced`: `conn.Publish(fmt.Sprintf(p.template, o.ID), ..)`, template from config default `"shop.orders.%s.placed"` | none in the repo (a producer with no consumer) | T2 (config-templated subject) |
| Contract | `payIntoEscrow`: ethers `new Contract(addr, ESCROW_ABI, signer).deposit(..)` | `Escrow.deposit` (`contracts/contracts/Escrow.sol`) | T10 |
| Contract | `Settler.Release`: `s.escrow.Transact(opts, "release", ..)` | `Escrow.release` | T10 |

**Not dead (T5 / T6).** `OrdersPage.refresh` / `pay` / `cancel` / `totalLabel` (bound in `orders.page.html`),
`ngOnInit` / `ngOnDestroy` and the constructors of reached components and services; the gqlgen resolvers
(`queryResolver.Products`, `mutationResolver.Restock`, `Resolver.Query` / `Mutation`), which gqlgen's generated executable
schema calls (not in the repo); `Escrow.deposit` / `release` (called through the ABI). Truly unused in this example:
`OrderService.get` / `place`, `ProductService.catalogue`, `admin` `lowStock` / `restock`, `PublishShipment`,
`Settler.Release` / `Balance`, `chain.Dial`. `GET /boom` in `orders_test.go` is a test route, not a product route.

## Baseline (glia `c4987609`, before the tweaks)

- HTTP_CALLS 4 of 5; GRAPHQL_CALLS 1 of 2 (`Restock`); WS_CONNECTS 0 (`ws:ws` / `ws_client:ws`); QUEUE nodes 0 (no
  producer, no consumer, the literal `Subscribe("shop.shipments.*")` included); SAME_AS 0.
- gaps: unpaired_endpoint 1 (`GET /api/orders${…}`, also a suspected_edge), unpaired_route 4 (`/live/orders`, `/graphql`,
  `GET /api/orders`, test-only `/boom`), dead_symbol 25 (the Angular lifecycle / template-bound methods, the gqlgen
  resolvers and the Escrow functions among them).
