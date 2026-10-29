"""The Sibyl templates, one module per transport. gen.py renders a case through
`TRANSPORTS[case["axes"]["transport"]]`, whose module provides `render(case, state)`,
`expected(case)` and `STATES`; a module may also hold `UNEXPRESSIBLE`, rows of
`((axis, value), (axis, value), why)` that gen.py adds to its own table, so a transport's
templates can state what its stacks cannot express.

`rest` (FH.11a) places a case and holds the REST clients and servers; `grpc`, `graphql`,
`queue` and `websocket` (FH.11b) share `channel` (placement through rest.py's layout, server
manifests, the expected answer), `payload` (the client's model per `contract`) and, for the two
message transports, `stream` (the receiving clients).

`evm` (FH.11c) is self-contained: a Solidity contract served to TypeScript and Go clients, its
drift a renamed function or event or a changed argument count rather than a renamed field. It
declares `VALUES` (the client / server values it renders: ts and go, solidity), which gen.py
reads to narrow the axes, and gen.py covers it only when asked (gen.OPT_IN_TRANSPORTS)."""

from . import evm, graphql, grpc, queue, rest, websocket

TRANSPORTS = {"rest": rest, "grpc": grpc, "graphql": graphql, "queue": queue,
              "websocket": websocket, "evm": evm}
