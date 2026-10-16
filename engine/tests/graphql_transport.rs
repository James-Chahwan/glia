//! DT.11b end to end: a GraphQL client's transport is an HTTP call.
//!
//! Two roots shaped like the `xcut-graphql-transport` bench fixture (and
//! examples-glia's admin client + api server): a TypeScript client whose
//! Apollo client is built on `http://localhost:8080/graphql`, with the
//! canonical graphql-ws subscription client (`createClient({ url: 'ws://..'
//! })`) in the same file and a Redis `createClient({ url })` in another, and
//! a gin + gqlgen server mounting `POST /graphql` and `GET
//! /graphql/playground`.
//!
//! The Apollo client mints `endpoint:POST:/graphql` (ENDPOINT_HIT `"via":
//! "graphql"`, host `localhost:8080`), CALLed from the STATE_VAR `client`;
//! HttpStackResolver pairs it with `POST /graphql` and nothing else. The
//! graphql-ws client dials a WebSocket and the Redis client is no GraphQL
//! client: neither mints an ENDPOINT. The server's SDL root field `items`
//! takes a multi-line argument list whose lines are no resolvers (the DT.1j
//! handoff).

use std::path::Path;

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::{GenerateResult, generate_many};

const CLIENT_PACKAGE: &str = "{ \"name\": \"client\", \"dependencies\": { \"@apollo/client\": \"^3.10.0\", \"graphql\": \"^16.8.1\", \"graphql-ws\": \"^5.16.0\", \"redis\": \"^4.6.0\" } }\n";
const CLIENT_TS: &str = "import { ApolloClient, InMemoryCache, gql } from '@apollo/client/core';
import { createClient } from 'graphql-ws';

export const client = new ApolloClient({
  uri: 'http://localhost:8080/graphql',
  cache: new InMemoryCache(),
});

export const wsClient = createClient({ url: 'ws://localhost:8080/subscriptions' });

export const PRODUCTS = gql`
  query Products {
    products {
      id
    }
  }
`;
";
const CACHE_TS: &str = "import { createClient } from 'redis';

export const cache = createClient({ url: 'http://cache:6379/kv' });
";
const GO_MOD: &str = "module example.com/shop/server\n\ngo 1.22\n\nrequire (\n\tgithub.com/99designs/gqlgen v0.17.49\n\tgithub.com/gin-gonic/gin v1.10.0\n)\n";
const MAIN_GO: &str = "package main

import (
\t\"log\"

\t\"github.com/99designs/gqlgen/graphql/handler\"
\t\"github.com/gin-gonic/gin\"
)

func main() {
\tsrv := handler.NewDefaultServer(nil)
\tr := gin.Default()
\tr.POST(\"/graphql\", gin.WrapH(srv))
\tr.GET(\"/graphql/playground\", gin.WrapH(srv))
\tlog.Fatal(r.Run(\":8080\"))
}
";
const SCHEMA: &str = "type Product {
  id: ID!
}

type Query {
  products: [Product!]!
  items(
    first: Int
    after: String
  ): [Product!]!
}
";

const TRANSPORT: &str = "endpoint:POST:/graphql";

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

fn build() -> (tempfile::TempDir, GenerateResult) {
    let d = tempfile::tempdir().expect("tempdir");
    let client = d.path().join("client");
    let server = d.path().join("server");
    write(&client, "package.json", CLIENT_PACKAGE);
    write(&client, "src/client.ts", CLIENT_TS);
    write(&client, "src/cache.ts", CACHE_TS);
    write(&server, "go.mod", GO_MOD);
    write(&server, "main.go", MAIN_GO);
    write(&server, "graph/schema.graphqls", SCHEMA);
    let roots = [client, server].map(|p| p.to_string_lossy().into_owned());
    let r = generate_many(&roots).expect("build");
    (d, r)
}

fn qnames(r: &GenerateResult, kind: NodeKindId) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .filter_map(move |n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The one node of `kind` named `qname`.
fn node(r: &GenerateResult, kind: NodeKindId, qname: &str) -> NodeId {
    let mut hits: Vec<NodeId> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter(move |n| {
                g.nav.kind_by_id.get(&n.id) == Some(&kind)
                    && g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)
            })
        })
        .map(|n| n.id)
        .collect();
    hits.dedup();
    assert_eq!(hits.len(), 1, "one {qname}: {:?}", qnames(r, kind));
    hits[0]
}

fn qname_of(r: &GenerateResult, id: NodeId) -> Option<String> {
    r.merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
}

/// The qnames of every `category` edge's `from` into `to`, sorted.
fn sources(r: &GenerateResult, to: NodeId, category: EdgeCategoryId) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .all_edges()
        .filter(|e| e.category == category && e.to == to)
        .filter_map(|e| qname_of(r, e.from))
        .collect();
    out.sort();
    out
}

/// The qnames of every `category` edge's `to` out of `from`, sorted.
fn targets(r: &GenerateResult, from: NodeId, category: EdgeCategoryId) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .all_edges()
        .filter(|e| e.category == category && e.from == from)
        .filter_map(|e| qname_of(r, e.to))
        .collect();
    out.sort();
    out
}

fn endpoint_hits(r: &GenerateResult, id: NodeId) -> Vec<String> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| n.id == id)
        .flat_map(|n| &n.cells)
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) | CellPayload::Text(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn apollo_transport_pairs_the_graphql_route() {
    let (_d, r) = build();

    let endpoints = qnames(&r, node_kind::ENDPOINT);
    assert_eq!(
        endpoints,
        vec![TRANSPORT.to_string()],
        "graphql-ws and redis mint no ENDPOINT"
    );

    let ep = node(&r, node_kind::ENDPOINT, TRANSPORT);
    let hits = endpoint_hits(&r, ep);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].contains(r#""via":"graphql""#), "{}", hits[0]);
    assert!(
        hits[0].contains(r#""host":"localhost:8080""#),
        "{}",
        hits[0]
    );
    assert!(
        hits[0].contains(r#""file":"src/client.ts","line":5"#),
        "{}",
        hits[0]
    );

    assert_eq!(
        sources(&r, ep, edge_category::CALLS),
        vec!["src::client::client".to_string()],
        "the call-initialised STATE_VAR owns the transport"
    );
    assert_eq!(
        targets(&r, ep, edge_category::HTTP_CALLS),
        vec!["POST /graphql".to_string()],
        "method + path: never GET /graphql/playground"
    );

    // The CALLS edge is located at the URL.
    let calls = r
        .merged
        .all_edges()
        .find(|e| e.category == edge_category::CALLS && e.to == ep)
        .expect("CALLS");
    let ev = glia_code_domain::evidence::Evidence::of(calls).expect("evidence");
    assert_eq!(ev.emitter, "extractor:graphql");
    assert_eq!(ev.rule.as_deref(), Some("graphql_transport"));
    assert_eq!(
        (ev.file.as_deref(), ev.line),
        (Some("src/client.ts"), Some(4))
    );

    // Control: the operation still pairs its resolver.
    let op = node(&r, node_kind::GRAPHQL_OPERATION, "graphql_op:Products");
    assert_eq!(
        targets(&r, op, edge_category::GRAPHQL_CALLS),
        vec!["graphql_resolver:products".to_string()]
    );
}

#[test]
fn multiline_sdl_arguments_mint_no_resolver() {
    let (_d, r) = build();
    assert_eq!(
        qnames(&r, node_kind::GRAPHQL_RESOLVER),
        vec![
            "graphql_resolver:Query".to_string(),
            "graphql_resolver:items".to_string(),
            "graphql_resolver:products".to_string(),
        ]
    );
}
