//! DF.10 end to end: an external WebSocket / gRPC / GraphQL client stays out
//! of pairing (DF.9) and says so, as CG.4b's external HTTP endpoints do.
//!
//! Two roots shaped like the `external-channel-clients` and
//! `grpc-external-dial` bench fixtures. The TS one is a workspace holding a
//! web app (`apps/web`) whose Apollo client sits on
//! `https://api.github.com/graphql` and whose WebSocket clients dial
//! `wss://stream.binance.com/ws` and `ws://localhost:8080/feed`, and a users
//! service (`services/users`) serving the `/ws` and `/feed` sockets and the
//! `getUser` resolver. The Go one dials `geocoder.googleapis.com:443` with
//! the stub of a service the repo also serves.
//!
//! DF.8 marks the public sites `"external":true`, DF.9 leaves those clients
//! unpaired, and `tag_synthetic_provenance` stamps them ORIGIN `external`;
//! the localhost `/feed` client is the in-build control. `effects` labels
//! the external sinks with their hosts (`external_hosts`), a GraphQL hit's
//! `hosts` list included.

use std::path::Path;

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use glia_engine::effects::{EffectRow, Effects, EffectsArgs, effects};
use glia_engine::{GenerateResult, generate_one};

const ROOT_PACKAGE: &str =
    "{ \"name\": \"mono\", \"private\": true, \"workspaces\": [\"apps/*\", \"services/*\"] }\n";
const WEB_PACKAGE: &str = "{ \"name\": \"web\", \"dependencies\": { \"@apollo/client\": \"^3.0.0\" } }\n";
const GITHUB_TS: &str = "import { ApolloClient, InMemoryCache, gql } from \"@apollo/client\";

export const gh = new ApolloClient({ uri: \"https://api.github.com/graphql\", cache: new InMemoryCache() });

export function viewer() {
  return gh.query({ query: gql`query GetUser { getUser { login } }` });
}

export function ticker(): WebSocket {
  return new WebSocket(\"wss://stream.binance.com/ws\");
}

export function feed(): WebSocket {
  return new WebSocket(\"ws://localhost:8080/feed\");
}
";
const USERS_PACKAGE: &str =
    "{ \"name\": \"users\", \"dependencies\": { \"@nestjs/graphql\": \"^12.0.0\", \"ws\": \"^8.0.0\" } }\n";
const SOCKET_JS: &str = "const { WebSocketServer } = require(\"ws\");
const wss = new WebSocketServer({ path: \"/ws\", port: 8080 });
wss.on(\"connection\", (s) => s.send(\"hi\"));
const feed = new WebSocketServer({ path: \"/feed\", port: 8081 });
feed.on(\"connection\", (s) => s.send(\"f\"));
";
const USER_RESOLVER_TS: &str = "import { Resolver, Query } from \"@nestjs/graphql\";

@Resolver()
export class UserResolver {
  @Query(() => String)
  getUser() {
    return \"u\";
  }
}
";

const CLIENT_GO: &str = "package main

import (
\t\"context\"

\t\"google.golang.org/grpc\"
)

func Lookup() {
\tconn, _ := grpc.Dial(\"geocoder.googleapis.com:443\")
\tc := NewGeocoderClient(conn)
\t_, _ = c.Lookup(context.Background(), nil)
}
";
const GO_MOD: &str = "module example.com/geo\n\ngo 1.22\n";
const GEO_PROTO: &str = "syntax = \"proto3\";
package geo.v1;
service Geocoder {
  rpc Lookup (Req) returns (Resp);
}
message Req { string q = 1; }
message Resp { string r = 1; }
";
const SERVER_GO: &str = "package main

import \"google.golang.org/grpc\"

type geocoderServer struct{ UnimplementedGeocoderServer }

func main() {
\ts := grpc.NewServer()
\tRegisterGeocoderServer(s, &geocoderServer{})
}
";

const WS_EXT: &str = "ws_client:/ws @apps/web";
const WS_FEED: &str = "ws_client:/feed @apps/web";
const GQL_EXT: &str = "graphql_op:GetUser @apps/web";
const GRPC_EXT: &str = "grpc_client:Geocoder";
const EXTERNAL: &str = r#"{"provenance":"external"}"#;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

fn channels() -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    let root = d.path();
    write(root, "package.json", ROOT_PACKAGE);
    write(root, "apps/web/package.json", WEB_PACKAGE);
    write(root, "apps/web/src/github.ts", GITHUB_TS);
    write(root, "services/users/package.json", USERS_PACKAGE);
    write(root, "services/users/src/socket.js", SOCKET_JS);
    write(root, "services/users/src/user.resolver.ts", USER_RESOLVER_TS);
    d
}

fn grpc_dial() -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    let root = d.path();
    write(root, "client/main.go", CLIENT_GO);
    write(root, "go.mod", GO_MOD);
    write(root, "proto/geo.proto", GEO_PROTO);
    write(root, "server/main.go", SERVER_GO);
    d
}

fn build(root: &Path) -> GenerateResult {
    generate_one(&root.to_string_lossy()).expect("build")
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
    assert_eq!(hits.len(), 1, "one {qname}: {:?}", all_qnames(r, kind));
    hits[0]
}

fn all_qnames(r: &GenerateResult, kind: NodeKindId) -> Vec<String> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .filter_map(move |n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect()
}

/// The `category` targets of `from`, by qname, sorted.
fn targets(r: &GenerateResult, from: NodeId, category: EdgeCategoryId) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .all_edges()
        .filter(|e| e.category == category && e.from == from)
        .filter_map(|e| {
            r.merged
                .graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&e.to).cloned())
        })
        .collect();
    out.sort();
    out
}

/// The payloads of `cell` on every entry of `id`, as text.
fn payloads(r: &GenerateResult, id: NodeId, cell: glia_core::CellTypeId) -> Vec<String> {
    r.merged
        .graphs
        .iter()
        .flat_map(|g| &g.nodes)
        .filter(|n| n.id == id)
        .flat_map(|n| &n.cells)
        .filter(|c| c.kind == cell)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) | CellPayload::Text(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

/// Every ENDPOINT_HIT of `id` is marked external, and there is one.
fn all_marked(r: &GenerateResult, id: NodeId) -> bool {
    let hits = payloads(r, id, cell_type::ENDPOINT_HIT);
    !hits.is_empty() && hits.iter().all(|h| h.contains("\"external\":true"))
}

/// The ORIGIN payloads of `id` are `external` on every entry, and there is one.
fn stamped_external(r: &GenerateResult, id: NodeId) -> bool {
    let origins = payloads(r, id, cell_type::ORIGIN);
    !origins.is_empty() && origins.iter().all(|o| o == EXTERNAL)
}

#[test]
fn ws_and_graphql_clients_are_external_and_unpaired() {
    let d = channels();
    let r = build(d.path());

    let ws = node(&r, node_kind::WS_CLIENT, WS_EXT);
    assert!(all_marked(&r, ws), "{:?}", payloads(&r, ws, cell_type::ENDPOINT_HIT));
    assert!(stamped_external(&r, ws), "{:?}", payloads(&r, ws, cell_type::ORIGIN));
    assert!(
        targets(&r, ws, edge_category::WS_CONNECTS).is_empty(),
        "binance's /ws is not this repo's /ws"
    );

    let gql = node(&r, node_kind::GRAPHQL_OPERATION, GQL_EXT);
    assert!(all_marked(&r, gql), "{:?}", payloads(&r, gql, cell_type::ENDPOINT_HIT));
    assert!(stamped_external(&r, gql), "{:?}", payloads(&r, gql, cell_type::ORIGIN));
    assert!(
        targets(&r, gql, edge_category::GRAPHQL_CALLS).is_empty(),
        "GitHub's getUser is not this repo's resolver"
    );

    // Control: the localhost client pairs and carries no ORIGIN.
    let feed = node(&r, node_kind::WS_CLIENT, WS_FEED);
    assert_eq!(
        targets(&r, feed, edge_category::WS_CONNECTS),
        vec!["ws:/feed @services/users".to_string()]
    );
    assert!(payloads(&r, feed, cell_type::ORIGIN).is_empty());
}

#[test]
fn grpc_client_is_external() {
    let d = grpc_dial();
    let r = build(d.path());

    let stub = node(&r, node_kind::GRPC_CLIENT, GRPC_EXT);
    assert!(all_marked(&r, stub), "{:?}", payloads(&r, stub, cell_type::ENDPOINT_HIT));
    assert!(stamped_external(&r, stub), "{:?}", payloads(&r, stub, cell_type::ORIGIN));
    assert!(
        targets(&r, stub, edge_category::GRPC_CALLS).is_empty(),
        "a dial at googleapis is not the repo's own Geocoder"
    );
}

fn effects_of(r: &GenerateResult, seed: &str) -> Effects {
    effects(&r.merged, &r.repo_labels, &[seed], &EffectsArgs::default()).expect("effects")
}

/// The one row of `a` whose sink is `qname`.
fn sink<'a>(a: &'a Effects, qname: &str) -> &'a EffectRow {
    let rows: Vec<&EffectRow> = a.effects.iter().filter(|r| r.qname == qname).collect();
    assert_eq!(rows.len(), 1, "one {qname}: {a:#?}");
    rows[0]
}

/// CJ.3's label on the channel sinks: the GraphQL op names the host of its
/// client's `hosts` list, the WebSocket client its dialled host, the gRPC
/// stub its dial target; none has a receiver. The localhost control names no
/// host and keeps its handler.
#[test]
fn effects_labels_external_channel_sinks() {
    let d = channels();
    let r = build(d.path());

    let viewer = effects_of(&r, "apps::web::src::github::viewer");
    let row = sink(&viewer, GQL_EXT);
    assert_eq!(row.class, "graphql_op");
    assert_eq!(row.external_hosts, vec!["api.github.com".to_string()]);
    assert!(row.downstream.is_empty(), "nothing pairs it: {row:#?}");

    let ticker = effects_of(&r, "apps::web::src::github::ticker");
    let row = sink(&ticker, WS_EXT);
    assert_eq!(row.class, "ws_send");
    assert_eq!(row.external_hosts, vec!["stream.binance.com".to_string()]);
    assert!(row.downstream.is_empty(), "nothing pairs it: {row:#?}");

    let feed = effects_of(&r, "apps::web::src::github::feed");
    let row = sink(&feed, WS_FEED);
    assert!(row.external_hosts.is_empty(), "{row:#?}");
    let receivers: Vec<&str> = row.downstream.iter().map(|t| t.qname.as_str()).collect();
    assert_eq!(receivers, ["ws:/feed @services/users"]);

    let d = grpc_dial();
    let r = build(d.path());
    let lookup = effects_of(&r, "client::main::Lookup");
    let row = sink(&lookup, GRPC_EXT);
    assert_eq!(row.class, "rpc_call");
    assert_eq!(row.external_hosts, vec!["geocoder.googleapis.com:443".to_string()]);
    assert!(row.downstream.is_empty(), "nothing pairs it: {row:#?}");
}
