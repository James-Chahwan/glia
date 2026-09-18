use std::collections::{HashMap, HashSet};

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

    // First pass: collect type names → NodeId for impl block resolution.
    let mut type_ids: HashMap<String, NodeId> = HashMap::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "struct_item" | "enum_item" | "trait_item" => {
                if let Some(name) = child.child_by_field_name("name") {
                    let name_str = text_of(name, src);
                    let kind = match child.kind() {
                        "struct_item" => node_kind::STRUCT,
                        "enum_item" => node_kind::ENUM,
                        "trait_item" => node_kind::INTERFACE,
                        _ => unreachable!(),
                    };
                    let qname = format!("{module_qname}::{name_str}");
                    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
                    type_ids.insert(name_str.to_string(), id);

                    acc.nodes.push(Node {
                        id,
                        repo,
                        confidence: Confidence::Strong,
                        cells: entity_cells(&child, src, file_rel_path),
                    });
                    acc.edges.push(Edge {
                        from: module_id,
                        to: id,
                        category: edge_category::DEFINES,
                        confidence: Confidence::Strong,
                    });
                    acc.nav.record(id, name_str, &qname, kind, Some(module_id));
                }
            }
            _ => {}
        }
    }

    // Second pass: functions, impl blocks, use statements.
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                visit_function(child, src, file_rel_path, module_qname, module_id, repo, &mut acc);
            }
            "impl_item" => {
                visit_impl(
                    child, src, file_rel_path, module_qname, module_id, repo, &type_ids, &mut acc,
                );
            }
            "const_item" | "static_item" => {
                visit_const_static(
                    child, src, file_rel_path, module_qname, module_id, repo, &mut acc,
                );
            }
            "use_declaration" => {
                collect_use(child, src, module_qname, &mut acc);
            }
            "attribute_item" => {
                visit_route_attr(child, src, file_rel_path, module_id, repo, &mut acc);
            }
            _ => {}
        }
    }

    scan_axum_routes(source, module_id, repo, &mut acc);
    scan_at_path_chains(source, repo, &mut acc);
    scan_salvo_routes(source, repo, &mut acc);

    if acc.client_endpoints > 0 {
        eprintln!(
            "[rust-http-client] {} endpoints in {}",
            acc.client_endpoints, file_rel_path
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
}

fn visit_function(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{module_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: module_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::FUNCTION, Some(module_id));

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
        let n = collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
        acc.client_endpoints += n;
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_impl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
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
    let parent_id = type_ids.get(base_name).copied().unwrap_or(module_id);

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
            let qname = format!("{module_qname}::{base_name}::{name}");
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
    module_qname: &str,
    module_id: NodeId,
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
    let qname = format!("{module_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: module_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::STATE_VAR, Some(module_id));
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

fn visit_route_attr(
    node: TsNode,
    src: &[u8],
    _file_rel: &str,
    module_id: NodeId,
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
                to: module_id,
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
        let win_end = (after + VERB_CHAIN_WINDOW).min(source.len());
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
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some(func) = n.child_by_field_name("function")
        {
            let qualifier = classify_call(func, src);
            acc.calls.push(CallSite { from, qualifier });
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
}
