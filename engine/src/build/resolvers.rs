//! Cross-graph resolver registration: every resolver a merged build runs, in
//! the order it runs them — which is also the order their cross edges land in.

use repo_graph_graph::{
    CliInvocationResolver, ConfigResolver, CronResolver, DbResolver, EventBusResolver,
    GraphQLStackResolver, GrpcStackResolver, HttpStackResolver, IacResolver, MergedGraph,
    MessageSchemaResolver, PackageResolver, QueueStackResolver, RpcStackResolver,
    SharedSchemaResolver, WebSocketStackResolver,
};

pub(super) fn run_all_resolvers(merged: &mut MergedGraph) {
    merged.run(&HttpStackResolver);
    merged.run(&GrpcStackResolver);
    merged.run(&RpcStackResolver);
    merged.run(&QueueStackResolver);
    merged.run(&GraphQLStackResolver);
    merged.run(&WebSocketStackResolver);
    merged.run(&EventBusResolver);
    merged.run(&SharedSchemaResolver);
    // A10.7 — MESSAGE_TYPE nodes joined across repos on the exact qname.
    merged.run(&MessageSchemaResolver);
    merged.run(&CliInvocationResolver);
    merged.run(&DbResolver);
    merged.run(&CronResolver);
    merged.run(&ConfigResolver);
    merged.run(&IacResolver);
    merged.run(&PackageResolver);
}
