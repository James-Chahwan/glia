use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use repo_graph_code_domain::endpoint::{ClientEndpoint, push_client_endpoint, url_to_path};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_scala::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc::default();

    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    acc.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: file_cells(&root, src, file_rel_path),
    });
    let module_simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    acc.nav
        .record(module_id, module_simple, module_qname, node_kind::MODULE, None);

    visit_top(root, src, file_rel_path, module_qname, module_id, module_id, repo, &mut acc);
    scan_scala_routes(source, repo, &mut acc);
    if acc.endpoint_hits > 0 {
        eprintln!(
            "[scala-http-client] {} endpoints in {}",
            acc.endpoint_hits, file_rel_path
        );
    }

    Ok(FileParse {
        nodes: acc.nodes,
        edges: acc.edges,
        imports: acc.imports,
        calls: acc.calls,
        refs: acc.refs,
        nav: acc.nav,
        properties: Default::default(),
    })
}

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    calls: Vec<CallSite>,
    refs: Vec<UnresolvedRef>,
    nav: CodeNav,
    /// Dedups the ENDPOINT node per `(method, path)` within a file.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Client HTTP call sites emitted in this file (drives the fired_on marker).
    endpoint_hits: usize,
}

fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_declaration" => collect_import(child, src, parent_qname, acc),
            "package_clause" => collect_package(child, src, parent_qname, acc),
            "object_definition" => {
                visit_type_def(child, src, file_rel, parent_qname, parent_id, module_id, repo, node_kind::CLASS, acc);
            }
            "class_definition" => {
                visit_type_def(child, src, file_rel, parent_qname, parent_id, module_id, repo, node_kind::CLASS, acc);
            }
            "trait_definition" => {
                visit_type_def(child, src, file_rel, parent_qname, parent_id, module_id, repo, node_kind::INTERFACE, acc);
            }
            "function_definition" | "val_definition" | "var_definition" => {
                visit_function(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_type_def(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    kind: repo_graph_core::NodeKindId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav.record(id, name, &qname, kind, Some(parent_id));

    emit_heritage_refs(&node, src, id, module_id, acc);

    if let Some(body) = node.child_by_field_name("body") {
        visit_body_members(body, src, file_rel, &qname, id, module_id, repo, acc);
    }
}

/// Scala class/trait/object heritage. The `extend`/`extends_clause` node holds
/// one or more `type` fields: `class Dog extends Animal with Runnable` →
/// `Animal` (first, the primary supertype) becomes INHERITS_FROM, each mixin
/// trait after `with` becomes IMPLEMENTS. We emit an `UnresolvedRef` with a
/// `Bare(TypeName)` qualifier; `resolve_refs` binds it to the uniquely-named
/// class/interface node across the repo and forms the heritage edge.
fn emit_heritage_refs(node: &TsNode, src: &[u8], from_id: NodeId, module_id: NodeId, acc: &mut Acc) {
    let Some(ext) = node.child_by_field_name("extend") else {
        return;
    };
    let mut cursor = ext.walk();
    let mut first = true;
    for ty in ext.children_by_field_name("type", &mut cursor) {
        // Strip generic args (`Ordered[Dog]` → `Ordered`) and take the trailing
        // simple name (`pkg.Animal` → `Animal`).
        let raw = text_of(ty, src);
        let base = raw.split('[').next().unwrap_or(raw).trim();
        let simple = base.rsplit(['.', ':']).next().unwrap_or(base).trim();
        if simple.is_empty() {
            continue;
        }
        let category = if first {
            edge_category::INHERITS_FROM
        } else {
            edge_category::IMPLEMENTS
        };
        first = false;
        acc.refs.push(UnresolvedRef {
            from: from_id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(simple.to_string()),
            category,
        });
    }
}

fn visit_body_members(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "function_definition" | "val_definition" | "var_definition" => {
                visit_method(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "object_definition" | "class_definition" => {
                visit_type_def(child, src, file_rel, parent_qname, parent_id, module_id, repo, node_kind::CLASS, acc);
            }
            "trait_definition" => {
                visit_type_def(child, src, file_rel, parent_qname, parent_id, module_id, repo, node_kind::INTERFACE, acc);
            }
            _ => {}
        }
    }
}

fn visit_function(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::FUNCTION, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        // Top-level `def` (parent is a MODULE): a bare `foo()` binds against the
        // module's top-level symbols, so keep it `Bare`.
        collect_calls_in(body, src, id, false, acc);
        let hits = collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
        acc.endpoint_hits += hits;
    }
}

fn visit_method(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::METHOD, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        // Method inside a type body (parent is CLASS/INTERFACE): an unqualified
        // `foo()` is an implicit `this.foo()` — a sibling method of the enclosing
        // type. Classify as `SelfMethod` so `resolve_calls` binds it against
        // `class_methods[<enclosing type>]` (a bare `module_symbols` lookup would
        // miss, since sibling methods live under the type, not the module).
        collect_calls_in(body, src, id, true, acc);
        let hits = collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
        acc.endpoint_hits += hits;
    }
}

fn collect_import(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let path = text.trim_start_matches("import ").trim();
    acc.imports.push(ImportStmt {
        from_module: from_module.to_string(),
        target: ImportTarget::Module {
            path: path.to_string(),
            alias: None,
        },
    });
}

fn collect_package(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let pkg = text.trim_start_matches("package ").trim();
    acc.imports.push(ImportStmt {
        from_module: from_module.to_string(),
        target: ImportTarget::Module {
            path: pkg.to_string(),
            alias: None,
        },
    });
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, in_type: bool, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some(func) = n.child_by_field_name("function")
        {
            let qualifier = classify_call(func, src, in_type);
            acc.calls.push(CallSite { from, qualifier });
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "function_definition" | "class_definition" | "object_definition" | "lambda_expression"
            ) {
                stack.push(child);
            }
        }
    }
}

fn classify_call(func_node: TsNode, src: &[u8], in_type: bool) -> CallQualifier {
    match func_node.kind() {
        "identifier" => {
            let name = text_of(func_node, src).to_string();
            // Inside a type body an unqualified call is an implicit self-call;
            // emit `SelfMethod` so it binds against the enclosing type's methods.
            // At module scope it stays `Bare` (binds against module symbols).
            if in_type {
                CallQualifier::SelfMethod(name)
            } else {
                CallQualifier::Bare(name)
            }
        }
        "field_expression" => {
            let obj = func_node
                .child_by_field_name("value")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            let field = func_node
                .child_by_field_name("field")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            if obj == "this" {
                CallQualifier::SelfMethod(field.to_string())
            } else if func_node
                .child_by_field_name("value")
                .is_some_and(|v| v.kind() == "identifier")
            {
                CallQualifier::Attribute {
                    base: obj.to_string(),
                    name: field.to_string(),
                }
            } else {
                CallQualifier::ComplexReceiver {
                    receiver: obj.to_string(),
                    name: field.to_string(),
                }
            }
        }
        _ => CallQualifier::ComplexReceiver {
            receiver: text_of(func_node, src).to_string(),
            name: String::new(),
        },
    }
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Client HTTP calls (Play WS / sttp / Akka HTTP) -> shared ENDPOINT nodes
// ---------------------------------------------------------------------------

const SCALA_HTTP_VERBS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Upper-case `s` if it names an HTTP verb.
fn http_verb(s: &str) -> Option<String> {
    let up = s.trim().to_ascii_uppercase();
    SCALA_HTTP_VERBS.contains(&up.as_str()).then_some(up)
}

/// Outbound HTTP call sites in a def/method body become shared ENDPOINT nodes
/// (+ a CALLS edge from the enclosing symbol) so `HttpStackResolver` can pair
/// them with a server ROUTE. Three Scala client shapes are recognised:
///   Play WS  `ws.url("…").get()`        — URL on `.url(…)`, verb further up the chain
///   sttp     `basicRequest.get(uri"…")` — verb on the call, path in the `uri` interpolator
///   Akka     `HttpRequest(uri = "…", method = HttpMethods.POST)` / `HttpRequest(GET, "…")`
/// Returns the number of call sites emitted.
fn collect_client_endpoints_in(
    body: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> usize {
    let mut hits = 0;
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some((method, raw, interpolated)) = client_call_candidate(n, src)
            && let Some(path) = url_to_path(&raw)
        {
            let pos = n.start_position();
            // A literal path with an explicit verb is Strong; an interpolated
            // path or a verb we had to infer is Medium (same rule as swift).
            let confidence = if interpolated {
                Confidence::Medium
            } else {
                Confidence::Strong
            };
            let ep = ClientEndpoint {
                method,
                path,
                file: file_rel.to_string(),
                line: pos.row + 1,
                col: pos.column + 1,
                confidence,
            };
            push_client_endpoint(
                repo,
                &ep,
                from,
                &mut acc.nodes,
                &mut acc.edges,
                &mut acc.nav,
                &mut acc.endpoint_seen,
            );
            hits += 1;
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            // A nested definition is visited (and attributed) on its own.
            if !matches!(
                child.kind(),
                "function_definition" | "class_definition" | "object_definition"
            ) {
                stack.push(child);
            }
        }
    }
    hits
}

/// `(method, raw_url, interpolated_or_inferred)` for a `call_expression` that is
/// a client HTTP call, else None.
fn client_call_candidate(n: TsNode, src: &[u8]) -> Option<(String, String, bool)> {
    let func = n.child_by_field_name("function")?;
    let args = n.child_by_field_name("arguments")?;
    match func.kind() {
        // Akka HTTP: `HttpRequest(…)` (also the inner call of `singleRequest(…)`).
        "identifier" if text_of(func, src) == "HttpRequest" => akka_http_request(args, src),
        "field_expression" => {
            let field = func
                .child_by_field_name("field")
                .map(|f| text_of(f, src))
                .unwrap_or("");
            if field == "url" {
                // Play WS: the URL is here, the verb is up the fluent chain.
                let (raw, interp) = first_string_arg(args, src)?;
                let (method, inferred) = match play_ws_verb(n, src) {
                    Some(v) => (v, false),
                    None => ("GET".to_string(), true),
                };
                Some((method, raw, interp || inferred))
            } else if let Some(verb) = http_verb(field) {
                // sttp: only a `uri"…"` interpolator argument counts, so a plain
                // `config.get("user." + id)` is not mistaken for an HTTP call.
                let arg = args.named_child(0)?;
                let is_uri = arg.kind() == "interpolated_string_expression"
                    && arg
                        .child_by_field_name("interpolator")
                        .map(|i| text_of(i, src))
                        == Some("uri");
                if !is_uri {
                    return None;
                }
                let (raw, interp) = scala_string_path(arg, src)?;
                Some((verb, raw, interp))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Walk UP a Play WS fluent chain from the `.url(…)` call to the verb call
/// (`.get()`, `.post(body)`, possibly past `.addHttpHeaders(…)` etc).
fn play_ws_verb(url_call: TsNode, src: &[u8]) -> Option<String> {
    let mut cur = url_call;
    for _ in 0..8 {
        let parent = cur.parent()?;
        if parent.kind() != "field_expression"
            || !parent
                .child_by_field_name("value")
                .is_some_and(|v| v == cur)
        {
            return None;
        }
        let field = parent
            .child_by_field_name("field")
            .map(|f| text_of(f, src))
            .unwrap_or("");
        if let Some(verb) = http_verb(field) {
            return Some(verb);
        }
        let grand = parent.parent()?;
        if grand.kind() != "call_expression" {
            return None;
        }
        cur = grand;
    }
    None
}

/// Akka `HttpRequest(…)` arguments: named `uri =` / `method =`, or the
/// positional `HttpRequest(GET, "…")` form. Verb defaults to GET.
fn akka_http_request(args: TsNode, src: &[u8]) -> Option<(String, String, bool)> {
    let mut method: Option<String> = None;
    let mut url: Option<(String, bool)> = None;
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        let (key, value) = if arg.kind() == "assignment_expression" {
            let key = arg
                .child_by_field_name("left")
                .map(|l| text_of(l, src))
                .unwrap_or("");
            match arg.child_by_field_name("right") {
                Some(right) => (key, right),
                None => continue,
            }
        } else {
            ("", arg)
        };
        if (key == "method" || key.is_empty())
            && method.is_none()
            && let Some(verb) = verb_of_expr(value, src)
        {
            method = Some(verb);
            continue;
        }
        if (key == "uri" || key.is_empty())
            && url.is_none()
            && let Some(s) = scala_string_path(value, src)
        {
            url = Some(s);
        }
    }
    let (raw, interp) = url?;
    let inferred = method.is_none();
    Some((method.unwrap_or_else(|| "GET".to_string()), raw, interp || inferred))
}

/// `GET` (identifier) or `HttpMethods.POST` (field_expression) → the verb.
fn verb_of_expr(n: TsNode, src: &[u8]) -> Option<String> {
    let text = match n.kind() {
        "identifier" => text_of(n, src),
        "field_expression" => n
            .child_by_field_name("field")
            .map(|f| text_of(f, src))
            .unwrap_or(""),
        _ => return None,
    };
    http_verb(text)
}

/// First string-ish argument of an `arguments` node.
fn first_string_arg(args: TsNode, src: &[u8]) -> Option<(String, bool)> {
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .find_map(|a| scala_string_path(a, src))
}

/// Reconstruct a scala `string` or `interpolated_string_expression`, replacing
/// every `$id` / `${expr}` with `${…}` so it normalises like a TS template path
/// (`normalise_http_path` collapses any segment containing `${` to `{}`).
/// Returns `(text_without_quotes, had_interpolation)`.
fn scala_string_path(n: TsNode, src: &[u8]) -> Option<(String, bool)> {
    match n.kind() {
        "string" => Some((text_of(n, src).trim_matches('"').to_string(), false)),
        "interpolated_string_expression" => {
            let mut cursor = n.walk();
            let lit = n
                .named_children(&mut cursor)
                .find(|c| c.kind() == "interpolated_string")?;
            let text = text_of(lit, src);
            let base = lit.start_byte();
            let mut out = String::new();
            let mut at = 0usize;
            let mut interpolated = false;
            let mut inner = lit.walk();
            for child in lit.named_children(&mut inner) {
                if child.kind() != "interpolation" {
                    continue;
                }
                let (start, end) = (
                    child.start_byte().saturating_sub(base),
                    child.end_byte().saturating_sub(base),
                );
                if start < at {
                    continue;
                }
                out.push_str(text.get(at..start).unwrap_or(""));
                out.push_str("${…}");
                at = end;
                interpolated = true;
            }
            out.push_str(text.get(at..).unwrap_or(""));
            Some((out.trim_matches('"').to_string(), interpolated))
        }
        _ => None,
    }
}

fn file_cells(root: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*root, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(repo_graph_doc::position_json(root, file_rel)),
        },
    ]
}

fn entity_cells(node: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    let mut cells = vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(repo_graph_doc::position_json(node, file_rel)),
        },
    ];
    if let Some(doc) = repo_graph_doc::leading_doc(node, src) {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    cells
}

// ============================================================================
// Scala route extraction (v0.4.11a R-scala)
// ============================================================================
//
// Coverage (text scan — Scala DSLs are deeply nested and awkward to track via
// tree-sitter alone):
//
//   Akka HTTP:   path("users") { ... }                 → ANY /users
//   http4s:      case GET -> Root / "users"            → GET /users
//                case POST -> Root / "u" / IntVar(id)  → POST /u/:id
//
// Play Framework's primary routing is a `conf/routes` file (not Scala source)
// parsed by sbt compiler — out of scope for this parser. Play annotation
// forms are rare in practice.

fn scan_scala_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    let mut seen = std::collections::HashSet::new();

    // Akka HTTP `path("X") {` — emit ANY /X.
    let needle = "path(";
    let mut idx = 0;
    while let Some(pos) = source[idx..].find(needle) {
        let start = idx + pos + needle.len();
        if let Some(path) = first_string_literal_scala(&source[start..])
            && is_pathlike(&path)
        {
            emit_scala_route("ANY", &path, repo, acc, &mut seen);
        }
        idx = start;
    }

    // http4s pattern `case METHOD -> Root / "seg" ...` — split by method
    // and accumulate segments.
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        let needle = format!("{method} -> Root");
        let mut idx = 0;
        while let Some(pos) = source[idx..].find(&needle) {
            let start = idx + pos + needle.len();
            let after = &source[start..];
            let path = collect_http4s_path(after);
            if !path.is_empty() {
                emit_scala_route(method, &path, repo, acc, &mut seen);
            } else {
                // Bare `METHOD -> Root` with no segments = root path.
                emit_scala_route(method, "/", repo, acc, &mut seen);
            }
            idx = start;
        }
    }
}

fn collect_http4s_path(after: &str) -> String {
    // Consume a sequence of `/ "segment"` or `/ IntVar(id)` tokens.
    let mut out = String::new();
    let mut rest = after;
    loop {
        let trimmed = rest.trim_start();
        let Some(slash_off) = trimmed.strip_prefix('/') else { break };
        let next = slash_off.trim_start();
        if let Some(lit_rest) = next.strip_prefix('"')
            && let Some(end) = lit_rest.find('"')
        {
            out.push('/');
            out.push_str(&lit_rest[..end]);
            rest = &lit_rest[end + 1..];
            continue;
        }
        // Segment variable: IntVar / LongVar / UUIDVar → treat as :id
        let next_bytes = next.as_bytes();
        if next_bytes.first().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') {
            let end = next
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(next.len());
            let ident = &next[..end];
            // Common http4s Var extractors — inject :id placeholder.
            if matches!(
                ident,
                "IntVar" | "LongVar" | "UUIDVar" | "IntPathVar" | "LongPathVar"
            ) {
                out.push_str("/:id");
                // Skip the `(id)` call tail if present.
                let tail = &next[end..];
                let tail = tail.trim_start();
                rest = if let Some(after_paren) = tail.strip_prefix('(')
                    && let Some(cp) = after_paren.find(')')
                {
                    &after_paren[cp + 1..]
                } else {
                    tail
                };
                continue;
            }
            // Unknown identifier — treat as dynamic segment.
            out.push_str("/:");
            out.push_str(ident);
            rest = &next[end..];
            continue;
        }
        break;
    }
    out
}

fn first_string_literal_scala(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let rest = &s[i + 1..];
                let end = rest.find('"')?;
                let lit = &rest[..end];
                if lit.is_empty() || lit.len() > 256 {
                    return None;
                }
                return Some(lit.to_string());
            }
            b')' | b'{' | b';' | b'\n' if i > 0 => return None,
            _ => i += 1,
        }
    }
    None
}

fn is_pathlike(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':' | '*'))
}

fn emit_scala_route(
    method: &str,
    path: &str,
    repo: RepoId,
    acc: &mut Acc,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let key = (method.to_string(), path.clone());
    if !seen.insert(key) {
        return;
    }
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(method.to_string()),
        }],
    });
    acc.nav
        .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    #[test]
    fn object_and_trait() {
        let source = r#"
trait UserService {
  def getUser(id: Int): User
}

object UserServiceImpl {
  def getUser(id: Int): User = {
    db.findById(id)
  }
}
"#;
        let fp = parse_file(source, "src/UserService.scala", "src::UserService", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
    }

    #[test]
    fn class_with_methods() {
        let source = r#"
class Config {
  def load(): Map[String, String] = {
    readFile("config.yml")
  }
  def save(): Unit = {}
}
"#;
        let fp = parse_file(source, "src/Config.scala", "src::Config", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 2);
    }

    #[test]
    fn intra_object_call_is_self_method() {
        // `compute` calls sibling `helper` bare inside `object Calc`. The parser
        // must emit `SelfMethod("helper")` (not `Bare`) so `resolve_calls` binds
        // it against the enclosing type's methods (`class_methods[Calc]`).
        let source = r#"
object Calc {
  def helper(x: Int): Int = x + 1

  def compute(n: Int): Int = {
    helper(n) * 2
  }
}
"#;
        let fp = parse_file(source, "app.scala", "app::Calc", repo()).unwrap();
        assert!(
            fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::SelfMethod("helper".to_string())),
            "expected SelfMethod(\"helper\") CallSite from compute(): {:?}",
            fp.calls
        );
        assert!(
            !fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::Bare("helper".to_string())),
            "intra-object bare call must be SelfMethod, not Bare: {:?}",
            fp.calls
        );
    }

    #[test]
    fn top_level_call_is_bare() {
        // A bare call from a top-level `def` (module scope) stays `Bare` so it
        // binds against module-level symbols.
        let source = r#"
def helper(x: Int): Int = x + 1

def compute(n: Int): Int = {
  helper(n) * 2
}
"#;
        let fp = parse_file(source, "app.scala", "app", repo()).unwrap();
        assert!(
            fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::Bare("helper".to_string())),
            "expected Bare(\"helper\") CallSite from top-level compute(): {:?}",
            fp.calls
        );
    }

    #[test]
    fn class_extends_emits_inherits_from_ref() {
        // `class Dog extends Animal` must emit an UnresolvedRef with a
        // Bare("Animal") qualifier under INHERITS_FROM, from the CLASS node.
        // A `with` mixin trait becomes an IMPLEMENTS ref. resolve_refs binds
        // the bare type name to the trait/class node across the repo.
        let source = r#"
trait Animal {
  def speak(): String
}

trait Runnable {
  def run(): Unit
}

class Dog extends Animal with Runnable {
  def speak(): String = "woof"
  def run(): Unit = {}
}
"#;
        let fp = parse_file(source, "src/Animals.scala", "src::Animals", repo()).unwrap();

        let dog_id = fp
            .nav
            .name_by_id
            .iter()
            .find(|(_, n)| **n == "Dog")
            .map(|(id, _)| *id)
            .expect("Dog CLASS node recorded");

        // INHERITS_FROM ref: Dog → Animal (primary supertype).
        assert!(
            fp.refs.iter().any(|r| r.from == dog_id
                && r.category == edge_category::INHERITS_FROM
                && r.qualifier == CallQualifier::Bare("Animal".to_string())),
            "expected INHERITS_FROM Bare(\"Animal\") ref from Dog: {:?}",
            fp.refs
        );

        // IMPLEMENTS ref: Dog → Runnable (mixin trait after `with`).
        assert!(
            fp.refs.iter().any(|r| r.from == dog_id
                && r.category == edge_category::IMPLEMENTS
                && r.qualifier == CallQualifier::Bare("Runnable".to_string())),
            "expected IMPLEMENTS Bare(\"Runnable\") ref from Dog: {:?}",
            fp.refs
        );

        // from_module must be the module node (whose binding table resolves).
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::Animals");
        assert!(
            fp.refs
                .iter()
                .all(|r| r.category != edge_category::INHERITS_FROM || r.from_module == module_id),
            "heritage refs must carry the module node as from_module: {:?}",
            fp.refs
        );
    }

    #[test]
    fn imports() {
        let source = r#"
import scala.collection.mutable
import akka.actor.ActorSystem
"#;
        let fp = parse_file(source, "src/App.scala", "src::App", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
    }

    fn route_id(method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            &format!("{method} {path}"),
        )
    }

    #[test]
    fn akka_http_path_routes_emit() {
        let source = r#"
val route = path("users") {
  get {
    complete("ok")
  }
} ~ path("admin") {
  post {
    complete("ok")
  }
}
"#;
        let fp = parse_file(source, "src/Routes.scala", "src::Routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/admin")));
    }

    fn endpoint_names(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ENDPOINT)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn play_ws_url_get_emits_endpoint() {
        // The URL sits on `.url(…)` but the verb is further up the fluent
        // chain — getting that wrong emits every Play WS call as GET.
        let source = r#"
class ApiClient(ws: WSClient) {
  def fetchUser(id: String) = ws.url(s"http://users-svc/api/users/$id").get()
  def createUser(body: String) = ws.url("/api/users").post(body)
  def headers(body: String) = ws.url("/api/orders").addHttpHeaders("a" -> "b").put(body)
}
"#;
        let fp = parse_file(source, "client/ApiClient.scala", "client::ApiClient", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /api/users/${…}".to_string(),
                "POST /api/users".to_string(),
                "PUT /api/orders".to_string(),
            ],
            "nav: {:?}",
            fp.nav.name_by_id
        );

        // Each ENDPOINT carries a CALLS edge from the enclosing METHOD.
        let fetch = fp
            .nav
            .name_by_id
            .iter()
            .find(|(id, n)| *n == "fetchUser" && fp.nav.kind_by_id[id] == node_kind::METHOD)
            .map(|(id, _)| *id)
            .expect("fetchUser METHOD recorded");
        let ep = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            "endpoint:GET:/api/users/${…}",
        );
        assert!(
            fp.edges
                .iter()
                .any(|e| e.from == fetch && e.to == ep && e.category == edge_category::CALLS),
            "expected CALLS fetchUser -> ENDPOINT: {:?}",
            fp.edges
        );
    }

    #[test]
    fn sttp_uri_interpolation_becomes_wildcard() {
        // sttp puts the path in a `uri"…"` interpolator on the verb call; any
        // `${…}` segment must survive so it normalises against `/items/{id}`.
        let source = r#"
object Client {
  def list() = basicRequest.get(uri"http://orders-svc/api/orders")
  def one(id: String) = basicRequest.post(uri"/api/items/$id").body("x")
}
"#;
        let fp = parse_file(source, "client/Client.scala", "client::Client", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /api/orders".to_string(),
                "POST /api/items/${…}".to_string(),
            ],
            "nav: {:?}",
            fp.nav.name_by_id
        );
    }

    #[test]
    fn akka_http_request_named_and_positional() {
        // `HttpRequest(uri = …)` defaults to GET; `method = HttpMethods.POST`
        // and the positional `HttpRequest(GET, "…")` form set it explicitly.
        let source = r#"
object Client {
  def a() = Http().singleRequest(HttpRequest(uri = "http://svc/api/orders"))
  def b() = Http().singleRequest(HttpRequest(method = HttpMethods.POST, uri = "/api/orders"))
  def c() = Http().singleRequest(HttpRequest(GET, "/api/things"))
}
"#;
        let fp = parse_file(source, "client/Akka.scala", "client::Akka", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /api/orders".to_string(),
                "GET /api/things".to_string(),
                "POST /api/orders".to_string(),
            ],
            "nav: {:?}",
            fp.nav.name_by_id
        );
    }

    #[test]
    fn scala_client_calls_emit_no_route() {
        // A client call site must never be mistaken for a server ROUTE — the
        // `scan_scala_routes` text scan is server-side only.
        let source = r#"
class ApiClient(ws: WSClient) {
  def fetchUser(id: String) = ws.url("/api/users").get()
}
"#;
        let fp = parse_file(source, "client/ApiClient.scala", "client::ApiClient", repo()).unwrap();
        assert_eq!(
            fp.nav
                .kind_by_id
                .values()
                .filter(|k| **k == node_kind::ROUTE)
                .count(),
            0,
            "client file emitted a ROUTE: {:?}",
            fp.nav.name_by_id
        );
        assert_eq!(endpoint_names(&fp), vec!["GET /api/users".to_string()]);
    }

    #[test]
    fn scala_non_url_string_is_dropped() {
        // `config.get("user." + id)` has a verb-shaped method name and a string
        // argument but is not an HTTP call; `ws.url("relative")` has no path.
        let source = r#"
class ApiClient(ws: WSClient, config: Config) {
  def cached(id: String) = config.get("user." + id)
  def bare() = ws.url("users-svc").get()
  def named() = client.post("/api/x")
}
"#;
        let fp = parse_file(source, "client/ApiClient.scala", "client::ApiClient", repo()).unwrap();
        assert!(
            endpoint_names(&fp).is_empty(),
            "non-HTTP calls produced ENDPOINTs: {:?}",
            endpoint_names(&fp)
        );
    }

    #[test]
    fn http4s_get_post_routes_emit() {
        let source = r#"
val service = HttpRoutes.of[IO] {
  case GET -> Root / "users" => Ok("list")
  case POST -> Root / "users" => Ok("created")
  case GET -> Root / "users" / IntVar(id) => Ok(s"user $id")
}
"#;
        let fp = parse_file(source, "src/Api.scala", "src::Api", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users/:id")));
    }
}
