use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

/// One `rpc` declaration inside a proto `service` block.
pub struct ProtoRpc {
    pub name: String,
    pub request: String,
    pub response: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    /// 0-indexed source line of the `rpc` keyword (same convention as
    /// `repo_graph_docs::position_json`).
    pub line: u32,
}

pub struct GrpcService {
    pub from: NodeId,
    pub service_name: String,
    /// Bare rpc names, kept for the INTENT text and existing callers.
    pub methods: Vec<String>,
    pub rpcs: Vec<ProtoRpc>,
    /// 0-indexed `service` keyword line / closing-brace line.
    pub start_line: u32,
    pub end_line: u32,
}

/// Everything a `.proto` file declares that the graph cares about.
pub struct ProtoFile {
    pub package: Option<String>,
    pub go_package: Option<String>,
    pub java_package: Option<String>,
    pub csharp_namespace: Option<String>,
    pub services: Vec<GrpcService>,
    pub line_count: u32,
}

/// The `option k = "v";` keys worth carrying onto the service node — they are
/// the evidence that pairs a proto service with the generated stub in a repo
/// that ships no `.proto` of its own.
const KEPT_OPTIONS: &[&str] = &["go_package", "java_package", "csharp_namespace"];

/// Drop a `// …` trailing comment, ignoring `//` inside a quoted option value.
fn strip_line_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => in_str = !in_str,
            b'/' if !in_str && i + 1 < b.len() && b[i + 1] == b'/' => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

/// `stream Foo` → `(true, "Foo")`; `Foo` → `(false, "Foo")`.
fn split_stream(raw: &str) -> (bool, String) {
    let t = raw.trim();
    match t.strip_prefix("stream ") {
        Some(rest) => (true, rest.trim().to_string()),
        None => (false, t.to_string()),
    }
}

fn parse_rpc_decl(trimmed: &str, line: u32) -> Option<ProtoRpc> {
    let rest = trimmed.strip_prefix("rpc ")?;
    let (name, after_name) = match rest.find('(') {
        Some(i) => (rest[..i].trim(), &rest[i + 1..]),
        // `rpc Foo;` with no signature — still a declared method.
        None => (rest.trim().trim_end_matches(';').trim(), ""),
    };
    if name.is_empty() {
        return None;
    }
    let mut request = String::new();
    let mut response = String::new();
    let mut client_streaming = false;
    let mut server_streaming = false;
    let mut after_req = "";
    if let Some(end) = after_name.find(')') {
        let (cs, r) = split_stream(&after_name[..end]);
        client_streaming = cs;
        request = r;
        after_req = &after_name[end + 1..];
    }
    if let Some(rp) = after_req.find("returns") {
        let tail = &after_req[rp..];
        if let Some(open) = tail.find('(') {
            let inner = &tail[open + 1..];
            if let Some(end) = inner.find(')') {
                let (ss, r) = split_stream(&inner[..end]);
                server_streaming = ss;
                response = r;
            }
        }
    }
    Some(ProtoRpc {
        name: name.to_string(),
        request,
        response,
        client_streaming,
        server_streaming,
        line,
    })
}

/// Line-aware `.proto` reader: package, the three generated-namespace options,
/// and every `service` with its `rpc`s and its source span. Parsers extract —
/// the graph crate turns this into nodes/edges (see `extract_grpc_service_nodes`).
pub fn parse_proto(source: &str, from: NodeId) -> ProtoFile {
    let mut out = ProtoFile {
        package: None,
        go_package: None,
        java_package: None,
        csharp_namespace: None,
        services: Vec::new(),
        line_count: 0,
    };
    let mut current: Option<GrpcService> = None;
    let mut depth: i32 = 0;
    let mut opened = false;
    let mut last_line: u32 = 0;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx as u32;
        last_line = line_no;
        out.line_count = line_no + 1;
        let code = strip_line_comment(raw_line);
        let trimmed = code.trim();

        if let Some(rest) = trimmed.strip_prefix("package ") {
            let name = rest.trim().trim_end_matches(';').trim();
            if !name.is_empty() && out.package.is_none() {
                out.package = Some(name.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("option ")
            && let Some((k, v)) = rest.split_once('=')
        {
            let key = k.trim();
            let val = v.trim().trim_end_matches(';').trim().trim_matches('"');
            if KEPT_OPTIONS.contains(&key) && !val.is_empty() {
                match key {
                    "go_package" => out.go_package = Some(val.to_string()),
                    "java_package" => out.java_package = Some(val.to_string()),
                    _ => out.csharp_namespace = Some(val.to_string()),
                }
            }
        }

        if let Some(rest) = trimmed.strip_prefix("service ") {
            // An unterminated previous service ends on the line before this one.
            if let Some(mut prev) = current.take() {
                prev.end_line = line_no.saturating_sub(1);
                out.services.push(prev);
            }
            let name = rest.split('{').next().unwrap_or("").trim();
            if !name.is_empty() {
                current = Some(GrpcService {
                    from,
                    service_name: name.to_string(),
                    methods: Vec::new(),
                    rpcs: Vec::new(),
                    start_line: line_no,
                    end_line: line_no,
                });
                depth = 0;
                opened = false;
            }
        } else if current.is_some()
            && let Some(rpc) = parse_rpc_decl(trimmed, line_no)
        {
            if let Some(svc) = current.as_mut() {
                svc.methods.push(rpc.name.clone());
                svc.rpcs.push(rpc);
            }
        }

        if current.is_some() {
            for b in code.bytes() {
                match b {
                    b'{' => {
                        depth += 1;
                        opened = true;
                    }
                    b'}' => depth -= 1,
                    _ => {}
                }
            }
            if opened && depth <= 0 {
                if let Some(mut svc) = current.take() {
                    svc.end_line = line_no;
                    out.services.push(svc);
                }
                depth = 0;
                opened = false;
            }
        }
    }

    if let Some(mut svc) = current.take() {
        svc.end_line = last_line;
        out.services.push(svc);
    }
    out
}

/// Back-compat shim: the services half of [`parse_proto`].
pub fn extract_grpc_from_proto(source: &str, from: NodeId) -> Vec<GrpcService> {
    parse_proto(source, from).services
}

pub struct GrpcNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

/// Everything one `.proto` contributes to the graph.
pub struct GrpcOut {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// Cells for the synthetic MODULE node the engine creates for the file.
    pub module_cells: Vec<Cell>,
    pub service_count: usize,
    pub rpc_count: usize,
    pub package: Option<String>,
}

fn json_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn position_cell(path: &str, start_line: u32, end_line: u32) -> Cell {
    Cell {
        kind: repo_graph_code_domain::cell_type::POSITION,
        payload: CellPayload::Json(format!(
            r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
            json_str(path),
            start_line,
            end_line
        )),
    }
}

pub fn extract_grpc_service_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> GrpcOut {
    let proto = parse_proto(source, module_id);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    let mut rpc_count = 0usize;

    // The RPC_PACKAGE cell is per-file, so build the payload once. Null-free:
    // a key absent from the .proto is absent from the JSON.
    let mut pkg_fields: Vec<String> = Vec::new();
    for (key, val) in [
        ("package", proto.package.as_deref()),
        ("go_package", proto.go_package.as_deref()),
        ("java_package", proto.java_package.as_deref()),
        ("csharp_namespace", proto.csharp_namespace.as_deref()),
    ] {
        if let Some(v) = val {
            pkg_fields.push(format!(r#""{}":"{}""#, key, json_str(v)));
        }
    }
    let pkg_payload = (!pkg_fields.is_empty()).then(|| format!("{{{}}}", pkg_fields.join(",")));

    for svc in &proto.services {
        // Package-qualified exactly as protobuf writes it (`.` inside the
        // proto-qualified name), so two `PaymentsService`s in different
        // packages stay distinguishable.
        let qname = match &proto.package {
            Some(pkg) => format!("grpc:{}.{}", pkg, svc.service_name),
            None => format!("grpc:{}", svc.service_name),
        };
        let svc_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRPC_SERVICE, &qname);
        let mut cells = vec![
            Cell {
                kind: repo_graph_code_domain::cell_type::INTENT,
                payload: CellPayload::Text(format!(
                    "gRPC service {} with {} methods",
                    svc.service_name,
                    svc.methods.len()
                )),
            },
            position_cell(path, svc.start_line, svc.end_line),
        ];
        if let Some(p) = &pkg_payload {
            cells.push(Cell {
                kind: repo_graph_code_domain::cell_type::RPC_PACKAGE,
                payload: CellPayload::Json(p.clone()),
            });
        }
        nodes.push(Node {
            id: svc_id,
            repo,
            confidence: Confidence::Strong,
            cells,
        });
        // nav `name` stays the bare service name: the qname carries the package.
        nav.record(
            svc_id,
            &svc.service_name,
            &qname,
            node_kind::GRPC_SERVICE,
            Some(module_id),
        );

        // One METHOD per rpc, in declaration order (never HashMap order — the
        // store's byte-identical gate depends on emission order).
        for rpc in &svc.rpcs {
            let m_qname = format!("{qname}::{}", rpc.name);
            let m_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &m_qname);
            let signature = format!(
                "rpc {}({}{}) returns ({}{})",
                rpc.name,
                if rpc.client_streaming { "stream " } else { "" },
                rpc.request,
                if rpc.server_streaming { "stream " } else { "" },
                rpc.response
            );
            nodes.push(Node {
                id: m_id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![
                    position_cell(path, rpc.line, rpc.line),
                    Cell {
                        kind: repo_graph_code_domain::cell_type::INTENT,
                        payload: CellPayload::Text(signature),
                    },
                ],
            });
            nav.record(m_id, &rpc.name, &m_qname, node_kind::METHOD, Some(svc_id));
            edges.push(Edge {
                from: svc_id,
                to: m_id,
                category: repo_graph_code_domain::edge_category::DEFINES,
                confidence: Confidence::Strong,
            });
            rpc_count += 1;
        }
    }

    let module_cells = vec![position_cell(
        path,
        0,
        proto.line_count.saturating_sub(1),
    )];

    GrpcOut {
        service_count: proto.services.len(),
        rpc_count,
        package: proto.package,
        nodes,
        edges,
        nav,
        module_cells,
    }
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
        let result = extract_grpc_service_nodes(source, "order.proto", module_id(), repo());
        // One GRPC_SERVICE + one METHOD per rpc.
        assert_eq!(result.nodes.len(), 2);
        assert_eq!(result.nav.kind_by_id[&result.nodes[0].id], node_kind::GRPC_SERVICE);
        assert_eq!(result.nav.kind_by_id[&result.nodes[1].id], node_kind::METHOD);
    }

    #[test]
    fn proto_service_is_package_qualified_positioned_and_has_rpc_methods() {
        let source = "syntax = \"proto3\";\n\npackage user;\n\noption go_package = \"example.com/proto/user\";\n\nservice UserService {\n  rpc GetUser (GetUserRequest) returns (User);\n  rpc Watch (WatchRequest) returns (stream Event);\n}\n";
        let result = extract_grpc_service_nodes(source, "server/user.proto", module_id(), repo());

        let svc_id = result.nodes[0].id;
        assert_eq!(
            result.nav.qname_by_id[&svc_id], "grpc:user.UserService",
            "package-qualified so two same-named services in different packages stay distinct"
        );
        // nav `name` stays bare: the package lives in the qname only.
        assert_eq!(result.nav.name_by_id[&svc_id], "UserService");

        let svc_cells = &result.nodes[0].cells;
        let pos = svc_cells
            .iter()
            .find(|c| c.kind == repo_graph_code_domain::cell_type::POSITION)
            .expect("service carries a POSITION cell");
        match &pos.payload {
            CellPayload::Json(j) => {
                assert!(j.contains("\"file\":\"server/user.proto\""), "got {j}");
                assert!(j.contains("\"start_line\":6"), "service keyword is line 6: {j}");
                assert!(j.contains("\"end_line\":9"), "closing brace is line 9: {j}");
            }
            other => panic!("POSITION must be Json, got {other:?}"),
        }
        let pkg = svc_cells
            .iter()
            .find(|c| c.kind == repo_graph_code_domain::cell_type::RPC_PACKAGE)
            .expect("service carries an RPC_PACKAGE cell");
        match &pkg.payload {
            CellPayload::Json(j) => {
                assert!(j.contains("\"package\":\"user\""), "got {j}");
                assert!(j.contains("\"go_package\":\"example.com/proto/user\""), "got {j}");
                assert!(!j.contains("java_package"), "absent options are omitted, not null: {j}");
            }
            other => panic!("RPC_PACKAGE must be Json, got {other:?}"),
        }

        let methods: Vec<&Node> = result
            .nodes
            .iter()
            .filter(|n| result.nav.kind_by_id[&n.id] == node_kind::METHOD)
            .collect();
        assert_eq!(methods.len(), 2, "one METHOD per rpc");
        assert_eq!(
            result.nav.qname_by_id[&methods[0].id],
            "grpc:user.UserService::GetUser"
        );
        assert_eq!(
            result.nav.qname_by_id[&methods[1].id],
            "grpc:user.UserService::Watch"
        );
        assert_eq!(result.nav.parent_of[&methods[0].id], svc_id);
        assert!(
            methods[0]
                .cells
                .iter()
                .any(|c| c.kind == repo_graph_code_domain::cell_type::POSITION),
            "each rpc METHOD is located at its declaration line"
        );
        assert!(
            methods[1].cells.iter().any(|c| matches!(
                &c.payload,
                CellPayload::Text(t) if t == "rpc Watch(WatchRequest) returns (stream Event)"
            )),
            "server-streaming is preserved in the INTENT signature"
        );

        let defines: Vec<&Edge> = result
            .edges
            .iter()
            .filter(|e| e.category == repo_graph_code_domain::edge_category::DEFINES)
            .collect();
        assert_eq!(defines.len(), 2, "service DEFINES each rpc METHOD");
        assert!(defines.iter().all(|e| e.from == svc_id));

        assert_eq!(result.service_count, 1);
        assert_eq!(result.rpc_count, 2);
        assert_eq!(result.package.as_deref(), Some("user"));
        assert_eq!(result.module_cells.len(), 1, "module gets a whole-file POSITION");
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
