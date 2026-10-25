"""The Sibyl templates, one module per transport. gen.py renders a case through
`TRANSPORTS[case["axes"]["transport"]]`, whose module provides `render(case, state)`,
`expected(case)` and `STATES`; a module may also hold `UNEXPRESSIBLE`, rows of
`((axis, value), (axis, value), why)` that gen.py adds to its own table, so a transport's
templates can state what its stacks cannot express.

`rest` (FH.11a) places a case and holds the REST clients and servers; `grpc`, `graphql`,
`queue` and `websocket` (FH.11b) share `channel` (placement through rest.py's layout, server
manifests, the expected answer), `payload` (the client's model per `contract`) and, for the two
message transports, `stream` (the receiving clients)."""

from . import graphql, grpc, queue, rest, websocket

TRANSPORTS = {"rest": rest, "grpc": grpc, "graphql": graphql, "queue": queue,
              "websocket": websocket}
