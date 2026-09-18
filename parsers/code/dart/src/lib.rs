use std::collections::HashSet;

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
// A3.5: `is_http_client_receiver` / `ident_before` were defined here first and
// now live in code_domain::endpoint, shared with ts_routes' Express scan. An
// empty receiver (a `..get` cascade) is not a client, so it still falls
// through to ROUTE emission in `scan_dart_routes`.
use repo_graph_code_domain::endpoint::{
    ClientEndpoint, HitExtras, client_url_split, ident_before, is_http_client_receiver,
    normalise_client_path, push_client_endpoint_with,
};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
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

    visit_top(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    scan_dart_routes(source, repo, &mut acc);

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
}

fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_or_export" => collect_import(child, src, parent_qname, acc),
            "class_declaration" => {
                visit_class(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "enum_declaration" => {
                visit_enum(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "function_signature" | "function_definition" | "top_level_definition" => {
                visit_function(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            // G19 — library-level `const`/`final NAME = expr;`. The hidden
            // `_top_level_definition` rule inlines the keyword + this list as
            // direct children of the program root.
            "static_final_declaration_list" => {
                visit_top_level_consts(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            _ => {}
        }
    }
}

fn visit_class(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = find_identifier(node, src) else {
        return;
    };
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, &qname);

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
    acc.nav.record(id, &name, &qname, node_kind::CLASS, Some(parent_id));

    // G12.5 — heritage: `extends Y` → INHERITS_FROM (superclass);
    // `implements I` and `with M` → IMPLEMENTS (interface/mixin).
    visit_class_heritage(node, src, id, repo, acc);

    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() == "class_body" {
            let mut c2 = child.walk();
            for member in child.named_children(&mut c2) {
                if member.kind() == "class_member" {
                    visit_class_member(member, src, file_rel, &qname, id, repo, acc);
                }
            }
        }
    }
}

/// G12.5: class heritage. The `superclass` field holds `extends <type>` plus an
/// optional `with` mixin clause (or, in the mixin-only form, just `with`). The
/// `interfaces` field holds the `implements` clause.
///   - `extends Y`  → INHERITS_FROM (class → superclass)
///   - `with M`     → IMPLEMENTS    (class → mixin)
///   - `implements I` → IMPLEMENTS  (class → interface)
fn visit_class_heritage(node: TsNode, src: &[u8], id: NodeId, repo: RepoId, acc: &mut Acc) {
    if let Some(superclass) = node.child_by_field_name("superclass") {
        // `extends <type>` arrives via the `type` field; mixins (`with`) nest as
        // a `mixins` child holding one or more `_type_not_void` types.
        if let Some(sc_type) = superclass.child_by_field_name("type") {
            emit_heritage_ref(text_of(sc_type, src), edge_category::INHERITS_FROM, id, repo, acc);
        }
        let mut sc_cursor = superclass.walk();
        for child in superclass.named_children(&mut sc_cursor) {
            if child.kind() == "mixins" {
                emit_mixin_or_interface_refs(child, src, id, repo, acc);
            }
        }
    }
    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        emit_mixin_or_interface_refs(interfaces, src, id, repo, acc);
    }
}

/// Emit an IMPLEMENTS edge per type in a `mixins` (`with`) or `interfaces`
/// (`implements`) clause. The `with`/`implements` keywords are anonymous, so the
/// named children are the type nodes themselves.
fn emit_mixin_or_interface_refs(
    clause: TsNode,
    src: &[u8],
    id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = clause.walk();
    for ty in clause.named_children(&mut cursor) {
        // Each type head is a `type_identifier` (or function/record type). Skip
        // trailing `type_arguments` so generics don't spawn spurious edges.
        if ty.kind() == "type_arguments" {
            continue;
        }
        emit_heritage_ref(text_of(ty, src), edge_category::IMPLEMENTS, id, repo, acc);
    }
}

fn emit_heritage_ref(
    raw: &str,
    category: repo_graph_core::EdgeCategoryId,
    from_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // Strip generic args (`Comparable<Foo>` → `Comparable`) and take the trailing
    // simple name (`pkg.Base` → `Base`). Graph crate resolves the target node.
    let base = raw.split('<').next().unwrap_or(raw).trim();
    let simple = base.rsplit(['.', ':']).next().unwrap_or(base).trim();
    if simple.is_empty() {
        return;
    }
    let target = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, simple);
    acc.edges.push(Edge {
        from: from_id,
        to: target,
        category,
        confidence: Confidence::Weak,
    });
}

fn visit_class_member(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() == "method_signature"
            && let Some(name) = find_method_name(child, src)
        {
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
                .record(id, &name, &qname, node_kind::METHOD, Some(parent_id));
        }
        if child.kind() == "function_body"
            && let Some(method_id) = acc.nodes.last().map(|n| n.id)
        {
            collect_calls_in(child, src, method_id, repo, file_rel, acc);
        }
    }
}

fn visit_enum(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name").or_else(|| {
        let mut c = node.walk();
        node.named_children(&mut c).find(|ch| ch.kind() == "identifier")
    }) else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENUM, &qname);

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
        .record(id, name, &qname, node_kind::ENUM, Some(parent_id));
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
    let Some(name) = find_method_name(node, src) else {
        return;
    };
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
        .record(id, &name, &qname, node_kind::FUNCTION, Some(parent_id));
}

/// G19: library-level `const`/`final` constants. The list holds one
/// `static_final_declaration` per declarator (`name = value`). Emits a STATE_VAR
/// node + DEFINES edge module→const for each.
///
/// Noise gate: skip when undocumented AND the initializer is a primitive literal
/// (number / string / bool). Documented or non-trivial initializers are kept.
fn visit_top_level_consts(
    list: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // The leading `///` doc precedes the `const`/`final` keyword, which is a
    // prev-sibling of this list (the `_top_level_definition` rule is hidden).
    // Anchor doc detection at that keyword so `leading_doc` reaches the comment.
    let doc_anchor = const_keyword_sibling(list).unwrap_or(list);
    let doc = repo_graph_doc::leading_doc(&doc_anchor, src);
    let has_doc = doc.is_some();

    let mut cursor = list.walk();
    for decl in list.named_children(&mut cursor) {
        if decl.kind() != "static_final_declaration" {
            continue;
        }
        let Some(name_node) = decl.child_by_field_name("name") else {
            continue;
        };
        // Noise gate: undocumented + literal-primitive initializer → skip.
        if !has_doc
            && let Some(value) = decl.child_by_field_name("value")
            && is_primitive_literal(value.kind())
        {
            continue;
        }
        let name = text_of(name_node, src);
        let qname = format!("{parent_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);

        // entity_cells gives CODE + POSITION (+ DOC when leading_doc sees it from
        // the node itself). Top-level consts carry the doc above the keyword, so
        // splice in the doc we resolved from the keyword anchor when present.
        let mut cells = entity_cells(&decl, src, file_rel);
        if let Some(ref d) = doc
            && !cells.iter().any(|c| c.kind == cell_type::DOC)
        {
            cells.push(Cell {
                kind: cell_type::DOC,
                payload: CellPayload::Text(d.clone()),
            });
        }
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
        });
        acc.nav
            .record(id, name, &qname, node_kind::STATE_VAR, Some(parent_id));
    }
}

/// Walk prev-siblings of a top-level declaration list to the `const`/`final`/
/// `late` keyword token, used as the doc-comment anchor.
fn const_keyword_sibling(list: TsNode) -> Option<TsNode> {
    let mut prev = list.prev_sibling();
    let mut hops = 0u32;
    while let Some(n) = prev {
        hops += 1;
        if hops > 8 {
            break;
        }
        match n.kind() {
            "const" | "final" | "late" => return Some(n),
            // Skip an optional type annotation / `augment` marker between the
            // keyword and the list.
            _ => prev = n.prev_sibling(),
        }
    }
    None
}

/// True for Dart primitive/atom literal initializer node kinds.
fn is_primitive_literal(kind: &str) -> bool {
    matches!(
        kind,
        "decimal_integer_literal"
            | "hex_integer_literal"
            | "decimal_floating_point_literal"
            | "string_literal"
            | "true"
            | "false"
            | "null_literal"
    )
}

fn find_identifier<'a>(node: TsNode<'a>, src: &'a [u8]) -> Option<String> {
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() == "identifier" {
            return Some(text_of(child, src).to_string());
        }
    }
    None
}

fn find_method_name<'a>(node: TsNode<'a>, src: &'a [u8]) -> Option<String> {
    if let Some(name) = find_identifier(node, src) {
        return Some(name);
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() == "function_signature" {
            return find_identifier(child, src);
        }
    }
    None
}

fn collect_import(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    if !text.starts_with("import") {
        return;
    }
    let path = text
        .trim_start_matches("import ")
        .trim_end_matches(';')
        .trim()
        .trim_matches('\'')
        .trim_matches('"');
    acc.imports.push(ImportStmt {
        from_module: from_module.to_string(),
        target: ImportTarget::Module {
            path: path.to_string(),
            alias: None,
        },
    });
}

fn collect_calls_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        // Pattern A: client HTTP call (`dio.get('/x')`) → ENDPOINT node so the
        // HttpStackResolver can pair it with a server ROUTE.
        try_detect_dart_endpoint(n, src, from, repo, file_rel, acc);
        match n.kind() {
            "selector_expression" => {
                if let Some(field) = n.child_by_field_name("field") {
                    let target = n.named_child(0).map(|c| text_of(c, src)).unwrap_or("");
                    let method = text_of(field, src);
                    if target == "this" {
                        acc.calls.push(CallSite {
                            from,
                            qualifier: CallQualifier::SelfMethod(method.to_string()),
                        });
                    } else if n.named_child(0).is_some_and(|c| c.kind() == "identifier") {
                        acc.calls.push(CallSite {
                            from,
                            qualifier: CallQualifier::Attribute {
                                base: target.to_string(),
                                name: method.to_string(),
                            },
                        });
                    }
                }
            }
            "identifier" => {
                if n.parent().is_some_and(|p| {
                    p.kind() == "arguments" || p.kind() == "argument_part"
                }) {
                    // skip — arguments, not calls
                } else if n.parent().is_some_and(|p| {
                    p.kind() == "selector_expression"
                }) {
                    // handled above
                }
            }
            _ => {}
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "function_expression" | "class_definition" | "function_definition"
            ) {
                stack.push(child);
            }
        }
    }
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

const HTTP_VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

/// Pattern A: detect a client HTTP call `<recv>.<verb>('<path>', ...)` and emit a
/// shared ENDPOINT node. tree-sitter-dart shapes `dio.get('/x')` as a node whose
/// first named children are `identifier(receiver)`, `selector(.verb)`,
/// `selector((args))`. `<recv>` must name an HTTP client (dio / http / *client /
/// api) — server routes (`router.get`, shelf cascades) are handled by
/// `scan_dart_routes`, which skips these client receivers so no phantom ROUTE.
fn try_detect_dart_endpoint(
    n: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut c = n.walk();
    let kids: Vec<TsNode> = n.named_children(&mut c).collect();
    if kids.len() < 3 || kids[0].kind() != "identifier" {
        return;
    }
    if !is_http_client_receiver(text_of(kids[0], src)) {
        return;
    }
    if kids[1].kind() != "selector" || kids[2].kind() != "selector" {
        return;
    }
    // Verb is the first identifier under the `.verb` selector.
    let Some(method) = first_identifier_text(kids[1], src) else {
        return;
    };
    let method_l = method.to_ascii_lowercase();
    if !HTTP_VERBS.contains(&method_l.as_str()) {
        return;
    }
    // Path is the first string literal in the argument selector.
    let Some(sl) = first_descendant_of_kind(kids[2], "string_literal") else {
        return;
    };
    // A3.3: normalise BEFORE the guard, so `dio.post('https://api/users')`
    // becomes `/users` instead of being dropped as "not a path".
    let raw = dart_string_path(sl, src);
    let (path, changed) = normalise_client_path(&raw);
    if !is_dart_request_path(&path) {
        return; // not a request path (full-URL var, non-path first arg, etc.)
    }
    // A11.5: the authority of the pre-normalisation literal, as `host`. Only
    // the host half is read: the path stays A3.3's normaliser's, so no qname
    // moves. `$base/users` has no authority; `https://$host/x` has a
    // placeholder one, which names no service.
    let (host, _) = client_url_split(&raw);
    let pos = n.start_position();
    let ep = ClientEndpoint {
        method: method_l.to_ascii_uppercase(),
        path,
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: Confidence::Strong,
    };
    let extras = HitExtras {
        raw: changed.then_some(raw.as_str()),
        host: host.as_deref(),
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

/// A normalised client path worth an ENDPOINT: absolute (`/users`), or an
/// interpolated base followed by a path (`$baseUrl/users` → `${…}/users`),
/// which the HTTP resolver's BaseFold tier pairs.
///
/// A path that is ONLY an interpolation (`'$url'` → `${…}`) is still rejected:
/// it names a whole URL held in a variable, and it would normalise to `/{}`,
/// which BaseFold folds to the root `/` — a guaranteed false pairing.
fn is_dart_request_path(path: &str) -> bool {
    path.starts_with('/') || path.starts_with("${…}/")
}

/// First descendant `identifier` text, depth-first.
fn first_identifier_text(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(text_of(node, src).to_string());
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if let Some(t) = first_identifier_text(child, src) {
            return Some(t);
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

/// Reconstruct a Dart string-literal path, replacing every `$id` / `${expr}`
/// interpolation with `${…}` so it normalises like a TS template path
/// (`normalise_http_path` collapses any segment containing `${` to `{}`).
fn dart_string_path(string_literal: TsNode, src: &[u8]) -> String {
    fn rec(n: TsNode, src: &[u8], out: &mut String) {
        let mut c = n.walk();
        for child in n.named_children(&mut c) {
            let k = child.kind();
            if k == "template_substitution" {
                out.push_str("${…}");
            } else if k.starts_with("template_chars") {
                out.push_str(text_of(child, src));
            } else {
                rec(child, src, out);
            }
        }
    }
    let mut out = String::new();
    rec(string_literal, src, &mut out);
    if out.is_empty() {
        out = text_of(string_literal, src)
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string();
    }
    out
}

// ============================================================================
// Dart route extraction (v0.4.11a R-dart)
// ============================================================================
//
// Two framework surfaces covered via text scan (robust to tree-sitter-dart's
// no-field-name quirk):
//
//   go_router navigation:  GoRoute(path: '/users', ...)  → page:/users (ANY)
//   shelf / shelf_router:  router.get('/users', handler) → GET /users
//                          ..post('/x', h)  (cascade)    → POST /x
//
// Shape B ROUTE nodes (METHOD <path> qname + Text ROUTE_METHOD cell) for the
// server routes; go_router pages take the `page:<path>` qname (LB.4c).

fn scan_dart_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Track emitted routes to dedup — a file may hit the same path twice
    // between the tree walk and the text scan.
    let mut seen = std::collections::HashSet::new();

    // go_router — look for `GoRoute(path:` token.
    let needle = "GoRoute(";
    let mut idx = 0;
    while let Some(pos) = source[idx..].find(needle) {
        let start = idx + pos + needle.len();
        if let Some(path) = extract_kwarg_string(&source[start..], "path") {
            // A3.4: go_router is BROWSER/app navigation, not a server
            // endpoint — mark it so the HTTP route index skips it.
            emit_dart_route("ANY", &path, repo, acc, &mut seen, true);
        }
        idx = start;
    }

    // shelf-style `.get('/...' / .post('/...' / etc. — SERVER routes only.
    // A client HTTP call (`dio.get('/x')`) has the same textual shape but is an
    // outbound ENDPOINT, handled by `try_detect_dart_endpoint`; skip it here (by
    // receiver name) so it is not mis-emitted as a phantom server ROUTE.
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let needle = format!(".{method}(");
        let mut idx = 0;
        while let Some(pos) = source[idx..].find(&needle) {
            let dot_at = idx + pos;
            let after = &source[dot_at + needle.len()..];
            if let Some(path) = first_string_literal_dart(after)
                && path.starts_with('/')
                && !is_http_client_receiver(ident_before(source, dot_at))
            {
                let verb = method.to_ascii_uppercase();
                emit_dart_route(&verb, &path, repo, acc, &mut seen, false);
            }
            idx = dot_at + needle.len();
        }
    }
}

fn extract_kwarg_string(s: &str, key: &str) -> Option<String> {
    // Looks for `path: '/x'` or `path: "/x"` allowing whitespace.
    let mut i = 0;
    while let Some(pos) = s[i..].find(key) {
        let start = i + pos + key.len();
        let rest = s[start..].trim_start();
        if let Some(after_colon) = rest.strip_prefix(':')
            && let Some(lit) = first_string_literal_dart(after_colon)
        {
            return Some(lit);
        }
        i = start;
    }
    None
}

fn first_string_literal_dart(s: &str) -> Option<String> {
    let trimmed = s.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let quote = match bytes[0] {
        b'\'' | b'"' => bytes[0],
        _ => return None,
    };
    let rest = &trimmed[1..];
    let end = rest.find(quote as char)?;
    let lit = &rest[..end];
    if lit.is_empty() || lit.len() > 256 {
        return None;
    }
    Some(lit.to_string())
}

/// Emit one ROUTE node. `nav` marks a *browser/app navigation* target
/// (go_router) as opposed to a real server route (shelf): A3.4 adds an ORIGIN
/// `provenance: nav_route` cell so the graph crate's HTTP route index skips it
/// and a same-app `dio.get('/users')` cannot pair to the app's own navigation
/// table. The node itself survives — only the pairing is suppressed.
///
/// LB.4c: a nav page lives in its own qname namespace — qname `page:<path>`,
/// display name `<path>` — so it never shares a NodeId with a server route on
/// the same path (`<METHOD> <path>`). ROUTE_METHOD stays the passed method.
///
/// Dart has no stats channel out of `parse_file`, so nav routes marked here are
/// not in the engine's `[extract] nav-routes marked: N` count; they ARE in that
/// line's `(page-qnamed P)` figure, which the engine counts off the parses'
/// `page:` qnames. The graph-side `[http] nav-routes excluded from route index`
/// marker covers them too.
fn emit_dart_route(
    method: &str,
    path: &str,
    repo: RepoId,
    acc: &mut Acc,
    seen: &mut std::collections::HashSet<(String, String)>,
    nav: bool,
) {
    let route_qname = if nav {
        format!("page:{path}")
    } else {
        format!("{method} {path}")
    };
    let display_name = if nav { path } else { route_qname.as_str() };
    let key = (method.to_string(), route_qname.clone());
    if !seen.insert(key) {
        return;
    }
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_qname);
    let mut cells = vec![Cell {
        kind: cell_type::ROUTE_METHOD,
        payload: CellPayload::Text(method.to_string()),
    }];
    if nav {
        cells.push(Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(r#"{"provenance":"nav_route"}"#.to_string()),
        });
    }
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Medium,
        cells,
    });
    acc.nav
        .record(route_id, display_name, &route_qname, node_kind::ROUTE, None);
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
    fn class_and_enum() {
        let source = r#"
class User {
  String name;
  void greet() {
    print('Hello $name');
  }
}

enum Status { active, inactive }
"#;
        let fp = parse_file(source, "lib/user.dart", "lib::user", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::ENUM).count(), 1);
    }

    #[test]
    fn imports() {
        let source = r#"
import 'package:flutter/material.dart';
import 'dart:async';
"#;
        let fp = parse_file(source, "lib/main.dart", "lib::main", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
    }

    #[test]
    fn top_level_function() {
        let source = r#"
void main() {
  runApp(MyApp());
}
"#;
        let fp = parse_file(source, "lib/main.dart", "lib::main", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 1);
    }

    #[test]
    fn heritage_implements_and_extends() {
        let source = r#"
class IFoo {}
class Base {}
class Mix {}
class X extends Base with Mix implements IFoo {
  void run() {}
}
"#;
        let fp = parse_file(source, "lib/x.dart", "lib::x", repo()).unwrap();
        let x_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::x::X");
        let base = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "Base");
        let mix = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "Mix");
        let ifoo = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "IFoo");
        // extends → INHERITS_FROM
        assert!(fp.edges.iter().any(|e| e.from == x_id
            && e.to == base
            && e.category == edge_category::INHERITS_FROM));
        // with → IMPLEMENTS
        assert!(fp.edges.iter().any(|e| e.from == x_id
            && e.to == mix
            && e.category == edge_category::IMPLEMENTS));
        // implements → IMPLEMENTS
        assert!(fp.edges.iter().any(|e| e.from == x_id
            && e.to == ifoo
            && e.category == edge_category::IMPLEMENTS));
    }

    #[test]
    fn implements_edge_only() {
        let source = "class IFoo {}\nclass X implements IFoo {}\n";
        let fp = parse_file(source, "lib/x.dart", "lib::x", repo()).unwrap();
        let x_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::x::X");
        let ifoo = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "IFoo");
        assert!(fp.edges.iter().any(|e| e.from == x_id
            && e.to == ifoo
            && e.category == edge_category::IMPLEMENTS));
    }

    #[test]
    fn library_const_with_doc_emits_state_var() {
        let source = "/// Fee.\nconst feeBps = 250;\n";
        let fp = parse_file(source, "lib/cfg.dart", "lib::cfg", repo()).unwrap();
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "lib::cfg::feeBps");
        // STATE_VAR node emitted (documented primitive survives the noise gate).
        let node = fp.nodes.iter().find(|n| n.id == id).expect("feeBps STATE_VAR node");
        assert_eq!(
            *fp.nav.kind_by_id.get(&id).unwrap(),
            node_kind::STATE_VAR
        );
        // Doc cell carried through.
        assert!(node.cells.iter().any(|c| c.kind == cell_type::DOC));
        // DEFINES edge module→const.
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::cfg");
        assert!(fp.edges.iter().any(|e| e.from == module_id
            && e.to == id
            && e.category == edge_category::DEFINES));
    }

    #[test]
    fn undocumented_primitive_const_is_gated() {
        let source = "const k = 1;\n";
        let fp = parse_file(source, "lib/cfg.dart", "lib::cfg", repo()).unwrap();
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "lib::cfg::k");
        assert!(!fp.nodes.iter().any(|n| n.id == id));
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
    fn go_router_routes_emit() {
        let source = r#"
final router = GoRouter(routes: [
  GoRoute(path: '/users', builder: (c, s) => UsersScreen()),
  GoRoute(path: '/users/:id', builder: (c, s) => UserDetail()),
]);
"#;
        let fp = parse_file(source, "lib/router.dart", "lib::router", repo()).unwrap();
        // LB.4c: go_router pages are `page:<path>`, display name the bare path,
        // never the `ANY <path>` shape a server route on the same path takes.
        for path in ["/users", "/users/:id"] {
            let id = page_id(path);
            let node = fp.nodes.iter().find(|n| n.id == id).expect("page node");
            assert_eq!(
                fp.nav.qname_by_id.get(&id).map(String::as_str),
                Some(format!("page:{path}").as_str())
            );
            assert_eq!(fp.nav.name_by_id.get(&id).map(String::as_str), Some(path));
            assert_eq!(fp.nav.kind_by_id.get(&id), Some(&node_kind::ROUTE));
            // ROUTE_METHOD and the A3.4 ORIGIN mark are unchanged.
            assert!(node.cells.iter().any(|c| c.kind == cell_type::ROUTE_METHOD
                && matches!(&c.payload, CellPayload::Text(m) if m == "ANY")));
            assert!(node.cells.iter().any(|c| c.kind == cell_type::ORIGIN
                && matches!(&c.payload, CellPayload::Json(j) if j.contains("\"provenance\":\"nav_route\""))));
            assert!(!fp.nodes.iter().any(|n| n.id == route_id("ANY", path)));
        }
    }

    fn page_id(path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            &format!("page:{path}"),
        )
    }

    #[test]
    fn go_router_page_and_shelf_route_on_one_path_are_two_nodes() {
        // LB.4c: the page and the server route serving `/users` in one file
        // keep distinct identities; only the server route keeps `<METHOD> <path>`
        // and only the page carries the nav_route ORIGIN mark.
        let source = r#"
final router = GoRouter(routes: [
  GoRoute(path: '/users', builder: (c, s) => UsersScreen()),
]);
final app = Router()
  ..get('/users', handleList);
"#;
        let fp = parse_file(source, "lib/app.dart", "lib::app", repo()).unwrap();
        let page = page_id("/users");
        let server = route_id("GET", "/users");
        assert_ne!(page, server);
        let page_node = fp.nodes.iter().find(|n| n.id == page).expect("page node");
        let server_node = fp
            .nodes
            .iter()
            .find(|n| n.id == server)
            .expect("server route");
        assert!(page_node.cells.iter().any(|c| c.kind == cell_type::ORIGIN));
        assert!(
            !server_node
                .cells
                .iter()
                .any(|c| c.kind == cell_type::ORIGIN)
        );
        assert_eq!(
            fp.nav.name_by_id.get(&server).map(String::as_str),
            Some("GET /users")
        );
        assert_eq!(
            fp.nav.qname_by_id.get(&server).map(String::as_str),
            Some("GET /users")
        );
    }

    #[test]
    fn shelf_routes_emit() {
        let source = r#"
import 'package:shelf_router/shelf_router.dart';

final app = Router()
  ..get('/users', handleList)
  ..post('/users', handleCreate);
"#;
        let fp = parse_file(source, "bin/server.dart", "bin::server", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
    }

    #[test]
    fn dio_client_call_emits_endpoint_not_route() {
        // Pattern A: `dio.get('/users/$id')` / `dio.post('/users')` in a class
        // method → ENDPOINT nodes (not phantom ROUTEs), with a CALLS edge from
        // the enclosing method. Path interpolation → `${…}`.
        let source = r#"class ApiClient {
  final Dio dio;
  Future<void> fetchUser(String id) async {
    final res = await dio.get('/users/$id');
    await dio.post('/users', data: body);
  }
}
"#;
        let fp = parse_file(source, "lib/api.dart", "lib::api", repo()).unwrap();
        let ep_get = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/users/${…}");
        let ep_post = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:POST:/users");
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
            !fp.nodes.iter().any(|n| n.id == route_id("GET", "/users/$id")),
            "client dio.get must not become a server ROUTE"
        );
        // CALLS edge from the enclosing method to each endpoint.
        assert!(
            fp.edges
                .iter()
                .any(|e| e.to == ep_get && e.category == edge_category::CALLS),
            "expected CALLS edge into the GET endpoint"
        );
    }

    #[test]
    fn shelf_router_still_emits_route_not_endpoint() {
        // Server routes must be unaffected by the client-endpoint change.
        let source = r#"
final app = Router()
  ..get('/things', handleList)
  ..post('/things', handleCreate);
"#;
        let fp = parse_file(source, "bin/server.dart", "bin::server", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/things")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/things")));
    }

    fn endpoint_hit(fp: &FileParse, id: NodeId) -> Option<String> {
        fp.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .find(|c| c.kind == cell_type::ENDPOINT_HIT)
            .and_then(|c| match &c.payload {
                CellPayload::Json(j) => Some(j.clone()),
                _ => None,
            })
    }

    /// A3.3 — an absolute-URL dio call used to be DROPPED (the path did not
    /// start with `/`). It now normalises to its request path, keeps the
    /// original literal as `raw`, and gets its CALLS edge; an interpolated base
    /// survives for BaseFold, while a whole-URL variable is still rejected.
    #[test]
    fn absolute_url_dio_call_emits_endpoint() {
        let source = r#"import 'package:dio/dio.dart';
class ApiClient {
  final Dio dio = Dio();
  Future<void> createUser(Map body) async {
    await dio.post('https://api.example.com/users', data: body);
    await dio.get('$baseUrl/users/$id?expand=1');
    await dio.get('$url');
    await dio.delete('/users');
  }
}
"#;
        let fp = parse_file(source, "lib/api_client.dart", "lib::api_client", repo()).unwrap();
        let ep = |m: &str, p: &str| {
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, &format!("endpoint:{m}:{p}"))
        };

        let post = ep("POST", "/users");
        let hit = endpoint_hit(&fp, post).expect("POST /users ENDPOINT from an absolute URL");
        let v: serde_json::Value = serde_json::from_str(&hit).unwrap();
        assert_eq!(v["path"], "/users");
        assert_eq!(v["raw"], "https://api.example.com/users");
        assert_eq!(v["host"], "api.example.com");
        let method = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "lib::api_client::ApiClient::createUser",
        );
        assert!(
            fp.edges
                .iter()
                .any(|e| e.from == method && e.to == post && e.category == edge_category::CALLS),
            "expected CALLS createUser -> POST /users"
        );
        assert!(
            !fp.nodes
                .iter()
                .any(|n| n.id == ep("POST", "https://api.example.com/users")),
            "the full URL must not become the endpoint qname"
        );

        // Interpolated base + query: base kept, query dropped, raw recorded.
        let based = endpoint_hit(&fp, ep("GET", "${…}/users/${…}"))
            .expect("interpolated-base endpoint must survive for BaseFold");
        let v: serde_json::Value = serde_json::from_str(&based).unwrap();
        assert_eq!(v["raw"], "${…}/users/${…}?expand=1");

        // A whole-URL variable is not a request path.
        assert!(!fp.nodes.iter().any(|n| n.id == ep("GET", "${…}")));

        // An unchanged path carries no `raw`.
        let plain = endpoint_hit(&fp, ep("DELETE", "/users")).expect("DELETE /users");
        assert!(!plain.contains("\"raw\""), "unchanged path must not carry raw: {plain}");
        // A11.5: nor a `host`; the interpolated base has none either.
        assert!(
            !plain.contains("\"host\""),
            "relative path must not carry host: {plain}"
        );
        assert!(
            !based.contains("\"host\""),
            "interpolated base has no host: {based}"
        );

        // And no phantom server ROUTE for any of these client calls.
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("DELETE", "/users")));
    }

    /// A11.5 — the dio literal's authority lands on ENDPOINT_HIT as `host`,
    /// AFTER `raw` (where the engine fold used to append it, so the folded
    /// bytes do not move); an interpolated authority (`https://$host/x`)
    /// names no service, so it writes `raw` but no `host`.
    #[test]
    fn dio_endpoint_carries_the_url_authority_as_host() {
        let source = r#"import 'package:dio/dio.dart';
class ApiClient {
  final Dio dio = Dio();
  Future<void> run() async {
    await dio.get('http://svc:8080/users?page=2');
    await dio.get('https://$host/orders');
  }
}
"#;
        let fp = parse_file(source, "lib/api_client.dart", "lib::api_client", repo()).unwrap();
        let ep = |m: &str, p: &str| {
            NodeId::from_parts(
                GRAPH_TYPE,
                repo(),
                node_kind::ENDPOINT,
                &format!("endpoint:{m}:{p}"),
            )
        };
        let users = endpoint_hit(&fp, ep("GET", "/users")).expect("GET /users");
        assert!(
            users.ends_with(r#","raw":"http://svc:8080/users?page=2","host":"svc:8080"}"#),
            "{users}"
        );
        let orders = endpoint_hit(&fp, ep("GET", "/orders")).expect("GET /orders");
        assert!(
            orders.contains(r#""raw":"https://${…}/orders""#),
            "{orders}"
        );
        assert!(!orders.contains("\"host\""), "{orders}");
    }
}
