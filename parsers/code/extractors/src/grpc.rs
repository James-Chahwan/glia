use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

use crate::anchor::{self, Anchor, line_of};

/// One `rpc` declaration inside a proto `service` block.
pub struct ProtoRpc {
    pub name: String,
    pub request: String,
    pub response: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    /// 0-indexed source line of the `rpc` keyword (same convention as
    /// `glia_doc::position_json`).
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
    /// A5.8: one anchor per stub construction site (see `crate::anchor`).
    /// The engine attaches them after the data-driven pass too, so both
    /// client passes end up located and owned.
    pub anchors: Vec<Anchor>,
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
        kind: glia_code_domain::cell_type::POSITION,
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
                kind: glia_code_domain::cell_type::INTENT,
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
                kind: glia_code_domain::cell_type::RPC_PACKAGE,
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
                        kind: glia_code_domain::cell_type::INTENT,
                        payload: CellPayload::Text(signature),
                    },
                ],
            });
            nav.record(m_id, &rpc.name, &m_qname, node_kind::METHOD, Some(svc_id));
            edges.push(Edge {
                from: svc_id,
                to: m_id,
                category: glia_code_domain::edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
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
    suffix_pattern_hits(source)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// [`suffix_pattern_names`] with every construction site: each name once, in
/// emission order, with the byte offsets of all the needles that recovered it.
fn suffix_pattern_hits(source: &str) -> Vec<(String, Vec<usize>)> {
    let mut names: Vec<(String, Vec<usize>)> = Vec::new();
    let mut slot: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
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
            match slot.get(&canonical) {
                Some(&i) => names[i].1.push(pos),
                None => {
                    slot.insert(canonical.clone(), names.len());
                    names.push((canonical, vec![pos]));
                }
            }
        }
    }
    names
}

/// Push one GRPC_CLIENT and an anchor for each of its construction `sites`
/// (byte offsets into `source`). Both client passes go through here.
fn push_client_node(
    out: &mut GrpcNodes,
    source: &str,
    sites: &[usize],
    name: &str,
    module_id: NodeId,
    repo: RepoId,
    evidence: Option<&Cell>,
) {
    let qname = format!("grpc_client:{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRPC_CLIENT, &qname);
    out.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Medium,
        cells: evidence.cloned().into_iter().collect(),
    });
    out.nav
        .record(id, name, &qname, node_kind::GRPC_CLIENT, Some(module_id));
    out.anchors.extend(sites.iter().map(|&at| Anchor {
        node: id,
        line: line_of(source, at),
    }));
}

/// How far into a client file the package-evidence scan reads. Every binding
/// the client passes recognise puts its imports at the top; the bound keeps a
/// `from … import` or `use …` deep in a function body out of the evidence.
const EVIDENCE_SCAN_LINES: usize = 120;
/// At most this many import paths ride on one client's evidence cell.
const EVIDENCE_MAX_PATHS: usize = 32;
/// Longer "paths" are not import paths; they are skipped, not truncated.
const EVIDENCE_MAX_PATH_LEN: usize = 200;

/// The text between the first quote (`"`, `'` or a backtick) in `s` and its
/// closing twin.
fn first_quoted(s: &str) -> Option<&str> {
    let start = s.find(['"', '\'', '`'])?;
    let quote = s.as_bytes()[start] as char;
    let rest = &s[start + 1..];
    rest.find(quote).map(|end| &rest[..end])
}

/// A quoted string that starts `s` (after leading whitespace), and nothing else:
/// `from './x'` qualifies, `from users where` does not.
fn leading_quoted(s: &str) -> Option<&str> {
    let t = s.trim_start();
    t.starts_with(['"', '\'', '`']).then(|| first_quoted(t)).flatten()
}

/// An unquoted import path: up to the first whitespace or `; { ( , =`, minus a
/// trailing wildcard (`.*` `._` `::*` `\*`) and dangling separators.
fn bare_path(s: &str) -> &str {
    let end = s
        .find(|c: char| c.is_whitespace() || matches!(c, ';' | '{' | '(' | ',' | '='))
        .unwrap_or(s.len());
    let mut p = &s[..end];
    for wildcard in [".*", "._", "::*", "\\*"] {
        if let Some(stripped) = p.strip_suffix(wildcard) {
            p = stripped;
            break;
        }
    }
    p.trim_end_matches(['.', ':', '\\'])
}

/// `line` minus the keyword `kw`, when `kw` is a whole leading word.
fn after_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    rest.starts_with(char::is_whitespace).then(|| rest.trim_start())
}

/// The import path one head-of-file line names, if any. `in_go_block` carries
/// Go's parenthesised `import ( … )` across lines.
fn import_path_of(line: &str, in_go_block: &mut bool) -> Option<String> {
    if *in_go_block {
        if line.starts_with(')') {
            *in_go_block = false;
            return None;
        }
        return first_quoted(line).map(str::to_string);
    }
    // Go `import (` opens a block; JS `import('x')` has a quote after the paren.
    if line.strip_prefix("import").map(str::trim) == Some("(") {
        *in_go_block = true;
        return None;
    }
    // Rust tonic `tonic::include_proto!("billing")` names the proto package itself;
    // JS `const pb = require('./gen/billing/x')`; TS `} from './gen/billing/x';`.
    for marker in ["include_proto!(", "require(", " from "] {
        if let Some(i) = line.find(marker)
            && let Some(q) = leading_quoted(&line[i + marker.len()..])
        {
            return Some(q.to_string());
        }
    }
    let path = if let Some(rest) = after_keyword(line, "import") {
        // Go / TS / Dart quote the path; Java, Kotlin, Scala, Swift, Python do not.
        match first_quoted(rest) {
            Some(q) => q,
            None => bare_path(rest.strip_prefix("static ").unwrap_or(rest).trim_start()),
        }
    } else if let Some(rest) = after_keyword(line, "from") {
        // Python `from gen.billing import payments_pb2_grpc`.
        leading_quoted(rest).or_else(|| rest.split_once(" import").map(|(m, _)| m.trim()))?
    } else if let Some(rest) = after_keyword(line, "using")
        .or_else(|| after_keyword(line, "global").and_then(|r| after_keyword(r, "using")))
    {
        // C# directive, never the `using (…)` / `using var x = …` statements.
        if rest.starts_with('(') || after_keyword(rest, "var").is_some() {
            return None;
        }
        let rest = rest.strip_prefix("static ").unwrap_or(rest);
        // `using Alias = Some.Namespace;` names the namespace after the `=`.
        let rest = rest.split_once('=').map_or(rest, |(_, target)| target.trim_start());
        bare_path(rest)
    } else if let Some(rest) = after_keyword(line, "use") {
        // Rust `use billing::client::X;`, PHP `use Billing\X;`.
        bare_path(rest)
    } else if let Some(rest) = line.strip_prefix("#include").or_else(|| line.strip_prefix("# include")) {
        let rest = rest.trim_start();
        match rest.strip_prefix('<') {
            Some(angled) => angled.split_once('>').map(|(p, _)| p)?,
            None => first_quoted(rest)?,
        }
    } else if line.starts_with("require") {
        // Ruby `require 'billing/payments_services_pb'` / `require_relative`.
        first_quoted(line)?
    } else {
        return None;
    };
    Some(path.to_string())
}

/// Every import-like path in the head of `source`, in source order, deduplicated:
/// Go `pb "example.com/gen/billing"` (single or block form), C# `using GreeterApi;`,
/// Java/Kotlin/Scala `import shop.billing.*;`, Python `from gen.billing import x`,
/// TS/JS/Dart `from './gen/billing/x'` / `require('x')`, Ruby `require 'x'`,
/// Rust `use billing::x;` / `include_proto!("billing")`, PHP `use Billing\X;`,
/// C++ `#include "billing/x.grpc.pb.h"`.
///
/// No package knowledge lives here: which of these paths (if any) names a proto
/// package is decided by the graph crate's gRPC resolver, against the build's
/// whole service set. Parsers extract; the graph crate resolves (A5.4).
pub fn client_package_evidence(source: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_go_block = false;
    for raw in source.lines().take(EVIDENCE_SCAN_LINES) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
            continue;
        }
        let Some(path) = import_path_of(line, &mut in_go_block) else { continue };
        if path.is_empty() || path.len() > EVIDENCE_MAX_PATH_LEN || out.contains(&path) {
            continue;
        }
        out.push(path);
        if out.len() == EVIDENCE_MAX_PATHS {
            break;
        }
    }
    out
}

/// The RPC_PACKAGE cell a GRPC_CLIENT carries: `{"imports":[…]}`, the package
/// evidence its file names. `None` when the file names nothing — the cell is
/// null-free, like the service's.
fn client_evidence_cell(source: &str) -> Option<Cell> {
    let imports = client_package_evidence(source);
    if imports.is_empty() {
        return None;
    }
    Some(Cell {
        kind: glia_code_domain::cell_type::RPC_PACKAGE,
        payload: CellPayload::Json(serde_json::json!({ "imports": imports }).to_string()),
    })
}

/// A decoded RPC_PACKAGE cell. A GRPC_SERVICE carries the declaring `.proto`'s
/// `package` and generated-namespace options; a GRPC_CLIENT carries `imports`
/// (see [`client_package_evidence`]). Absent keys decode as `None` / empty, and
/// unknown keys are ignored, so either shape decodes through this one type.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct RpcPackageCell {
    pub package: Option<String>,
    pub go_package: Option<String>,
    pub java_package: Option<String>,
    pub csharp_namespace: Option<String>,
    pub imports: Vec<String>,
}

impl RpcPackageCell {
    /// `None` for a payload that is not a JSON object.
    pub fn parse(payload: &str) -> Option<Self> {
        serde_json::from_str(payload).ok()
    }
}

/// The suffix-convention client pass: recognises `<Foo>Service` / `<Foo>Svc`
/// stubs with no knowledge of the build's `.proto` files. Kept as the fallback
/// for repos whose contract lives outside the build; the data-driven pass is
/// [`extract_known_grpc_client_nodes`].
///
/// Each client carries its file's package evidence ([`client_evidence_cell`]),
/// which the resolver uses to pick one service when a bare name is declared in
/// more than one proto package (A5.4).
pub fn extract_grpc_client_nodes(source: &str, module_id: NodeId, repo: RepoId) -> GrpcNodes {
    let mut out = GrpcNodes {
        nodes: Vec::new(),
        nav: CodeNav::default(),
        anchors: Vec::new(),
    };
    let hits = suffix_pattern_hits(source);
    if hits.is_empty() {
        return out;
    }
    let evidence = client_evidence_cell(source);
    for (canonical, sites) in hits {
        push_client_node(
            &mut out,
            source,
            &sites,
            &canonical,
            module_id,
            repo,
            evidence.as_ref(),
        );
    }
    out
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
    /// The service's `rpc` names in declaration order (A5.3): the server pass
    /// reads them to find the methods that implement the service.
    pub rpcs: Vec<String>,
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
            rpcs: svc.methods,
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
    // ASP.NET Core client factory: a Program.cs holding only the registration
    // `AddGrpcClient<Greeter.GreeterClient>(…)` needs no `using Grpc.*` at all.
    "AddGrpcClient<",
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
    // C# client factory / DI: `services.AddGrpcClient<Greeter.GreeterClient>(`
    "Client>(",
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
    let mut out = GrpcNodes {
        nodes: Vec::new(),
        nav: CodeNav::default(),
        anchors: Vec::new(),
    };
    if known.is_empty() || !file_has_grpc_context(source) {
        return out;
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
    // Computed on the first hit only: most gRPC-context files mint no client here.
    let mut evidence: Option<Option<Cell>> = None;
    for name in names {
        if seen.contains(name) {
            continue;
        }
        // Every construction site of this service's stub, in offset order.
        let mut sites: Vec<usize> = Vec::new();
        for suffix in CLIENT_SUFFIXES {
            let needle = format!("{name}{suffix}");
            let mut search_from = 0;
            while let Some(rel) = source[search_from..].find(&needle) {
                let pos = search_from + rel;
                search_from = pos + needle.len();
                let prefix = &source[ident_start(bytes, pos)..pos];
                if prefix.is_empty() || prefix == "New" {
                    sites.push(pos);
                }
            }
        }
        if !sites.is_empty() {
            sites.sort_unstable();
            sites.dedup();
            seen.insert(name.to_string());
            let cell = evidence.get_or_insert_with(|| client_evidence_cell(source));
            push_client_node(&mut out, source, &sites, name, module_id, repo, cell.as_ref());
        }
    }
    out
}

// ---- A5.3: gRPC server-impl detection --------------------------------------

/// How a server needle ties a proto service to the code that serves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerShape {
    /// The needle sits in the header or body of the type that implements the
    /// service (`: Greeter.GreeterBase`, Go's embedded
    /// `pb.UnimplementedGreeterServer`): the implementing type is the innermost
    /// CLASS / STRUCT whose span holds the needle.
    Base,
    /// A registration call that binds some implementation into a server
    /// (`pb.RegisterGreeterServer(`, `add_GreeterServicer_to_server(`). It
    /// names no type.
    Register,
}

/// One server-side construction shape: `pre` + service name + `post`.
struct ServerNeedle {
    /// Literal before the service name. The byte before it must not be an
    /// identifier byte, so `mustEmbedUnimplementedGreeterServer` is no hit.
    pre: &'static str,
    /// Literal after the service name. When it ends in an identifier byte, the
    /// byte after it must not be one (`GreeterBase` yes, `GreeterBaseline` no).
    post: &'static str,
    shape: ServerShape,
    /// Specific to generated gRPC code, so it stands without the
    /// [`file_has_grpc_context`] gate. A servicer module whose only import is
    /// its own generated package still names its base type.
    strict: bool,
}

/// Server-side shapes, one or more per gRPC binding. The service name between
/// `pre` and `post` is never a guess: it must be a service the build's own
/// `.proto` files declare.
const SERVER_NEEDLES: &[ServerNeedle] = &[
    // Go: `type server struct { pb.UnimplementedGreeterServer }`
    ServerNeedle { pre: "Unimplemented", post: "Server", shape: ServerShape::Base, strict: true },
    // Java: `class GreeterImpl extends GreeterGrpc.GreeterImplBase`
    ServerNeedle { pre: "", post: "ImplBase", shape: ServerShape::Base, strict: true },
    // Kotlin (grpc-kotlin): `: GreeterGrpcKt.GreeterCoroutineImplBase()`
    ServerNeedle { pre: "", post: "CoroutineImplBase", shape: ServerShape::Base, strict: true },
    // Python: `class Greeter(greeter_pb2_grpc.GreeterServicer)`
    ServerNeedle { pre: "", post: "Servicer", shape: ServerShape::Base, strict: true },
    // C#: `class GreeterService : Greeter.GreeterBase`
    ServerNeedle { pre: "", post: "Base", shape: ServerShape::Base, strict: false },
    // C++ `: public Greeter::Service`, Ruby `< Helloworld::Greeter::Service`
    ServerNeedle { pre: "", post: "::Service", shape: ServerShape::Base, strict: false },
    // Dart: `class GreeterService extends GreeterServiceBase`
    ServerNeedle { pre: "", post: "ServiceBase", shape: ServerShape::Base, strict: false },
    // Go: `pb.RegisterGreeterServer(s, &server{})`
    ServerNeedle { pre: "Register", post: "Server(", shape: ServerShape::Register, strict: false },
    // Python: `greeter_pb2_grpc.add_GreeterServicer_to_server(Greeter(), server)`
    ServerNeedle { pre: "add_", post: "Servicer_to_server(", shape: ServerShape::Register, strict: true },
    // Rust tonic: `.add_service(GreeterServer::new(greeter))`
    ServerNeedle { pre: "", post: "Server::new(", shape: ServerShape::Register, strict: false },
    ServerNeedle { pre: "", post: "Server::with_interceptor(", shape: ServerShape::Register, strict: false },
    // Scala (ScalaPB): `GreeterGrpc.bindService(new GreeterImpl, ec)`
    ServerNeedle { pre: "", post: "Grpc.bindService(", shape: ServerShape::Register, strict: false },
];

/// Tokens every strict [`SERVER_NEEDLES`] entry contains. A file with neither
/// gRPC context nor one of these cannot hold a server: the engine skips it
/// before looking up its parse.
const STRICT_SERVER_TOKENS: &[&str] = &["Unimplemented", "ImplBase", "Servicer"];

/// The word before a needle that makes the hit a declaration of the generated
/// base itself (`type UnimplementedGreeterServer struct`, `class
/// GreeterServicer(object)`, `def add_GreeterServicer_to_server`), not a use.
const DECL_KEYWORDS: &[&str] = &[
    "class", "struct", "type", "func", "def", "interface", "trait", "fn", "enum", "object",
    "module", "record", "protocol",
];

/// Head-of-file banners protoc plugins write. Generated gRPC code declares
/// every base and registration function a server needle keys on, so a file
/// carrying one is never a server implementation. Matched case-folded.
const GENERATED_BANNERS: &[&str] = &[
    "do not edit",
    "do not modify",
    "<auto-generated",
    "@generated",
    "code generated by",
    "annotation.generated",
    "grpcgenerated",
];

/// How far into a file [`GENERATED_BANNERS`] are looked for.
const GENERATED_SCAN_LINES: usize = 40;

/// True when `source` carries a code-generator banner near its head.
pub fn is_generated_source(source: &str) -> bool {
    source.lines().take(GENERATED_SCAN_LINES).any(|line| {
        let lower = line.to_ascii_lowercase();
        GENERATED_BANNERS.iter().any(|b| lower.contains(b))
    })
}

/// Cheap pre-check for [`extract_grpc_server_nodes`]: false only when the file
/// can hold no server needle at all, so the caller may skip looking up its parse.
pub fn may_hold_grpc_server(source: &str) -> bool {
    file_has_grpc_context(source) || STRICT_SERVER_TOKENS.iter().any(|t| source.contains(t))
}

/// Where one server needle fired.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerHit {
    name: String,
    at: usize,
    shape: ServerShape,
    /// Rust `impl Greeter for MyGreeter`: the implementing type, by name. The
    /// impl block sits outside the struct's span, so the span lookup cannot
    /// find it.
    impl_for: Option<String>,
}

/// Start of the line holding byte `at`.
fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// True when the hit at `at` is inside a line comment (the line's code starts
/// with `//`, `#`, `*` or `/*`).
fn in_comment_line(source: &str, at: usize) -> bool {
    let head = source[line_start(source, at)..at].trim_start();
    head.starts_with("//") || head.starts_with('#') || head.starts_with('*') || head.starts_with("/*")
}

/// True when the identifier before `at` (whitespace skipped) is a declaration
/// keyword: the hit names the thing being declared.
fn follows_decl_keyword(source: &str, at: usize) -> bool {
    let bytes = source.as_bytes();
    let mut end = at;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == at {
        return false;
    }
    let word = &source[ident_start(bytes, end)..end];
    DECL_KEYWORDS.contains(&word)
}

/// Every `pre` + known name + `post` hit of `needle` in `source`, as
/// (name, byte offset of the needle start).
fn needle_hits(source: &str, needle: &ServerNeedle, known: &BTreeMap<&str, BTreeSet<String>>) -> Vec<(String, usize)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let post_ident_len = needle.post.bytes().take_while(|b| is_ident_byte(*b)).count();
    let (post_ident, post_rest) = needle.post.split_at(post_ident_len);
    let tail_is_ident = needle.post.bytes().last().is_some_and(is_ident_byte);
    if needle.pre.is_empty() {
        // Anchor on `post`; the name is the identifier run that ends where it starts.
        let mut from = 0;
        while let Some(rel) = source[from..].find(needle.post) {
            let pos = from + rel;
            from = pos + needle.post.len();
            let end = pos + needle.post.len();
            if tail_is_ident && bytes.get(end).is_some_and(|b| is_ident_byte(*b)) {
                continue;
            }
            let start = ident_start(bytes, pos);
            let name = &source[start..pos];
            if !name.is_empty() && known.contains_key(name) {
                out.push((name.to_string(), start));
            }
        }
    } else {
        // Anchor on `pre`; the name is the identifier run after it, minus the
        // identifier head of `post`, and the rest of `post` must follow.
        let mut from = 0;
        while let Some(rel) = source[from..].find(needle.pre) {
            let pos = from + rel;
            from = pos + needle.pre.len();
            if pos > 0 && is_ident_byte(bytes[pos - 1]) {
                continue;
            }
            let run_start = pos + needle.pre.len();
            let mut run_end = run_start;
            while run_end < bytes.len() && is_ident_byte(bytes[run_end]) {
                run_end += 1;
            }
            let Some(name) = source[run_start..run_end].strip_suffix(post_ident) else { continue };
            if name.is_empty() || !source[run_end..].starts_with(post_rest) {
                continue;
            }
            if known.contains_key(name) {
                out.push((name.to_string(), pos));
            }
        }
    }
    out
}

/// `RouteGuide` → `route_guide`: the module tonic generates a service's server
/// trait into (`route_guide_server::RouteGuide`).
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The last `::` segment of a Rust path, minus generic arguments.
fn last_path_segment(path: &str) -> &str {
    let seg = path.rsplit("::").next().unwrap_or(path);
    seg.split('<').next().unwrap_or(seg).trim()
}

/// Rust tonic `impl Greeter for MyGreeter {` (or `impl greeter_server::Greeter
/// for …`), only in files that name the generated `greeter_server` module.
fn rust_trait_impl_hits(source: &str, known: &BTreeMap<&str, BTreeSet<String>>) -> Vec<ServerHit> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let at = offset;
        offset += line.len();
        let code = line.trim_start();
        let Some(rest) = code.strip_prefix("impl") else { continue };
        if !rest.starts_with([' ', '<']) {
            continue;
        }
        let Some((trait_part, type_part)) = rest.split_once(" for ") else { continue };
        let name = last_path_segment(trait_part.trim());
        if !known.contains_key(name) || !source.contains(&format!("{}_server", snake_case(name))) {
            continue;
        }
        let ty = type_part.trim_start().trim_start_matches('&');
        let end = ty.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).unwrap_or(ty.len());
        let ty = last_path_segment(&ty[..end]);
        if ty.is_empty() {
            continue;
        }
        out.push(ServerHit {
            name: name.to_string(),
            at: at + (line.len() - code.len()),
            shape: ServerShape::Base,
            impl_for: Some(ty.to_string()),
        });
    }
    out
}

/// Node `server.addService(GreeterService, impl)` (grpc-tools / ts-proto) and
/// `server.addService(helloProto.Greeter.service, impl)` (proto-loader).
fn node_add_service_hits(source: &str, known: &BTreeMap<&str, BTreeSet<String>>) -> Vec<ServerHit> {
    const NEEDLE: &str = "addService(";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = source[from..].find(NEEDLE) {
        let pos = from + rel;
        from = pos + NEEDLE.len();
        let args = &source[from..];
        let end = args.find([',', ')']).unwrap_or(args.len());
        let segs: Vec<&str> = args[..end].trim().split('.').map(str::trim).collect();
        let Some(&last) = segs.last() else { continue };
        let name = if last == "service" && segs.len() >= 2 {
            // proto-loader: `<pkg>.<Service>.service`
            segs[segs.len() - 2]
        } else {
            // grpc-tools / ts-proto name the definition `<Service>Service`;
            // a service already called `FooService` gets `FooServiceService`.
            match last.strip_suffix("Service") {
                Some(n) if known.contains_key(n) => n,
                _ => last,
            }
        };
        if known.contains_key(name) {
            out.push(ServerHit {
                name: name.to_string(),
                at: pos,
                shape: ServerShape::Register,
                impl_for: None,
            });
        }
    }
    out
}

/// Every server needle hit in `source`, in offset order.
fn server_hits(source: &str, known: &BTreeMap<&str, BTreeSet<String>>) -> Vec<ServerHit> {
    let ctx = file_has_grpc_context(source);
    let mut hits: Vec<ServerHit> = Vec::new();
    for needle in SERVER_NEEDLES {
        if !needle.strict && !ctx {
            continue;
        }
        for (name, at) in needle_hits(source, needle, known) {
            hits.push(ServerHit {
                name,
                at,
                shape: needle.shape,
                impl_for: None,
            });
        }
    }
    if ctx {
        hits.extend(rust_trait_impl_hits(source, known));
        hits.extend(node_add_service_hits(source, known));
    }
    hits.retain(|h| !in_comment_line(source, h.at) && !follows_decl_keyword(source, h.at));
    hits.sort_by(|a, b| (a.at, &a.name).cmp(&(b.at, &b.name)));
    hits.dedup_by(|a, b| a.at == b.at && a.name == b.name);
    hits
}

/// An rpc or method name folded for comparison across bindings: `SayHello`
/// (Go, C#, Python), `sayHello` (Java, Node) and `say_hello` (Rust, Ruby) agree.
fn fold_rpc_name(name: &str) -> String {
    name.chars().filter(|c| *c != '_').map(|c| c.to_ascii_lowercase()).collect()
}

/// The server pass (A5.3): one GRPC_SERVER per (proto service, file) for every
/// `known` service this file implements or registers, with anchors that tie it
/// to the code that serves the service.
///
/// `nodes` / `nav` are the file's own parse. They locate:
/// - the implementing type: the innermost CLASS / STRUCT holding a
///   [`ServerShape::Base`] needle (via [`anchor::build_span_index`]), or the
///   type a Rust `impl <Service> for <Type>` names;
/// - the methods that implement the service's rpcs: methods of that type whose
///   name folds to an rpc name ([`fold_rpc_name`]). With no implementing type
///   (a registration-only file such as Node's `addService`), any METHOD /
///   FUNCTION in the file whose name folds to an rpc counts.
///
/// Anchors: every Base needle line (the marker's POSITION lands on the first),
/// plus each rpc method's first line, so [`anchor::attach`] emits
/// `grpc_server:<S> --HANDLED_BY--> <method>`. A registration line is anchored
/// only when no rpc method was found; its enclosing function (`main`,
/// `serve`) is then the owner, which is what binds the service in.
///
/// Generated gRPC code ([`is_generated_source`]) never yields a server: it
/// declares every base type the needles key on.
pub fn extract_grpc_server_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
    known: &[ProtoServiceRef],
    nodes: &[Node],
    nav: &CodeNav,
) -> GrpcNodes {
    let mut out = GrpcNodes {
        nodes: Vec::new(),
        nav: CodeNav::default(),
        anchors: Vec::new(),
    };
    if known.is_empty() || !may_hold_grpc_server(source) || is_generated_source(source) {
        return out;
    }
    // Service name -> folded rpc names, unioned across same-named services in
    // different packages (they share one marker, like clients do).
    let mut by_name: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for svc in known {
        if svc.name.is_empty() || !svc.name.bytes().all(is_ident_byte) {
            continue;
        }
        by_name
            .entry(svc.name.as_str())
            .or_default()
            .extend(svc.rpcs.iter().map(|r| fold_rpc_name(r)));
    }
    let hits = server_hits(source, &by_name);
    if hits.is_empty() {
        return out;
    }

    let types = anchor::build_span_index(nodes, nav, &[node_kind::CLASS, node_kind::STRUCT]);
    let kind_of = |id: &NodeId| nav.kind_by_id.get(id).copied();
    let evidence = client_evidence_cell(source);
    let mut grouped: BTreeMap<&str, Vec<&ServerHit>> = BTreeMap::new();
    for h in &hits {
        grouped.entry(h.name.as_str()).or_default().push(h);
    }

    for (name, hits) in grouped {
        let rpcs = by_name.get(name).cloned().unwrap_or_default();
        // The implementing types (membership only, so order is irrelevant).
        let mut impl_types: Vec<NodeId> = Vec::new();
        for h in hits.iter().filter(|h| h.shape == ServerShape::Base) {
            match &h.impl_for {
                Some(ty) => impl_types.extend(nodes.iter().map(|n| n.id).filter(|id| {
                    matches!(kind_of(id), Some(k) if k == node_kind::CLASS || k == node_kind::STRUCT)
                        && nav.name_by_id.get(id).is_some_and(|n| n == ty)
                })),
                None => impl_types.extend(anchor::owner_of_line(&types, line_of(source, h.at))),
            }
        }
        let rpc_methods: Vec<u32> = nodes
            .iter()
            .filter(|n| {
                let is_fn = matches!(kind_of(&n.id), Some(k) if k == node_kind::METHOD || k == node_kind::FUNCTION);
                let parent_ok = impl_types.is_empty()
                    || nav.parent_of.get(&n.id).is_some_and(|p| impl_types.contains(p));
                is_fn
                    && parent_ok
                    && nav.name_by_id.get(&n.id).is_some_and(|m| rpcs.contains(&fold_rpc_name(m)))
            })
            .filter_map(|n| anchor::position_span(n).map(|(start, _)| start))
            .collect();

        let qname = format!("grpc_server:{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::GRPC_SERVER, &qname);
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: evidence.iter().cloned().collect(),
        });
        out.nav
            .record(id, name, &qname, node_kind::GRPC_SERVER, Some(module_id));
        let mut lines: Vec<u32> = hits
            .iter()
            .filter(|h| h.shape == ServerShape::Base || rpc_methods.is_empty())
            .map(|h| line_of(source, h.at))
            .collect();
        lines.extend(rpc_methods);
        out.anchors
            .extend(lines.into_iter().map(|line| Anchor { node: id, line }));
    }
    out
}

// ---- LA.17 (A10.13): Connect / Twirp, proto-service RPC over HTTP ----------

/// The proto-over-HTTP stacks [`extract_proto_rpc_nodes`] reads. Both key on
/// the build's `.proto` services like the gRPC passes, but mint method-level
/// RPC_PROCEDURE / RPC_CALL nodes that `RpcStackResolver` pairs on the exact
/// `<proto package>.<Service>.<Method>` path, never the service-level gRPC
/// family: Twirp is not gRPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtoRpcFamily {
    Connect,
    Twirp,
}

/// Imports that mark a Go file as a Connect (connect-go) file.
const CONNECT_GO_IMPORTS: &[&str] = &["connectrpc.com/connect", "github.com/bufbuild/connect-go"];
/// Imports that mark a TS / JS file as a connect-es file.
const CONNECT_ES_IMPORTS: &[&str] = &["@connectrpc/connect", "@bufbuild/connect"];
/// Twirp's generated constructors. Looked for only in a Twirp repo: a Twirp
/// client file imports nothing but its generated package, so the gate is the
/// repo's go.mod, not the file.
const TWIRP_TOKENS: &[&str] = &["ProtobufClient(", "JSONClient(", "Server("];
/// Parser tags of the TS family, where connect-es clients live.
const TS_FAMILY: &[&str] = &["typescript", "angular", "vue"];

/// Go `<pkg>.New<Service><suffix>(`: `(suffix, family, is server)`. Longest
/// suffix first, so `NewHatProtobufClient(` is Twirp's before it is Connect's
/// `Client` with the unknown service `HatProtobuf`.
const GO_CONSTRUCTORS: &[(&str, ProtoRpcFamily, bool)] = &[
    ("ProtobufClient", ProtoRpcFamily::Twirp, false),
    ("JSONClient", ProtoRpcFamily::Twirp, false),
    ("Handler", ProtoRpcFamily::Connect, true),
    ("Client", ProtoRpcFamily::Connect, false),
    ("Server", ProtoRpcFamily::Twirp, true),
];

/// connect-es client factories, each taking the service descriptor first.
const CONNECT_ES_FACTORIES: &[&str] = &[
    "createClient(",
    "createPromiseClient(",
    "createCallbackClient(",
];

/// Per-file tallies of [`extract_proto_rpc_nodes`]; the engine sums them into
/// its `[proto-rpc]` marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProtoRpcCounts {
    pub connect_procedures: usize,
    pub connect_calls: usize,
    pub twirp_procedures: usize,
    pub twirp_calls: usize,
    /// Needle hits on a service name two known services in different proto
    /// packages share: skipped, never guessed.
    pub ambiguous: usize,
    /// Procedures no method of the registered type implements (an rpc served
    /// by the `Unimplemented…` embed): contained by the module, never
    /// HANDLED_BY the function that registers the service.
    pub unowned: usize,
}

impl ProtoRpcCounts {
    pub fn add(&mut self, other: ProtoRpcCounts) {
        self.connect_procedures += other.connect_procedures;
        self.connect_calls += other.connect_calls;
        self.twirp_procedures += other.twirp_procedures;
        self.twirp_calls += other.twirp_calls;
        self.ambiguous += other.ambiguous;
        self.unowned += other.unowned;
    }

    /// True when the pass saw anything worth a marker line.
    pub fn any(&self) -> bool {
        self.connect_procedures
            + self.connect_calls
            + self.twirp_procedures
            + self.twirp_calls
            + self.ambiguous
            > 0
    }
}

/// What [`extract_proto_rpc_nodes`] adds to one file's parse. The nodes arrive
/// finished — POSITION cell and owner edge (HANDLED_BY / USES) or the module
/// CONTAINS fallback — so the engine grafts them without an anchor pass.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ProtoRpcNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    pub counts: ProtoRpcCounts,
}

/// Cheap pre-check for [`extract_proto_rpc_nodes`]: false only when `source`
/// can hold no Connect or Twirp needle, so the engine may skip its parse lookup.
pub fn may_hold_proto_rpc(source: &str, twirp_repo: bool) -> bool {
    CONNECT_GO_IMPORTS
        .iter()
        .chain(CONNECT_ES_IMPORTS)
        .any(|t| source.contains(t))
        || (twirp_repo && TWIRP_TOKENS.iter().any(|t| source.contains(t)))
}

/// One constructor / factory call that names a known service.
struct ProtoRpcHit {
    family: ProtoRpcFamily,
    server: bool,
    service: String,
    /// Byte offset of the needle: the Go package qualifier, or the factory name.
    at: usize,
    /// Byte offset of the call's `(`.
    open: usize,
}

/// A known service's proto package and rpc names (declaration order).
struct RpcService<'a> {
    package: Option<&'a str>,
    rpcs: Vec<&'a str>,
}

impl RpcService<'_> {
    /// `<package>.<Service>.<rpc>`, or `<Service>.<rpc>` for a package-less proto.
    fn path(&self, service: &str, rpc: &str) -> String {
        match self.package {
            Some(p) => format!("{p}.{service}.{rpc}"),
            None => format!("{service}.{rpc}"),
        }
    }
}

/// Service name -> its package and rpcs; `None` marks a name two known services
/// in different packages share. Same-package duplicates (one `.proto` copied
/// into two repos) union their rpcs.
fn proto_rpc_services(known: &[ProtoServiceRef]) -> BTreeMap<&str, Option<RpcService<'_>>> {
    let mut out: BTreeMap<&str, Option<RpcService<'_>>> = BTreeMap::new();
    for svc in known {
        if svc.name.is_empty() || !svc.name.bytes().all(is_ident_byte) {
            continue;
        }
        let rpcs = svc
            .rpcs
            .iter()
            .map(String::as_str)
            .filter(|r| !r.is_empty() && r.bytes().all(is_ident_byte));
        match out.get_mut(svc.name.as_str()) {
            None => {
                out.insert(
                    svc.name.as_str(),
                    Some(RpcService {
                        package: svc.package.as_deref(),
                        rpcs: rpcs.collect(),
                    }),
                );
            }
            Some(Some(s)) if s.package == svc.package.as_deref() => {
                for r in rpcs {
                    if !s.rpcs.contains(&r) {
                        s.rpcs.push(r);
                    }
                }
            }
            Some(slot) => *slot = None,
        }
    }
    out
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// End of the identifier that starts at `i` (`i` itself when there is none).
fn ident_end(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && is_ident_byte(bytes[i]) {
        i += 1;
    }
    i
}

/// End of a dotted identifier path (`pb.Hat`, `gen.eliza.ElizaService`) at `i`.
fn path_end(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (is_ident_byte(bytes[i]) || bytes[i] == b'.') {
        i += 1;
    }
    i
}

/// Byte offset of the `)` closing the `(` at `open`, skipping quoted literals.
fn matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(_) if b == b'\\' => i += 1,
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => match b {
                b'"' | b'\'' | b'`' => quote = Some(b),
                b'(' => depth += 1,
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Go `<pkg>.New<Service><suffix>(` for every enabled family, package-qualified
/// only: a hand-written `NewGreeterServer()` constructor is never a hit.
fn go_constructor_hits(
    source: &str,
    connect: bool,
    twirp: bool,
    services: &BTreeMap<&str, Option<RpcService<'_>>>,
) -> Vec<ProtoRpcHit> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = source[from..].find(".New") {
        let dot = from + rel;
        from = dot + ".New".len();
        let at = ident_start(bytes, dot);
        let run_end = ident_end(bytes, from);
        if at == dot || bytes.get(run_end) != Some(&b'(') {
            continue;
        }
        let run = &source[from..run_end];
        for &(suffix, family, server) in GO_CONSTRUCTORS {
            let enabled = match family {
                ProtoRpcFamily::Connect => connect,
                ProtoRpcFamily::Twirp => twirp,
            };
            let Some(service) = run.strip_suffix(suffix) else {
                continue;
            };
            if enabled && !service.is_empty() && services.contains_key(service) {
                out.push(ProtoRpcHit {
                    family,
                    server,
                    service: service.to_string(),
                    at,
                    open: run_end,
                });
                break;
            }
        }
    }
    out
}

/// connect-es `createClient(<Service>, transport)` and its two siblings; the
/// descriptor may be namespaced (`eliza.ElizaService`).
fn connect_es_hits(
    source: &str,
    services: &BTreeMap<&str, Option<RpcService<'_>>>,
) -> Vec<ProtoRpcHit> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    for factory in CONNECT_ES_FACTORIES {
        let mut from = 0;
        while let Some(rel) = source[from..].find(factory) {
            let at = from + rel;
            from = at + factory.len();
            if at > 0 && is_ident_byte(bytes[at - 1]) {
                continue;
            }
            let arg_start = skip_ws(bytes, from);
            let arg_end = path_end(bytes, arg_start);
            let service = source[arg_start..arg_end].rsplit('.').next().unwrap_or("");
            let closed = matches!(bytes.get(skip_ws(bytes, arg_end)), Some(b',' | b')'));
            if closed && !service.is_empty() && services.contains_key(service) {
                out.push(ProtoRpcHit {
                    family: ProtoRpcFamily::Connect,
                    server: false,
                    service: service.to_string(),
                    at,
                    open: from - 1,
                });
            }
        }
    }
    out
}

/// `&T{` / `T{` / `&pkg.T{` at `i` -> `T`.
fn struct_literal_type(source: &str, i: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let mut i = i;
    if bytes.get(i) == Some(&b'&') {
        i = skip_ws(bytes, i + 1);
    }
    let end = path_end(bytes, i);
    let ty = source[i..end].rsplit('.').next().unwrap_or("");
    (!ty.is_empty() && bytes.get(skip_ws(bytes, end)) == Some(&b'{')).then_some(ty)
}

/// The struct a Go constructor's first argument builds: `&T{}` / `T{}` in
/// place, or an identifier the same file binds to one (`srv := &T{}`).
fn go_impl_type(source: &str, open: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let arg = skip_ws(bytes, open + 1);
    if let Some(ty) = struct_literal_type(source, arg) {
        return Some(ty);
    }
    let var_end = ident_end(bytes, arg);
    if var_end == arg || !matches!(bytes.get(skip_ws(bytes, var_end)), Some(b',' | b')')) {
        return None;
    }
    let var = &source[arg..var_end];
    let mut from = 0;
    while let Some(rel) = source[from..].find(var) {
        let pos = from + rel;
        from = pos + var.len();
        if (pos > 0 && is_ident_byte(bytes[pos - 1]))
            || bytes.get(from).is_some_and(|b| is_ident_byte(*b))
        {
            continue;
        }
        let op = skip_ws(bytes, from);
        let rest = &source[op..];
        let value = if rest.starts_with(":=") {
            op + 2
        } else if rest.starts_with('=') && !rest.starts_with("==") {
            op + 1
        } else {
            continue;
        };
        if let Some(ty) = struct_literal_type(source, skip_ws(bytes, value)) {
            return Some(ty);
        }
    }
    None
}

/// The identifier the call at `at` is bound to on its own line: `client :=`,
/// `client =`, `const client =`, `const client: T =`, `s.client =`, or an
/// object / struct literal key `client:` -> `client`.
fn bound_identifier(source: &str, at: usize) -> Option<&str> {
    let head = source[line_start(source, at)..at].trim_end();
    let lhs = if let Some(l) = head.strip_suffix(":=") {
        l
    } else if let Some(l) = head.strip_suffix('=') {
        if l.ends_with(['=', '!', '<', '>']) {
            return None;
        }
        // A TS annotation (`const client: PromiseClient<…> =`) ends the binding.
        l.split(':').next().unwrap_or(l)
    } else {
        head.strip_suffix(':')?
    };
    let lhs = lhs.trim_end();
    let name = &lhs[ident_start(lhs.as_bytes(), lhs.len())..];
    (!name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit())).then_some(name)
}

/// Every `<binding>.<method>(` in `source` with an identifier boundary before
/// the binding (`s.client.Say(` counts, `myclient.Say(` does not), as
/// `(byte offset, method)`.
fn member_calls<'s>(source: &'s str, binding: &str) -> Vec<(usize, &'s str)> {
    let bytes = source.as_bytes();
    let needle = format!("{binding}.");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = source[from..].find(&needle) {
        let pos = from + rel;
        from = pos + needle.len();
        let m_end = ident_end(bytes, from);
        if (pos > 0 && is_ident_byte(bytes[pos - 1]))
            || m_end == from
            || bytes.get(m_end) != Some(&b'(')
        {
            continue;
        }
        out.push((pos, &source[from..m_end]));
    }
    out
}

/// LA.17 (A10.13): Connect and Twirp. One RPC_PROCEDURE per proto rpc of every
/// known service this file registers, one RPC_CALL per rpc it calls, keyed
/// `rpc:<proto package>.<Service>.<Method>` / `rpc_call:…` so the unchanged
/// `RpcStackResolver` pairs them on the exact path Connect itself routes on
/// (`/<package>.<Service>/<Method>`).
///
/// Needles, each gated so a gRPC or plain HTTP file never hits:
/// - Go Connect (the file imports connect-go): server
///   `<pkg>.New<Svc>Handler(&T{})`, client `<pkg>.New<Svc>Client(…)`;
/// - Go Twirp (`twirp_repo`: the repo requires twitchtv/twirp): server
///   `<pkg>.New<Svc>Server(&T{})`, client `<pkg>.New<Svc>ProtobufClient(…)` /
///   `<pkg>.New<Svc>JSONClient(…)`;
/// - TS connect-es (the file imports it): `createClient(<Svc>, …)`,
///   `createPromiseClient(`, `createCallbackClient(`.
///
/// A procedure whose rpc a method of `T` implements (the file's own parse:
/// METHOD, nav parent a STRUCT / CLASS named `T`) is HANDLED_BY that method and
/// located at its span; any other procedure is located at the registration and
/// contained by the module, never HANDLED_BY the registering function.
///
/// A call is `<binding>.<rpc>(` on the identifier the constructor is bound to
/// (TS `say` folds to `Say`), or a method chained onto the constructor call;
/// other methods (`client.Close()`) mint nothing. Each call is located at its
/// first site; every site's innermost METHOD / FUNCTION USES it, a site no
/// function holds gives the module CONTAINS edge.
///
/// A service name two known services in different packages share is skipped
/// and counted `ambiguous`. Generated code ([`is_generated_source`]) and
/// needles after a declaration keyword (`func NewElizaServiceClient(`) never
/// hit.
#[allow(clippy::too_many_arguments)]
pub fn extract_proto_rpc_nodes(
    source: &str,
    path: &str,
    lang: &str,
    module_id: NodeId,
    repo: RepoId,
    known: &[ProtoServiceRef],
    nodes: &[Node],
    nav: &CodeNav,
    twirp_repo: bool,
) -> ProtoRpcNodes {
    let mut out = ProtoRpcNodes::default();
    if known.is_empty() || is_generated_source(source) {
        return out;
    }
    let is_go = lang == "go";
    let imports_any = |needles: &[&str]| needles.iter().any(|n| source.contains(n));
    let connect = if is_go {
        imports_any(CONNECT_GO_IMPORTS)
    } else {
        TS_FAMILY.contains(&lang) && imports_any(CONNECT_ES_IMPORTS)
    };
    let twirp = is_go && twirp_repo;
    if !connect && !twirp {
        return out;
    }
    let services = proto_rpc_services(known);
    let mut hits = if is_go {
        go_constructor_hits(source, connect, twirp, &services)
    } else {
        connect_es_hits(source, &services)
    };
    hits.retain(|h| !in_comment_line(source, h.at) && !follows_decl_keyword(source, h.at));
    hits.sort_by_key(|h| h.at);

    let mut servers: BTreeMap<&str, Vec<&ProtoRpcHit>> = BTreeMap::new();
    let mut clients: BTreeMap<&str, Vec<&ProtoRpcHit>> = BTreeMap::new();
    for h in &hits {
        if !matches!(services.get(h.service.as_str()), Some(Some(_))) {
            out.counts.ambiguous += 1;
            continue;
        }
        let side = if h.server { &mut servers } else { &mut clients };
        side.entry(h.service.as_str()).or_default().push(h);
    }

    let kind_of = |id: &NodeId| nav.kind_by_id.get(id).copied();
    let edge = |from: NodeId, to: NodeId, category| Edge {
        from,
        to,
        category,
        confidence: Confidence::Medium,
        cells: Vec::new(),
    };
    for (name, group) in servers {
        let (Some(Some(svc)), Some(first)) = (services.get(name), group.first()) else {
            continue;
        };
        let impl_types: BTreeSet<&str> = group
            .iter()
            .filter_map(|h| go_impl_type(source, h.open))
            .collect();
        let type_ids: Vec<NodeId> = nodes
            .iter()
            .map(|n| n.id)
            .filter(|id| {
                matches!(kind_of(id), Some(k) if k == node_kind::STRUCT || k == node_kind::CLASS)
                    && nav
                        .name_by_id
                        .get(id)
                        .is_some_and(|n| impl_types.contains(n.as_str()))
            })
            .collect();
        let registration = line_of(source, first.at);
        for rpc in &svc.rpcs {
            let rpc_path = svc.path(name, rpc);
            let qname = format!("rpc:{rpc_path}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::RPC_PROCEDURE, &qname);
            let method = nodes.iter().find(|n| {
                kind_of(&n.id) == Some(node_kind::METHOD)
                    && nav
                        .parent_of
                        .get(&n.id)
                        .is_some_and(|p| type_ids.contains(p))
                    && nav
                        .name_by_id
                        .get(&n.id)
                        .is_some_and(|m| fold_rpc_name(m) == fold_rpc_name(rpc))
            });
            let span = method.and_then(|m| {
                m.cells
                    .iter()
                    .find(|c| c.kind == glia_code_domain::cell_type::POSITION)
            });
            let position = span
                .cloned()
                .unwrap_or_else(|| anchor::position_cell(path, registration));
            match method {
                Some(m) => out.edges.push(edge(
                    id,
                    m.id,
                    glia_code_domain::edge_category::HANDLED_BY,
                )),
                None => {
                    out.edges.push(edge(
                        module_id,
                        id,
                        glia_code_domain::edge_category::CONTAINS,
                    ));
                    out.counts.unowned += 1;
                }
            }
            out.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![position],
            });
            out.nav.record(
                id,
                &format!("{name}.{rpc}"),
                &qname,
                node_kind::RPC_PROCEDURE,
                Some(module_id),
            );
            match first.family {
                ProtoRpcFamily::Connect => out.counts.connect_procedures += 1,
                ProtoRpcFamily::Twirp => out.counts.twirp_procedures += 1,
            }
        }
    }

    let bytes = source.as_bytes();
    let owners = anchor::build_owner_index(nodes, nav);
    for (name, group) in clients {
        let (Some(Some(svc)), Some(first)) = (services.get(name), group.first()) else {
            continue;
        };
        let folded: Vec<String> = svc.rpcs.iter().map(|r| fold_rpc_name(r)).collect();
        let mut sites: Vec<Vec<usize>> = vec![Vec::new(); svc.rpcs.len()];
        let mut add_site = |at: usize, method: &str| {
            let key = fold_rpc_name(method);
            if let Some(ix) = folded.iter().position(|f| *f == key)
                && !in_comment_line(source, at)
            {
                sites[ix].push(at);
            }
        };
        for h in &group {
            if let Some(binding) = bound_identifier(source, h.at) {
                for (at, method) in member_calls(source, binding) {
                    add_site(at, method);
                }
            }
            // `pb.NewHatProtobufClient(…).MakeHat(…)`: a call chained onto the constructor.
            if let Some(close) = matching_paren(bytes, h.open)
                && bytes.get(close + 1) == Some(&b'.')
            {
                let m_end = ident_end(bytes, close + 2);
                if m_end > close + 2 && bytes.get(m_end) == Some(&b'(') {
                    add_site(close + 1, &source[close + 2..m_end]);
                }
            }
        }
        for (rpc, mut at) in svc.rpcs.iter().zip(sites) {
            at.sort_unstable();
            at.dedup();
            let Some(&first_site) = at.first() else {
                continue;
            };
            let qname = format!("rpc_call:{}", svc.path(name, rpc));
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::RPC_CALL, &qname);
            let mut linked: Vec<NodeId> = Vec::new();
            for site in at {
                let owner =
                    anchor::owner_of_line(&owners, line_of(source, site)).unwrap_or(module_id);
                if !linked.contains(&owner) {
                    linked.push(owner);
                    out.edges.push(if owner == module_id {
                        edge(
                            module_id,
                            id,
                            glia_code_domain::edge_category::CONTAINS,
                        )
                    } else {
                        edge(owner, id, glia_code_domain::edge_category::USES)
                    });
                }
            }
            out.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![anchor::position_cell(path, line_of(source, first_site))],
            });
            out.nav.record(
                id,
                &format!("{name}.{rpc}"),
                &qname,
                node_kind::RPC_CALL,
                Some(module_id),
            );
            match first.family {
                ProtoRpcFamily::Connect => out.counts.connect_calls += 1,
                ProtoRpcFamily::Twirp => out.counts.twirp_calls += 1,
            }
        }
    }
    out
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
            .find(|c| c.kind == glia_code_domain::cell_type::POSITION)
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
            .find(|c| c.kind == glia_code_domain::cell_type::RPC_PACKAGE)
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
                .any(|c| c.kind == glia_code_domain::cell_type::POSITION),
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
            .filter(|e| e.category == glia_code_domain::edge_category::DEFINES)
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
            rpcs: Vec::new(),
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
    fn both_client_passes_anchor_every_construction_site() {
        // Suffix pass: two stubs of one service in two functions.
        let go = "import \"google.golang.org/grpc\"\n\nfunc A() {\n\tc := pb.NewUserServiceClient(conn)\n}\n\nfunc B() {\n\tc := pb.NewUserServiceClient(conn)\n}\n";
        let out = extract_grpc_client_nodes(go, module_id(), repo());
        assert_eq!(client_qnames(&out), vec!["grpc_client:UserService".to_string()]);
        let id = out.nodes[0].id;
        assert_eq!(
            out.anchors,
            vec![Anchor { node: id, line: 3 }, Anchor { node: id, line: 7 }]
        );

        // Data-driven pass: the C# fixture shape, one site.
        let cs = "using Grpc.Net.Client;\nclass H {\n  void F() {\n    var c = new Greeter.GreeterClient(ch);\n  }\n}";
        let out = extract_known_grpc_client_nodes(cs, module_id(), repo(), &[svc("Greeter")]);
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 3 }]);

        // No client, no anchor.
        let none = extract_grpc_client_nodes("package main\n", module_id(), repo());
        assert!(none.nodes.is_empty() && none.anchors.is_empty());
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
    fn data_driven_needle_matches_the_csharp_client_factory_registration() {
        // Program.cs of the ASP.NET Core client factory: no `using Grpc.*` at
        // all, only the registration. `AddGrpcClient<` is the gRPC context.
        let program = "using Demo;\n\nvar builder = WebApplication.CreateBuilder(args);\nbuilder.Services.AddGrpcClient<Greeter.GreeterClient>(o =>\n{\n    o.Address = new Uri(\"https://localhost:5001\");\n});\n";
        let out = extract_known_grpc_client_nodes(program, module_id(), repo(), &[svc("Greeter")]);
        assert_eq!(client_qnames(&out), vec!["grpc_client:Greeter".to_string()]);
        assert_eq!(out.anchors, vec![Anchor { node: out.nodes[0].id, line: 3 }]);
        // A generic that is not the generated client is no hit.
        let other = "builder.Services.AddGrpcClient<Greeter.GreeterClient>(o => {});\nvar x = Get<LegacyGreeterClient>(y);\n";
        let out = extract_known_grpc_client_nodes(other, module_id(), repo(), &[svc("Greeter")]);
        assert_eq!(out.anchors.len(), 1, "LegacyGreeterClient is not Greeter");
    }

    // ---- A5.3: server-impl detection -------------------------------------

    fn greeter() -> ProtoServiceRef {
        let mut s = svc("Greeter");
        s.package = Some("helloworld".to_string());
        s.rpcs = vec!["SayHello".to_string(), "SayHelloStream".to_string()];
        s
    }

    /// The server pass over `source` with no parse (no types, no methods).
    fn servers(source: &str) -> GrpcNodes {
        extract_grpc_server_nodes(source, module_id(), repo(), &[greeter()], &[], &CodeNav::default())
    }

    fn server_lines(out: &GrpcNodes) -> Vec<u32> {
        out.anchors.iter().map(|a| a.line).collect()
    }

    #[test]
    fn server_pass_matches_each_binding_shape_exactly_once() {
        // (source, 0-indexed line of the needle that must anchor the marker)
        let cases: &[(&str, &str, u32)] = &[
            (
                "go",
                "package main\n\nimport (\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/hello/pb\"\n)\n\ntype server struct {\n\tpb.UnimplementedGreeterServer\n}\n",
                8,
            ),
            (
                "java",
                "package demo;\n\nimport io.grpc.stub.StreamObserver;\n\npublic class GreeterImpl extends GreeterGrpc.GreeterImplBase {\n}\n",
                4,
            ),
            (
                "python",
                "import grpc\nimport helloworld_pb2_grpc\n\n\nclass Svc(helloworld_pb2_grpc.GreeterServicer):\n    pass\n",
                4,
            ),
            (
                "csharp",
                "using Grpc.Core;\n\nnamespace GreeterApi.Services\n{\n    public class GreeterService : Greeter.GreeterBase\n    {\n    }\n}\n",
                4,
            ),
            (
                "kotlin",
                "import io.grpc.ServerBuilder\n\nclass HelloWorldService : GreeterGrpcKt.GreeterCoroutineImplBase() {\n}\n",
                2,
            ),
            (
                "cpp",
                "#include <grpcpp/grpcpp.h>\n\nclass GreeterServiceImpl final : public Greeter::Service {\n};\n",
                2,
            ),
            (
                "ruby",
                "require 'grpc'\nrequire 'helloworld_services_pb'\n\nclass GreeterServer < Helloworld::Greeter::Service\nend\n",
                3,
            ),
            (
                "dart",
                "import 'package:grpc/grpc.dart';\n\nclass GreeterService extends GreeterServiceBase {\n}\n",
                2,
            ),
            (
                "rust",
                "use tonic::{transport::Server, Request};\nuse hello_world::greeter_server::{Greeter, GreeterServer};\n\n#[tonic::async_trait]\nimpl Greeter for MyGreeter {\n}\n",
                4,
            ),
            (
                "node",
                "import { Server } from \"@grpc/grpc-js\";\nimport { GreeterService } from \"./gen/greeter_grpc_pb\";\n\nconst server = new Server();\nserver.addService(GreeterService, { sayHello });\n",
                4,
            ),
            (
                "node proto-loader",
                "const grpc = require('@grpc/grpc-js');\nconst server = new grpc.Server();\nserver.addService(helloProto.Greeter.service, { sayHello: sayHello });\n",
                2,
            ),
            (
                "scala",
                "import io.grpc.ServerBuilder\n\nobject Main {\n  val svc = GreeterGrpc.bindService(new GreeterImpl, ec)\n}\n",
                3,
            ),
        ];
        for (lang, source, line) in cases {
            let out = servers(source);
            assert_eq!(
                out.nodes
                    .iter()
                    .map(|n| out.nav.qname_by_id[&n.id].clone())
                    .collect::<Vec<_>>(),
                vec!["grpc_server:Greeter".to_string()],
                "{lang}: exactly one marker"
            );
            let id = out.nodes[0].id;
            assert_eq!(out.nav.kind_by_id[&id], node_kind::GRPC_SERVER, "{lang}");
            assert_eq!(out.nav.name_by_id[&id], "Greeter", "{lang}");
            assert_eq!(out.nav.parent_of[&id], module_id(), "{lang}");
            assert_eq!(server_lines(&out), vec![*line], "{lang}: anchored at the needle");
        }
    }

    #[test]
    fn server_pass_registration_lines_join_one_marker_per_file() {
        // The grpcio tutorial file: base class AND registration, one marker.
        let source = "import grpc\nimport greeter_pb2_grpc\n\n\nclass Greeter(greeter_pb2_grpc.GreeterServicer):\n    def SayHello(self, request, context):\n        return None\n\n\ndef serve():\n    server = grpc.server(None)\n    greeter_pb2_grpc.add_GreeterServicer_to_server(Greeter(), server)\n";
        let out = servers(source);
        assert_eq!(out.nodes.len(), 1);
        // No parse, so no rpc method was found: the registration line anchors too.
        assert_eq!(server_lines(&out), vec![4, 11]);
        // Go: embed + `Register…Server(`.
        let go = "import \"google.golang.org/grpc\"\n\ntype server struct {\n\tpb.UnimplementedGreeterServer\n}\n\nfunc main() {\n\ts := grpc.NewServer()\n\tpb.RegisterGreeterServer(s, &server{})\n}\n";
        let out = servers(go);
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(server_lines(&out), vec![3, 8]);
    }

    #[test]
    fn server_pass_ignores_files_with_no_server_shape() {
        let negatives: &[(&str, &str)] = &[
            ("client stub", "using Grpc.Net.Client;\nvar c = new Greeter.GreeterClient(channel);\nawait c.SayHelloAsync(req);\n"),
            ("prose", "// The Greeter service greets. See GreeterBase docs.\nfn main() {}\n"),
            ("longer name", "using Grpc.Core;\nclass X : LegacyGreeterBase {}\nclass Y : GreeterBaseline {}\n"),
            ("no context for a loose needle", "class UserRepo : GreeterBase {}\n"),
            ("commented out", "import grpc\n# class G(pb2_grpc.GreeterServicer):\n"),
            ("unknown service", "import grpc\nclass G(pb2_grpc.FarewellServicer):\n    pass\n"),
            ("tonic trait of another service", "use tonic::Request;\nimpl Greeter for MyGreeter {}\n"),
        ];
        for (what, source) in negatives {
            let out = servers(source);
            assert!(out.nodes.is_empty() && out.anchors.is_empty(), "{what}: got {:?}", out.nav.qname_by_id);
        }
    }

    #[test]
    fn server_pass_skips_generated_code() {
        // protoc output declares every base the needles key on; committed
        // generated files must not look like servers.
        let go = "// Code generated by protoc-gen-go-grpc. DO NOT EDIT.\npackage pb\n\nimport grpc \"google.golang.org/grpc\"\n\n// UnimplementedGreeterServer must be embedded.\ntype UnimplementedGreeterServer struct {\n}\n\nfunc (UnimplementedGreeterServer) SayHello() {}\n\nfunc RegisterGreeterServer(s grpc.ServiceRegistrar, srv GreeterServer) {}\n";
        let py = "# Generated by the gRPC Python protocol compiler plugin. DO NOT EDIT!\nimport grpc\n\n\nclass GreeterServicer(object):\n    pass\n\n\ndef add_GreeterServicer_to_server(servicer, server):\n    pass\n";
        let cs = "// <auto-generated>\n//     Generated by the protocol buffer compiler.\n// </auto-generated>\nusing grpc = global::Grpc.Core;\npublic static partial class Greeter\n{\n  public abstract partial class GreeterBase {}\n  public static grpc::ServerServiceDefinition BindService(GreeterBase serviceImpl) { return null; }\n}\n";
        for source in [go, py, cs] {
            assert!(is_generated_source(source));
            assert!(servers(source).nodes.is_empty(), "generated: {source}");
        }
        // Declarations are rejected even without the banner.
        let bare = "import grpc\n\nclass GreeterServicer(object):\n    pass\n\ndef add_GreeterServicer_to_server(servicer, server):\n    pass\n";
        assert!(servers(bare).nodes.is_empty());
    }

    /// A node of `kind` named `name` spanning `start..=end`, recorded in `nav`.
    fn parse_node(
        nav: &mut CodeNav,
        kind: glia_core::NodeKindId,
        name: &str,
        qname: &str,
        span: (u32, u32),
        parent: NodeId,
    ) -> Node {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        nav.record(id, name, qname, kind, Some(parent));
        Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![position_cell("svc.cs", span.0, span.1)],
        }
    }

    #[test]
    fn server_pass_anchors_the_implementing_types_rpc_methods() {
        let source = "using Grpc.Core;\n\npublic class GreeterService : Greeter.GreeterBase\n{\n    public override Task<HelloReply> SayHello(HelloRequest r, ServerCallContext c)\n    {\n        return Helper();\n    }\n\n    Task<HelloReply> Helper() => null;\n}\n\npublic class Wrapper\n{\n    public void SayHello() {}\n}\n";
        let mut nav = CodeNav::default();
        let m = module_id();
        let impl_class = parse_node(&mut nav, node_kind::CLASS, "GreeterService", "svc::GreeterService", (2, 10), m);
        let say = parse_node(&mut nav, node_kind::METHOD, "SayHello", "svc::GreeterService::SayHello", (4, 7), impl_class.id);
        let helper = parse_node(&mut nav, node_kind::METHOD, "Helper", "svc::GreeterService::Helper", (9, 9), impl_class.id);
        let wrapper = parse_node(&mut nav, node_kind::CLASS, "Wrapper", "svc::Wrapper", (12, 15), m);
        let decoy = parse_node(&mut nav, node_kind::METHOD, "SayHello", "svc::Wrapper::SayHello", (14, 14), wrapper.id);
        let nodes = vec![impl_class, say, helper, wrapper, decoy];
        let out = extract_grpc_server_nodes(source, m, repo(), &[greeter()], &nodes, &nav);
        assert_eq!(out.nodes.len(), 1);
        // The base line (POSITION) and SayHello's first line; not Helper, and
        // not the same-named method of a class that does not extend the base.
        assert_eq!(server_lines(&out), vec![2, 4]);

        // A registration-only file (Node `addService`): no implementing type, so
        // any function named after an rpc serves it, and the registration line
        // is not anchored once one is found.
        let node_src = "import { Server } from \"@grpc/grpc-js\";\n\nfunction sayHello(call, cb) {\n  cb(null, null);\n}\n\nconst server = new Server();\nserver.addService(GreeterService, { sayHello });\n";
        let mut nav = CodeNav::default();
        let f = parse_node(&mut nav, node_kind::FUNCTION, "sayHello", "server::sayHello", (2, 4), m);
        let out = extract_grpc_server_nodes(node_src, m, repo(), &[greeter()], &[f], &nav);
        assert_eq!(server_lines(&out), vec![2]);

        // Rust: the impl block is outside the struct's span; the type is found
        // by the name after `for`, and `say_hello` folds to `SayHello`.
        let rs = "use tonic::Request;\nuse hello::greeter_server::Greeter;\n\npub struct MyGreeter {}\n\nimpl Greeter for MyGreeter {\n    async fn say_hello(&self) {}\n}\n";
        let mut nav = CodeNav::default();
        let st = parse_node(&mut nav, node_kind::STRUCT, "MyGreeter", "main::MyGreeter", (3, 3), m);
        let method = parse_node(&mut nav, node_kind::METHOD, "say_hello", "main::MyGreeter::say_hello", (6, 6), st.id);
        let out = extract_grpc_server_nodes(rs, m, repo(), &[greeter()], &[st, method], &nav);
        assert_eq!(server_lines(&out), vec![5, 6]);
    }

    #[test]
    fn server_marker_carries_its_files_package_evidence() {
        let source = "using Grpc.Core;\nusing GreeterApi;\n\npublic class GreeterService : Greeter.GreeterBase {}\n";
        let out = servers(source);
        assert_eq!(out.nodes.len(), 1);
        let evidence: Vec<RpcPackageCell> = out.nodes[0]
            .cells
            .iter()
            .filter_map(|c| match &c.payload {
                CellPayload::Json(j) if c.kind == glia_code_domain::cell_type::RPC_PACKAGE => {
                    RpcPackageCell::parse(j)
                }
                _ => None,
            })
            .collect();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].imports, vec!["Grpc.Core", "GreeterApi"]);
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
        assert_eq!(refs[0].rpcs, vec!["SayHello".to_string()], "A5.3: the rpc names ride along");
        assert!(refs[1].rpcs.is_empty());
    }

    // ---- A5.4: client package evidence -----------------------------------

    fn evidence_of(out: &GrpcNodes) -> Vec<RpcPackageCell> {
        out.nodes
            .iter()
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == glia_code_domain::cell_type::RPC_PACKAGE)
            .map(|c| match &c.payload {
                CellPayload::Json(j) => RpcPackageCell::parse(j).expect("evidence decodes"),
                other => panic!("RPC_PACKAGE must be Json, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn evidence_reads_go_import_block_and_single_imports() {
        let block = "package main\n\nimport (\n\t\"context\"\n\n\t\"google.golang.org/grpc\"\n\n\tpb \"example.com/gen/billing\"\n)\n\nfunc main() {}\n";
        assert_eq!(
            client_package_evidence(block),
            vec!["context", "google.golang.org/grpc", "example.com/gen/billing"]
        );
        let single = "package main\nimport pb \"example.com/gen/legacy\"\nimport \"fmt\"\n";
        assert_eq!(client_package_evidence(single), vec!["example.com/gen/legacy", "fmt"]);
    }

    #[test]
    fn evidence_reads_each_binding_shape() {
        let cases: &[(&str, &[&str])] = &[
            ("using Grpc.Net.Client;\nusing static GreeterApi.Helpers;\nusing Api = GreeterApi.V2;\nglobal using GreeterApi;\n", &["Grpc.Net.Client", "GreeterApi.Helpers", "GreeterApi.V2", "GreeterApi"]),
            ("import io.grpc.ManagedChannel;\nimport shop.billing.*;\nimport static shop.legacy.Util.helper;\n", &["io.grpc.ManagedChannel", "shop.billing", "shop.legacy.Util.helper"]),
            ("import grpc\nfrom gen.billing import payments_pb2_grpc\nimport gen.legacy.payments_pb2 as lp\n", &["grpc", "gen.billing", "gen.legacy.payments_pb2"]),
            ("import * as grpc from '@grpc/grpc-js';\nimport {\n  PaymentsServiceClient,\n} from './gen/billing/payments_grpc_pb';\nconst x = require(\"./gen/legacy/payments_pb\");\n", &["@grpc/grpc-js", "./gen/billing/payments_grpc_pb", "./gen/legacy/payments_pb"]),
            ("use tonic::transport::Channel;\npub mod billing { tonic::include_proto!(\"billing\"); }\nuse billing::payments_service_client::{PaymentsServiceClient};\n", &["tonic::transport::Channel", "billing", "billing::payments_service_client"]),
            ("require 'grpc'\nrequire_relative 'gen/billing/payments_services_pb'\n", &["grpc", "gen/billing/payments_services_pb"]),
            ("#include <grpcpp/grpcpp.h>\n#include \"billing/payments.grpc.pb.h\"\n", &["grpcpp/grpcpp.h", "billing/payments.grpc.pb.h"]),
            ("<?php\nuse Billing\\PaymentsServiceClient;\n", &["Billing\\PaymentsServiceClient"]),
            ("import 'package:grpc/grpc.dart';\nimport 'package:shop/gen/billing/payments.pbgrpc.dart';\n", &["package:grpc/grpc.dart", "package:shop/gen/billing/payments.pbgrpc.dart"]),
        ];
        for (source, want) in cases {
            assert_eq!(&client_package_evidence(source), want, "source: {source}");
        }
    }

    #[test]
    fn evidence_ignores_statements_prose_and_the_file_body() {
        let source = "using (var channel = GrpcChannel.ForAddress(url)) {}\nusing var ch = GrpcChannel.ForAddress(url);\nselect id\nfrom users where id = 1\n// import billing.Nope;\n";
        assert!(client_package_evidence(source).is_empty(), "got {:?}", client_package_evidence(source));
        let mut deep = "x = 1\n".repeat(EVIDENCE_SCAN_LINES);
        deep.push_str("from gen.billing import payments_pb2_grpc\n");
        assert!(client_package_evidence(&deep).is_empty(), "past the head-of-file bound");
    }

    #[test]
    fn evidence_is_deduplicated_and_capped() {
        let mut source = String::from("import grpc\nimport grpc\n");
        for i in 0..(EVIDENCE_MAX_PATHS + 8) {
            source.push_str(&format!("import pkg{i}\n"));
        }
        let got = client_package_evidence(&source);
        assert_eq!(got.len(), EVIDENCE_MAX_PATHS);
        assert_eq!(got.iter().filter(|p| *p == "grpc").count(), 1);
    }

    #[test]
    fn fallback_client_carries_its_files_package_evidence() {
        let source = "package main\n\nimport (\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/gen/billing\"\n)\n\nfunc Charge() {\n\tc := pb.NewPaymentsServiceClient(conn)\n}\n";
        let out = extract_grpc_client_nodes(source, module_id(), repo());
        assert_eq!(client_qnames(&out), vec!["grpc_client:PaymentsService".to_string()], "qname stays bare");
        let evidence = evidence_of(&out);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].imports, vec!["google.golang.org/grpc", "example.com/gen/billing"]);
        assert_eq!(evidence[0].package, None, "a client names no package of its own");
    }

    #[test]
    fn data_driven_client_carries_evidence_and_no_evidence_means_no_cell() {
        let source = "using Grpc.Net.Client;\nusing GreeterApi;\nvar c = new Greeter.GreeterClient(channel);";
        let out = extract_known_grpc_client_nodes(source, module_id(), repo(), &[svc("Greeter")]);
        let evidence = evidence_of(&out);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].imports, vec!["Grpc.Net.Client", "GreeterApi"]);

        // A stub with no import line at all carries no RPC_PACKAGE cell.
        let bare = extract_grpc_client_nodes("c := pb.NewOrderServiceClient(conn)", module_id(), repo());
        assert_eq!(bare.nodes.len(), 1);
        assert!(bare.nodes[0].cells.is_empty(), "null-free: no evidence, no cell");
    }

    #[test]
    fn rpc_package_cell_decodes_the_service_payload_too() {
        let source = "syntax = \"proto3\";\npackage billing;\noption go_package = \"example.com/gen/billing;billingpb\";\noption csharp_namespace = \"Shop.Billing\";\nservice PaymentsService {\n  rpc Charge (Req) returns (Resp);\n}\n";
        let out = extract_grpc_service_nodes(source, "billing/payments.proto", module_id(), repo());
        let payload = out.nodes[0]
            .cells
            .iter()
            .find_map(|c| match (&c.payload, c.kind == glia_code_domain::cell_type::RPC_PACKAGE) {
                (CellPayload::Json(j), true) => Some(j.clone()),
                _ => None,
            })
            .expect("service carries RPC_PACKAGE");
        let decoded = RpcPackageCell::parse(&payload).expect("decodes");
        assert_eq!(decoded.package.as_deref(), Some("billing"));
        assert_eq!(decoded.go_package.as_deref(), Some("example.com/gen/billing;billingpb"));
        assert_eq!(decoded.csharp_namespace.as_deref(), Some("Shop.Billing"));
        assert_eq!(decoded.java_package, None);
        assert!(decoded.imports.is_empty());
        assert_eq!(RpcPackageCell::parse("not json"), None);
    }

    // ---- LA.17: Connect / Twirp ------------------------------------------

    fn eliza() -> ProtoServiceRef {
        let mut s = svc("ElizaService");
        s.package = Some("connectrpc.eliza.v1".to_string());
        s.rpcs = vec!["Say".to_string(), "Introduce".to_string()];
        s
    }

    fn haberdasher() -> ProtoServiceRef {
        let mut s = svc("Haberdasher");
        s.package = Some("example.haberdasher".to_string());
        s.rpcs = vec!["MakeHat".to_string()];
        s
    }

    const CONNECT_SERVER: &str = "package main\n\nimport (\n\t\"context\"\n\t\"net/http\"\n\n\t\"connectrpc.com/connect\"\n\telizav1 \"example.com/eliza/gen/eliza/v1\"\n\t\"example.com/eliza/gen/eliza/v1/elizav1connect\"\n)\n\ntype elizaServer struct {\n\telizav1connect.UnimplementedElizaServiceHandler\n}\n\nfunc (s *elizaServer) Say(ctx context.Context, req *connect.Request[elizav1.SayRequest]) (*connect.Response[elizav1.SayResponse], error) {\n\treturn connect.NewResponse(&elizav1.SayResponse{}), nil\n}\n\nfunc main() {\n\tmux := http.NewServeMux()\n\tpath, handler := elizav1connect.NewElizaServiceHandler(&elizaServer{})\n\tmux.Handle(path, handler)\n}\n";

    /// The parse of [`CONNECT_SERVER`]: STRUCT elizaServer, its METHOD Say and
    /// FUNCTION main, spans as the Go parser reports them.
    fn connect_server_parse() -> (Vec<Node>, CodeNav, NodeId) {
        let mut nav = CodeNav::default();
        let m = module_id();
        let ty = parse_node(
            &mut nav,
            node_kind::STRUCT,
            "elizaServer",
            "cmd::main::elizaServer",
            (11, 13),
            m,
        );
        let say = parse_node(
            &mut nav,
            node_kind::METHOD,
            "Say",
            "cmd::main::elizaServer::Say",
            (15, 17),
            ty.id,
        );
        let main = parse_node(
            &mut nav,
            node_kind::FUNCTION,
            "main",
            "cmd::main::main",
            (19, 23),
            m,
        );
        let say_id = say.id;
        (vec![ty, say, main], nav, say_id)
    }

    fn proto_rpc(
        source: &str,
        lang: &str,
        known: &[ProtoServiceRef],
        nodes: &[Node],
        nav: &CodeNav,
        twirp: bool,
    ) -> ProtoRpcNodes {
        extract_proto_rpc_nodes(
            source,
            "main.go",
            lang,
            module_id(),
            repo(),
            known,
            nodes,
            nav,
            twirp,
        )
    }

    fn rpc_qnames(out: &ProtoRpcNodes, kind: glia_core::NodeKindId) -> Vec<String> {
        out.nodes
            .iter()
            .filter(|n| out.nav.kind_by_id[&n.id] == kind)
            .map(|n| out.nav.qname_by_id[&n.id].clone())
            .collect()
    }

    fn rpc_id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn edges_of(
        out: &ProtoRpcNodes,
        cat: glia_core::EdgeCategoryId,
    ) -> Vec<(NodeId, NodeId)> {
        out.edges
            .iter()
            .filter(|e| e.category == cat)
            .map(|e| (e.from, e.to))
            .collect()
    }

    fn position_of(out: &ProtoRpcNodes, id: NodeId) -> Option<String> {
        out.nodes
            .iter()
            .find(|n| n.id == id)?
            .cells
            .iter()
            .find_map(|c| match &c.payload {
                CellPayload::Json(j) if c.kind == glia_code_domain::cell_type::POSITION => {
                    Some(j.clone())
                }
                _ => None,
            })
    }

    use glia_code_domain::edge_category as ec;

    #[test]
    fn connect_handler_emits_one_procedure_per_rpc_owned_by_impl_method() {
        let (nodes, nav, say_method) = connect_server_parse();
        let out = proto_rpc(CONNECT_SERVER, "go", &[eliza()], &nodes, &nav, false);
        assert_eq!(
            rpc_qnames(&out, node_kind::RPC_PROCEDURE),
            vec![
                "rpc:connectrpc.eliza.v1.ElizaService.Say".to_string(),
                "rpc:connectrpc.eliza.v1.ElizaService.Introduce".to_string(),
            ],
            "one procedure per proto rpc, in declaration order, package-qualified"
        );
        assert!(rpc_qnames(&out, node_kind::RPC_CALL).is_empty());
        let say = rpc_id(
            node_kind::RPC_PROCEDURE,
            "rpc:connectrpc.eliza.v1.ElizaService.Say",
        );
        assert_eq!(edges_of(&out, ec::HANDLED_BY), vec![(say, say_method)]);
        // Located at the implementing method's span, not the registration line.
        assert_eq!(
            position_of(&out, say).as_deref(),
            Some(r#"{"file":"svc.cs","start_line":15,"end_line":17}"#)
        );
        assert_eq!(out.nav.name_by_id[&say], "ElizaService.Say");
        assert_eq!(out.nav.parent_of[&say], module_id());
        let c = out.counts;
        assert_eq!(
            (
                c.connect_procedures,
                c.connect_calls,
                c.twirp_procedures,
                c.unowned
            ),
            (2, 0, 0, 1)
        );

        // The impl bound to a variable first (`srv := &elizaServer{}`) is the same type.
        let via_var = CONNECT_SERVER.replace(
            "\tpath, handler := elizav1connect.NewElizaServiceHandler(&elizaServer{})",
            "\tsrv := &elizaServer{}\n\tpath, handler := elizav1connect.NewElizaServiceHandler(srv)",
        );
        let out = proto_rpc(&via_var, "go", &[eliza()], &nodes, &nav, false);
        assert_eq!(edges_of(&out, ec::HANDLED_BY), vec![(say, say_method)]);
    }

    #[test]
    fn unimplemented_rpc_is_contained_not_handled() {
        let (nodes, nav, _) = connect_server_parse();
        let out = proto_rpc(CONNECT_SERVER, "go", &[eliza()], &nodes, &nav, false);
        let intro = rpc_id(
            node_kind::RPC_PROCEDURE,
            "rpc:connectrpc.eliza.v1.ElizaService.Introduce",
        );
        assert!(
            !out.edges
                .iter()
                .any(|e| e.from == intro && e.category == ec::HANDLED_BY),
            "an rpc served by the Unimplemented embed is never HANDLED_BY main"
        );
        assert_eq!(edges_of(&out, ec::CONTAINS), vec![(module_id(), intro)]);
        // Located at the registration line (0-indexed 21).
        assert_eq!(
            position_of(&out, intro).as_deref(),
            Some(r#"{"file":"main.go","start_line":21,"end_line":21}"#)
        );
        // A registration whose impl type has no method in this file owns nothing.
        let out = proto_rpc(
            CONNECT_SERVER,
            "go",
            &[eliza()],
            &[],
            &CodeNav::default(),
            false,
        );
        assert!(edges_of(&out, ec::HANDLED_BY).is_empty());
        assert_eq!(out.counts.unowned, 2);
    }

    #[test]
    fn connect_client_calls_only_rpc_methods() {
        let source = "package main\n\nimport (\n\t\"context\"\n\t\"net/http\"\n\n\t\"connectrpc.com/connect\"\n\t\"example.com/eliza/gen/eliza/v1/elizav1connect\"\n)\n\nfunc main() {\n\tclient := elizav1connect.NewElizaServiceClient(http.DefaultClient, \"http://localhost:8080\")\n\tres, _ := client.Say(context.Background(), connect.NewRequest(nil))\n\tclient.Ping()\n\tmyclient.Introduce(nil)\n\t_ = res\n}\n\nfunc once() {\n\telizav1connect.NewElizaServiceClient(http.DefaultClient, \"u\").Introduce(context.Background(), nil)\n}\n";
        let mut nav = CodeNav::default();
        let m = module_id();
        let main = parse_node(
            &mut nav,
            node_kind::FUNCTION,
            "main",
            "main::main",
            (10, 16),
            m,
        );
        let once = parse_node(
            &mut nav,
            node_kind::FUNCTION,
            "once",
            "main::once",
            (18, 20),
            m,
        );
        let (main_id, once_id) = (main.id, once.id);
        let out = proto_rpc(source, "go", &[eliza()], &[main, once], &nav, false);
        assert_eq!(
            rpc_qnames(&out, node_kind::RPC_CALL),
            vec![
                "rpc_call:connectrpc.eliza.v1.ElizaService.Say".to_string(),
                "rpc_call:connectrpc.eliza.v1.ElizaService.Introduce".to_string(),
            ],
            "Say on the binding, Introduce chained on the constructor; not Ping, not myclient"
        );
        assert!(
            rpc_qnames(&out, node_kind::RPC_PROCEDURE).is_empty(),
            "a client is no server"
        );
        let say = rpc_id(
            node_kind::RPC_CALL,
            "rpc_call:connectrpc.eliza.v1.ElizaService.Say",
        );
        let intro = rpc_id(
            node_kind::RPC_CALL,
            "rpc_call:connectrpc.eliza.v1.ElizaService.Introduce",
        );
        assert_eq!(
            edges_of(&out, ec::USES),
            vec![(main_id, say), (once_id, intro)]
        );
        assert_eq!(
            position_of(&out, say).as_deref(),
            Some(r#"{"file":"main.go","start_line":12,"end_line":12}"#)
        );
        assert_eq!(out.counts.connect_calls, 2);

        // The same client in a plain gRPC file (no connect import) is not Connect.
        let grpc = source.replace("\"connectrpc.com/connect\"", "\"google.golang.org/grpc\"");
        assert!(
            proto_rpc(&grpc, "go", &[eliza()], &[], &CodeNav::default(), false)
                .nodes
                .is_empty()
        );
    }

    #[test]
    fn connect_es_lower_camel_calls_map_to_rpc_names() {
        let source = "import { createClient } from \"@connectrpc/connect\";\nimport { createConnectTransport } from \"@connectrpc/connect-web\";\nimport { ElizaService } from \"./gen/eliza_pb\";\n\nconst transport = createConnectTransport({ baseUrl: \"http://localhost:8080\" });\nconst client = createClient(ElizaService, transport);\n\nexport async function talk(sentence: string) {\n  const res = await client.say({ sentence });\n  client.close();\n  return res.sentence;\n}\n\nclient.introduce({ name: \"boot\" });\n";
        let mut nav = CodeNav::default();
        let talk = parse_node(
            &mut nav,
            node_kind::FUNCTION,
            "talk",
            "src::eliza::talk",
            (7, 11),
            module_id(),
        );
        let talk_id = talk.id;
        let out = extract_proto_rpc_nodes(
            source,
            "src/eliza.ts",
            "typescript",
            module_id(),
            repo(),
            &[eliza()],
            &[talk],
            &nav,
            false,
        );
        let say = rpc_id(
            node_kind::RPC_CALL,
            "rpc_call:connectrpc.eliza.v1.ElizaService.Say",
        );
        let intro = rpc_id(
            node_kind::RPC_CALL,
            "rpc_call:connectrpc.eliza.v1.ElizaService.Introduce",
        );
        assert_eq!(
            out.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![say, intro]
        );
        assert_eq!(edges_of(&out, ec::USES), vec![(talk_id, say)]);
        // A module-level call has no enclosing function: the module holds it.
        assert_eq!(edges_of(&out, ec::CONTAINS), vec![(module_id(), intro)]);
        assert_eq!(
            position_of(&out, say).as_deref(),
            Some(r#"{"file":"src/eliza.ts","start_line":8,"end_line":8}"#)
        );
        // Annotated and promise-client bindings read the same way.
        let annotated = source.replace(
            "const client = createClient(ElizaService, transport);",
            "const client: PromiseClient<typeof ElizaService> = createPromiseClient(eliza.ElizaService, transport);",
        );
        let out = extract_proto_rpc_nodes(
            &annotated,
            "src/eliza.ts",
            "typescript",
            module_id(),
            repo(),
            &[eliza()],
            &[],
            &CodeNav::default(),
            false,
        );
        assert_eq!(out.nodes.len(), 2);
        // Without the connect-es import the factory is somebody else's.
        let plain = source.replace("@connectrpc/connect", "./my-connect");
        let out = extract_proto_rpc_nodes(
            &plain,
            "src/eliza.ts",
            "typescript",
            module_id(),
            repo(),
            &[eliza()],
            &[],
            &CodeNav::default(),
            false,
        );
        assert!(out.nodes.is_empty());
    }

    const TWIRP_CLIENT: &str = "package main\n\nimport (\n\t\"context\"\n\t\"net/http\"\n\n\tpb \"example.com/twirp/rpc/haberdasher\"\n)\n\nfunc main() {\n\tclient := pb.NewHaberdasherProtobufClient(\"http://localhost:8080\", &http.Client{})\n\that, _ := client.MakeHat(context.Background(), &pb.Size{Inches: 12})\n\t_ = hat\n}\n";

    #[test]
    fn twirp_needs_the_repo_gate() {
        assert!(!may_hold_proto_rpc(TWIRP_CLIENT, false));
        assert!(may_hold_proto_rpc(TWIRP_CLIENT, true));
        let out = proto_rpc(
            TWIRP_CLIENT,
            "go",
            &[haberdasher()],
            &[],
            &CodeNav::default(),
            false,
        );
        assert!(
            out.nodes.is_empty(),
            "no twitchtv/twirp in the repo: not Twirp"
        );
        let out = proto_rpc(
            TWIRP_CLIENT,
            "go",
            &[haberdasher()],
            &[],
            &CodeNav::default(),
            true,
        );
        assert_eq!(
            rpc_qnames(&out, node_kind::RPC_CALL),
            vec!["rpc_call:example.haberdasher.Haberdasher.MakeHat".to_string()]
        );
        // The gate is Go's: a TS file in a Twirp repo reads nothing.
        let out = extract_proto_rpc_nodes(
            TWIRP_CLIENT,
            "a.ts",
            "typescript",
            module_id(),
            repo(),
            &[haberdasher()],
            &[],
            &CodeNav::default(),
            true,
        );
        assert!(out.nodes.is_empty());
    }

    #[test]
    fn twirp_server_and_client() {
        let server = "package main\n\nimport (\n\t\"context\"\n\t\"net/http\"\n\n\tpb \"example.com/twirp/rpc/haberdasher\"\n)\n\ntype HaberdasherServer struct{}\n\nfunc (s *HaberdasherServer) MakeHat(ctx context.Context, size *pb.Size) (*pb.Hat, error) {\n\treturn &pb.Hat{}, nil\n}\n\nfunc NewGreeterServer() *HaberdasherServer { return nil }\n\nfunc main() {\n\ttwirpHandler := pb.NewHaberdasherServer(&HaberdasherServer{})\n\thttp.ListenAndServe(\":8080\", twirpHandler)\n}\n";
        let mut nav = CodeNav::default();
        let m = module_id();
        let ty = parse_node(
            &mut nav,
            node_kind::STRUCT,
            "HaberdasherServer",
            "cmd::server::main::HaberdasherServer",
            (9, 9),
            m,
        );
        let make = parse_node(
            &mut nav,
            node_kind::METHOD,
            "MakeHat",
            "cmd::server::main::HaberdasherServer::MakeHat",
            (11, 13),
            ty.id,
        );
        let make_id = make.id;
        let mut greeter = svc("Greeter");
        greeter.rpcs = vec!["SayHello".to_string()];
        let out = proto_rpc(
            server,
            "go",
            &[greeter, haberdasher()],
            &[ty, make],
            &nav,
            true,
        );
        let proc_id = rpc_id(
            node_kind::RPC_PROCEDURE,
            "rpc:example.haberdasher.Haberdasher.MakeHat",
        );
        assert_eq!(
            out.nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![proc_id],
            "unqualified NewGreeterServer() is no hit"
        );
        assert_eq!(edges_of(&out, ec::HANDLED_BY), vec![(proc_id, make_id)]);
        let c = out.counts;
        assert_eq!(
            (
                c.twirp_procedures,
                c.twirp_calls,
                c.connect_procedures,
                c.unowned
            ),
            (1, 0, 0, 0)
        );

        let out = proto_rpc(
            TWIRP_CLIENT,
            "go",
            &[haberdasher()],
            &[],
            &CodeNav::default(),
            true,
        );
        assert_eq!(
            (out.counts.twirp_calls, out.counts.twirp_procedures),
            (1, 0)
        );
        // A JSON client and a chained call read the same way.
        let json = "package main\n\nfunc main() {\n\tpb.NewHaberdasherJSONClient(\"u\", &http.Client{}).MakeHat(ctx, &pb.Size{})\n}\n";
        let out = proto_rpc(json, "go", &[haberdasher()], &[], &CodeNav::default(), true);
        assert_eq!(
            rpc_qnames(&out, node_kind::RPC_CALL),
            vec!["rpc_call:example.haberdasher.Haberdasher.MakeHat".to_string()]
        );
    }

    #[test]
    fn ambiguous_service_name_is_skipped() {
        let mut other = eliza();
        other.package = Some("legacy.eliza".to_string());
        let (nodes, nav, _) = connect_server_parse();
        let out = proto_rpc(CONNECT_SERVER, "go", &[eliza(), other], &nodes, &nav, false);
        assert!(
            out.nodes.is_empty() && out.edges.is_empty(),
            "never guess the package"
        );
        assert_eq!(out.counts.ambiguous, 1);
        assert!(out.counts.any());
        // The same service declared twice in ONE package (a copied .proto) is not ambiguous.
        let mut copy = eliza();
        copy.go_package = Some("example.com/copy".to_string());
        let out = proto_rpc(CONNECT_SERVER, "go", &[eliza(), copy], &nodes, &nav, false);
        assert_eq!(
            (out.counts.connect_procedures, out.counts.ambiguous),
            (2, 0)
        );
    }

    #[test]
    fn generated_connect_file_is_ignored() {
        let generated = "// Code generated by protoc-gen-connect-go. DO NOT EDIT.\n\npackage elizav1connect\n\nimport (\n\tconnect \"connectrpc.com/connect\"\n)\n\nfunc NewElizaServiceClient(httpClient connect.HTTPClient, baseURL string) ElizaServiceClient {\n\treturn &elizaServiceClient{say: connect.NewClient[SayRequest, SayResponse](httpClient, baseURL)}\n}\n\nfunc NewElizaServiceHandler(svc ElizaServiceHandler) (string, http.Handler) {\n\treturn \"/connectrpc.eliza.v1.ElizaService/\", nil\n}\n";
        assert!(
            proto_rpc(generated, "go", &[eliza()], &[], &CodeNav::default(), false)
                .nodes
                .is_empty()
        );
        // Without the banner, a declaration keyword before the needle still rejects it.
        let decl = "import \"connectrpc.com/connect\"\n\nfunc elizav1connect.NewElizaServiceHandler(svc ElizaServiceHandler) {}\n";
        assert!(
            proto_rpc(decl, "go", &[eliza()], &[], &CodeNav::default(), false)
                .nodes
                .is_empty()
        );
    }
}
