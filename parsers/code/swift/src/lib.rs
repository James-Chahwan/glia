use std::collections::HashSet;
use std::sync::OnceLock;

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use repo_graph_code_domain::endpoint::{
    ClientEndpoint, HitExtras, client_url_split, push_client_endpoint_with,
};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_swift::LANGUAGE.into();
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

    let top = visit_top(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    if top.types > 0 && qname_debug() {
        eprintln!(
            "[qname] swift: {} top-level types scoped to {} ({} extensions, {} file-private kept) file={file_rel_path}",
            top.types,
            type_scope(module_qname),
            top.extensions,
            top.file_private
        );
    }

    if acc.self_calls + acc.super_calls > 0 && swift_debug() {
        eprintln!(
            "[swift-calls] self={} super={} file={file_rel_path}",
            acc.self_calls, acc.super_calls
        );
    }

    scan_vapor_routes(source, repo, &mut acc);

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
    /// Dedup for client-HTTP ENDPOINT nodes (Pattern A) — one node per
    /// (method, path) even if the same endpoint is called twice in a file.
    endpoint_seen: HashSet<NodeId>,
    /// LB.7c: type nodes already emitted by this file, so a same-file
    /// `extension T` folds onto `T`'s node instead of pushing a second node,
    /// a second DEFINES edge and a second `children_of` entry. Lookup only.
    type_seen: HashSet<NodeId>,
    /// LA.36a: `self.m()` / `self?.m()` / `Self.m()` and `super.m()` call
    /// sites of this file, for the `[swift-calls]` marker only.
    self_calls: usize,
    super_calls: usize,
}

/// LB.7c: a Swift type belongs to its MODULE (the target directory), not its
/// file, so drop the file-stem segment the engine's `path_to_qname` puts last:
/// `Sources::Shop::Widget` -> `Sources::Shop`, a repo-root file (`Widget`) ->
/// `""`. Swift forbids two same-named module-level types, so this never merges
/// two declarations, and an `extension Widget` in `Widget+Extras.swift` gets
/// the qname (and NodeId) of the class it extends.
///
/// The type `Widget` of `Widget.swift` therefore shares its qname with the
/// file MODULE (different kind, different NodeId); `MergedGraph::pick_primary`
/// ranks the declaration over the container, so qname lookups land on it.
fn type_scope(module_qname: &str) -> &str {
    module_qname.rsplit_once("::").map_or("", |(dir, _stem)| dir)
}

/// `scope::name`, or the bare `name` for the empty (repo-root) scope.
fn scoped(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}::{name}")
    }
}

/// `GLIA_QNAME_DEBUG=1` turns on the per-file `[qname] swift:` marker, read
/// once. Off by default: it would print for every Swift file of a build.
///   `GLIA_QNAME_DEBUG=1 glia analyze <repo> 2>&1 | grep '\[qname\] swift:'`
fn qname_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_QNAME_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

/// `GLIA_SWIFT_DEBUG=1` turns on the per-file `[swift-calls]` marker (self /
/// super call sites), read once. Off by default: nearly every Swift file has
/// self calls.
///   `GLIA_SWIFT_DEBUG=1 glia analyze <repo> 2>&1 | grep '\[swift-calls\]'`
fn swift_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_SWIFT_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

/// Top-level type declarations of one file, for the `[qname] swift:` marker.
/// `file_private` counts every top-level entry whose qname keeps the file
/// segment: private / fileprivate declarations and the same-file extensions
/// of them (it can overlap `extensions`).
#[derive(Default)]
struct TopLevelTypes {
    types: usize,
    extensions: usize,
    file_private: usize,
}

/// A top-level type declared (not extended) in this file: its name, kind and
/// whether it is private / fileprivate (file-scoped).
struct Declared<'a> {
    name: &'a str,
    kind: repo_graph_core::NodeKindId,
    file_private: bool,
}

fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> TopLevelTypes {
    let scope = type_scope(module_qname);
    // Pre-scan, in source order: the types this file DECLARES, so an
    // extension of one of them takes its kind (a `struct Part` + `extension
    // Part` pair is one STRUCT node) and its scope (a private type's
    // extension stays in the file). Vec + first match, never a HashMap walk.
    let mut declared: Vec<Declared> = Vec::new();
    {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if matches!(child.kind(), "class_declaration" | "protocol_declaration")
                && !is_extension(child)
                && let Some(name_node) = child.child_by_field_name("name")
            {
                declared.push(Declared {
                    name: text_of(name_node, src),
                    kind: swift_type_kind(child),
                    file_private: is_file_private(child, src),
                });
            }
        }
    }

    let mut top = TopLevelTypes::default();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_declaration" => collect_import(child, src, module_qname, acc),
            "class_declaration" | "protocol_declaration" => {
                let ext = is_extension(child);
                let (kind, file_private) = if ext {
                    // An extension of a type declared in another file (or
                    // outside the repo: `extension String`) stays CLASS.
                    let name = child
                        .child_by_field_name("name")
                        .map(|n| text_of(n, src))
                        .unwrap_or("");
                    declared
                        .iter()
                        .find(|d| d.name == name)
                        .map_or((node_kind::CLASS, false), |d| (d.kind, d.file_private))
                } else {
                    (swift_type_kind(child), is_file_private(child, src))
                };
                let qscope = if file_private { module_qname } else { scope };
                if visit_type(child, src, file_rel, qscope, module_id, repo, kind, acc) {
                    top.types += 1;
                    top.extensions += usize::from(ext);
                    top.file_private += usize::from(file_private);
                }
            }
            "function_declaration" => {
                visit_function(child, src, file_rel, module_qname, module_id, repo, acc);
            }
            _ => {}
        }
    }
    top
}

/// `extension T { … }` — tree-sitter-swift 0.7 parses it as a
/// `class_declaration` whose `declaration_kind` field is the `extension`
/// keyword (actor / class / enum / extension / struct).
fn is_extension(node: TsNode) -> bool {
    node.child_by_field_name("declaration_kind")
        .is_some_and(|k| k.kind() == "extension")
}

/// `private` / `fileprivate` at top level is FILE scope in Swift: such a type
/// keeps the file segment in its qname, so two files may each declare
/// `private enum Constants` without merging. Reads the `modifiers` child's
/// `visibility_modifier` (exact text; `private(set)` is a setter modifier and
/// never file scope).
fn is_file_private(node: TsNode, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| c.kind() == "modifiers")
        .any(|mods| {
            let mut c = mods.walk();
            mods.named_children(&mut c).any(|m| {
                m.kind() == "visibility_modifier"
                    && matches!(text_of(m, src).trim(), "private" | "fileprivate")
            })
        })
}

fn swift_type_kind(node: TsNode) -> repo_graph_core::NodeKindId {
    // tree-sitter-swift 0.7 uses `class_declaration` for class/struct/enum/actor.
    // The keyword is the first unnamed child.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            match child.kind() {
                "struct" => return node_kind::STRUCT,
                "enum" => return node_kind::ENUM,
                "actor" => return node_kind::CLASS,
                "protocol" => return node_kind::INTERFACE,
                _ => {}
            }
        }
    }
    node_kind::CLASS
}

/// Emit one type (class / struct / enum / actor / protocol / extension) and
/// its members. `scope` is what the type hangs off: the module scope
/// ([`type_scope`]) for a top-level type or extension, the file module for a
/// private / fileprivate one, the outer type's qname for a nested one.
/// Returns whether the type was emitted (the `[qname] swift:` marker counts
/// them). A type this file already emitted (`class Widget` + `extension
/// Widget`) folds onto the existing node: its cells are appended — the
/// declaration's first, so POSITION locates the declaration — and no second
/// DEFINES edge or nav entry is written.
#[allow(clippy::too_many_arguments)]
fn visit_type(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &str,
    parent_id: NodeId,
    repo: RepoId,
    kind: repo_graph_core::NodeKindId,
    acc: &mut Acc,
) -> bool {
    let Some(name_node) = node.child_by_field_name("name") else {
        return false;
    };
    let name = text_of(name_node, src);
    let qname = scoped(scope, name);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);

    let cells = entity_cells(&node, src, file_rel);
    if acc.type_seen.insert(id) {
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells,
        });
        acc.edges.push(Edge {
            from: parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav.record(id, name, &qname, kind, Some(parent_id));
    } else if let Some(existing) = acc.nodes.iter_mut().find(|n| n.id == id) {
        if is_extension(node) {
            existing.cells.extend(cells);
        } else {
            // The declaration follows an extension of it in this file.
            let ext_cells = std::mem::replace(&mut existing.cells, cells);
            existing.cells.extend(ext_cells);
        }
    }

    // tree-sitter-swift 0.7 uses class_body / enum_class_body — find by suffix.
    let body = {
        let mut c = node.walk();
        node.named_children(&mut c)
            .find(|ch| ch.kind().ends_with("_body"))
    };
    if let Some(body) = body {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            match child.kind() {
                "function_declaration" => {
                    visit_method(child, src, file_rel, &qname, id, repo, acc);
                }
                "class_declaration" => {
                    let nested_kind = swift_type_kind(child);
                    visit_type(child, src, file_rel, &qname, id, repo, nested_kind, acc);
                }
                _ => {}
            }
        }
    }
    true
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::FUNCTION, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
        collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::METHOD, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
        collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
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

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some(func) = n.named_child(0)
        {
            let qualifier = classify_call(func, src);
            match qualifier {
                CallQualifier::SelfMethod(_) => acc.self_calls += 1,
                CallQualifier::SuperMethod(_) => acc.super_calls += 1,
                _ => {}
            }
            acc.calls.push(CallSite { from, qualifier });
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "function_declaration" | "class_declaration" | "closure_expression"
            ) {
                stack.push(child);
            }
        }
    }
}

/// LA.36a: a navigation callee is classified by the KIND of its target node,
/// and every qualifier carries the bare member name (`helper`, never
/// `.helper`):
/// - `self.m()` / `self?.m()` (a `self_expression` target; the `?` is an
///   anonymous sibling, not part of it) and `Self.m()` (the enclosing type, so
///   a static member of it) -> `SelfMethod(m)`, bound by the graph against the
///   enclosing CLASS / STRUCT / ENUM's methods;
/// - `super.m()` -> `SuperMethod(m)` (left unresolved by the graph);
/// - `x.m()` -> `Attribute { x, m }`; any other receiver -> `ComplexReceiver`.
///
/// A suffix with no readable name falls back to the whole-callee
/// `ComplexReceiver`, as a non-navigation callee does.
fn classify_call(func_node: TsNode, src: &[u8]) -> CallQualifier {
    let whole_callee = || CallQualifier::ComplexReceiver {
        receiver: text_of(func_node, src).to_string(),
        name: String::new(),
    };
    match func_node.kind() {
        "simple_identifier" => CallQualifier::Bare(text_of(func_node, src).to_string()),
        "navigation_expression" => {
            let name = nav_member_name(func_node, src);
            let Some(target) = func_node.named_child(0).filter(|_| !name.is_empty()) else {
                return whole_callee();
            };
            let name = name.to_string();
            let target_text = text_of(target, src);
            match target.kind() {
                "self_expression" => CallQualifier::SelfMethod(name),
                "super_expression" => CallQualifier::SuperMethod(name),
                "simple_identifier" if target_text == "Self" => CallQualifier::SelfMethod(name),
                "simple_identifier" => CallQualifier::Attribute {
                    base: target_text.to_string(),
                    name,
                },
                _ => CallQualifier::ComplexReceiver {
                    receiver: target_text.to_string(),
                    name,
                },
            }
        }
        _ => whole_callee(),
    }
}

/// tree-sitter-swift's `navigation_suffix` spans the dot (`.helper`); the
/// member name is its own `suffix` field (a `simple_identifier`, or an
/// `integer_literal` for a tuple index). `""` when either is absent.
fn nav_member_name<'a>(nav_expr: TsNode<'a>, src: &'a [u8]) -> &'a str {
    nav_expr
        .child_by_field_name("suffix")
        .and_then(|s| s.child_by_field_name("suffix"))
        .map(|n| text_of(n, src))
        .unwrap_or("")
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

const HTTP_VERBS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Pattern A: detect Swift `URLSession` client HTTP calls in a function/method
/// body and emit shared ENDPOINT nodes so the HttpStackResolver can pair them
/// with a server ROUTE.
///
/// The request PATH lives in the `URL(string: "…")` literal (a plain
/// `dataTask(with: url)` only sees a variable). The HTTP verb defaults to GET;
/// if the same body sets `request.httpMethod = "POST"` we adopt that verb
/// (`URLRequest.httpMethod` is the only way to change it). One httpMethod per
/// body is the norm; when present it applies to that body's URL(s).
fn collect_client_endpoints_in(
    body: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    // `request.httpMethod = "VERB"` in this body overrides the GET default.
    let explicit_method = body_http_method(body, src);
    let method = explicit_method.clone().unwrap_or_else(|| "GET".to_string());

    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        // The literal's path is the ENDPOINT's identity; its authority rides on
        // ENDPOINT_HIT as `host` (A11.5).
        if let Some((raw_path, interpolated)) = url_string_literal_path(n, src)
            && let (host, Some(path)) = client_url_split(&raw_path)
        {
            let pos = n.start_position();
            // Plain literal + default verb → Strong; interpolated path or a verb
            // inferred indirectly from httpMethod → Medium.
            let confidence = if interpolated || explicit_method.is_some() {
                Confidence::Medium
            } else {
                Confidence::Strong
            };
            let ep = ClientEndpoint {
                method: method.clone(),
                path,
                file: file_rel.to_string(),
                line: pos.row + 1,
                col: pos.column + 1,
                confidence,
            };
            let extras = HitExtras {
                host: host.as_deref(),
                ..HitExtras::default()
            };
            push_client_endpoint_with(
                repo,
                &ep,
                extras,
                from,
                &mut acc.nodes,
                &mut acc.edges,
                &mut acc.nav,
                &mut acc.endpoint_seen,
            );
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            // Don't cross into a nested declaration — its own visit handles it.
            if !matches!(child.kind(), "function_declaration" | "class_declaration") {
                stack.push(child);
            }
        }
    }
}

/// If `n` is a `URL(string: "…")` call, return the reconstructed URL string and
/// whether it contained interpolation. `URL(string: someVar)` (no literal) → None.
fn url_string_literal_path(n: TsNode, src: &[u8]) -> Option<(String, bool)> {
    if n.kind() != "call_expression" {
        return None;
    }
    let callee = n.named_child(0)?;
    if callee.kind() != "simple_identifier" || text_of(callee, src) != "URL" {
        return None;
    }
    let sl = first_descendant_of_kind(n, "line_string_literal")?;
    Some(swift_string_path(sl, src))
}

/// Reconstruct a Swift `line_string_literal`, replacing every `\(expr)`
/// interpolation with `${…}` so it normalises like a TS template path
/// (`normalise_http_path` collapses any segment containing `${` to `{}`).
/// Returns `(text, had_interpolation)`.
fn swift_string_path(string_literal: TsNode, src: &[u8]) -> (String, bool) {
    let mut out = String::new();
    let mut interpolated = false;
    let mut c = string_literal.walk();
    for child in string_literal.named_children(&mut c) {
        match child.kind() {
            "line_str_text" => out.push_str(text_of(child, src)),
            "interpolated_expression" => {
                out.push_str("${…}");
                interpolated = true;
            }
            _ => {}
        }
    }
    (out, interpolated)
}

/// Scan a body for `<x>.httpMethod = "VERB"` and return the upper-cased verb.
fn body_http_method(body: TsNode, src: &[u8]) -> Option<String> {
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "assignment"
            && let Some(target) = n.child_by_field_name("target")
            && text_of(target, src).trim_end().ends_with(".httpMethod")
            && let Some(result) = n.child_by_field_name("result")
            && result.kind() == "line_string_literal"
        {
            let (verb, _) = swift_string_path(result, src);
            let verb = verb.trim().to_ascii_uppercase();
            if HTTP_VERBS.contains(&verb.as_str()) {
                return Some(verb);
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

fn first_descendant_of_kind<'a>(node: TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if let Some(found) = first_descendant_of_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn scan_vapor_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    let methods: &[(&str, &str)] = &[
        (".get(", "GET"),
        (".post(", "POST"),
        (".put(", "PUT"),
        (".patch(", "PATCH"),
        (".delete(", "DELETE"),
        (".head(", "HEAD"),
        (".options(", "OPTIONS"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let start = search_from + rel + needle.len();
            let end = find_call_end(&source[start..]).unwrap_or(0);
            if end == 0 {
                search_from = start;
                continue;
            }
            let args = &source[start..start + end];
            let path = vapor_path_from_args(args);
            if !path.is_empty() {
                let route_name = format!("{method} /{path}");
                if seen.insert(route_name.clone()) {
                    emit_vapor_route(method, &format!("/{path}"), repo, acc);
                }
            }
            search_from = start + end + 1;
        }
    }
}

fn find_call_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn vapor_path_from_args(args: &str) -> String {
    let bytes = args.as_bytes();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                let s = &args[start..j];
                parts.push(s.trim_start_matches(':').to_string());
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    parts.join("/")
}

fn emit_vapor_route(method: &str, path: &str, repo: RepoId, acc: &mut Acc) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    #[test]
    fn class_and_struct() {
        let source = r#"
class User {
    var name: String
    func greet() -> String {
        return "Hello \(name)"
    }
}

struct Point {
    var x: Int
    var y: Int
}
"#;
        let fp = parse_file(source, "Sources/Models.swift", "Sources::Models", repo()).unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"User"));
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"greet"));
        // LB.7c: module-scoped (`Sources::User`), not file-scoped
        // (`Sources::Models::User`).
        let qnames: Vec<&str> = fp.nav.qname_by_id.values().map(|s| s.as_str()).collect();
        assert!(qnames.contains(&"Sources::User"), "{qnames:?}");
        assert!(qnames.contains(&"Sources::Point"), "{qnames:?}");
        assert!(qnames.contains(&"Sources::User::greet"), "{qnames:?}");
        assert!(!qnames.iter().any(|q| q.starts_with("Sources::Models::")), "{qnames:?}");
    }

    fn id(kind: repo_graph_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn fixture(file: &str) -> String {
        let path = format!(
            "{}/../../../bench/substrate-gap/fixtures/swift-module-qnames/Sources/Shop/{file}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn node_count(fp: &FileParse, id: NodeId) -> usize {
        fp.nodes.iter().filter(|n| n.id == id).count()
    }

    fn start_lines(fp: &FileParse, id: NodeId) -> Vec<String> {
        let node = fp.nodes.iter().find(|n| n.id == id).expect("node");
        node.cells
            .iter()
            .filter(|c| c.kind == cell_type::POSITION)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(j) => j
                    .split("\"start_line\":")
                    .nth(1)
                    .and_then(|s| s.split(',').next())
                    .map(str::to_string),
                _ => None,
            })
            .collect()
    }

    /// LB.7c: Swift types and extensions hang off the module (the target
    /// directory), so a cross-file `extension Widget` lands on the class's
    /// node; a same-file `extension Part` of `struct Part` takes its kind and
    /// node; private / fileprivate top-level types keep the file segment.
    /// Drives the swift-module-qnames fixture's three files as the engine does.
    #[test]
    fn types_and_extensions_are_module_scoped() {
        let widget = parse_file(
            &fixture("Widget.swift"),
            "Sources/Shop/Widget.swift",
            "Sources::Shop::Widget",
            repo(),
        )
        .unwrap();
        let extras = parse_file(
            &fixture("Widget+Extras.swift"),
            "Sources/Shop/Widget+Extras.swift",
            "Sources::Shop::Widget+Extras",
            repo(),
        )
        .unwrap();
        let part = parse_file(
            &fixture("Part.swift"),
            "Sources/Shop/Part.swift",
            "Sources::Shop::Part",
            repo(),
        )
        .unwrap();

        // No doubled stem: CLASS Sources::Shop::Widget holds run / helper and
        // the same-file extension's extra, folded into ONE node.
        let class = id(node_kind::CLASS, "Sources::Shop::Widget");
        assert_eq!(node_count(&widget, class), 1);
        assert_eq!(widget.nav.kind_by_id.get(&class), Some(&node_kind::CLASS));
        assert!(!widget.nav.qname_by_id.values().any(|q| q == "Sources::Shop::Widget::Widget"));
        for m in ["run", "helper", "extra"] {
            let mid = id(node_kind::METHOD, &format!("Sources::Shop::Widget::{m}"));
            assert_eq!(widget.nav.parent_of.get(&mid), Some(&class), "{m}");
        }
        let module = id(node_kind::MODULE, "Sources::Shop::Widget");
        let defines = |fp: &FileParse, from: NodeId, to: NodeId| {
            fp.edges
                .iter()
                .filter(|e| e.from == from && e.to == to && e.category == edge_category::DEFINES)
                .count()
        };
        assert_eq!(defines(&widget, module, class), 1, "one DEFINES for class + extension");
        assert_eq!(
            widget.nav.children_of.get(&module).map(|c| c.iter().filter(|x| **x == class).count()),
            Some(1)
        );
        // The declaration's POSITION first (line 0), the extension's second.
        assert_eq!(start_lines(&widget, class), vec!["0", "10"]);

        // The cross-file extension mints the SAME NodeId and holds `more`.
        assert_eq!(node_count(&extras, class), 1);
        let more = id(node_kind::METHOD, "Sources::Shop::Widget::more");
        assert_eq!(extras.nav.parent_of.get(&more), Some(&class));
        assert!(!extras.nav.qname_by_id.values().any(|q| q == "Sources::Shop::Widget+Extras::Widget"));

        // Control: the private enums keep their file segment, two nodes.
        let c1 = id(node_kind::ENUM, "Sources::Shop::Widget::Constants");
        let c2 = id(node_kind::ENUM, "Sources::Shop::Widget+Extras::Constants");
        assert_ne!(c1, c2);
        assert_eq!(node_count(&widget, c1), 1);
        assert_eq!(node_count(&extras, c2), 1);
        for fp in [&widget, &extras] {
            assert!(!fp.nav.qname_by_id.values().any(|q| q == "Sources::Shop::Constants"));
        }

        // struct Part + extension Part: one STRUCT node, no CLASS twin.
        let strukt = id(node_kind::STRUCT, "Sources::Shop::Part");
        assert_eq!(node_count(&part, strukt), 1);
        assert_eq!(part.nav.kind_by_id.get(&strukt), Some(&node_kind::STRUCT));
        assert_eq!(node_count(&part, id(node_kind::CLASS, "Sources::Shop::Part")), 0);
        for m in ["weight", "heavy"] {
            let mid = id(node_kind::METHOD, &format!("Sources::Shop::Part::{m}"));
            assert_eq!(part.nav.parent_of.get(&mid), Some(&strukt), "{m}");
        }

        // An extension of a type declared nowhere in the file stays CLASS, at
        // module scope.
        let string_ext = parse_file(
            "extension String {\n    func shout() -> String { return self }\n}\n",
            "Sources/Shop/String+Shout.swift",
            "Sources::Shop::String+Shout",
            repo(),
        )
        .unwrap();
        let string_class = id(node_kind::CLASS, "Sources::Shop::String");
        assert_eq!(node_count(&string_ext, string_class), 1);
        let shout = id(node_kind::METHOD, "Sources::Shop::String::shout");
        assert_eq!(string_ext.nav.parent_of.get(&shout), Some(&string_class));
    }

    /// LB.7c edges of the rule: a private type's same-file extension stays in
    /// the file; `fileprivate` is file scope too; an extension written above
    /// its declaration still folds with the declaration's POSITION first; a
    /// type nested in an extension hangs off the extended type; free
    /// functions keep the file scope; a repo-root file has an empty scope.
    #[test]
    fn file_private_types_and_extension_order() {
        let source = r#"
extension Late {
    func a() -> Int { return 1 }
    struct Inner {}
}

struct Late {}

private struct Helper {}

extension Helper {
    func h() -> Int { return 2 }
}

fileprivate class Box {}

public final class Open {}

func topLevel() -> Int { return 0 }
"#;
        let fp = parse_file(source, "Sources/Shop/A.swift", "Sources::Shop::A", repo()).unwrap();
        let qnames: Vec<&str> = fp.nav.qname_by_id.values().map(|s| s.as_str()).collect();

        let late = id(node_kind::STRUCT, "Sources::Shop::Late");
        assert_eq!(node_count(&fp, late), 1);
        assert_eq!(node_count(&fp, id(node_kind::CLASS, "Sources::Shop::Late")), 0);
        assert_eq!(start_lines(&fp, late), vec!["6", "1"], "declaration POSITION first");
        let a = id(node_kind::METHOD, "Sources::Shop::Late::a");
        assert_eq!(fp.nav.parent_of.get(&a), Some(&late));
        let inner = id(node_kind::STRUCT, "Sources::Shop::Late::Inner");
        assert_eq!(fp.nav.parent_of.get(&inner), Some(&late));

        let helper = id(node_kind::STRUCT, "Sources::Shop::A::Helper");
        assert_eq!(node_count(&fp, helper), 1);
        let h = id(node_kind::METHOD, "Sources::Shop::A::Helper::h");
        assert_eq!(fp.nav.parent_of.get(&h), Some(&helper));
        assert!(!qnames.contains(&"Sources::Shop::Helper"), "{qnames:?}");

        assert_eq!(node_count(&fp, id(node_kind::CLASS, "Sources::Shop::A::Box")), 1);
        assert_eq!(node_count(&fp, id(node_kind::CLASS, "Sources::Shop::Open")), 1);
        assert_eq!(node_count(&fp, id(node_kind::FUNCTION, "Sources::Shop::A::topLevel")), 1);

        let root = parse_file("class Widget {}\n", "Widget.swift", "Widget", repo()).unwrap();
        assert_eq!(node_count(&root, id(node_kind::CLASS, "Widget")), 1);
        assert_eq!(type_scope("Widget"), "");
        assert_eq!(type_scope("Sources::Shop::Widget+Extras"), "Sources::Shop");
    }

    #[test]
    fn protocol_and_enum() {
        let source = r#"
protocol Drawable {
    func draw()
}

enum Color {
    case red, green, blue
}
"#;
        let fp = parse_file(source, "Sources/Types.swift", "Sources::Types", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::ENUM).count(), 1);
    }

    #[test]
    fn vapor_routes_basic() {
        let source = r#"
import Vapor

func routes(_ app: Application) throws {
    app.get("users") { req in "list" }
    app.post("users") { req in "create" }
    app.get("users", ":id") { req in "show" }
    app.delete("users", ":id") { req in "destroy" }
}
"#;
        let fp = parse_file(source, "Sources/App/routes.swift", "Sources::App::routes", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /users"));
        assert!(route_names.contains(&"POST /users"));
        assert!(route_names.contains(&"GET /users/id"));
        assert!(route_names.contains(&"DELETE /users/id"));
    }

    #[test]
    fn imports() {
        let source = r#"
import Foundation
import Vapor
"#;
        let fp = parse_file(source, "Sources/App.swift", "Sources::App", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
    }

    #[test]
    fn urlsession_client_call_emits_endpoint_not_route() {
        // Pattern A: URLSession `URL(string:)` client calls in a function →
        // ENDPOINT nodes (not phantom ROUTEs), with a CALLS edge from the
        // enclosing function. GET default; `httpMethod = "POST"` adopts POST.
        // Absolute URL host is stripped; `\(id)` interpolation → `${…}`.
        let source = r#"
import Foundation

func fetchUser(id: String) {
    let url = URL(string: "https://api.example.com/users/\(id)")!
    let task = URLSession.shared.dataTask(with: url) { data, response, error in
    }
    task.resume()
}

func createUser(body: Data) {
    let url = URL(string: "https://api.example.com/users")!
    var request = URLRequest(url: url)
    request.httpMethod = "POST"
    let task = URLSession.shared.dataTask(with: request) { data, response, error in
    }
    task.resume()
}
"#;
        let fp = parse_file(source, "Sources/Api.swift", "Sources::Api", repo()).unwrap();

        let ep_get =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/users/${…}");
        let ep_post =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:POST:/users");

        assert!(
            fp.nodes.iter().any(|n| n.id == ep_get),
            "expected GET /users/${{…}} ENDPOINT node"
        );
        assert!(
            fp.nodes.iter().any(|n| n.id == ep_post),
            "expected POST /users ENDPOINT node"
        );
        // Must NOT emit phantom ROUTE nodes for the client calls.
        assert!(
            !fp.nodes
                .iter()
                .any(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE)),
            "client URLSession calls must not become server ROUTEs"
        );
        // CALLS edge from the enclosing function into each endpoint.
        assert!(
            fp.edges
                .iter()
                .any(|e| e.to == ep_get && e.category == edge_category::CALLS),
            "expected CALLS edge into the GET endpoint"
        );
        assert!(
            fp.edges
                .iter()
                .any(|e| e.to == ep_post && e.category == edge_category::CALLS),
            "expected CALLS edge into the POST endpoint"
        );
    }

    /// A11.5 — `URL(string:)`'s authority lands on the ENDPOINT_HIT cell as
    /// `host`; an interpolated authority (`\(base)`) names no service.
    #[test]
    fn urlsession_endpoint_carries_the_url_authority_as_host() {
        let source = r#"
import Foundation

func listUsers() {
    let url = URL(string: "https://api.example.com/users")!
    URLSession.shared.dataTask(with: url) { data, response, error in }.resume()
}

func listOrders(base: String) {
    let url = URL(string: "https://\(base)/orders")!
    URLSession.shared.dataTask(with: url) { data, response, error in }.resume()
}
"#;
        let fp = parse_file(source, "Sources/Api.swift", "Sources::Api", repo()).unwrap();
        let hit = |qname: &str| -> String {
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, qname);
            let node = fp.nodes.iter().find(|n| n.id == id).expect("ENDPOINT node");
            match &node.cells[0].payload {
                CellPayload::Json(j) if node.cells[0].kind == cell_type::ENDPOINT_HIT => j.clone(),
                other => panic!("not an ENDPOINT_HIT json cell: {other:?}"),
            }
        };
        let users = hit("endpoint:GET:/users");
        assert!(
            users.ends_with(r#","confidence":"strong","host":"api.example.com"}"#),
            "{users}"
        );
        let orders = hit("endpoint:GET:/orders");
        assert!(!orders.contains("host"), "{orders}");
    }

    /// LA.36a: the call qualifiers emitted from the method `method` of type
    /// `W` in `source` (a repo-root file, so the type's qname is `W`).
    fn calls_from(source: &str, method: &str) -> Vec<CallQualifier> {
        let fp = parse_file(source, "W.swift", "W", repo()).unwrap();
        let from = id(node_kind::METHOD, &format!("W::{method}"));
        fp.calls
            .into_iter()
            .filter(|c| c.from == from)
            .map(|c| c.qualifier)
            .collect()
    }

    #[test]
    fn self_call_name_drops_the_navigation_dot() {
        let calls = calls_from("class W {\n func a() { self.b() }\n func b() {}\n}\n", "a");
        assert_eq!(calls, vec![CallQualifier::SelfMethod("b".into())]);
    }

    #[test]
    fn optional_self_in_a_closure_is_self_method() {
        let source = "class W {\n func a() {\n  schedule { [weak self] in\n   self?.g()\n  }\n }\n func g() {}\n}\n";
        let calls = calls_from(source, "a");
        assert!(calls.contains(&CallQualifier::SelfMethod("g".into())), "{calls:?}");
        assert!(calls.contains(&CallQualifier::Bare("schedule".into())), "{calls:?}");
    }

    #[test]
    fn upper_self_is_self_method() {
        let source = "class W {\n func a() -> W { return Self.make() }\n static func make() -> W { return W() }\n}\n";
        assert_eq!(calls_from(source, "a"), vec![CallQualifier::SelfMethod("make".into())]);
    }

    #[test]
    fn super_call_is_super_method() {
        let source = "class W: Base {\n override func k() { super.k() }\n}\n";
        assert_eq!(calls_from(source, "k"), vec![CallQualifier::SuperMethod("k".into())]);
    }

    #[test]
    fn identifier_receiver_is_attribute_without_dot() {
        let source = "class W {\n func a() { repo.find() }\n}\n";
        assert_eq!(
            calls_from(source, "a"),
            vec![CallQualifier::Attribute { base: "repo".into(), name: "find".into() }]
        );
    }

    #[test]
    fn chained_receiver_is_complex_without_dot() {
        let source = "class W {\n func a() { self.items.map { } }\n}\n";
        assert_eq!(
            calls_from(source, "a"),
            vec![CallQualifier::ComplexReceiver {
                receiver: "self.items".into(),
                name: "map".into()
            }]
        );
    }
}
