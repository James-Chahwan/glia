use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

pub struct GrpcService {
    pub from: NodeId,
    pub service_name: String,
    pub methods: Vec<String>,
}

pub fn extract_grpc_from_proto(source: &str, from: NodeId) -> Vec<GrpcService> {
    let mut services = Vec::new();
    let mut current_service: Option<String> = None;
    let mut current_methods = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("service ") {
            if let Some(current) = current_service.take() {
                services.push(GrpcService {
                    from,
                    service_name: current,
                    methods: std::mem::take(&mut current_methods),
                });
            }
            let name = rest.split('{').next().unwrap_or("").trim();
            if !name.is_empty() {
                current_service = Some(name.to_string());
            }
        } else if trimmed.starts_with("rpc ") {
            let rest = trimmed.strip_prefix("rpc ").unwrap_or("");
            let method = rest.split('(').next().unwrap_or("").trim();
            if !method.is_empty() {
                current_methods.push(method.to_string());
            }
        }
    }

    if let Some(current) = current_service {
        services.push(GrpcService {
            from,
            service_name: current,
            methods: current_methods,
        });
    }

    services
}

pub struct GrpcNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

pub fn extract_grpc_service_nodes(source: &str, module_id: NodeId, repo: RepoId) -> GrpcNodes {
    let services = extract_grpc_from_proto(source, module_id);
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();

    for svc in &services {
        let qname = format!("grpc:{}", svc.service_name);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRPC_SERVICE, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: repo_graph_code_domain::cell_type::INTENT,
                payload: CellPayload::Text(format!(
                    "gRPC service {} with {} methods",
                    svc.service_name,
                    svc.methods.len()
                )),
            }],
        });
        nav.record(id, &svc.service_name, &qname, node_kind::GRPC_SERVICE, Some(module_id));
    }

    GrpcNodes { nodes, nav }
}

/// Idiomatic gRPC client construction patterns across languages. Each entry
/// is (needle, suffix-to-append-to-prefix) — the needle anchors the pattern,
/// and the suffix is what was consumed from the proto's service name. By
/// reconstructing `<prefix><suffix>` we recover the canonical `<Foo>Service`
/// (or `<Foo>Svc`) form that matches the proto-extracted service node.
///
/// Examples:
///   - Go     `pb.NewCartServiceClient(conn)` → needle `ServiceClient(`,
///            walks back from needle start to find `Cart`, drops leading `New`,
///            emits `CartService`.
///   - Python `cart_service_pb2_grpc.CartServiceStub(channel)` → needle
///            `ServiceStub(`, prefix `Cart`, emits `CartService`.
///   - Java   `CartServiceGrpc.newBlockingStub(channel)` → needle
///            `ServiceGrpc.newBlockingStub(`, prefix `Cart`, emits `CartService`.
///   - C#     `new Cart.CartServiceClient(channel)` → needle `ServiceClient(`,
///            prefix `Cart`, emits `CartService`.
///   - Node   `new pb.CartServiceClient(addr, ...)` → same as C#/Go shape.
const GRPC_CLIENT_PATTERNS: &[(&str, &str)] = &[
    ("ServiceClient(", "Service"),
    ("SvcClient(", "Svc"),
    ("ServiceStub(", "Service"),
    ("SvcStub(", "Svc"),
    ("ServiceGrpc.newBlockingStub(", "Service"),
    ("ServiceGrpc.newStub(", "Service"),
    ("ServiceGrpc.newFutureStub(", "Service"),
];

pub fn extract_grpc_client_nodes(source: &str, module_id: NodeId, repo: RepoId) -> GrpcNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();
    let bytes = source.as_bytes();

    for &(needle, suffix) in GRPC_CLIENT_PATTERNS {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            // Walk back from `pos` to find the identifier ending right at the
            // start of the needle. That identifier is the proto service name's
            // prefix (e.g. `Cart` from `pb.NewCart` + `ServiceClient(`).
            let mut start = pos;
            while start > 0
                && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
            {
                start -= 1;
            }
            let raw_prefix = &source[start..pos];
            // Drop the Go-idiom `New` prefix — `NewCart` → `Cart` so the
            // canonical reconstruction matches the proto declaration.
            let prefix = raw_prefix.strip_prefix("New").unwrap_or(raw_prefix);
            // Need at least one character for a meaningful service name.
            if prefix.is_empty() {
                search_from = pos + needle.len();
                continue;
            }
            let canonical = format!("{prefix}{suffix}");
            if !seen.insert(canonical.clone()) {
                search_from = pos + needle.len();
                continue;
            }
            let qname = format!("grpc_client:{canonical}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRPC_CLIENT, &qname);
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![],
            });
            nav.record(id, &canonical, &qname, node_kind::GRPC_CLIENT, Some(module_id));
            search_from = pos + needle.len();
        }
    }

    GrpcNodes { nodes, nav }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn parses_proto_service() {
        let source = r#"
service UserService {
  rpc GetUser (GetUserRequest) returns (User);
  rpc ListUsers (ListUsersRequest) returns (ListUsersResponse);
}
"#;
        let services = extract_grpc_from_proto(source, module_id());
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].service_name, "UserService");
        assert_eq!(services[0].methods.len(), 2);
        assert_eq!(services[0].methods[0], "GetUser");
    }

    #[test]
    fn multiple_services() {
        let source = r#"
service Auth {
  rpc Login (LoginReq) returns (Token);
}

service Users {
  rpc Get (GetReq) returns (User);
}
"#;
        let services = extract_grpc_from_proto(source, module_id());
        assert_eq!(services.len(), 2);
    }

    #[test]
    fn service_nodes_from_proto() {
        let source = "service OrderService {\n  rpc Place (Req) returns (Resp);\n}";
        let result = extract_grpc_service_nodes(source, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nav.kind_by_id[&result.nodes[0].id], node_kind::GRPC_SERVICE);
    }

    #[test]
    fn client_nodes_from_code() {
        let source = "conn := grpc.Dial(addr)\nclient := pb.NewOrderServiceClient(conn)";
        let result = extract_grpc_client_nodes(source, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nav.kind_by_id[&result.nodes[0].id], node_kind::GRPC_CLIENT);
        let qname = result.nav.qname_by_id.values().next().unwrap();
        // Must reconstruct the canonical proto service name (`OrderService`),
        // not the bare prefix or `New`-prefixed variant.
        assert_eq!(qname, "grpc_client:OrderService");
    }

    #[test]
    fn client_extraction_reconstructs_canonical_name_across_languages() {
        // All four lines reference the same proto service `CartService`. The
        // resolver indexes services as `CartService`; client extraction must
        // emit the same canonical form for cross-edge matching to fire.
        let source = r#"
// Go
client := pb.NewCartServiceClient(conn)
// Python
stub = cart_pb2_grpc.CartServiceStub(channel)
// Java
var blocking = CartServiceGrpc.newBlockingStub(channel)
// C# / Node
var client2 = new pb.CartServiceClient(channel)
"#;
        let result = extract_grpc_client_nodes(source, module_id(), repo());
        let names: Vec<String> = result
            .nav
            .qname_by_id
            .values()
            .cloned()
            .collect();
        assert_eq!(names.len(), 1, "all four lines collapse onto CartService");
        assert_eq!(names[0], "grpc_client:CartService");
    }

    #[test]
    fn client_extraction_handles_svc_suffix() {
        let source = "client := pb.NewOrderSvcClient(conn)";
        let result = extract_grpc_client_nodes(source, module_id(), repo());
        let names: Vec<String> = result.nav.qname_by_id.values().cloned().collect();
        assert_eq!(names, vec!["grpc_client:OrderSvc".to_string()]);
    }

    #[test]
    fn client_extraction_does_not_match_unrelated_clients() {
        // `httpClient(...)`, `redisClient(...)`, `dbClient(...)` should not
        // emit gRPC client nodes — needle requires `Service` / `Svc` prefix.
        let source = r#"
let http = new HttpClient(config);
let redis = createRedisClient(opts);
let db = makeDbClient(uri);
"#;
        let result = extract_grpc_client_nodes(source, module_id(), repo());
        assert!(
            result.nodes.is_empty(),
            "non-Service-prefixed Client(...) calls must not match"
        );
    }
}
