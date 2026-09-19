//! Cross-graph resolver registration: every resolver a merged build runs, in
//! the order it runs them — which is also the order their cross edges land in.

use repo_graph_code_domain::evidence;
use repo_graph_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, DbResolver, EventBusResolver,
    GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver, MergedGraph,
    MessageSchemaResolver, PackageResolver, QueueStackResolver, RpcStackResolver,
    SharedSchemaResolver, WebSocketStackResolver,
};

pub(super) fn run_all_resolvers(merged: &mut MergedGraph) {
    // LC.3a: resolvers only append to `cross_edges`, so the range each one
    // appended is stamped `resolver:<name>`. A resolver that attached its own
    // evidence (with a rule, LC.3c) keeps it: a stamp never overrides.
    macro_rules! run {
        ($name:literal, $resolver:expr) => {{
            let n = merged.cross_edges.len();
            merged.run(&$resolver);
            if let Some(added) = merged.cross_edges.get_mut(n..) {
                evidence::stamp_missing(added, concat!("resolver:", $name));
            }
        }};
    }
    run!("http", HttpStackResolver);
    run!("grpc", GrpcStackResolver);
    run!("rpc", RpcStackResolver);
    run!("queue", QueueStackResolver);
    run!("graphql", GraphQLStackResolver);
    run!("websocket", WebSocketStackResolver);
    run!("eventbus", EventBusResolver);
    run!("shared_schema", SharedSchemaResolver);
    // A10.7 — MESSAGE_TYPE nodes joined across repos on the exact qname.
    run!("message_schema", MessageSchemaResolver);
    run!("cli", CliInvocationResolver);
    run!("db", DbResolver);
    run!("cron", CronResolver);
    run!("config", ConfigResolver);
    run!("iac", IacResolver);
    run!("package", PackageResolver);
}
