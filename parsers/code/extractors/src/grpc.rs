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

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Start of the identifier that ends exactly at byte `pos`. Walks back over
/// ASCII identifier bytes only, so the result is always a char boundary.
fn ident_start(bytes: &[u8], pos: usize) -> usize {
    let mut start = pos;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    start
}

/// The canonical service names the suffix-convention patterns recover from
/// `source`, deduplicated, in emission order.
fn suffix_pattern_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bytes = source.as_bytes();

    for &(needle, suffix) in GRPC_CLIENT_PATTERNS {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            search_from = pos + needle.len();
            // The identifier ending right at the start of the needle is the
            // proto service name's prefix (e.g. `Cart` from `pb.NewCart` +
            // `ServiceClient(`).
            let raw_prefix = &source[ident_start(bytes, pos)..pos];
            // Drop the Go-idiom `New` prefix — `NewCart` → `Cart` so the
            // canonical reconstruction matches the proto declaration.
            let prefix = raw_prefix.strip_prefix("New").unwrap_or(raw_prefix);
            // Need at least one character for a meaningful service name.
            if prefix.is_empty() {
                continue;
            }
            let canonical = format!("{prefix}{suffix}");
            if seen.insert(canonical.clone()) {
                names.push(canonical);
            }
        }
    }
    names
}

fn push_client_node(
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
    name: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    let qname = format!("grpc_client:{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRPC_CLIENT, &qname);
    nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![],
    });
    nav.record(id, name, &qname, node_kind::GRPC_CLIENT, Some(module_id));
}

/// The suffix-convention client pass: recognises `<Foo>Service` / `<Foo>Svc`
/// stubs with no knowledge of the build's `.proto` files. Kept as the fallback
/// for repos whose contract lives outside the build; the data-driven pass is
/// [`extract_known_grpc_client_nodes`].
pub fn extract_grpc_client_nodes(source: &str, module_id: NodeId, repo: RepoId) -> GrpcNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for canonical in suffix_pattern_names(source) {
        push_client_node(&mut nodes, &mut nav, &canonical, module_id, repo);
    }
    GrpcNodes { nodes, nav }
}

/// One gRPC service a `.proto` in the build declares: the key the data-driven
/// client needles are generated from. `name` is the bare service name (what
/// generated stubs are named after); the rest is the declaring file's package
/// and generated-namespace options. Field order is the derived sort order, so
/// a sorted set is name-first and independent of walk order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProtoServiceRef {
    pub name: String,
    pub package: Option<String>,
    pub go_package: Option<String>,
    pub java_package: Option<String>,
    pub csharp_namespace: Option<String>,
}

/// Every service `source` (a `.proto`) declares, as [`ProtoServiceRef`]s.
/// Reads through [`parse_proto`]: there is one proto reader, not two.
pub fn proto_service_refs(source: &str) -> Vec<ProtoServiceRef> {
    // `from` only rides along on `GrpcService`; the refs never carry it.
    let ProtoFile {
        package,
        go_package,
        java_package,
        csharp_namespace,
        services,
        ..
    } = parse_proto(source, NodeId(0));
    services
        .into_iter()
        .map(|svc| ProtoServiceRef {
            name: svc.service_name,
            package: package.clone(),
            go_package: go_package.clone(),
            java_package: java_package.clone(),
            csharp_namespace: csharp_namespace.clone(),
        })
        .collect()
}

/// Evidence that a file speaks gRPC. The data-driven pass only runs in files
/// that carry one: without the gate, a proto service literally named `Http`
/// would mint a GRPC_CLIENT from every `new HttpClient(...)` in the repo.
const GRPC_CONTEXT_NEEDLES: &[&str] = &[
    // Go
    "google.golang.org/grpc",
    "grpc.Dial",
    "grpc.NewClient",
    // Java / Kotlin / Scala
    "io.grpc",
    // Node
    "@grpc/grpc-js",
    // C#
    "Grpc.Net.Client",
    "GrpcChannel",
    "Grpc.Core",
    // Python
    "import grpc",
    "grpc.aio",
    "grpc.insecure_channel",
    "_pb2_grpc",
    // Rust
    "tonic::",
    // Ruby
    "require 'grpc'",
    "require \"grpc\"",
    "_services_pb",
    // C++
    "grpcpp",
    // Dart
    "package:grpc/",
];

/// True when `source` carries any [`GRPC_CONTEXT_NEEDLES`] entry.
pub fn file_has_grpc_context(source: &str) -> bool {
    GRPC_CONTEXT_NEEDLES.iter().any(|n| source.contains(n))
}

/// What follows a service name where a generated client is constructed, across
/// the gRPC bindings. The service name itself comes from the build's `.proto`s.
const CLIENT_SUFFIXES: &[&str] = &[
    // Go `pb.NewGreeterClient(`, C# `new Greeter.GreeterClient(`, Node, Dart
    "Client(",
    // Python `helloworld_pb2_grpc.GreeterStub(`
    "Stub(",
    // Rust tonic `GreeterClient::new(` / `GreeterClient::connect(`
    "Client::new(",
    "Client::connect(",
    "Client.new(",
    // Java `GreeterGrpc.newBlockingStub(`
    "Grpc.newBlockingStub(",
    "Grpc.newStub(",
    "Grpc.newFutureStub(",
    // Scala (ScalaPB) `GreeterGrpc.blockingStub(`
    "Grpc.blockingStub(",
    "Grpc.stub(",
    // Ruby `Helloworld::Greeter::Stub.new(`
    "::Stub.new(",
    // C++ `Greeter::NewStub(`
    "::NewStub(",
];

/// The data-driven client pass: a GRPC_CLIENT for every `known` service whose
/// generated stub is constructed in `source`, whatever the service is named.
/// Returns ONLY the clients the suffix fallback ([`extract_grpc_client_nodes`])
/// does not already emit for this source, so running both passes over one file
/// yields each client once.
///
/// A hit needs the identifier that ends at the service name to be empty or the
/// Go `New`, so `pb.NewGreeterClient(`, `helloworld_pb2_grpc.GreeterStub(`,
/// `new Greeter.GreeterClient(` and `greeter_client::GreeterClient::connect(`
/// all match, while `NewLegacyPaymentsClient(` does not mint `Payments`. Files
/// without gRPC context ([`file_has_grpc_context`]) yield nothing.
pub fn extract_known_grpc_client_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
    known: &[ProtoServiceRef],
) -> GrpcNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    if known.is_empty() || !file_has_grpc_context(source) {
        return GrpcNodes { nodes, nav };
    }
    // The fallback's names count as already emitted: one node per client.
    let mut seen: std::collections::HashSet<String> =
        suffix_pattern_names(source).into_iter().collect();
    // Longest name first, then by name: a fixed order however `known` arrived.
    // Same-named services from different packages share one needle set.
    let mut names: Vec<&str> = known
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| !n.is_empty() && n.bytes().all(is_ident_byte))
        .collect();
    names.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    names.dedup();

    let bytes = source.as_bytes();
    for name in names {
        if seen.contains(name) {
            continue;
        }
        let hit = CLIENT_SUFFIXES.iter().any(|suffix| {
            let needle = format!("{name}{suffix}");
            let mut search_from = 0;
            while let Some(rel) = source[search_from..].find(&needle) {
                let pos = search_from + rel;
                search_from = pos + needle.len();
                let prefix = &source[ident_start(bytes, pos)..pos];
                if prefix.is_empty() || prefix == "New" {
                    return true;
                }
            }
            false
        });
        if hit {
            seen.insert(name.to_string());
            push_client_node(&mut nodes, &mut nav, name, module_id, repo);
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

    fn svc(name: &str) -> ProtoServiceRef {
        ProtoServiceRef {
            name: name.to_string(),
            package: None,
            go_package: None,
            java_package: None,
            csharp_namespace: None,
        }
    }

    fn client_qnames(out: &GrpcNodes) -> Vec<String> {
        out.nodes
            .iter()
            .map(|n| out.nav.qname_by_id[&n.id].clone())
            .collect()
    }

    #[test]
    fn data_driven_needle_matches_suffixless_service() {
        let source = "using Grpc.Net.Client;\nvar c = new Greeter.GreeterClient(channel);";
        let out = extract_known_grpc_client_nodes(source, module_id(), repo(), &[svc("Greeter")]);
        assert_eq!(client_qnames(&out), vec!["grpc_client:Greeter".to_string()]);
        let id = out.nodes[0].id;
        assert_eq!(out.nav.kind_by_id[&id], node_kind::GRPC_CLIENT);
        assert_eq!(out.nav.name_by_id[&id], "Greeter");
        assert_eq!(out.nav.parent_of[&id], module_id());
        // The suffix fallback alone is blind to it — that is the gap.
        assert!(extract_grpc_client_nodes(source, module_id(), repo()).nodes.is_empty());
    }

    #[test]
    fn data_driven_needle_requires_grpc_context() {
        let source = "var c = new Greeter.GreeterClient(channel);";
        let out = extract_known_grpc_client_nodes(source, module_id(), repo(), &[svc("Greeter")]);
        assert!(out.nodes.is_empty(), "no gRPC context in the file → no client");
        // A service literally named `Http` must not turn HttpClient into gRPC.
        let http = "let http = new HttpClient(config);";
        let out = extract_known_grpc_client_nodes(http, module_id(), repo(), &[svc("Http")]);
        assert!(out.nodes.is_empty());
    }

    #[test]
    fn data_driven_needle_does_not_match_longer_identifier() {
        let source = "import \"google.golang.org/grpc\"\nc := pb.NewLegacyPaymentsClient(conn)";
        let out = extract_known_grpc_client_nodes(source, module_id(), repo(), &[svc("Payments")]);
        assert!(out.nodes.is_empty(), "LegacyPayments is not Payments");
    }

    #[test]
    fn data_driven_needle_matches_each_binding_shape() {
        let cases = [
            "import \"google.golang.org/grpc\"\nc := pb.NewGreeterClient(conn)",
            "import grpc\nstub = helloworld_pb2_grpc.GreeterStub(channel)",
            "import io.grpc.ManagedChannel;\nvar s = GreeterGrpc.newBlockingStub(channel);",
            "use tonic::transport::Channel;\nlet c = greeter_client::GreeterClient::connect(addr).await?;",
            "use tonic::transport::Channel;\nlet c = GreeterClient::new(channel);",
            "require 'grpc'\nstub = Helloworld::Greeter::Stub.new(addr, creds)",
            "#include <grpcpp/grpcpp.h>\nauto stub = Greeter::NewStub(channel);",
        ];
        for source in cases {
            let out =
                extract_known_grpc_client_nodes(source, module_id(), repo(), &[svc("Greeter")]);
            assert_eq!(
                client_qnames(&out),
                vec!["grpc_client:Greeter".to_string()],
                "missed: {source}"
            );
        }
    }

    #[test]
    fn data_driven_pass_skips_what_the_fallback_already_emits() {
        // `OrderService` is both a known proto service and a suffix-convention
        // match: the fallback owns it, the data-driven pass must not repeat it.
        let source = "conn := grpc.Dial(addr)\nclient := pb.NewOrderServiceClient(conn)";
        let known = [svc("OrderService"), svc("Greeter")];
        let out = extract_known_grpc_client_nodes(source, module_id(), repo(), &known);
        assert!(out.nodes.is_empty(), "got {:?}", client_qnames(&out));
        assert_eq!(
            client_qnames(&extract_grpc_client_nodes(source, module_id(), repo())),
            vec!["grpc_client:OrderService".to_string()]
        );
    }

    #[test]
    fn data_driven_pass_is_order_independent_and_deduplicated() {
        let source = "import grpc\na = pb2_grpc.AuthStub(ch)\nb = pb2_grpc.UsersStub(ch)\nc = pb2_grpc.AuthStub(ch2)";
        let mut other_pkg = svc("Auth");
        other_pkg.package = Some("v2".to_string());
        let forward = [svc("Auth"), svc("Users"), other_pkg.clone()];
        let backward = [other_pkg, svc("Users"), svc("Auth")];
        let a = extract_known_grpc_client_nodes(source, module_id(), repo(), &forward);
        let b = extract_known_grpc_client_nodes(source, module_id(), repo(), &backward);
        let expected = vec!["grpc_client:Users".to_string(), "grpc_client:Auth".to_string()];
        assert_eq!(client_qnames(&a), expected, "one node per name, longest first");
        assert_eq!(client_qnames(&b), expected);
    }

    #[test]
    fn proto_service_refs_carry_package_and_options() {
        let source = "syntax = \"proto3\";\npackage helloworld;\noption csharp_namespace = \"GreeterApi\";\nservice Greeter {\n  rpc SayHello (HelloRequest) returns (HelloReply);\n}\nservice Health {}\n";
        let refs = proto_service_refs(source);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Greeter", "Health"]);
        assert!(refs.iter().all(|r| r.package.as_deref() == Some("helloworld")));
        assert!(refs.iter().all(|r| r.csharp_namespace.as_deref() == Some("GreeterApi")));
        assert!(refs.iter().all(|r| r.go_package.is_none() && r.java_package.is_none()));
    }
}
