# Sibyl matrix - glia 0.5.3 (build 0.5.3+pc092236393099675)

`[sibyl] cases=46 pairs_covered=399/399 pass=3 fail=43 (pair=14 drift_s=3 clear_c=3 client=4)`

| case | pair | drift_s | clear_c | client | pass |
|---|---|---|---|---|---|
| grpc-001-ts-go-gin | yes | no | no | no | no |
| graphql-002-dart-go-gin | no | no | no | no | no |
| websocket-003-kotlin-go-gin | no | no | no | no | no |
| rest-004-swift-go-gin | no | no | no | no | no |
| queue-005-python-go-gin | no | no | no | no | no |
| websocket-006-ts-ts-nest | no | no | no | no | no |
| grpc-007-dart-ts-nest | yes | no | no | no | no |
| rest-008-kotlin-ts-nest | yes | no | no | no | no |
| graphql-009-swift-ts-nest | no | no | no | no | no |
| graphql-010-python-ts-nest | no | no | no | no | no |
| queue-011-ts-ts-express | no | no | no | no | no |
| websocket-012-dart-ts-express | no | no | no | no | no |
| graphql-013-kotlin-ts-express | no | no | no | no | no |
| rest-014-swift-ts-express | yes | no | no | no | no |
| rest-015-python-ts-express | no | no | no | no | no |
| graphql-016-ts-java-spring | no | no | no | no | no |
| rest-017-dart-java-spring | yes | no | no | no | no |
| grpc-018-kotlin-java-spring | no | no | no | no | no |
| websocket-019-swift-java-spring | no | no | no | no | no |
| websocket-020-python-java-spring | no | no | no | no | no |
| rest-021-ts-python-fastapi | no | no | no | no | no |
| websocket-022-dart-python-fastapi | yes | no | no | no | no |
| queue-023-kotlin-python-fastapi | no | no | no | no | no |
| graphql-024-swift-python-fastapi | no | no | no | no | no |
| rest-025-python-python-fastapi | no | no | no | no | no |
| graphql-026-ts-rust-axum | no | no | no | no | no |
| grpc-027-dart-rust-axum | yes | no | no | no | no |
| queue-028-kotlin-rust-axum | yes | no | no | no | no |
| websocket-029-swift-rust-axum | no | no | no | no | no |
| rest-030-python-rust-axum | no | no | no | no | no |
| queue-031-ts-csharp-aspnet | no | no | no | no | no |
| rest-032-dart-csharp-aspnet | no | no | no | no | no |
| graphql-033-kotlin-csharp-aspnet | no | no | no | no | no |
| grpc-034-swift-csharp-aspnet | no | no | no | no | no |
| grpc-035-python-csharp-aspnet | yes | no | no | no | no |
| evm-036-ts-solidity | no | no | no | no | no |
| evm-037-go-solidity | yes | no | no | no | no |
| websocket-038-swift-csharp-aspnet | no | no | no | no | no |
| queue-039-ts-java-spring | no | no | no | no | no |
| queue-040-kotlin-ts-nest | no | no | no | no | no |
| evm-041-go-solidity | yes | yes | yes | yes | yes |
| evm-042-go-solidity | yes | yes | yes | yes | yes |
| rest-043-dart-go-gin | no | no | no | no | no |
| evm-044-go-solidity | yes | yes | yes | yes | yes |
| grpc-045-swift-rust-axum | no | no | no | no | no |
| graphql-046-ts-ts-express | yes | no | no | yes | no |

## By axis value

| value | cases | passed | level | pair | drift_s | clear_c | client |
|---|---|---|---|---|---|---|---|
| client=dart | 8 | 0 | none | 4 | 0 | 0 | 0 |
| client=go | 4 | 3 | partial | 4 | 3 | 3 | 3 |
| client=kotlin | 8 | 0 | none | 2 | 0 | 0 | 0 |
| client=python | 7 | 0 | none | 1 | 0 | 0 | 0 |
| client=swift | 9 | 0 | none | 1 | 0 | 0 | 0 |
| client=ts | 10 | 0 | none | 2 | 0 | 0 | 1 |
| contract=declared-type | 10 | 0 | none | 4 | 0 | 0 | 0 |
| contract=generated | 15 | 1 | none | 5 | 1 | 1 | 2 |
| contract=plain-literal | 11 | 1 | none | 3 | 1 | 1 | 1 |
| contract=schema-object | 10 | 1 | none | 2 | 1 | 1 | 1 |
| packaging=multi-repo | 14 | 1 | none | 7 | 1 | 1 | 2 |
| packaging=single | 16 | 1 | none | 4 | 1 | 1 | 1 |
| packaging=workspace | 16 | 1 | none | 3 | 1 | 1 | 1 |
| server=csharp-aspnet | 6 | 0 | none | 1 | 0 | 0 | 0 |
| server=go-gin | 6 | 0 | none | 1 | 0 | 0 | 0 |
| server=java-spring | 6 | 0 | none | 1 | 0 | 0 | 0 |
| server=python-fastapi | 5 | 0 | none | 1 | 0 | 0 | 0 |
| server=rust-axum | 6 | 0 | none | 2 | 0 | 0 | 0 |
| server=solidity | 5 | 3 | partial | 4 | 3 | 3 | 3 |
| server=ts-express | 6 | 0 | none | 2 | 0 | 0 | 1 |
| server=ts-nest | 6 | 0 | none | 2 | 0 | 0 | 0 |
| transport=evm | 5 | 3 | partial | 4 | 3 | 3 | 3 |
| transport=graphql | 9 | 0 | none | 1 | 0 | 0 | 1 |
| transport=grpc | 7 | 0 | none | 4 | 0 | 0 | 0 |
| transport=queue | 7 | 0 | none | 1 | 0 | 0 | 0 |
| transport=rest | 10 | 0 | none | 3 | 0 | 0 | 0 |
| transport=websocket | 8 | 0 | none | 1 | 0 | 0 | 0 |
| wiring_call=direct | 17 | 1 | none | 6 | 1 | 1 | 1 |
| wiring_call=generated-sdk | 8 | 1 | none | 3 | 1 | 1 | 2 |
| wiring_call=wrapper | 21 | 1 | none | 5 | 1 | 1 | 1 |
| wiring_source=const | 21 | 2 | none | 9 | 2 | 2 | 3 |
| wiring_source=enum-prefix | 7 | 0 | none | 0 | 0 | 0 | 0 |
| wiring_source=literal | 18 | 1 | none | 5 | 1 | 1 | 1 |

## Chain cells (FH.10)

| chain | cases | passed | level | pair | drift_s | clear_c | client |
|---|---|---|---|---|---|---|---|
| client_wrapper_endpoint | 21 | 1 | none | 5 | 1 | 1 | 1 |
| const_prefix | 28 | 2 | none | 9 | 2 | 2 | 3 |
| sfc_service_endpoint | 0 | 0 | n/a | 0 | 0 | 0 | 0 |
| workspace_sdk_route | 16 | 1 | none | 3 | 1 | 1 | 1 |

## Matrix cells

| cell | cases | passed | level | pair | drift_s | clear_c | client |
|---|---|---|---|---|---|---|---|
| csharp/graphql | 1 | 0 | none | 0 | 0 | 0 | 0 |
| csharp/grpc | 2 | 0 | none | 1 | 0 | 0 | 0 |
| csharp/http_server | 1 | 0 | none | 0 | 0 | 0 | 0 |
| csharp/kafka | 1 | 0 | none | 0 | 0 | 0 | 0 |
| csharp/ws | 1 | 0 | none | 0 | 0 | 0 | 0 |
| dart/graphql | 1 | 0 | none | 0 | 0 | 0 | 0 |
| dart/grpc | 2 | 0 | none | 2 | 0 | 0 | 0 |
| dart/http_client | 3 | 0 | none | 1 | 0 | 0 | 0 |
| dart/ws | 2 | 0 | none | 1 | 0 | 0 | 0 |
| go/evm | 4 | 3 | partial | 4 | 3 | 3 | 3 |
| go/graphql | 1 | 0 | none | 0 | 0 | 0 | 0 |
| go/grpc | 1 | 0 | none | 1 | 0 | 0 | 0 |
| go/http_server | 2 | 0 | none | 0 | 0 | 0 | 0 |
| go/kafka | 1 | 0 | none | 0 | 0 | 0 | 0 |
| go/ws | 1 | 0 | none | 0 | 0 | 0 | 0 |
| java/graphql | 1 | 0 | none | 0 | 0 | 0 | 0 |
| java/grpc | 1 | 0 | none | 0 | 0 | 0 | 0 |
| java/http_server | 1 | 0 | none | 1 | 0 | 0 | 0 |
| java/kafka | 1 | 0 | none | 0 | 0 | 0 | 0 |
| java/ws | 2 | 0 | none | 0 | 0 | 0 | 0 |
| kotlin/graphql | 2 | 0 | none | 0 | 0 | 0 | 0 |
| kotlin/grpc | 1 | 0 | none | 0 | 0 | 0 | 0 |
| kotlin/http_client | 1 | 0 | none | 1 | 0 | 0 | 0 |
| kotlin/kafka | 3 | 0 | none | 1 | 0 | 0 | 0 |
| kotlin/ws | 1 | 0 | none | 0 | 0 | 0 | 0 |
| python/graphql | 2 | 0 | none | 0 | 0 | 0 | 0 |
| python/grpc | 1 | 0 | none | 1 | 0 | 0 | 0 |
| python/http_client | 3 | 0 | none | 0 | 0 | 0 | 0 |
| python/http_server | 2 | 0 | none | 0 | 0 | 0 | 0 |
| python/kafka | 2 | 0 | none | 0 | 0 | 0 | 0 |
| python/ws | 2 | 0 | none | 1 | 0 | 0 | 0 |
| rust/graphql | 1 | 0 | none | 0 | 0 | 0 | 0 |
| rust/grpc | 2 | 0 | none | 1 | 0 | 0 | 0 |
| rust/http_server | 1 | 0 | none | 0 | 0 | 0 | 0 |
| rust/kafka | 1 | 0 | none | 1 | 0 | 0 | 0 |
| rust/ws | 1 | 0 | none | 0 | 0 | 0 | 0 |
| solidity/evm | 5 | 3 | partial | 4 | 3 | 3 | 3 |
| swift/graphql | 2 | 0 | none | 0 | 0 | 0 | 0 |
| swift/grpc | 2 | 0 | none | 0 | 0 | 0 | 0 |
| swift/http_client | 2 | 0 | none | 1 | 0 | 0 | 0 |
| swift/ws | 3 | 0 | none | 0 | 0 | 0 | 0 |
| typescript/evm | 1 | 0 | none | 0 | 0 | 0 | 0 |
| typescript/graphql | 6 | 0 | none | 1 | 0 | 0 | 1 |
| typescript/grpc | 2 | 0 | none | 2 | 0 | 0 | 0 |
| typescript/http_client | 1 | 0 | none | 0 | 0 | 0 | 0 |
| typescript/http_server | 3 | 0 | none | 2 | 0 | 0 | 0 |
| typescript/kafka | 4 | 0 | none | 0 | 0 | 0 | 0 |
| typescript/ws | 2 | 0 | none | 0 | 0 | 0 | 0 |

## By axis pair

| a | b | cases | passed | share | pair | drift_s | clear_c | client |
|---|---|---|---|---|---|---|---|---|
| client=dart | contract=declared-type | 3 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| client=dart | contract=generated | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=dart | contract=plain-literal | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=dart | contract=schema-object | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=dart | packaging=multi-repo | 2 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| client=dart | packaging=single | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=dart | packaging=workspace | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=dart | transport=graphql | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=dart | transport=grpc | 2 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| client=dart | transport=rest | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=dart | transport=websocket | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=dart | wiring_call=direct | 4 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| client=dart | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=dart | wiring_call=wrapper | 3 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| client=dart | wiring_source=const | 5 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| client=dart | wiring_source=enum-prefix | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=dart | wiring_source=literal | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=go | contract=generated | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| client=go | contract=plain-literal | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| client=go | contract=schema-object | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| client=go | packaging=multi-repo | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| client=go | packaging=single | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| client=go | packaging=workspace | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| client=go | transport=evm | 4 | 3 | 0.75 | 4 | 3 | 3 | 3 |
| client=go | wiring_call=direct | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| client=go | wiring_call=generated-sdk | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| client=go | wiring_call=wrapper | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| client=go | wiring_source=const | 3 | 2 | 0.667 | 3 | 2 | 2 | 2 |
| client=go | wiring_source=literal | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| client=kotlin | contract=declared-type | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | contract=generated | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | contract=plain-literal | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | packaging=multi-repo | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | packaging=single | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | packaging=workspace | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | transport=graphql | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | transport=grpc | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | transport=queue | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | transport=rest | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | wiring_call=direct | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | wiring_call=generated-sdk | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | wiring_call=wrapper | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | wiring_source=const | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=kotlin | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=kotlin | wiring_source=literal | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=python | contract=declared-type | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | contract=generated | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=python | contract=plain-literal | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | contract=schema-object | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | packaging=multi-repo | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=python | packaging=single | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | packaging=workspace | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | transport=graphql | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | transport=grpc | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=python | transport=queue | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | transport=rest | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | wiring_call=direct | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | wiring_call=wrapper | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=python | wiring_source=const | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | wiring_source=enum-prefix | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=python | wiring_source=literal | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=swift | contract=declared-type | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=swift | contract=generated | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | contract=plain-literal | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | packaging=multi-repo | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | packaging=single | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | packaging=workspace | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=swift | transport=graphql | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | transport=grpc | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | transport=rest | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=swift | transport=websocket | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | wiring_call=direct | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=swift | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | wiring_call=wrapper | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | wiring_source=const | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | wiring_source=enum-prefix | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=swift | wiring_source=literal | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=ts | contract=declared-type | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=ts | contract=generated | 3 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| client=ts | contract=plain-literal | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | contract=schema-object | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | packaging=multi-repo | 4 | 0 | 0.0 | 2 | 0 | 0 | 1 |
| client=ts | packaging=single | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | packaging=workspace | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | transport=evm | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | transport=graphql | 3 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| client=ts | transport=grpc | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=ts | transport=queue | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | transport=rest | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | wiring_call=direct | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | wiring_call=generated-sdk | 1 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| client=ts | wiring_call=wrapper | 8 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| client=ts | wiring_source=const | 2 | 0 | 0.0 | 2 | 0 | 0 | 1 |
| client=ts | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| client=ts | wiring_source=literal | 6 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=declared-type | packaging=multi-repo | 4 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| contract=declared-type | packaging=single | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=declared-type | packaging=workspace | 3 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| contract=declared-type | wiring_call=direct | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=declared-type | wiring_call=wrapper | 7 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| contract=declared-type | wiring_source=const | 4 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| contract=declared-type | wiring_source=enum-prefix | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=declared-type | wiring_source=literal | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=generated | packaging=multi-repo | 3 | 0 | 0.0 | 3 | 0 | 0 | 1 |
| contract=generated | packaging=single | 7 | 1 | 0.143 | 2 | 1 | 1 | 1 |
| contract=generated | packaging=workspace | 5 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=generated | wiring_call=direct | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=generated | wiring_call=generated-sdk | 8 | 1 | 0.125 | 3 | 1 | 1 | 2 |
| contract=generated | wiring_call=wrapper | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=generated | wiring_source=const | 8 | 1 | 0.125 | 4 | 1 | 1 | 2 |
| contract=generated | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=generated | wiring_source=literal | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=plain-literal | packaging=multi-repo | 4 | 1 | 0.25 | 2 | 1 | 1 | 1 |
| contract=plain-literal | packaging=single | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=plain-literal | packaging=workspace | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=plain-literal | wiring_call=direct | 5 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| contract=plain-literal | wiring_call=wrapper | 6 | 1 | 0.167 | 1 | 1 | 1 | 1 |
| contract=plain-literal | wiring_source=const | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=plain-literal | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=plain-literal | wiring_source=literal | 5 | 1 | 0.2 | 2 | 1 | 1 | 1 |
| contract=schema-object | packaging=multi-repo | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=schema-object | packaging=single | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| contract=schema-object | packaging=workspace | 4 | 1 | 0.25 | 1 | 1 | 1 | 1 |
| contract=schema-object | wiring_call=direct | 5 | 1 | 0.2 | 2 | 1 | 1 | 1 |
| contract=schema-object | wiring_call=wrapper | 5 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=schema-object | wiring_source=const | 5 | 1 | 0.2 | 1 | 1 | 1 | 1 |
| contract=schema-object | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| contract=schema-object | wiring_source=literal | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=csharp-aspnet | client=dart | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | client=kotlin | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | client=python | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=csharp-aspnet | client=swift | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | client=ts | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | contract=declared-type | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | contract=generated | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=csharp-aspnet | contract=plain-literal | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | packaging=multi-repo | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=csharp-aspnet | packaging=single | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | packaging=workspace | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | transport=graphql | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | transport=grpc | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=csharp-aspnet | transport=queue | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | transport=rest | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | wiring_call=direct | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | wiring_call=wrapper | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=csharp-aspnet | wiring_source=const | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=csharp-aspnet | wiring_source=literal | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=go-gin | client=dart | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | client=kotlin | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | client=python | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | client=swift | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | client=ts | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=go-gin | contract=declared-type | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=go-gin | contract=generated | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | contract=plain-literal | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | contract=schema-object | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | packaging=multi-repo | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=go-gin | packaging=single | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | packaging=workspace | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | transport=graphql | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | transport=grpc | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=go-gin | transport=queue | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | transport=rest | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | wiring_call=direct | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | wiring_call=wrapper | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=go-gin | wiring_source=const | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=go-gin | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=go-gin | wiring_source=literal | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | client=dart | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=java-spring | client=kotlin | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | client=python | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | client=swift | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | client=ts | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | contract=declared-type | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | contract=generated | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | contract=plain-literal | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=java-spring | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | packaging=multi-repo | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=java-spring | packaging=single | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | packaging=workspace | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | transport=graphql | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | transport=grpc | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | transport=queue | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | transport=rest | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=java-spring | transport=websocket | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | wiring_call=direct | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=java-spring | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | wiring_call=wrapper | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=java-spring | wiring_source=const | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=java-spring | wiring_source=literal | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | client=dart | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=python-fastapi | client=kotlin | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | client=python | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | client=swift | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | client=ts | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | contract=declared-type | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | contract=generated | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | contract=plain-literal | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | contract=schema-object | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=python-fastapi | packaging=multi-repo | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | packaging=single | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=python-fastapi | packaging=workspace | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | transport=graphql | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | transport=queue | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | transport=rest | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | transport=websocket | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=python-fastapi | wiring_call=direct | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=python-fastapi | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | wiring_call=wrapper | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | wiring_source=const | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | wiring_source=enum-prefix | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=python-fastapi | wiring_source=literal | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | client=dart | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | client=kotlin | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | client=python | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | client=swift | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | client=ts | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | contract=declared-type | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | contract=generated | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | contract=plain-literal | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | packaging=multi-repo | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | packaging=single | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | packaging=workspace | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | transport=graphql | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | transport=grpc | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | transport=queue | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | transport=rest | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | wiring_call=direct | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | wiring_call=generated-sdk | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=rust-axum | wiring_call=wrapper | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | wiring_source=const | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=rust-axum | wiring_source=literal | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=solidity | client=go | 4 | 3 | 0.75 | 4 | 3 | 3 | 3 |
| server=solidity | client=ts | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=solidity | contract=generated | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| server=solidity | contract=plain-literal | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| server=solidity | contract=schema-object | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| server=solidity | packaging=multi-repo | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| server=solidity | packaging=single | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| server=solidity | packaging=workspace | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| server=solidity | transport=evm | 5 | 3 | 0.6 | 4 | 3 | 3 | 3 |
| server=solidity | wiring_call=direct | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| server=solidity | wiring_call=generated-sdk | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| server=solidity | wiring_call=wrapper | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| server=solidity | wiring_source=const | 3 | 2 | 0.667 | 3 | 2 | 2 | 2 |
| server=solidity | wiring_source=literal | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| server=ts-express | client=dart | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | client=kotlin | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | client=python | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | client=swift | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-express | client=ts | 2 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| server=ts-express | contract=declared-type | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-express | contract=generated | 2 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| server=ts-express | contract=plain-literal | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | packaging=multi-repo | 2 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| server=ts-express | packaging=single | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | packaging=workspace | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-express | transport=graphql | 2 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| server=ts-express | transport=queue | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | transport=rest | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-express | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | wiring_call=direct | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-express | wiring_call=generated-sdk | 1 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| server=ts-express | wiring_call=wrapper | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | wiring_source=const | 2 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| server=ts-express | wiring_source=enum-prefix | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-express | wiring_source=literal | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | client=dart | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | client=kotlin | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | client=python | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | client=swift | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | client=ts | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | contract=declared-type | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | contract=generated | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | contract=plain-literal | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | packaging=multi-repo | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | packaging=single | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | packaging=workspace | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | transport=graphql | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | transport=grpc | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | transport=queue | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | transport=rest | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | transport=websocket | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | wiring_call=direct | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | wiring_call=generated-sdk | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | wiring_call=wrapper | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| server=ts-nest | wiring_source=const | 3 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| server=ts-nest | wiring_source=enum-prefix | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| server=ts-nest | wiring_source=literal | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=evm | contract=generated | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| transport=evm | contract=plain-literal | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| transport=evm | contract=schema-object | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| transport=evm | packaging=multi-repo | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| transport=evm | packaging=single | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| transport=evm | packaging=workspace | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| transport=evm | wiring_call=direct | 2 | 1 | 0.5 | 2 | 1 | 1 | 1 |
| transport=evm | wiring_call=generated-sdk | 1 | 1 | 1.0 | 1 | 1 | 1 | 1 |
| transport=evm | wiring_call=wrapper | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| transport=evm | wiring_source=const | 3 | 2 | 0.667 | 3 | 2 | 2 | 2 |
| transport=evm | wiring_source=literal | 2 | 1 | 0.5 | 1 | 1 | 1 | 1 |
| transport=graphql | contract=declared-type | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=graphql | contract=generated | 3 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| transport=graphql | contract=plain-literal | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=graphql | contract=schema-object | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=graphql | packaging=multi-repo | 3 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| transport=graphql | packaging=single | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=graphql | packaging=workspace | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=graphql | wiring_call=direct | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=graphql | wiring_call=generated-sdk | 2 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| transport=graphql | wiring_call=wrapper | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=graphql | wiring_source=const | 4 | 0 | 0.0 | 1 | 0 | 0 | 1 |
| transport=graphql | wiring_source=literal | 5 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=grpc | contract=declared-type | 4 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| transport=grpc | contract=generated | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=grpc | packaging=multi-repo | 3 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| transport=grpc | packaging=single | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=grpc | packaging=workspace | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=grpc | wiring_call=generated-sdk | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=grpc | wiring_call=wrapper | 5 | 0 | 0.0 | 4 | 0 | 0 | 0 |
| transport=grpc | wiring_source=const | 4 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| transport=grpc | wiring_source=literal | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=queue | contract=declared-type | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | contract=generated | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | contract=plain-literal | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=queue | contract=schema-object | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | packaging=multi-repo | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | packaging=single | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=queue | packaging=workspace | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | wiring_call=direct | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=queue | wiring_call=wrapper | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | wiring_source=const | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=queue | wiring_source=literal | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=rest | contract=declared-type | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=rest | contract=generated | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=rest | contract=plain-literal | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=rest | contract=schema-object | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=rest | packaging=multi-repo | 4 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| transport=rest | packaging=single | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=rest | packaging=workspace | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=rest | wiring_call=direct | 3 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| transport=rest | wiring_call=generated-sdk | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=rest | wiring_call=wrapper | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=rest | wiring_source=const | 6 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| transport=rest | wiring_source=enum-prefix | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=rest | wiring_source=literal | 1 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=websocket | contract=declared-type | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | contract=generated | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | contract=plain-literal | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | contract=schema-object | 2 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=websocket | packaging=multi-repo | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | packaging=single | 3 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=websocket | packaging=workspace | 4 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | wiring_call=direct | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| transport=websocket | wiring_call=wrapper | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | wiring_source=const | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | wiring_source=enum-prefix | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| transport=websocket | wiring_source=literal | 4 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| wiring_call=direct | packaging=multi-repo | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| wiring_call=direct | packaging=single | 6 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| wiring_call=direct | packaging=workspace | 6 | 1 | 0.167 | 2 | 1 | 1 | 1 |
| wiring_call=generated-sdk | packaging=multi-repo | 2 | 0 | 0.0 | 2 | 0 | 0 | 1 |
| wiring_call=generated-sdk | packaging=single | 4 | 1 | 0.25 | 1 | 1 | 1 | 1 |
| wiring_call=generated-sdk | packaging=workspace | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_call=wrapper | packaging=multi-repo | 7 | 1 | 0.143 | 4 | 1 | 1 | 1 |
| wiring_call=wrapper | packaging=single | 6 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_call=wrapper | packaging=workspace | 8 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| wiring_source=const | packaging=multi-repo | 9 | 0 | 0.0 | 5 | 0 | 0 | 1 |
| wiring_source=const | packaging=single | 4 | 1 | 0.25 | 2 | 1 | 1 | 1 |
| wiring_source=const | packaging=workspace | 8 | 1 | 0.125 | 2 | 1 | 1 | 1 |
| wiring_source=const | wiring_call=direct | 8 | 1 | 0.125 | 3 | 1 | 1 | 1 |
| wiring_source=const | wiring_call=generated-sdk | 6 | 1 | 0.167 | 3 | 1 | 1 | 2 |
| wiring_source=const | wiring_call=wrapper | 7 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| wiring_source=enum-prefix | packaging=multi-repo | 1 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_source=enum-prefix | packaging=single | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_source=enum-prefix | packaging=workspace | 3 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_source=enum-prefix | wiring_call=direct | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_source=enum-prefix | wiring_call=wrapper | 5 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_source=literal | packaging=multi-repo | 4 | 1 | 0.25 | 2 | 1 | 1 | 1 |
| wiring_source=literal | packaging=single | 9 | 0 | 0.0 | 2 | 0 | 0 | 0 |
| wiring_source=literal | packaging=workspace | 5 | 0 | 0.0 | 1 | 0 | 0 | 0 |
| wiring_source=literal | wiring_call=direct | 7 | 0 | 0.0 | 3 | 0 | 0 | 0 |
| wiring_source=literal | wiring_call=generated-sdk | 2 | 0 | 0.0 | 0 | 0 | 0 | 0 |
| wiring_source=literal | wiring_call=wrapper | 9 | 1 | 0.111 | 2 | 1 | 1 | 1 |
