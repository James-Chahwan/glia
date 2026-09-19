use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use repo_graph_code_domain::endpoint::{
    ClientEndpoint, push_client_endpoint, route_qname, url_to_path,
};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
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
    acc.file_module = Some(module_id);

    // The file MODULE stays `nodes[0]`: engine `apply_rpc_needles` pairs a
    // parse to its file by `fp.nodes.first()`.
    let file_scope = Scope {
        qname: module_qname.to_string(),
        id: module_id,
    };
    visit_items(root, &file_scope, src, file_rel_path, repo, &mut acc);

    scan_axum_routes(source, module_id, repo, &mut acc);
    scan_at_path_chains(source, repo, &mut acc);
    scan_salvo_routes(source, repo, &mut acc);

    if acc.client_endpoints > 0 {
        eprintln!(
            "[rust-http-client] {} endpoints in {}",
            acc.client_endpoints, file_rel_path
        );
    }
    if acc.window_snaps > 0 {
        eprintln!(
            "[rust-routes] verb windows snapped to a char boundary: {} in {}",
            acc.window_snaps, file_rel_path
        );
    }
    if acc.macro_calls > 0 && rust_debug_enabled() {
        eprintln!(
            "[rust-macro-calls] {file_rel_path}: {} call site(s) in {} macro invocation(s)",
            acc.macro_calls, acc.macro_invocations
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
    /// Dedups the ENDPOINT node across this file (the CALLS edge is per site).
    endpoint_seen: HashSet<NodeId>,
    /// Outbound HTTP call sites seen in this file — drives the fired_on marker.
    client_endpoints: usize,
    /// Call sites found inside macro arguments (LA.2 `[rust-macro-calls]` marker).
    macro_calls: usize,
    /// Macro invocations whose token tree was scanned, nested ones included.
    macro_invocations: usize,
    /// Verb-chain windows whose end fell inside a multibyte char and was
    /// snapped down to a char boundary (LA.25b `[rust-routes]` marker).
    window_snaps: usize,
    /// The file MODULE: the `from_module` of every enum-variant USES ref
    /// (LA.3). `None` only when a test drives `collect_calls_in` directly.
    file_module: Option<NodeId>,
}

/// Where an item is declared: the file MODULE, or an inline `mod x { .. }`
/// PACKAGE (LA.3). Items take their qname prefix and DEFINES parent from it.
struct Scope {
    qname: String,
    id: NodeId,
}

/// Every item of one container (`source_file`, or an inline mod's
/// `declaration_list`) under `scope`. Types first, so an `impl` anywhere in
/// the container finds its type; then fns, impls, consts, uses, route
/// attributes and nested inline mods in source order. `type_ids` is per
/// container: an `impl Foo` inside `mod tests` does not see a file-level
/// `Foo` (it parents to the PACKAGE, like an impl whose type is in another
/// file).
fn visit_items(
    container: TsNode,
    scope: &Scope,
    src: &[u8],
    file_rel: &str,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut type_ids: HashMap<String, NodeId> = HashMap::new();
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        if matches!(child.kind(), "struct_item" | "enum_item" | "trait_item") {
            visit_type(child, scope, src, file_rel, repo, &mut type_ids, acc);
        }
    }

    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        match child.kind() {
            "function_item" => visit_function(child, src, file_rel, scope, repo, acc),
            "impl_item" => visit_impl(child, src, file_rel, scope, repo, &type_ids, acc),
            "const_item" | "static_item" => {
                visit_const_static(child, src, file_rel, scope, repo, acc);
            }
            // A `use` inside an inline mod is recorded against the PACKAGE
            // qname. `resolve_imports_python` keys `from_module` on MODULEs
            // only, so it binds nothing yet (LA.1b) - never the file module,
            // which would leak a test-only import into file-level resolution.
            "use_declaration" => collect_use(child, src, &scope.qname, acc),
            "attribute_item" => visit_route_attr(child, src, file_rel, scope.id, repo, acc),
            "mod_item" => visit_mod(child, scope, src, file_rel, repo, acc),
            _ => {}
        }
    }
}

/// A STRUCT / ENUM / INTERFACE under `scope`; an enum's variants become
/// ATTRIBUTE children (the kind Python gives an `Enum` member), joined by
/// HAS_ATTRIBUTE. Unit, tuple and struct variants alike; a discriminant is
/// part of the variant's CODE.
fn visit_type(
    node: TsNode,
    scope: &Scope,
    src: &[u8],
    file_rel: &str,
    repo: RepoId,
    type_ids: &mut HashMap<String, NodeId>,
    acc: &mut Acc,
) {
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    let name_str = text_of(name, src);
    let kind = match node.kind() {
        "struct_item" => node_kind::STRUCT,
        "enum_item" => node_kind::ENUM,
        _ => node_kind::INTERFACE,
    };
    let qname = format!("{}::{name_str}", scope.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
    type_ids.insert(name_str.to_string(), id);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: scope.id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav.record(id, name_str, &qname, kind, Some(scope.id));

    if kind != node_kind::ENUM {
        return;
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for variant in body.named_children(&mut cursor) {
        if variant.kind() != "enum_variant" {
            continue;
        }
        let Some(vname) = variant.child_by_field_name("name").map(|n| text_of(n, src)) else {
            continue;
        };
        let vq = format!("{qname}::{vname}");
        let vid = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ATTRIBUTE, &vq);
        acc.nodes.push(Node {
            id: vid,
            repo,
            confidence: Confidence::Strong,
            cells: entity_cells(&variant, src, file_rel),
        });
        acc.edges.push(Edge {
            from: id,
            to: vid,
            category: edge_category::HAS_ATTRIBUTE,
            confidence: Confidence::Strong,
        });
        acc.nav
            .record(vid, vname, &vq, node_kind::ATTRIBUTE, Some(id));
    }
}

/// An inline `mod x { .. }`: a PACKAGE node (qname `<scope>::x`, the kind
/// C# / PHP / Ruby / C++ / Elixir give an in-file namespace) joined to its
/// scope by CONTAINS, then every item inside it under the PACKAGE's scope.
/// POSITION + DOC only: a module block's CODE would repeat every child's.
/// `mod x;` has no body and emits nothing - the file module exists on its own.
fn visit_mod(node: TsNode, scope: &Scope, src: &[u8], file_rel: &str, repo: RepoId, acc: &mut Acc) {
    let (Some(body), Some(name_node)) = (
        node.child_by_field_name("body"),
        node.child_by_field_name("name"),
    ) else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{}::{name}", scope.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE, &qname);
    let mut cells = vec![Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(repo_graph_doc::position_json(&node, file_rel)),
    }];
    if let Some(doc) = repo_graph_doc::leading_doc(&node, src) {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells,
    });
    acc.edges.push(Edge {
        from: scope.id,
        to: id,
        category: edge_category::CONTAINS,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::PACKAGE, Some(scope.id));
    let inner = Scope { qname, id };
    visit_items(body, &inner, src, file_rel, repo, acc);
}

/// `GLIA_RUST_DEBUG=1` turns on the `[rust-macro-calls]` marker, read once. Off
/// by default: macro-argument calls occur in most Rust files, so a per-file line
/// would drown a normal build's stderr.
fn rust_debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("GLIA_RUST_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0"))
}

fn visit_function(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{}::{name}", scope.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: scope.id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::FUNCTION, Some(scope.id));

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
        let n = collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
        acc.client_endpoints += n;
    }
}

fn visit_impl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    repo: RepoId,
    type_ids: &HashMap<String, NodeId>,
    acc: &mut Acc,
) {
    // `impl Foo { ... }` or `impl Trait for Foo { ... }`
    // Find the target type name — it's the `type` field.
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let type_name = text_of(type_node, src);
    // Strip generic parameters: `Foo<T>` → `Foo`
    let base_name = type_name.split('<').next().unwrap_or(type_name);
    let parent_id = type_ids.get(base_name).copied().unwrap_or(scope.id);

    // G12.5 — Rust has no `extends`; a trait impl `impl Trait for Type` carries
    // the `trait` field. Emit IMPLEMENTS (Type → trait) only when the trait is
    // in-file; skip external traits rather than fabricate a target.
    if let Some(trait_node) = node.child_by_field_name("trait") {
        let trait_name = text_of(trait_node, src);
        let trait_base = trait_name.split('<').next().unwrap_or(trait_name);
        if let Some(&trait_id) = type_ids.get(trait_base) {
            acc.edges.push(Edge {
                from: parent_id,
                to: trait_id,
                category: edge_category::IMPLEMENTS,
                confidence: Confidence::Strong,
            });
        }
    }

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "function_item" {
            let Some(name_node) = child.child_by_field_name("name") else {
                continue;
            };
            let name = text_of(name_node, src);
            let qname = format!("{}::{base_name}::{name}", scope.qname);
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);

            acc.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: entity_cells(&child, src, file_rel),
            });
            acc.edges.push(Edge {
                from: parent_id,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
            });
            acc.nav
                .record(id, name, &qname, node_kind::METHOD, Some(parent_id));

            if let Some(fn_body) = child.child_by_field_name("body") {
                collect_calls_in(fn_body, src, id, acc);
                let n = collect_client_endpoints_in(fn_body, src, id, repo, file_rel, acc);
                acc.client_endpoints += n;
            }
        }
    }
}

/// G19 — module-level `const`/`static` surfaced as STATE_VAR.
///
/// Noise gate: skip undocumented literal-primitive constants (e.g.
/// `const X: u32 = 1;`); keep documented ones or non-literal initialisers.
fn visit_const_static(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };

    let has_doc = repo_graph_doc::leading_doc(&node, src).is_some();
    let value_is_literal = node.child_by_field_name("value").is_some_and(|v| {
        matches!(
            v.kind(),
            "integer_literal" | "string_literal" | "boolean_literal" | "float_literal"
        )
    });
    if !has_doc && value_is_literal {
        return;
    }

    let name = text_of(name_node, src);
    let qname = format!("{}::{name}", scope.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: scope.id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::STATE_VAR, Some(scope.id));
}

fn collect_use(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    // `use crate::foo::bar;` or `use crate::foo::{bar, baz};` or `use super::foo;`
    let text = text_of(node, src);
    let trimmed = text.trim_start_matches("use ").trim_end_matches(';').trim();

    if trimmed.starts_with("crate::") {
        let path = trimmed.trim_start_matches("crate::");
        if let Some(brace_pos) = path.find('{') {
            // `crate::foo::{bar, baz}` — multiple symbol imports
            let base = path[..brace_pos].trim_end_matches("::");
            let names_part = &path[brace_pos + 1..].trim_end_matches('}');
            for name in names_part.split(',') {
                let name = name.trim();
                if name == "self" || name.is_empty() {
                    continue;
                }
                let (actual_name, alias) = if let Some((n, a)) = name.split_once(" as ") {
                    (n.trim(), Some(a.trim().to_string()))
                } else {
                    (name, None)
                };
                acc.imports.push(ImportStmt {
                    from_module: from_module.to_string(),
                    target: ImportTarget::Symbol {
                        module: base.to_string(),
                        name: actual_name.to_string(),
                        alias,
                        level: 0,
                    },
                });
            }
        } else if let Some((module, name)) = path.rsplit_once("::") {
            // `crate::foo::bar` — single symbol import
            let (actual_name, alias) = if let Some((n, a)) = name.split_once(" as ") {
                (n.trim(), Some(a.trim().to_string()))
            } else {
                (name, None)
            };
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Symbol {
                    module: module.to_string(),
                    name: actual_name.to_string(),
                    alias,
                    level: 0,
                },
            });
        } else {
            // `crate::foo` — module import
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Module {
                    path: path.to_string(),
                    alias: None,
                },
            });
        }
    } else if trimmed.starts_with("super::") {
        let path = trimmed.trim_start_matches("super::");
        if let Some((module, name)) = path.rsplit_once("::") {
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Symbol {
                    module: format!("super::{module}"),
                    name: name.to_string(),
                    alias: None,
                    level: 1,
                },
            });
        } else {
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Module {
                    path: format!("super::{path}"),
                    alias: None,
                },
            });
        }
    }
    // External crate imports (std::, etc.) — skip, won't resolve internally.
}

/// `scope_id`: the file MODULE, or the inline-mod PACKAGE the attribute sits in.
fn visit_route_attr(
    node: TsNode,
    src: &[u8],
    _file_rel: &str,
    scope_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let text = text_of(node, src);
    // Actix/Rocket: #[get("/path")] or #[post("/path")]
    let methods = ["get", "post", "put", "delete", "patch", "head", "options"];
    for method in &methods {
        let prefix = format!("#[{method}(\"");
        if let Some(rest) = text.strip_prefix(&prefix)
            && let Some(end) = rest.find('"')
        {
            let path = &rest[..end];
            let method_upper = method.to_uppercase();
            let route_name = route_qname(&method_upper, path);
            let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
            acc.nodes.push(Node {
                id: route_id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![Cell {
                    kind: cell_type::ROUTE_METHOD,
                    payload: CellPayload::Text(method_upper.clone()),
                }],
            });
            acc.edges.push(Edge {
                from: route_id,
                to: scope_id,
                category: edge_category::HANDLED_BY,
                confidence: Confidence::Strong,
            });
            acc.nav.record(
                route_id,
                &route_name,
                &route_name,
                node_kind::ROUTE,
                None,
            );
        }
    }
}

fn scan_axum_routes(source: &str, module_id: NodeId, repo: RepoId, acc: &mut Acc) {
    // Axum: Router::new().route("/path", get(handler).post(handler2))
    //                    .route("/users/:id", get(get_user))
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let needle = ".route(";
    let mut search_from = 0;
    let bytes = source.as_bytes();
    while let Some(rel) = source[search_from..].find(needle) {
        let start = search_from + rel + needle.len();
        let Some(path_end) = source[start..].find('"') else {
            search_from = start;
            continue;
        };
        let q_start = start + path_end + 1;
        let mut j = q_start;
        while j < bytes.len() && bytes[j] != b'"' {
            if bytes[j] == b'\\' && j + 1 < bytes.len() {
                j += 2;
            } else {
                j += 1;
            }
        }
        if j >= bytes.len() {
            break;
        }
        let path = &source[q_start..j];
        let Some(call_end) = find_matching_paren(&source[start..]) else {
            search_from = j + 1;
            continue;
        };
        let args_text = &source[start..start + call_end];
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            let pat = format!("{method}(");
            if contains_method_call(args_text, &pat) {
                let mu = method.to_ascii_uppercase();
                let route_name = route_qname(&mu, path);
                if seen.insert(route_name.clone()) {
                    emit_axum_route(&mu, path, repo, acc);
                    // `get(handler)` names the handler fn — link ROUTE→handler
                    // via a HANDLED_BY ref (graph's resolve_refs binds Bare by
                    // name, incl. same-module fns).
                    if let Some(handler) = extract_handler_name(args_text, &pat) {
                        let route_id = NodeId::from_parts(
                            GRAPH_TYPE,
                            repo,
                            node_kind::ROUTE,
                            &route_name,
                        );
                        acc.refs.push(UnresolvedRef {
                            from: route_id,
                            from_module: module_id,
                            qualifier: CallQualifier::Bare(handler),
                            category: edge_category::HANDLED_BY,
                        });
                    }
                }
            }
        }
        search_from = start + call_end + 1;
    }
}

fn find_matching_paren(s: &str) -> Option<usize> {
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

/// Extract the handler name from an axum verb-wrapper call inside `hay`,
/// e.g. `get(list_users)` → `list_users`, `post(api::create)` → `create`.
/// `pat` is the boundary-checked verb pattern like `"get("`. Returns the
/// last `::` path segment (the fn's bare name — what resolve_refs binds).
/// Closures (`get(|| ...)`) and empty/non-identifier args yield `None`.
fn extract_handler_name(hay: &str, pat: &str) -> Option<String> {
    let bytes = hay.as_bytes();
    let pat_bytes = pat.as_bytes();
    let mut i = 0;
    while i + pat_bytes.len() <= bytes.len() {
        if &bytes[i..i + pat_bytes.len()] == pat_bytes {
            let prev_ok = i == 0 || {
                let p = bytes[i - 1];
                !(p.is_ascii_alphanumeric() || p == b'_')
            };
            if prev_ok {
                let arg_start = i + pat_bytes.len();
                let mut j = arg_start;
                // Read a Rust path: identifier chars plus `::` separators.
                while j < bytes.len() {
                    let c = bytes[j];
                    if c.is_ascii_alphanumeric() || c == b'_' || c == b':' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                let token = &hay[arg_start..j];
                let name = token.rsplit("::").next().unwrap_or("");
                if !name.is_empty()
                    && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                {
                    return Some(name.to_string());
                }
                return None;
            }
        }
        i += 1;
    }
    None
}

fn contains_method_call(hay: &str, pat: &str) -> bool {
    // Match `pat` with a preceding non-word char (so `get(` matches, but not `target(`).
    let bytes = hay.as_bytes();
    let pat_bytes = pat.as_bytes();
    let mut i = 0;
    while i + pat_bytes.len() <= bytes.len() {
        if &bytes[i..i + pat_bytes.len()] == pat_bytes {
            let prev_ok = i == 0 || {
                let p = bytes[i - 1];
                !(p.is_ascii_alphanumeric() || p == b'_')
            };
            if prev_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

// ----------------------------------------------------------------------------
// Tide / Poem: `app.at("/path").get(handler).post(other)` chain
// Salvo:       `Router::with_path("/path").get(h).post(h2)`
//
// Common shape: a path-anchor call (`.at(...)` or `Router::with_path(...)`)
// followed by chained verb method calls. After the anchor's closing paren,
// scan a small window for `.<verb>(` substrings and emit one Route per
// matching verb sharing the path's NodeId.
//
// Warp (`warp::path!("a" / u32 / "b").and(warp::get())`) is skipped — its
// macro DSL composes path segments at compile time and would need real
// expansion to canonicalise. Re-evaluate in v0.5+.
// ----------------------------------------------------------------------------

const HTTP_VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

/// Window size after the path-anchor's closing `)` to scan for chained verbs.
/// Long enough to cover a multi-verb chain; short enough that we don't bleed
/// into the next statement.
const VERB_CHAIN_WINDOW: usize = 256;

fn scan_at_path_chains(source: &str, repo: RepoId, acc: &mut Acc) {
    scan_path_anchor_chain(source, ".at(", repo, acc);
}

fn scan_salvo_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    scan_path_anchor_chain(source, "Router::with_path(", repo, acc);
}

fn scan_path_anchor_chain(source: &str, needle: &str, repo: RepoId, acc: &mut Acc) {
    // Suppress duplicate (method, path) entries within a single source pass so
    // that nested `.at(...)` calls in a builder chain don't double-emit.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut search_from = 0;
    let bytes = source.as_bytes();
    while let Some(rel) = source[search_from..].find(needle) {
        let arg_start = search_from + rel + needle.len();
        // First arg must be a string literal beginning with `/` to qualify as a
        // URL path (rejects e.g. `slice.at(0)` or `Router::with_path(name)`).
        let Some(quote_off) = source[arg_start..].find('"') else {
            search_from = arg_start;
            continue;
        };
        let path_start = arg_start + quote_off + 1;
        let mut j = path_start;
        while j < bytes.len() && bytes[j] != b'"' {
            if bytes[j] == b'\\' && j + 1 < bytes.len() {
                j += 2;
            } else {
                j += 1;
            }
        }
        if j >= bytes.len() {
            break;
        }
        let path = &source[path_start..j];
        // `/`-prefix is the conventional gate against `.at(0)` etc.
        if !path.starts_with('/') {
            search_from = j + 1;
            continue;
        }
        // Find closing paren of the path-anchor call to bound the verb window.
        let Some(close) = find_matching_paren(&source[arg_start..]) else {
            search_from = j + 1;
            continue;
        };
        let after = arg_start + close + 1;
        // The path quote and the call's closing paren can be found
        // independently (the quote scan ignores paren depth; the paren scan
        // treats `"..."` as opaque). If `)` closes *before* the path quote we
        // matched, the candidate is a stray `.at(` inside a comment or string
        // and the `"` belongs to unrelated source further on — skip it.
        if after <= j {
            search_from = j + 1;
            continue;
        }
        // Verbs may live inside the call (Poem / Axum-style:
        // `.at("/p", get(h).post(h2))`) or chained after (Tide / Salvo:
        // `.at("/p").get(h).post(h2)`). One combined window catches both.
        let in_args_start = j + 1;
        // The window end is a raw byte offset: snap it down so a multibyte
        // char straddling the cut can't panic the slice (and drop the file).
        let raw_end = (after + VERB_CHAIN_WINDOW).min(source.len());
        let win_end = source.floor_char_boundary(raw_end);
        if win_end != raw_end {
            acc.window_snaps += 1;
        }
        let window = &source[in_args_start..win_end];

        for verb in HTTP_VERBS {
            let pat_dotted = format!(".{verb}(");
            let pat_bare = format!("{verb}(");
            // Inside-args style uses bare `get(handler)`; chained style uses
            // `.get(handler)`. `contains_method_call` enforces a non-word char
            // before the bare form so it doesn't match `target(` etc.
            if window.contains(&pat_dotted) || contains_method_call(window, &pat_bare) {
                let mu = verb.to_ascii_uppercase();
                let key = route_qname(&mu, path);
                if seen.insert(key.clone()) {
                    emit_axum_route(&mu, path, repo, acc);
                }
            }
        }
        search_from = after;
    }
}

/// Emit one legacy-shape ROUTE. The qname goes through the shared builder
/// (LB.5), so a relative `.route("widgets", …)` is `GET /widgets` — the key
/// every caller's `seen` set and HANDLED_BY ref id are built from too.
fn emit_axum_route(method: &str, path: &str, repo: RepoId, acc: &mut Acc) {
    let route_name = route_qname(method, path);
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

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    // (base, variant) pairs this fn already references (LA.3): a 40-arm
    // match over one enum is one USES ref per variant, not per arm.
    let mut variants: HashSet<(String, String)> = HashSet::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "call_expression" => {
                // `MatchTier::Suffix(3)` constructs a tuple variant: its callee
                // is a USES ref (the scoped_identifier arm), never a CallSite.
                if let Some(func) = n.child_by_field_name("function")
                    && variant_path(func, src).is_none()
                {
                    let qualifier = classify_call(func, src);
                    acc.calls.push(CallSite { from, qualifier });
                }
            }
            // A macro's arguments are a flat token tree, never a call_expression,
            // so the walk below finds nothing in them: scan the tokens instead.
            "macro_invocation" => {
                let found = collect_macro_calls(n, src, from, acc);
                acc.macro_calls += found;
            }
            // `MatchTier::Exact` in value or pattern position, a tuple-variant
            // constructor or pattern. Only the outermost path of a nested
            // `a::B::C` is read: its children are never pushed, and a path's
            // segments hold no expression.
            "scoped_identifier" => {
                push_variant_ref(n, src, from, &mut variants, acc);
                continue;
            }
            // `MatchTier::Named { n: 1 }` / `MatchTier::Named { n }` patterns.
            "struct_expression" | "struct_pattern" => {
                let field = if n.kind() == "struct_expression" {
                    "name"
                } else {
                    "type"
                };
                if let Some(name) = n.child_by_field_name(field) {
                    push_variant_ref(name, src, from, &mut variants, acc);
                }
            }
            // A type path names no variant, and a `use` inside a body is an
            // import, not a reference.
            "scoped_type_identifier" | "use_declaration" => continue,
            _ => {}
        }
        // Don't recurse into nested function items (closures are ok).
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if child.kind() != "function_item" {
                stack.push(child);
            }
        }
    }
}

fn classify_call(func_node: TsNode, src: &[u8]) -> CallQualifier {
    match func_node.kind() {
        "identifier" => CallQualifier::Bare(text_of(func_node, src).to_string()),
        "field_expression" => {
            let obj = func_node
                .child_by_field_name("value")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            let field = func_node
                .child_by_field_name("field")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            if obj == "self" {
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
        "scoped_identifier" => {
            let path = func_node
                .child_by_field_name("path")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            let name = func_node
                .child_by_field_name("name")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            CallQualifier::Attribute {
                base: path.to_string(),
                name: name.to_string(),
            }
        }
        _ => CallQualifier::ComplexReceiver {
            receiver: text_of(func_node, src).to_string(),
            name: String::new(),
        },
    }
}

// ============================================================================
// Enum-variant references (LA.3)
// ============================================================================

/// `(base, Variant)` when `node` is a `scoped_identifier` /
/// `scoped_type_identifier` whose name and the last segment of whose path
/// both start with an ASCII uppercase letter: `MatchTier::Exact`,
/// `crate::tier::MatchTier::Suffix`, `Self::Named`. The case test is the whole
/// heuristic: an associated const (`Self::MAX`) passes it and stays an
/// unresolved ref; `u32::MAX` and `endpoint::url_to_path` fail it.
fn variant_path(node: TsNode, src: &[u8]) -> Option<(String, String)> {
    if !matches!(node.kind(), "scoped_identifier" | "scoped_type_identifier") {
        return None;
    }
    let name = text_of(node.child_by_field_name("name")?, src);
    let base = text_of(node.child_by_field_name("path")?, src);
    let upper = |s: &str| s.bytes().next().is_some_and(|b| b.is_ascii_uppercase());
    (upper(name) && upper(&last_path_segment(base))).then(|| (base.to_string(), name.to_string()))
}

/// The last `::` segment of a path, generic arguments dropped
/// (`Wrap::<u8>` -> `Wrap`, `a::B` -> `B`).
fn last_path_segment(path: &str) -> String {
    let mut plain = String::with_capacity(path.len());
    let mut depth = 0usize;
    for ch in path.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(ch),
            _ => {}
        }
    }
    let plain = plain.trim_end_matches(':');
    plain
        .rsplit("::")
        .next()
        .unwrap_or(plain)
        .trim()
        .to_string()
}

/// One USES ref from `from` to the variant `node` names, once per
/// (base, variant) inside one fn. The graph crate binds it (an imported enum
/// through `resolve_refs`, a same-file / path-qualified / `Self` one through
/// `rust_paths`).
fn push_variant_ref(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    seen: &mut HashSet<(String, String)>,
    acc: &mut Acc,
) {
    let (Some((base, name)), Some(from_module)) = (variant_path(node, src), acc.file_module) else {
        return;
    };
    if !seen.insert((base.clone(), name.clone())) {
        return;
    }
    acc.refs.push(UnresolvedRef {
        from,
        from_module,
        qualifier: CallQualifier::Attribute { base, name },
        category: edge_category::USES,
    });
}

// ============================================================================
// Calls inside macro arguments (LA.2)
// ============================================================================
//
// tree-sitter-rust does not parse macro arguments as expressions: `run!(f(x))`
// is a `macro_invocation` over a flat `token_tree` of identifiers, literals,
// punctuation and nested token trees. Re-parsing the text as an expression
// would cost a second parse, shift every position, and mis-parse DSL macros
// whose arguments are not expressions. Instead the token tree is scanned for
// call-shaped runs and each becomes a CallSite of the same shape
// `classify_call` gives the equivalent non-macro call. Any run the scanner
// does not recognise yields no CallSite.

/// Macros whose second argument is a pattern (`matches!(v, Some(Wrap(_)))`).
/// Only the scrutinee, an `if` guard and any trailing arguments are expressions.
const PATTERN_MACROS: &[&str] = &["matches", "assert_matches", "debug_assert_matches"];

/// Never a callee name, however a token tree lexes it.
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern", "false",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "static", "struct", "trait", "true", "type", "unsafe", "use", "where", "while",
    "yield",
];

/// A path directly after one of these names what it defines or binds
/// (`fn f(x: u32)`, `struct W(u32)`, `let W(x) = w`), never a call.
const DEFINING_KEYWORDS: &[&str] = &["fn", "struct", "let"];

/// Token trees nest one level per bracket pair; deeper input is not scanned.
const MACRO_TT_DEPTH: usize = 32;

/// Calls written inside a macro invocation's arguments. Returns the number of
/// CallSites pushed.
fn collect_macro_calls(invocation: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) -> usize {
    let mut cursor = invocation.walk();
    let Some(tt) = invocation
        .named_children(&mut cursor)
        .find(|c| c.kind() == "token_tree")
    else {
        return 0;
    };
    let name = invocation
        .child_by_field_name("macro")
        .map(|m| text_of(m, src))
        .unwrap_or("");
    let name = name.rsplit("::").next().unwrap_or(name);
    acc.macro_invocations += 1;
    scan_token_tree(tt, src, from, PATTERN_MACROS.contains(&name), 0, acc)
}

/// A token tree opened by `(`: an argument list. `[..]` and `{..}` never are.
fn is_arg_list(n: TsNode) -> bool {
    n.kind() == "token_tree" && n.child(0).is_some_and(|c| c.kind() == "(")
}

fn is_operand(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "self" | "super" | "crate" | "primitive_type" | "metavariable"
    ) || kind.ends_with("_literal")
}

/// Scan one token tree. `pattern_tail`: the tree is a pattern macro's argument
/// list, so tokens from the first top-level `,` up to an `if` guard or the next
/// top-level `,` are a pattern and are skipped.
fn scan_token_tree(
    tt: TsNode,
    src: &[u8],
    from: NodeId,
    pattern_tail: bool,
    depth: usize,
    acc: &mut Acc,
) -> usize {
    if depth >= MACRO_TT_DEPTH {
        return 0;
    }
    let mut cursor = tt.walk();
    let toks: Vec<TsNode> = tt.children(&mut cursor).collect();
    let kind_at = |k: usize| toks.get(k).map(|t| t.kind()).unwrap_or("");
    let mut found = 0usize;
    // 0 = scrutinee, 1 = inside the pattern, 2 = past it.
    let mut pattern_phase = if pattern_tail { 0u8 } else { 2 };
    let mut i = 0usize;
    while i < toks.len() {
        let t = toks[i];
        match (pattern_phase, t.kind()) {
            (0, ",") => pattern_phase = 1,
            (1, "," | "if") => pattern_phase = 2,
            _ => {}
        }
        if pattern_phase == 1 {
            i += 1;
            continue;
        }

        // `#[attr(..)]` / `#![attr]`: an attribute's arguments are never calls.
        if t.kind() == "#" {
            let bracket = if kind_at(i + 1) == "!" { i + 2 } else { i + 1 };
            if toks.get(bracket).is_some_and(|b| {
                b.kind() == "token_tree" && b.child(0).is_some_and(|c| c.kind() == "[")
            }) {
                i = bracket + 1;
                continue;
            }
        }

        // METHOD CALL: `<receiver> . name ( .. )`.
        if t.kind() == "."
            && kind_at(i + 1) == "identifier"
            && toks.get(i + 2).is_some_and(|a| is_arg_list(*a))
        {
            let name = text_of(toks[i + 1], src).to_string();
            acc.calls.push(CallSite {
                from,
                qualifier: method_qualifier(&toks, i, name, src),
            });
            found += 1 + scan_token_tree(toks[i + 2], src, from, false, depth + 1, acc);
            i += 3;
            continue;
        }

        // PATH CALL / NESTED MACRO: `a::b::c ( .. )` / `a::b ! ( .. )`. A name
        // after `.` is a field (methods matched above); after `::` it is the
        // tail of a path this scanner did not start (`<T as Tr>::f`, `::<T>`).
        let after_joiner = i > 0 && matches!(kind_at(i - 1), "." | "::");
        if matches!(
            t.kind(),
            "identifier" | "self" | "super" | "crate" | "primitive_type"
        ) && !after_joiner
        {
            let mut segments = vec![text_of(t, src)];
            let mut j = i + 1;
            while kind_at(j) == "::" && kind_at(j + 1) == "identifier" {
                segments.push(text_of(toks[j + 1], src));
                j += 2;
            }
            if kind_at(j) == "!" && kind_at(j + 1) == "token_tree" {
                let last = segments.last().copied().unwrap_or("");
                acc.macro_invocations += 1;
                found += scan_token_tree(
                    toks[j + 1],
                    src,
                    from,
                    PATTERN_MACROS.contains(&last),
                    depth + 1,
                    acc,
                );
                i = j + 2;
                continue;
            }
            if let Some(args) = toks.get(j).copied().filter(|a| is_arg_list(*a)) {
                let defining = i > 0 && {
                    let prev = toks[i - 1];
                    DEFINING_KEYWORDS.contains(&prev.kind())
                        || DEFINING_KEYWORDS.contains(&text_of(prev, src))
                };
                let qualifier = match segments.as_slice() {
                    [name] if t.kind() == "identifier" && !RUST_KEYWORDS.contains(name) => {
                        Some(CallQualifier::Bare((*name).to_string()))
                    }
                    [_] => None,
                    [base @ .., name] => Some(CallQualifier::Attribute {
                        base: base.join("::"),
                        name: (*name).to_string(),
                    }),
                    [] => None,
                };
                if let Some(qualifier) = qualifier.filter(|_| !defining) {
                    acc.calls.push(CallSite { from, qualifier });
                    found += 1;
                }
                found += scan_token_tree(args, src, from, false, depth + 1, acc);
                i = j + 1;
                continue;
            }
            i = j;
            continue;
        }

        if t.kind() == "token_tree" {
            found += scan_token_tree(t, src, from, false, depth + 1, acc);
        }
        i += 1;
    }
    found
}

/// Qualifier for `<receiver>.name(..)` where `toks[dot]` is the `.`: the same
/// shapes `classify_call` gives a `field_expression` callee.
fn method_qualifier(toks: &[TsNode], dot: usize, name: String, src: &[u8]) -> CallQualifier {
    let recv = dot.checked_sub(1).map(|k| toks[k]);
    let lone = dot < 2 || !matches!(toks[dot - 2].kind(), "." | "::");
    match recv {
        Some(r) if lone && r.kind() == "self" => CallQualifier::SelfMethod(name),
        Some(r) if lone && r.kind() == "identifier" => CallQualifier::Attribute {
            base: text_of(r, src).to_string(),
            name,
        },
        _ => CallQualifier::ComplexReceiver {
            receiver: receiver_text(toks, dot, src),
            name,
        },
    }
}

/// Text of the postfix chain ending just before `toks[dot]` (`a.b`,
/// `f(x)?`, `format!(..)`), kept verbatim like `classify_call`'s receiver.
fn receiver_text(toks: &[TsNode], dot: usize, src: &[u8]) -> String {
    let mut start = dot;
    while start > 0 {
        let prev = toks[start - 1].kind();
        let next = toks.get(start).map(|t| t.kind()).unwrap_or("");
        let joins = start == dot
            || match prev {
                "." | "::" | "?" => true,
                "!" => next == "token_tree",
                k if is_operand(k) || k == "token_tree" => {
                    matches!(next, "." | "::" | "?" | "!" | "token_tree")
                }
                _ => false,
            };
        if !joins {
            break;
        }
        start -= 1;
    }
    toks[start..dot].iter().map(|t| text_of(*t, src)).collect()
}

// ============================================================================
// Outbound HTTP (reqwest) — client ENDPOINT emission
// ============================================================================
//
// A Rust service that only published its axum ROUTEs was a one-way node in a
// polyglot graph: inbound calls resolved, outbound ones vanished. Here each
// fn/method body is walked for reqwest call sites and each becomes a shared
// ENDPOINT node (qname `endpoint:<METHOD>:<path>` — what HttpStackResolver
// pairs with a server ROUTE) plus a CALLS edge from the enclosing fn.
//
// `.get(` is ubiquitous in Rust (HashMap / Vec / headers / Option chains), so
// `url_to_path` is the gate: a literal that is not a request path
// (`"default"`, `"x-trace-id"`) and any non-literal argument are both dropped.

/// Receiver-method names that name an HTTP verb on a reqwest client.
const CLIENT_VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head"];

/// Walk `body` for outbound HTTP calls, emitting one ENDPOINT per (method,path)
/// and one CALLS edge per call site. Returns the number of call sites found.
fn collect_client_endpoints_in(
    body: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> usize {
    let mut found = 0usize;
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some(func) = n.child_by_field_name("function")
            && let Some(args) = n.child_by_field_name("arguments")
            && let Some((method, raw, confidence)) = client_call_target(func, args, src)
            && let Some(path) = url_to_path(&raw)
        {
            let pos = n.start_position();
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
            found += 1;
        }
        // Don't recurse into nested function items (closures are ok).
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if child.kind() != "function_item" {
                stack.push(child);
            }
        }
    }
    found
}

/// Classify a call as an outbound HTTP request → `(VERB, raw url, confidence)`.
/// Recognises `client.<verb>(url)`, `client.request(Method::VERB, url)` and the
/// free functions `reqwest::get` / `reqwest::blocking::get`.
fn client_call_target(
    func: TsNode,
    args: TsNode,
    src: &[u8],
) -> Option<(String, String, Confidence)> {
    match func.kind() {
        "field_expression" => {
            let field = text_of(func.child_by_field_name("field")?, src);
            if CLIENT_VERBS.contains(&field) {
                let (raw, conf) = url_arg(args.named_child(0)?, src)?;
                return Some((field.to_ascii_uppercase(), raw, conf));
            }
            if field == "request" {
                let verb = method_const_verb(args.named_child(0)?, src)?;
                let (raw, conf) = url_arg(args.named_child(1)?, src)?;
                return Some((verb, raw, conf));
            }
            None
        }
        "scoped_identifier" => {
            if text_of(func.child_by_field_name("name")?, src) != "get" {
                return None;
            }
            let path = text_of(func.child_by_field_name("path")?, src);
            if path != "reqwest" && path != "reqwest::blocking" {
                return None;
            }
            let (raw, conf) = url_arg(args.named_child(0)?, src)?;
            Some(("GET".to_string(), raw, conf))
        }
        _ => None,
    }
}

/// `Method::GET` → `"GET"`. Any other first-argument shape (or a non-verb
/// associated item) → None, so `client.request(build(), url)` is dropped.
fn method_const_verb(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() != "scoped_identifier" {
        return None;
    }
    let name = text_of(node.child_by_field_name("name")?, src).to_ascii_uppercase();
    CLIENT_VERBS
        .iter()
        .any(|v| v.eq_ignore_ascii_case(&name))
        .then_some(name)
}

/// The URL argument: a `string_literal`/`raw_string_literal` (Strong), or
/// `format!("…{}…", …)` with the holes rewritten to the `${…}` substitution
/// marker `normalise_http_path` collapses (Medium). A variable → None.
fn url_arg(node: TsNode, src: &[u8]) -> Option<(String, Confidence)> {
    match node.kind() {
        "string_literal" | "raw_string_literal" => {
            Some((string_content(node, src), Confidence::Strong))
        }
        "macro_invocation" => {
            if text_of(node.child_by_field_name("macro")?, src) != "format" {
                return None;
            }
            let mut cursor = node.walk();
            let tt = node
                .named_children(&mut cursor)
                .find(|c| c.kind() == "token_tree")?;
            let mut inner = tt.walk();
            let lit = tt
                .named_children(&mut inner)
                .find(|c| matches!(c.kind(), "string_literal" | "raw_string_literal"))?;
            Some((
                expand_format_holes(&string_content(lit, src)),
                Confidence::Medium,
            ))
        }
        _ => None,
    }
}

/// Text between the quotes. An empty literal has no `string_content` child.
fn string_content(lit: TsNode, src: &[u8]) -> String {
    let mut cursor = lit.walk();
    lit.named_children(&mut cursor)
        .find(|c| c.kind() == "string_content")
        .map(|c| text_of(c, src).to_string())
        .unwrap_or_default()
}

/// `"/api/users/{}"` and `"/api/v/{name}"` → `"/api/users/${…}"`. `{{`/`}}` are
/// escaped braces in a format string and pass through as one literal brace.
fn expand_format_holes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '{' => {
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                }
                out.push_str("${\u{2026}}");
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            _ => out.push(c),
        }
    }
    out
}

// ============================================================================
// Helpers
// ============================================================================

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
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
    fn structs_and_functions() {
        let source = r#"
pub struct User {
    name: String,
}

pub fn create_user(name: &str) -> User {
    User { name: name.to_string() }
}
"#;
        let fp = parse_file(source, "src/models.rs", "myapp::models", repo()).unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"models"));
        assert!(names.contains(&"User"));
        assert!(names.contains(&"create_user"));
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::STRUCT).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 1);
    }

    #[test]
    fn impl_methods() {
        let source = r#"
struct Foo;

impl Foo {
    pub fn bar(&self) -> i32 { 42 }
    fn baz(&mut self) {}
}
"#;
        let fp = parse_file(source, "src/foo.rs", "myapp::foo", repo()).unwrap();
        let methods: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::METHOD)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(methods.contains(&"bar"));
        assert!(methods.contains(&"baz"));
        assert_eq!(methods.len(), 2);
    }

    #[test]
    fn enums_and_traits() {
        let source = r#"
pub enum Color {
    Red,
    Green,
    Blue,
}

pub trait Drawable {
    fn draw(&self);
}
"#;
        let fp = parse_file(source, "src/lib.rs", "myapp", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::ENUM).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        // LA.3: the three variants are ATTRIBUTE children of the ENUM.
        let color = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENUM, "myapp::Color");
        let mut variants: Vec<&str> = fp.nav.children_of[&color]
            .iter()
            .filter(|c| fp.nav.kind_by_id[*c] == node_kind::ATTRIBUTE)
            .map(|c| fp.nav.qname_by_id[c].as_str())
            .collect();
        variants.sort_unstable();
        assert_eq!(
            variants,
            vec![
                "myapp::Color::Blue",
                "myapp::Color::Green",
                "myapp::Color::Red"
            ]
        );
    }

    // ---- LA.3: inline mods and enum variants --------------------------------

    /// bench/substrate-gap/fixtures/rust-inline-mod-enum/src/lib.rs, verbatim.
    const INLINE_FIXTURE: &str = r#"pub enum MatchTier { Exact, BaseFold, Suffix(u8), Named { n: u32 } }

impl MatchTier {
    pub fn rank(&self) -> u8 { self.weight() }
    fn weight(&self) -> u8 { 1 }
}

pub fn helper() -> u8 { 0 }

pub mod endpoint {
    pub fn url_to_path(s: &str) -> String { let _ = helper(); s.to_string() }
    fn helper() -> u8 { 1 }
    pub mod inner {
        pub fn deep() {}
    }
}

pub fn tier() -> MatchTier { MatchTier::BaseFold }
pub fn mk() -> MatchTier { MatchTier::Suffix(3) }
pub fn named() -> MatchTier { MatchTier::Named { n: 1 } }
pub fn use_ep() -> String { endpoint::url_to_path("x") }
pub fn use_inner() { endpoint::inner::deep() }
pub fn classify(t: MatchTier) -> u8 {
    match t { MatchTier::Exact => 0, MatchTier::Suffix(_) => 2, _ => 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tier_is_base_fold() { let _ = tier(); }
}
"#;

    fn nid(kind: repo_graph_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn has_edge(
        fp: &FileParse,
        from: NodeId,
        to: NodeId,
        cat: repo_graph_core::EdgeCategoryId,
    ) -> bool {
        fp.edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == cat)
    }

    #[test]
    fn inline_mod_items_are_nodes() {
        let fp = parse_file(INLINE_FIXTURE, "src/lib.rs", "m", repo()).unwrap();
        let file = nid(node_kind::MODULE, "m");
        let ep = nid(node_kind::PACKAGE, "m::endpoint");
        let inner = nid(node_kind::PACKAGE, "m::endpoint::inner");
        let tests = nid(node_kind::PACKAGE, "m::tests");
        assert_eq!(fp.nodes[0].id, file, "the file MODULE stays nodes[0]");
        for (pkg, parent) in [(ep, file), (inner, ep), (tests, file)] {
            assert_eq!(fp.nav.kind_by_id.get(&pkg), Some(&node_kind::PACKAGE));
            assert_eq!(fp.nav.parent_of.get(&pkg), Some(&parent));
            assert!(has_edge(&fp, parent, pkg, edge_category::CONTAINS));
        }
        for (q, parent) in [
            ("m::endpoint::url_to_path", ep),
            ("m::endpoint::helper", ep),
            ("m::endpoint::inner::deep", inner),
            ("m::tests::tier_is_base_fold", tests),
        ] {
            let id = nid(node_kind::FUNCTION, q);
            assert_eq!(fp.nav.parent_of.get(&id), Some(&parent), "{q}");
            assert!(has_edge(&fp, parent, id, edge_category::DEFINES), "{q}");
        }
        assert!(
            !fp.nav.qname_by_id.values().any(|q| q == "m::url_to_path"),
            "a fn inside `mod endpoint` never flattens into the file module"
        );
        // A PACKAGE carries POSITION, never CODE (its children carry theirs).
        let ep_node = fp.nodes.iter().find(|n| n.id == ep).unwrap();
        let kinds: Vec<_> = ep_node.cells.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![cell_type::POSITION]);
        // `use super::*` inside `mod tests` is recorded against the PACKAGE.
        assert!(fp.imports.iter().any(|i| i.from_module == "m::tests"
            && matches!(&i.target, ImportTarget::Module { path, .. } if path == "super::*")));
        // The bare call inside the mod is the mod fn's CallSite.
        let url = nid(node_kind::FUNCTION, "m::endpoint::url_to_path");
        assert!(
            fp.calls
                .iter()
                .any(|c| c.from == url && c.qualifier == bare("helper"))
        );
    }

    /// Top-level items keep their qnames and relative order: an inline mod's
    /// nodes slot in at its position, and nothing before or after moves.
    #[test]
    fn top_level_order_is_unchanged() {
        let fp = parse_file(INLINE_FIXTURE, "src/lib.rs", "m", repo()).unwrap();
        let top: Vec<&str> = fp
            .nodes
            .iter()
            .filter(|n| fp.nav.parent_of.get(&n.id) == Some(&nid(node_kind::MODULE, "m")))
            .map(|n| fp.nav.qname_by_id[&n.id].as_str())
            .collect();
        assert_eq!(
            top,
            vec![
                "m::MatchTier",
                "m::helper",
                "m::endpoint",
                "m::tier",
                "m::mk",
                "m::named",
                "m::use_ep",
                "m::use_inner",
                "m::classify",
                "m::tests",
            ]
        );
    }

    #[test]
    fn mod_without_body_emits_nothing() {
        let fp = parse_file(
            "mod api;\npub mod util;\nfn f() {}\n",
            "src/lib.rs",
            "m",
            repo(),
        )
        .unwrap();
        assert!(!fp.nav.kind_by_id.values().any(|k| *k == node_kind::PACKAGE));
        assert_eq!(fp.nodes.len(), 2, "the file MODULE and `f`");
    }

    #[test]
    fn enum_variants_are_attributes() {
        let fp = parse_file(INLINE_FIXTURE, "src/lib.rs", "m", repo()).unwrap();
        let tier = nid(node_kind::ENUM, "m::MatchTier");
        for v in ["Exact", "BaseFold", "Suffix", "Named"] {
            let id = nid(node_kind::ATTRIBUTE, &format!("m::MatchTier::{v}"));
            assert!(has_edge(&fp, tier, id, edge_category::HAS_ATTRIBUTE), "{v}");
            assert_eq!(fp.nav.parent_of.get(&id), Some(&tier), "{v}");
            assert_eq!(fp.nav.name_by_id[&id], v);
        }
        let attrs = fp
            .nav
            .kind_by_id
            .values()
            .filter(|k| **k == node_kind::ATTRIBUTE)
            .count();
        assert_eq!(attrs, 4);
        // A tuple / struct variant's CODE is the whole variant.
        let named = nid(node_kind::ATTRIBUTE, "m::MatchTier::Named");
        let code = fp.nodes.iter().find(|n| n.id == named).and_then(|n| {
            n.cells.iter().find_map(|c| match &c.payload {
                CellPayload::Text(t) if c.kind == cell_type::CODE => Some(t.as_str()),
                _ => None,
            })
        });
        assert_eq!(code, Some("Named { n: u32 }"));
    }

    /// `(from fn name, base, variant)` of every USES ref.
    fn uses_refs(fp: &FileParse) -> Vec<(String, String, String)> {
        fp.refs
            .iter()
            .filter(|r| r.category == edge_category::USES)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Attribute { base, name } => Some((
                    fp.nav.name_by_id[&r.from].clone(),
                    base.clone(),
                    name.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn variant_refs_are_uses_refs() {
        let fp = parse_file(INLINE_FIXTURE, "src/lib.rs", "m", repo()).unwrap();
        let mut refs = uses_refs(&fp);
        refs.sort();
        let r = |f: &str, v: &str| (f.to_string(), "MatchTier".to_string(), v.to_string());
        assert_eq!(
            refs,
            vec![
                r("classify", "Exact"),
                r("classify", "Suffix"),
                r("mk", "Suffix"),
                r("named", "Named"),
                r("tier", "BaseFold"),
            ]
        );
        let file = nid(node_kind::MODULE, "m");
        assert!(
            fp.refs
                .iter()
                .all(|r| r.category != edge_category::USES || r.from_module == file)
        );
        // `MatchTier::Suffix(3)` constructs a variant: no CallSite.
        let mk = nid(node_kind::FUNCTION, "m::mk");
        assert!(
            !fp.calls.iter().any(|c| c.from == mk),
            "{:?}",
            fn_calls(&fp, "mk")
        );
        // The path calls into the inline mod stay CallSites.
        assert_eq!(
            fn_calls(&fp, "use_ep"),
            vec![attr("endpoint", "url_to_path")]
        );
        assert_eq!(
            fn_calls(&fp, "use_inner"),
            vec![attr("endpoint::inner", "deep")]
        );
    }

    #[test]
    fn variant_refs_dedupe_and_skip_non_variants() {
        let source = r#"
pub enum Op { Add, Sub(u8), Mul { k: u8 } }
impl Op {
    fn flip(&self) -> Op {
        match self { Self::Add => Self::Sub(1), Op::Sub(_) => Op::Add, Op::Mul { .. } => Op::Add }
    }
}
fn other() -> u32 {
    use crate::Op::Add;
    let _ = Self::MAX;
    let _ = u32::MAX + node_kind::MODULE;
    let _ = crate::ops::Op::Mul { k: 2 };
    let _ = Box::new(Wrap::<u8>::Inner);
    endpoint::url_to_path("x")
}
"#;
        let fp = parse_file(source, "src/lib.rs", "m", repo()).unwrap();
        let mut refs = uses_refs(&fp);
        refs.sort();
        let r = |f: &str, b: &str, v: &str| (f.to_string(), b.to_string(), v.to_string());
        assert_eq!(
            refs,
            vec![
                r("flip", "Op", "Add"),
                r("flip", "Op", "Mul"),
                r("flip", "Op", "Sub"),
                r("flip", "Self", "Add"),
                r("flip", "Self", "Sub"),
                r("other", "Self", "MAX"),
                r("other", "Wrap::<u8>", "Inner"),
                r("other", "crate::ops::Op", "Mul"),
            ],
            "Op::Add twice in `flip` is one ref; `use` paths, `u32::MAX` and \
             lower-case paths are not variant refs; `Self::MAX` passes the case test"
        );
        // Box::new stays a call; only the variant constructions are dropped.
        let flip = nid(node_kind::METHOD, "m::Op::flip");
        assert!(!fp.calls.iter().any(|c| c.from == flip));
        assert_eq!(
            fn_calls(&fp, "other"),
            vec![attr("endpoint", "url_to_path"), attr("Box", "new")]
        );
    }

    #[test]
    fn const_static_noise_gate_and_implements() {
        let source = r#"
/// Fee.
pub const FEE_BPS: u32 = 250;

const X: u32 = 1;

pub struct Foo;

pub trait Display {
    fn fmt(&self);
}

impl Display for Foo {
    fn fmt(&self) {}
}
"#;
        let fp = parse_file(source, "src/lib.rs", "myapp", repo()).unwrap();

        // G19: documented const → STATE_VAR; undocumented literal const skipped.
        let state_vars: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::STATE_VAR)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(state_vars.contains(&"FEE_BPS"));
        assert!(!state_vars.contains(&"X"));
        assert_eq!(state_vars.len(), 1);

        // FEE_BPS carries its doc cell.
        let fee_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "myapp::FEE_BPS");
        let fee_node = fp.nodes.iter().find(|n| n.id == fee_id).unwrap();
        assert!(fee_node.cells.iter().any(|c| c.kind == cell_type::DOC));

        // G12.5: Foo implements in-file trait Display → IMPLEMENTS edge.
        let foo_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STRUCT, "myapp::Foo");
        let display_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, "myapp::Display");
        assert!(fp.edges.iter().any(|e| e.category == edge_category::IMPLEMENTS
            && e.from == foo_id
            && e.to == display_id));
    }

    #[test]
    fn use_imports() {
        let source = r#"
use crate::models::User;
use crate::db;
use crate::auth::{login, logout};
use std::io::Read;
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        // std import is skipped (external)
        assert_eq!(fp.imports.len(), 4); // User, db, login, logout
    }

    #[test]
    fn self_method_calls() {
        let source = r#"
struct Server;

impl Server {
    fn handle(&self) {
        self.validate();
        self.respond();
    }
    fn validate(&self) {}
    fn respond(&self) {}
}
"#;
        let fp = parse_file(source, "src/server.rs", "myapp::server", repo()).unwrap();
        let self_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::SelfMethod(_)))
            .collect();
        assert_eq!(self_calls.len(), 2);
    }

    #[test]
    fn axum_routes_basic() {
        let source = r#"
async fn list_users() {}
async fn create_user() {}
async fn get_user() {}

fn app() -> Router {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/:id", get(get_user))
}
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /users"));
        assert!(route_names.contains(&"POST /users"));
        assert!(route_names.contains(&"GET /users/:id"));
    }

    #[test]
    fn axum_routes_emit_handled_by_refs() {
        let source = r#"
async fn list_users() {}
async fn create_user() {}
async fn get_user() {}

fn app() -> Router {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route("/users/:id", get(get_user))
}
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        // Each verb-wrapper (`get(fn)`, `post(fn)`) yields a HANDLED_BY ref
        // from its ROUTE node to the handler fn, as a Bare qualifier.
        let handlers: Vec<&str> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert!(handlers.contains(&"list_users"), "handlers: {handlers:?}");
        assert!(handlers.contains(&"create_user"), "handlers: {handlers:?}");
        assert!(handlers.contains(&"get_user"), "handlers: {handlers:?}");

        // The ROUTE `GET /users` must be the `from` of the list_users ref.
        let route_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "GET /users");
        assert!(fp.refs.iter().any(|r| r.category == edge_category::HANDLED_BY
            && r.from == route_id
            && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "list_users")));

        // Path-qualified handlers reduce to the fn's bare name.
        assert_eq!(
            extract_handler_name("get(api::handlers::show)", "get("),
            Some("show".to_string())
        );
        // Closures don't name a handler fn — skipped.
        assert_eq!(extract_handler_name("get(|| async {})", "get("), None);
    }

    fn route_names(fp: &FileParse) -> Vec<&str> {
        fp.nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect()
    }

    /// LB.5 — relative axum / actix path literals get the one canonical
    /// leading `/`, the HANDLED_BY ref hangs off that canonical node, and a
    /// relative + slashed registration of one path is one node.
    #[test]
    fn relative_route_literals_are_canonical() {
        let source = r#"
use axum::{Router, routing::get};

pub fn app() -> Router {
    Router::new()
        .route("widgets", get(list_widgets))
        .route("/widgets", get(list_widgets))
}

#[post("gadgets")]
async fn make_gadget() {}
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        let mut names = route_names(&fp);
        names.sort_unstable();
        assert_eq!(names, vec!["GET /widgets", "POST /gadgets"]);
        let widgets = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "GET /widgets");
        assert_eq!(
            fp.nodes.iter().filter(|n| n.id == widgets).count(),
            1,
            "relative and slashed registrations are one node"
        );
        assert!(
            fp.refs
                .iter()
                .any(|r| r.category == edge_category::HANDLED_BY
                    && r.from == widgets
                    && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "list_widgets"))
        );
    }

    #[test]
    fn tide_at_chain_emits_per_method() {
        let source = r#"
fn app() -> tide::Server<()> {
    let mut app = tide::new();
    app.at("/health").get(health_handler);
    app.at("/users").get(list_users).post(create_user);
    app
}
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        let names = route_names(&fp);
        assert!(names.contains(&"GET /health"));
        assert!(names.contains(&"GET /users"));
        assert!(names.contains(&"POST /users"));
    }

    #[test]
    fn poem_at_chain_emits_per_method() {
        let source = r#"
fn app() -> poem::Route {
    Route::new()
        .at("/api/users", get(list_users).post(create_user))
        .at("/api/users/:id", put(update_user).delete(delete_user))
}
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        let names = route_names(&fp);
        assert!(names.contains(&"GET /api/users"));
        assert!(names.contains(&"POST /api/users"));
        assert!(names.contains(&"PUT /api/users/:id"));
        assert!(names.contains(&"DELETE /api/users/:id"));
    }

    #[test]
    fn salvo_with_path_chain_emits_per_method() {
        let source = r#"
fn app() -> salvo::Router {
    Router::with_path("/health").get(health_handler);
    Router::with_path("/users").get(list).post(create).delete(remove);
}
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        let names = route_names(&fp);
        assert!(names.contains(&"GET /health"));
        assert!(names.contains(&"GET /users"));
        assert!(names.contains(&"POST /users"));
        assert!(names.contains(&"DELETE /users"));
    }

    #[test]
    fn at_chain_skips_non_path_first_arg() {
        // `slice.at(0)`, `vec.at(idx)`, etc. — `.at(...)` is more general than
        // routes. The path-`/` filter rejects them.
        let source = r#"
fn run(items: &[&str]) {
    let _ = items.at(0).get(0);
    let _ = lookup.at("cache-key").get();
}
"#;
        let fp = parse_file(source, "src/main.rs", "myapp", repo()).unwrap();
        let names = route_names(&fp);
        assert!(names.is_empty(), "non-`/` `.at(...)` args must not emit routes");
    }

    /// LA.25b — the verb window after a Tide / Poem `.at("/p")` or a Salvo
    /// `Router::with_path("/p")` ends `VERB_CHAIN_WINDOW` bytes past the
    /// anchor's `)`. A 4-byte char starting at `after + 255` straddles that
    /// cut; before the snap the slice panicked and per-file isolation dropped
    /// the whole file.
    #[test]
    fn at_chain_window_cut_inside_a_multibyte_char() {
        const WIDE: char = '\u{1F600}';
        let cases = [
            ("app.at(\"/health\")", ".get(health)", "GET /health"),
            ("Router::with_path(\"/users\")", ".get(list)", "GET /users"),
        ];
        for (anchor, chain, route) in cases {
            let head = format!("fn app() {{\n    {anchor}");
            let after = head.len();
            assert_eq!(head.as_bytes()[after - 1], b')');
            let body = format!("{head}{chain};\n    // ");
            let pad = "x".repeat(after + VERB_CHAIN_WINDOW - 1 - body.len());
            let source = format!("{body}{pad}{WIDE}\n}}\n");
            assert_eq!(source.find(WIDE), Some(after + 255), "{anchor}");
            assert!(!source.is_char_boundary(after + VERB_CHAIN_WINDOW));

            let fp = parse_file(&source, "src/main.rs", "myapp", repo());
            assert!(fp.is_ok(), "{anchor}: parse failed");
            let fp = fp.unwrap();
            assert!(
                route_names(&fp).contains(&route),
                "{anchor}: {route} missing, got {:?}",
                route_names(&fp)
            );

            let mut acc = Acc::default();
            scan_at_path_chains(&source, repo(), &mut acc);
            scan_salvo_routes(&source, repo(), &mut acc);
            assert_eq!(acc.window_snaps, 1, "{anchor}: one window snapped");
        }
    }

    fn endpoint_names(fp: &FileParse) -> Vec<&str> {
        fp.nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ENDPOINT))
            .map(|(_, n)| n.as_str())
            .collect()
    }

    fn id_of(fp: &FileParse, name: &str) -> NodeId {
        *fp.nav
            .name_by_id
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| id)
            .expect("node not found")
    }

    const REQWEST_CLIENT: &str = r#"
use reqwest::Client;
use std::collections::HashMap;

struct ApiClient {
    client: Client,
    cache: HashMap<String, String>,
}

impl ApiClient {
    async fn fetch_user(&self, id: &str) -> String {
        self.client.get(format!("http://users-svc/api/users/{}", id)).send().await
    }

    async fn create_user(&self, body: String) -> String {
        self.client.post("/api/users").body(body).send().await
    }

    fn cached(&self, id: &str) -> Option<&String> {
        self.cache.get(id)
    }
}
"#;

    #[test]
    fn reqwest_client_get_emits_endpoint() {
        let fp = parse_file(REQWEST_CLIENT, "src/client.rs", "myapp", repo()).unwrap();
        let names = endpoint_names(&fp);
        assert!(
            names.contains(&"POST /api/users"),
            "literal client.post path missing: {names:?}"
        );
        // Free function + explicit-method forms land on the same node shape.
        let fp2 = parse_file(
            "fn f() { let _ = reqwest::get(\"http://svc/api/ping\"); }",
            "src/f.rs",
            "myapp",
            repo(),
        )
        .unwrap();
        assert!(endpoint_names(&fp2).contains(&"GET /api/ping"));
        let fp3 = parse_file(
            "fn f() { let _ = c.request(Method::DELETE, \"http://svc/api/ping\"); }",
            "src/f.rs",
            "myapp",
            repo(),
        )
        .unwrap();
        assert!(endpoint_names(&fp3).contains(&"DELETE /api/ping"));

        // CALLS edge: the enclosing method points at the ENDPOINT it hits.
        let caller = id_of(&fp, "create_user");
        let ep = id_of(&fp, "POST /api/users");
        assert!(
            fp.edges.iter().any(|e| e.from == caller
                && e.to == ep
                && e.category == edge_category::CALLS),
            "no CALLS edge create_user -> POST /api/users"
        );
    }

    #[test]
    fn reqwest_format_macro_path_becomes_wildcard() {
        let fp = parse_file(REQWEST_CLIENT, "src/client.rs", "myapp", repo()).unwrap();
        let names = endpoint_names(&fp);
        assert!(
            names.contains(&"GET /api/users/${\u{2026}}"),
            "format! host+hole path not normalised: {names:?}"
        );
        // Named holes normalise identically.
        let fp2 = parse_file(
            "fn f() { let _ = c.put(format!(\"/api/v/{name}\")); }",
            "src/f.rs",
            "myapp",
            repo(),
        )
        .unwrap();
        assert!(endpoint_names(&fp2).contains(&"PUT /api/v/${\u{2026}}"));
    }

    #[test]
    fn rust_client_calls_emit_no_route() {
        // A pure client file has no `.route(` / `.at(` / `Router::with_path(`
        // needle, so the server-route scanners must stay silent — an outbound
        // call must never be mistaken for an inbound route.
        let fp = parse_file(REQWEST_CLIENT, "src/client.rs", "myapp", repo()).unwrap();
        assert!(
            route_names(&fp).is_empty(),
            "client call sites minted phantom ROUTEs: {:?}",
            route_names(&fp)
        );
    }

    #[test]
    fn rust_map_get_is_dropped() {
        // `.get(` is ubiquitous in Rust. `url_to_path` is the only gate, and it
        // must reject both a variable key and a non-path string literal.
        let source = r#"
use std::collections::HashMap;

fn lookup(map: &HashMap<String, String>, headers: &HashMap<String, String>) -> usize {
    let _ = map.get("default");
    let _ = map.get(&key);
    let _ = headers.get("x-trace-id");
    let _ = items.get(0);
    0
}
"#;
        let fp = parse_file(source, "src/util.rs", "myapp", repo()).unwrap();
        assert!(
            endpoint_names(&fp).is_empty(),
            "map/header `.get(` became ENDPOINTs: {:?}",
            endpoint_names(&fp)
        );
    }

    // ---- LA.2: calls inside macro arguments --------------------------------

    /// bench/substrate-gap/fixtures/rust-macro-arg-calls/src/lib.rs, verbatim.
    const MACRO_FIXTURE: &str = r#"macro_rules! run {
    ($call:expr) => {{
        let _ = $call;
    }};
}

pub struct Wrap(pub u32);

pub fn helper(x: u32) -> u32 { x + 1 }
pub fn fmt_id(x: u32) -> String { format!("id-{}", x) }
pub fn total(v: Vec<u32>) -> u32 { v.len() as u32 }

pub fn entry() -> u32 {
    run!(helper(2));
    println!("{}", fmt_id(helper(1)));
    assert_eq!(helper(0), 1);
    total(vec![helper(3)])
}

pub fn quiet() { println!("fmt_id(1) is not a call"); }

pub fn is_wrap(w: Option<Wrap>) -> bool { matches!(w, Some(Wrap(_))) }

pub struct Svc;
impl Svc {
    pub fn go(&self) -> String { format!("{}", self.name()) }
    fn name(&self) -> String { String::new() }
}
"#;

    /// Qualifiers of every CallSite whose caller is the FUNCTION `m::<name>`.
    fn fn_calls(fp: &FileParse, name: &str) -> Vec<CallQualifier> {
        let id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            &format!("m::{name}"),
        );
        fp.calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| c.qualifier.clone())
            .collect()
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.to_string())
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    /// Calls of `fn f() { <body> }`.
    fn body_calls(body: &str) -> Vec<CallQualifier> {
        let source = format!("fn f() {{ {body} }}\n");
        let fp = parse_file(&source, "src/m.rs", "m", repo()).unwrap();
        fn_calls(&fp, "f")
    }

    #[test]
    fn macro_arg_calls_are_extracted() {
        let source = format!(
            "{MACRO_FIXTURE}\nmod util {{}}\n\
             pub fn paths() -> Vec<u32> {{ vec![util::scaled(3), crate::util::scaled(4)] }}\n"
        );
        let fp = parse_file(&source, "src/lib.rs", "m", repo()).unwrap();
        let entry = fn_calls(&fp, "entry");
        assert_eq!(
            entry.iter().filter(|q| **q == bare("helper")).count(),
            4,
            "{entry:?}"
        );
        assert_eq!(
            entry.iter().filter(|q| **q == bare("fmt_id")).count(),
            1,
            "{entry:?}"
        );
        assert_eq!(
            entry.iter().filter(|q| **q == bare("total")).count(),
            1,
            "{entry:?}"
        );
        assert_eq!(
            entry.len(),
            6,
            "no CallSite for the macro names themselves: {entry:?}"
        );

        let paths = fn_calls(&fp, "paths");
        assert_eq!(
            paths,
            vec![attr("util", "scaled"), attr("crate::util", "scaled")]
        );
    }

    #[test]
    fn string_literal_in_macro_is_not_a_call() {
        let fp = parse_file(MACRO_FIXTURE, "src/lib.rs", "m", repo()).unwrap();
        assert!(
            fn_calls(&fp, "quiet").is_empty(),
            "{:?}",
            fn_calls(&fp, "quiet")
        );
    }

    #[test]
    fn matches_pattern_is_not_a_call() {
        let fp = parse_file(MACRO_FIXTURE, "src/lib.rs", "m", repo()).unwrap();
        assert!(
            fn_calls(&fp, "is_wrap").is_empty(),
            "{:?}",
            fn_calls(&fp, "is_wrap")
        );

        // The scrutinee, an `if` guard and trailing arguments stay expressions.
        let calls = body_calls(
            "assert_matches!(load(1), Some(Wrap(n)) if check(n), \"{}\", why(2)); \
             std::matches!(w, Wrap(_) | Other(_));",
        );
        assert_eq!(calls, vec![bare("load"), bare("check"), bare("why")]);
    }

    #[test]
    fn self_method_inside_format() {
        let fp = parse_file(MACRO_FIXTURE, "src/lib.rs", "m", repo()).unwrap();
        let go = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "m::Svc::go");
        let calls: Vec<&CallQualifier> = fp
            .calls
            .iter()
            .filter(|c| c.from == go)
            .map(|c| &c.qualifier)
            .collect();
        assert_eq!(calls, vec![&CallQualifier::SelfMethod("name".to_string())]);
    }

    #[test]
    fn nested_macro_recurses() {
        let calls = body_calls("let _ = vec![format!(\"{}\", f(1))];");
        assert_eq!(calls, vec![bare("f")]);
    }

    #[test]
    fn macro_method_receivers_match_classify_call() {
        let calls = body_calls(
            "log!(x.go(1) && !y.is_ok(), a.b.c(), format!(\"{}\", 1).len(), load()?.id(), \
             Self::new(2), u32::from(3));",
        );
        assert_eq!(
            calls,
            vec![
                attr("x", "go"),
                attr("y", "is_ok"),
                CallQualifier::ComplexReceiver {
                    receiver: "a.b".to_string(),
                    name: "c".to_string()
                },
                CallQualifier::ComplexReceiver {
                    receiver: "format!(\"{}\", 1)".to_string(),
                    name: "len".to_string(),
                },
                bare("load"),
                CallQualifier::ComplexReceiver {
                    receiver: "load()?".to_string(),
                    name: "id".to_string()
                },
                attr("Self", "new"),
                attr("u32", "from"),
            ]
        );
    }

    #[test]
    fn macro_definitions_attributes_and_blocks_are_not_calls() {
        let calls = body_calls(
            "m!(#[cfg(feature = \"x\")] fn g(x: u32) { h(x) } \
             let Wrap(a) = w; struct T(u32); Foo { a: k(1) } \
             <T as Tr>::assoc(1), foo::<u8>(2), if(3));",
        );
        assert_eq!(calls, vec![bare("h"), bare("k")]);
    }

    #[test]
    fn macro_token_tree_depth_is_capped() {
        let deep = format!("m!({}helper(1){});", "(".repeat(40), ")".repeat(40));
        assert!(body_calls(&deep).is_empty());
        let shallow = format!("m!({}helper(1){});", "(".repeat(20), ")".repeat(20));
        assert_eq!(body_calls(&shallow), vec![bare("helper")]);
    }

    #[test]
    fn macro_counters_match_marker() {
        // The `[rust-macro-calls]` line reports these two counters.
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(MACRO_FIXTURE, None).unwrap();
        let src = MACRO_FIXTURE.as_bytes();
        let from = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::f");
        let mut acc = Acc::default();
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "function_item"
                && let Some(body) = n.child_by_field_name("body")
            {
                collect_calls_in(body, src, from, &mut acc);
            }
            let mut cursor = n.walk();
            stack.extend(n.named_children(&mut cursor));
        }
        assert_eq!((acc.macro_calls, acc.macro_invocations), (6, 8));
    }
}
