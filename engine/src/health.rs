//! Boundary health (0.5.4, FH batch): per mechanism (HTTP, gRPC, queues, WebSocket,
//! GraphQL, events, ..), how much of a repo's cross-boundary surface the graph pairs,
//! leaves unpaired or cannot see, reported as an answer. Owner FH.2a fills this slot;
//! FH.2b and FH.9 read it by module path (`glia_engine::health::..`), never flattened.
