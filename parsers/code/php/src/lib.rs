use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use std::collections::HashMap;
use std::sync::OnceLock;
use tree_sitter::{Node as TsNode, Parser};

use repo_graph_code_domain::endpoint::{self, ClientEndpoint, push_client_endpoint};
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
    let lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
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
    acc.nav.record(
        module_id,
        module_simple,
        module_qname,
        node_kind::MODULE,
        None,
    );

    visit_children(
        root,
        src,
        file_rel_path,
        module_qname,
        module_id,
        repo,
        &mut acc,
    );

    scan_laravel_routes(source, file_rel_path, module_id, repo, &mut acc);
    scan_slim_routes(source, module_id, repo, &mut acc);

    if !acc.endpoint_seen.is_empty() {
        eprintln!(
            "[php-http-client] {} endpoints in {file_rel_path}",
            acc.endpoint_seen.len()
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
    /// ENDPOINT ids already minted in THIS file — `push_client_endpoint` dedups
    /// the node through it while still pushing one CALLS edge per call site.
    endpoint_seen: std::collections::HashSet<NodeId>,
}

fn visit_children(
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
            "namespace_definition" => {
                visit_namespace(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "class_declaration" => {
                visit_class(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "interface_declaration" => {
                visit_interface(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "enum_declaration" => {
                visit_enum(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "function_definition" => {
                visit_function(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "namespace_use_declaration" => collect_use(child, src, parent_qname, acc),
            _ => {}
        }
    }
}

fn visit_namespace(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    _parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = name.replace('\\', "::");
    let ns_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE, &qname);
    let simple = qname.rsplit("::").next().unwrap_or(&qname);

    acc.nodes.push(Node {
        id: ns_id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: ns_id,
        category: edge_category::CONTAINS,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(ns_id, simple, &qname, node_kind::PACKAGE, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        visit_children(body, src, file_rel, &qname, ns_id, repo, acc);
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
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
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
    acc.nav
        .record(id, name, &qname, node_kind::CLASS, Some(parent_id));

    // Symfony composes a controller's class-level `#[Route('/prefix')]` onto
    // every action template, so the prefix must be known before the body walk.
    let class_prefix = class_route_prefix(node, src);
    let mut composed = 0usize;
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "method_declaration" {
                composed +=
                    visit_method(child, src, file_rel, &qname, id, repo, &class_prefix, acc);
            }
        }
    }

    // The controller's OWN `#[Route]` still becomes a route node (unchanged
    // behaviour); it is the prefix itself, so it composes against nothing.
    check_route_attrs(node, src, id, repo, "", acc);

    if composed > 0 && !class_prefix.is_empty() {
        eprintln!(
            "[php-routes] composed {composed} attribute routes under '{class_prefix}' in {file_rel}"
        );
    }
}

fn visit_interface(
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
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INTERFACE, &qname);

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
        .record(id, name, &qname, node_kind::INTERFACE, Some(parent_id));
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
    let Some(name_node) = node.child_by_field_name("name") else {
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

    let types = local_receiver_types(node, src);
    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc, &types);
        collect_client_endpoints_in(body, src, id, repo, file_rel, &types, acc);
    }
}

/// Returns how many `#[Route]` attribute routes this method contributed, so
/// `visit_class` can report the composition in its `[php-routes]` marker.
fn visit_method(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    class_prefix: &str,
    acc: &mut Acc,
) -> usize {
    let Some(name_node) = node.child_by_field_name("name") else {
        return 0;
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

    let types = local_receiver_types(node, src);
    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc, &types);
        collect_client_endpoints_in(body, src, id, repo, file_rel, &types, acc);
    }

    check_route_attrs(node, src, id, repo, class_prefix, acc)
}

/// The PHP attributes declared directly ON `node` — its `attributes` field,
/// never the ones nested inside its body. `class_declaration` and
/// `method_declaration` both expose `attributes: attribute_list`, which holds
/// `attribute_group`s of `attribute`s (tree-sitter-php 0.24 node-types).
///
/// Returns `(simple attribute name, attribute node)`: a leading namespace
/// qualifier (`\App\Route`, `\Symfony\...\Route`) is stripped, and the node
/// is handed back so the string/kwarg helpers run over THAT attribute's text
/// only instead of the whole declaration.
fn own_attributes<'a>(node: TsNode<'a>, src: &'a [u8]) -> Vec<(String, TsNode<'a>)> {
    let mut out = Vec::new();
    let Some(list) = node.child_by_field_name("attributes") else {
        return out;
    };
    let mut list_cursor = list.walk();
    for group in list.named_children(&mut list_cursor) {
        if group.kind() != "attribute_group" {
            continue;
        }
        let mut group_cursor = group.walk();
        for attr in group.named_children(&mut group_cursor) {
            if attr.kind() != "attribute" {
                continue;
            }
            let mut attr_cursor = attr.walk();
            let name_node = attr
                .named_children(&mut attr_cursor)
                .find(|c| matches!(c.kind(), "name" | "qualified_name" | "relative_name"));
            let Some(name_node) = name_node else {
                continue;
            };
            let raw = text_of(name_node, src);
            let simple = raw.rsplit('\\').next().unwrap_or(raw).trim();
            out.push((simple.to_string(), attr));
        }
    }
    out
}

/// A controller's class-level `#[Route('/prefix')]` template, or `""` when the
/// class carries no `Route` attribute. Symfony prepends this to every action
/// template in the class.
fn class_route_prefix(class_node: TsNode, src: &[u8]) -> String {
    for (name, attr) in own_attributes(class_node, src) {
        if name != "Route" {
            continue;
        }
        if let Some((path, _)) = extract_first_string(text_of(attr, src)) {
            return path;
        }
    }
    String::new()
}

/// Emit the route(s) declared by the `#[Route(...)]` attributes ON `node`
/// itself, composed under `class_prefix` (empty at class level and for a
/// prefix-less controller, where composition is a pass-through).
///
/// Reads the `attributes` FIELD rather than text-scanning the declaration:
/// scanning a `class_declaration` re-found every method attribute in the body
/// and re-emitted each action route against the CLASS as handler.
///
/// Returns the number of `#[Route]` attributes that produced a route.
fn check_route_attrs(
    node: TsNode,
    src: &[u8],
    handler_id: NodeId,
    repo: RepoId,
    class_prefix: &str,
    acc: &mut Acc,
) -> usize {
    let mut emitted = 0usize;
    for (name, attr) in own_attributes(node, src) {
        if name != "Route" {
            continue;
        }
        // Scoped to this attribute, so a `methods:` kwarg can never be read off
        // the NEXT attribute the way the old `find_attr_end` fallback could.
        let attr_text = text_of(attr, src);
        let Some((path, _)) = extract_first_string(attr_text) else {
            continue;
        };
        // Symfony's rule is prefix-always: `#[Route('/{id}')]` under a class
        // `#[Route('/api/v1/users')]` is `/api/v1/users/{id}`, and an EMPTY
        // action template is the prefix itself.
        let full = if path.is_empty() {
            endpoint::abs_path(class_prefix)
        } else {
            endpoint::abs_path(&endpoint::join_path(class_prefix, &path))
        };
        let methods = parse_methods_kwarg(attr_text);
        if methods.is_empty() {
            emit_route_strong("ANY", &full, handler_id, repo, acc);
        } else {
            for m in methods {
                emit_route_strong(&m, &full, handler_id, repo, acc);
            }
        }
        emitted += 1;
    }
    emitted
}

fn extract_first_string(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' {
            let delim = c;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != delim {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                return Some((s[start..j].to_string(), j + 1));
            }
            return None;
        }
        i += 1;
    }
    None
}

fn parse_methods_kwarg(attr_text: &str) -> Vec<String> {
    let Some(pos) = attr_text.find("methods:") else {
        return Vec::new();
    };
    let after = &attr_text[pos + 8..];
    let Some(open) = after.find('[') else {
        return Vec::new();
    };
    let Some(close) = after[open..].find(']') else {
        return Vec::new();
    };
    let inner = &after[open + 1..open + close];
    let mut out = Vec::new();
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' {
            let delim = c;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != delim {
                j += 1;
            }
            if j < bytes.len() {
                let m = inner[start..j].to_ascii_uppercase();
                if !m.is_empty() {
                    out.push(m);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

// ============================================================================
// Laravel route-group prefix composition
// ============================================================================
//
// `Route::prefix('api/v1')->group(function () { Route::get('/users', ...); })`
// declares the shared path segment on the GROUP, not on the verb call, so a
// scanner that reads only the verb's first string argument emits `GET /users`
// and drops `/api/v1` entirely. Grouped routes are the default organisation of
// every non-toy Laravel API, so that is most of the Laravel server surface.
//
// The Laravel scanner is byte-offset substring-driven rather than an AST walk,
// so the composition state is expressed as byte RANGES rather than the scope
// STACK the Phoenix/Rails walkers use: a pre-pass records `(open_brace,
// close_brace, prefix)` for every group body, and each verb's already-computed
// path offset is then looked up against those ranges. Nested groups fall out
// for free — ranges nest, and `prefix_at` folds every containing range
// outermost-first.
//
// NOT covered: heredoc/nowdoc bodies (`<<<EOT ... EOT;`) are not recognised by
// `matching_brace`, so a `{` inside one shifts the range. No fixture exercises
// that and PHP route files do not idiomatically contain heredocs.

/// A group body as `(open_brace_index, close_brace_index, prefix_literal)`.
/// `close_brace_index` is the index OF the `}`, so the containment test is a
/// strict `open < offset < close`.
type PrefixRange = (usize, usize, String);

/// Index just past the string literal whose opening delimiter is at `start`.
/// Returns `bytes.len()` for an unterminated literal, which terminates the
/// caller's scan rather than looping.
fn skip_php_string(bytes: &[u8], start: usize) -> usize {
    let delim = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == delim {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// Index just past the end of the line containing `start`.
fn skip_php_line(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    i + 1
}

/// Index of the `}` matching the `{` at `open`, counting braces but SKIPPING
/// those inside single/double-quoted strings, `//` and `#` line comments and
/// `/* */` block comments. `None` when the source is unbalanced (truncated
/// file, or a brace hidden in a construct this scanner does not model) —
/// callers must then drop the group rather than defaulting to end-of-file,
/// which would sweep every later route into it.
fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' => {
                i = skip_php_string(bytes, i);
                continue;
            }
            // `#[Attr]` is a PHP 8 attribute, not a comment.
            b'#' if bytes.get(i + 1) != Some(&b'[') => {
                i = skip_php_line(bytes, i);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = skip_php_line(bytes, i);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = match source[i + 2..].find("*/") {
                    Some(rel) => i + 2 + rel + 2,
                    None => return None,
                };
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Range of the closure body that follows `from`, for the `…, function () { … }`
/// tail shared by both group spellings.
///
/// Bounded deliberately: a bare `Route::prefix('x')` assigned to a variable has
/// no closure at all, and an unbounded `find("function")` would attach the NEXT
/// group's body to it. The scan stops at the first `;` (statement end) or `}`
/// (enclosing block end) seen before the `function` keyword, and yields `None`.
fn closure_body_range(source: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b';' | b'}' => return None,
            b'\'' | b'"' => {
                i = skip_php_string(bytes, i);
                continue;
            }
            b'f' if source[i..].starts_with("function") => {
                let open = i + source[i..].find('{')?;
                let close = matching_brace(source, open)?;
                return Some((open, close));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Byte ranges of every Laravel route-group body, paired with the path prefix
/// that group contributes. Two spellings are recognised:
///
///   a) `Route::prefix('api/v1')->group(function () { … })`, and the chained
///      builder `Route::middleware('auth')->prefix('api')->group(…)`,
///   b) `Route::group(['prefix' => 'admin', …], function () { … })`.
///
/// A `Route::group([...])` with no `prefix` key (middleware-only grouping)
/// contributes no range: it changes nothing about the path.
fn laravel_prefix_ranges(source: &str) -> Vec<PrefixRange> {
    let mut out: Vec<PrefixRange> = Vec::new();

    // (a) chained builder. `Route::prefix(` and `->prefix(` are disjoint
    // needles — neither text contains the other.
    for needle in ["Route::prefix(", "->prefix("] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let start = search_from + rel + needle.len();
            search_from = start;
            let Some((prefix, consumed)) = extract_first_string(&source[start..]) else {
                continue;
            };
            search_from = start + consumed.max(1);
            if prefix.is_empty() {
                continue;
            }
            if let Some((open, close)) = closure_body_range(source, start + consumed) {
                out.push((open, close, prefix));
            }
        }
    }

    // (b) array-options form.
    let needle = "Route::group(";
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find(needle) {
        let start = search_from + rel + needle.len();
        search_from = start;
        let rest = source[start..].trim_start();
        let skipped = source[start..].len() - rest.len();
        let Some((inner, consumed)) = extract_bracket_list(rest) else {
            continue;
        };
        let after = start + skipped + consumed;
        search_from = after;
        let Some(prefix) = array_option(&inner, "prefix") else {
            continue;
        };
        if prefix.is_empty() {
            continue;
        }
        if let Some((open, close)) = closure_body_range(source, after) {
            out.push((open, close, prefix));
        }
    }

    out
}

/// Value of `'<key>' => '<string>'` inside a PHP array literal's inner text.
fn array_option(inner: &str, key: &str) -> Option<String> {
    for quote in ['\'', '"'] {
        let needle = format!("{quote}{key}{quote}");
        if let Some(pos) = inner.find(&needle) {
            let after = &inner[pos + needle.len()..];
            let after = after.trim_start();
            let after = after.strip_prefix("=>")?;
            let (val, _) = extract_first_string(after)?;
            return Some(val);
        }
    }
    None
}

/// The composed prefix contributed by every group whose body contains
/// `offset`, outermost first. `""` when the offset sits outside every group.
fn prefix_at(ranges: &[PrefixRange], offset: usize) -> String {
    let mut hits: Vec<&PrefixRange> = ranges
        .iter()
        .filter(|(open, close, _)| offset > *open && offset < *close)
        .collect();
    hits.sort_by_key(|(open, _, _)| *open);
    let mut out = String::new();
    for (_, _, prefix) in hits {
        out = endpoint::join_path(&out, prefix);
    }
    out
}

fn scan_laravel_routes(
    source: &str,
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let ranges = laravel_prefix_ranges(source);
    let mut grouped = 0usize;
    let methods: &[(&str, &str)] = &[
        ("Route::get(", "GET"),
        ("Route::post(", "POST"),
        ("Route::put(", "PUT"),
        ("Route::patch(", "PATCH"),
        ("Route::delete(", "DELETE"),
        ("Route::options(", "OPTIONS"),
        ("Route::any(", "ANY"),
    ];
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let start = search_from + rel + needle.len();
            if let Some((path, consumed)) = extract_first_string(&source[start..]) {
                let prefix = prefix_at(&ranges, start);
                if !prefix.is_empty() {
                    grouped += 1;
                }
                let full = endpoint::abs_path(&endpoint::join_path(&prefix, &path));
                let route_id = emit_route_medium(method, &full, module_id, repo, acc);
                // Bind the route to its controller action via a HANDLED_BY ref;
                // the graph resolves the Attribute base+name against the class.
                if let Some(qualifier) = extract_laravel_handler(&source[start + consumed..]) {
                    acc.refs.push(UnresolvedRef {
                        from: route_id,
                        from_module: module_id,
                        qualifier,
                        category: edge_category::HANDLED_BY,
                    });
                }
                search_from = start + consumed.max(1);
            } else {
                search_from = start;
            }
        }
    }
    // Route::resource('/users', UserController::class) → REST 7
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("Route::resource(") {
        let start = search_from + rel + "Route::resource(".len();
        if let Some((path, consumed)) = extract_first_string(&source[start..]) {
            let prefix = prefix_at(&ranges, start);
            let full = endpoint::abs_path(&endpoint::join_path(&prefix, &path));
            for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                if !prefix.is_empty() {
                    grouped += 1;
                }
                emit_route_medium(m, &full, module_id, repo, acc);
            }
            search_from = start + consumed.max(1);
        } else {
            search_from = start;
        }
    }
    // Route::apiResource → REST 5 (no create/edit)
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("Route::apiResource(") {
        let start = search_from + rel + "Route::apiResource(".len();
        if let Some((path, consumed)) = extract_first_string(&source[start..]) {
            let prefix = prefix_at(&ranges, start);
            let full = endpoint::abs_path(&endpoint::join_path(&prefix, &path));
            for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                if !prefix.is_empty() {
                    grouped += 1;
                }
                emit_route_medium(m, &full, module_id, repo, acc);
            }
            search_from = start + consumed.max(1);
        } else {
            search_from = start;
        }
    }

    if grouped > 0 {
        eprintln!(
            "[php-routes] laravel {grouped} routes under {} prefix groups in {file_rel}",
            ranges.len()
        );
    }
}

// ============================================================================
// Slim route extraction
// ============================================================================
//
// Slim 4 idiom: `$app->get('/path', $handler)`, with verbs `get` / `post` /
// `put` / `patch` / `delete` / `options` / `any`. Plus `$app->map(['GET', ...],
// '/path', $h)` for multi-method routes.
//
// Substring-driven like the Laravel scanner. The `->get(` literal disambiguates
// from method-name suffixes (`->getName(` won't match because the `(` follows
// `getName`, not `get`). Path-must-start-with-`/` filter rejects arbitrary
// `$cache->get('key')` style false positives.
//
// `$app->group('/api', function ($g) { ... })` prefix tracking is STILL skipped
// here, and is now the only remaining group gap in this file: the Laravel
// scanner composes `Route::prefix(...)->group(...)` and
// `Route::group(['prefix' => ...], ...)` via `laravel_prefix_ranges`. Slim has
// no substrate-gap fixture, so wiring the same ranges through `$app->group` is
// a follow-up, not drive-by work.

fn scan_slim_routes(source: &str, module_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let methods: &[(&str, &str)] = &[
        ("->get(", "GET"),
        ("->post(", "POST"),
        ("->put(", "PUT"),
        ("->patch(", "PATCH"),
        ("->delete(", "DELETE"),
        ("->options(", "OPTIONS"),
        ("->any(", "ANY"),
    ];
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let arrow = search_from + rel;
            let start = arrow + needle.len();
            if !slim_receiver_is_bare_var(source, arrow) {
                search_from = start;
                continue;
            }
            if let Some((path, consumed)) = extract_first_string(&source[start..]) {
                if path.starts_with('/') {
                    emit_route_medium(method, &path, module_id, repo, acc);
                }
                search_from = start + consumed.max(1);
            } else {
                search_from = start + 1;
            }
        }
    }
    // `$app->map(['GET', 'POST'], '/path', $h)` — variable methods + one path.
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("->map(") {
        let arrow = search_from + rel;
        let start = arrow + "->map(".len();
        if !slim_receiver_is_bare_var(source, arrow) {
            search_from = start;
            continue;
        }
        let tail = &source[start..];
        if let Some((methods_text, after_methods)) = extract_bracket_list(tail) {
            // After the closing `]`, find the next `,` then the path string.
            if let Some(comma_off) = source[start + after_methods..].find(',') {
                let path_start = start + after_methods + comma_off + 1;
                if let Some((path, consumed)) = extract_first_string(&source[path_start..]) {
                    if path.starts_with('/') {
                        for raw in methods_text.split(',') {
                            let m = raw
                                .trim()
                                .trim_matches(|c| c == '\'' || c == '"')
                                .to_ascii_uppercase();
                            if !m.is_empty() {
                                emit_route_medium(&m, &path, module_id, repo, acc);
                            }
                        }
                    }
                    search_from = path_start + consumed.max(1);
                    continue;
                }
            }
        }
        search_from = start + 1;
    }
}

/// Is the receiver of the `->` at byte `arrow` a BARE `$var`?
///
/// Slim's router idiom is always a bare variable — `$app->get('/x', $h)`,
/// `$group->post(...)`. A member chain (`$this->client->get('/api/users')`) is a
/// Guzzle / Symfony HttpClient *outbound* call, and minting a ROUTE for it made
/// every outbound call look like an inbound one (the client half is owned by
/// `collect_client_endpoints_in`). So the receiver must be a `$identifier` that
/// is not itself reached through `->` or `::`.
///
/// Byte-wise throughout: `source` is arbitrary UTF-8 and a `&str` slice on a
/// non-char boundary would panic.
fn slim_receiver_is_bare_var(source: &str, arrow: usize) -> bool {
    let bytes = source.as_bytes();
    let mut i = arrow;
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }
    if i == 0 || bytes[i - 1] != b'$' {
        return false; // `$this->client->get(` / a bare `->get(` in prose
    }
    i -= 1; // step onto the `$`
    if i >= 2
        && ((bytes[i - 2] == b'-' && bytes[i - 1] == b'>')
            || (bytes[i - 2] == b':' && bytes[i - 1] == b':'))
    {
        return false; // `$obj->$http->get(` / `self::$client->get(`
    }
    true
}

/// Read a bracket-list `[...]` starting at `s[0]` (which must be `[`).
/// Returns the inner text and the offset just past the closing `]`.
fn extract_bracket_list(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.first().copied() != Some(b'[') {
        return None;
    }
    let mut depth = 1usize;
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((s[1..i].to_string(), i + 1));
                }
            }
            b'\'' | b'"' => {
                let delim = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != delim {
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

fn emit_route_strong(method: &str, path: &str, handler_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(method.to_string()),
        }],
    });
    acc.edges.push(Edge {
        from: route_id,
        to: handler_id,
        category: edge_category::HANDLED_BY,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);
}

fn emit_route_medium(
    method: &str,
    path: &str,
    _module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> NodeId {
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
    route_id
}

/// Parse a Laravel route handler that follows the path argument and return a
/// HANDLED_BY qualifier for it. Supports the two idiomatic controller-action
/// shapes:
///   `[UserController::class, 'index']`  → Attribute{ base: UserController, name: index }
///   `'UserController@index'`            → Attribute{ base: UserController, name: index }
/// Closures / invokable single-class handlers (`SomeController::class`) yield
/// `None` (no specific method to bind to).
fn extract_laravel_handler(after_path: &str) -> Option<CallQualifier> {
    let bytes = after_path.as_bytes();
    let mut i = 0;
    // Skip whitespace and the comma separating path from handler.
    while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let rest = &after_path[i..];
    if bytes[i] == b'[' {
        // Array callable: [Controller::class, 'method']
        let (inner, _) = extract_bracket_list(rest)?;
        let mut parts = inner.splitn(2, ',');
        let first = parts.next()?.trim();
        let second = parts.next()?.trim();
        let base = first
            .trim_end_matches("::class")
            .trim()
            .rsplit('\\')
            .next()
            .unwrap_or(first)
            .to_string();
        let name = second.trim_matches(|c| c == '\'' || c == '"').to_string();
        if base.is_empty() || name.is_empty() {
            return None;
        }
        return Some(CallQualifier::Attribute { base, name });
    }
    if bytes[i] == b'\'' || bytes[i] == b'"' {
        // String callable: 'Controller@method' (classic Laravel style).
        let (s, _) = extract_first_string(rest)?;
        if let Some((base, name)) = s.split_once('@') {
            let base = base.rsplit('\\').next().unwrap_or(base).trim().to_string();
            let name = name.trim().to_string();
            if !base.is_empty() && !name.is_empty() {
                return Some(CallQualifier::Attribute { base, name });
            }
        }
    }
    None
}

fn collect_use(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let path = text.trim_start_matches("use ").trim_end_matches(';').trim();

    if let Some(last_bs) = path.rfind('\\') {
        let module_part = &path[..last_bs];
        let name = &path[last_bs + 1..];
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Symbol {
                module: module_part.replace('\\', "::"),
                name: name.to_string(),
                alias: None,
                level: 0,
            },
        });
    } else {
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Module {
                path: path.replace('\\', "::"),
                alias: None,
            },
        });
    }
}

/// `GLIA_PHP_DEBUG=1` turns on the `[php-local-bind]` marker, read once. Off by
/// default: parsers run per file inside a panic-suppressed loop and must stay
/// silent on a normal build.
fn bind_debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("GLIA_PHP_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Type names that are never a resolvable class, so never a receiver binding.
const NON_CLASS_TYPES: &[&str] = &[
    "string", "int", "float", "bool", "array", "mixed", "void", "callable", "iterable", "object",
    "self", "static", "parent", "null", "never", "false", "true",
];

/// Local `$var` -> class-name bindings inside ONE function/method.
///
/// PHP's dominant intra-file dispatch is `$x = new Thing(); $x->m();`, and that
/// binding is a same-file, same-scope syntactic fact — the parser can settle it
/// exactly the way it already settles `$this->m()` as `SelfMethod`. Nothing
/// cross-file is decided here: the call site just carries the CLASS name
/// instead of the raw `$var` text, and the graph crate resolves it as usual.
///
/// Covers `$x = new Cls()` and type-hinted parameters (`f(Cls $x)`, including
/// constructor property promotion). Nested closures / classes are skipped with
/// the same skip set `collect_calls_in` uses, so their bindings do not leak.
/// Last write in source order wins, so a rebinding shadows.
fn local_receiver_types(func: TsNode, src: &[u8]) -> HashMap<String, String> {
    let mut types: HashMap<String, String> = HashMap::new();
    let mut stack = vec![func];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "assignment_expression" => {
                if let (Some(lhs), Some(rhs)) = (
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                ) {
                    if lhs.kind() == "variable_name" && rhs.kind() == "object_creation_expression" {
                        if let Some(cls) = created_class_name(rhs, src) {
                            types.insert(text_of(lhs, src).to_string(), cls);
                        }
                    }
                }
            }
            "simple_parameter" | "property_promotion_parameter" => {
                if let (Some(ty), Some(nm)) =
                    (n.child_by_field_name("type"), n.child_by_field_name("name"))
                {
                    if let Some(cls) = class_type_name(text_of(ty, src)) {
                        types.insert(text_of(nm, src).to_string(), cls);
                    }
                }
            }
            _ => {}
        }
        let mut cursor = n.walk();
        let kids: Vec<TsNode> = n.named_children(&mut cursor).collect();
        // Push reversed so `pop` yields source order — later rebindings win.
        for child in kids.into_iter().rev() {
            if !matches!(
                child.kind(),
                "function_definition"
                    | "class_declaration"
                    | "anonymous_function_creation_expression"
            ) {
                stack.push(child);
            }
        }
    }
    types
}

/// `new Foo(...)` / `new \App\Foo(...)` -> `Foo`. `object_creation_expression`
/// carries no fields, so read the first named child; anything that is not a
/// static class reference (`new $cls()`, `new class {}`) binds nothing.
fn created_class_name(node: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let first = node.named_children(&mut cursor).next()?;
    match first.kind() {
        "name" | "qualified_name" | "relative_name" => class_type_name(text_of(first, src)),
        _ => None,
    }
}

/// A type / class reference's text -> its bare class name, or `None` when it is
/// a builtin or not a single class (union, intersection, empty).
fn class_type_name(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_start_matches('?').trim_start_matches('\\');
    let last = trimmed.rsplit('\\').next().unwrap_or(trimmed);
    if last.is_empty() || last.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    if !last.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None; // union/intersection/generic-ish text — not a single class
    }
    if NON_CLASS_TYPES.iter().any(|b| b.eq_ignore_ascii_case(last)) {
        return None;
    }
    Some(last.to_string())
}

fn collect_calls_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    acc: &mut Acc,
    types: &HashMap<String, String>,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "function_call_expression" => {
                if let Some(func) = n.child_by_field_name("function") {
                    acc.calls.push(CallSite {
                        from,
                        qualifier: CallQualifier::Bare(text_of(func, src).to_string()),
                    });
                }
            }
            "member_call_expression" => {
                let obj = n
                    .child_by_field_name("object")
                    .map(|o| text_of(o, src))
                    .unwrap_or("");
                let name = n
                    .child_by_field_name("name")
                    .map(|o| text_of(o, src))
                    .unwrap_or("");
                if obj == "$this" {
                    acc.calls.push(CallSite {
                        from,
                        qualifier: CallQualifier::SelfMethod(name.to_string()),
                    });
                } else if let Some(cls) = types.get(obj) {
                    if bind_debug_enabled() {
                        eprintln!("[php-local-bind] {obj} -> {cls}::{name}");
                    }
                    // Locally bound receiver (`$x = new Cls()` / `f(Cls $x)`):
                    // hand the graph resolver the CLASS name, which it already
                    // knows how to look up, instead of the `$var` text, which
                    // can never hit `module_import_bindings`.
                    acc.calls.push(CallSite {
                        from,
                        qualifier: CallQualifier::Attribute {
                            base: cls.clone(),
                            name: name.to_string(),
                        },
                    });
                } else {
                    acc.calls.push(CallSite {
                        from,
                        qualifier: CallQualifier::Attribute {
                            base: obj.to_string(),
                            name: name.to_string(),
                        },
                    });
                }
            }
            "scoped_call_expression" => {
                let scope = n
                    .child_by_field_name("scope")
                    .map(|o| text_of(o, src))
                    .unwrap_or("");
                let name = n
                    .child_by_field_name("name")
                    .map(|o| text_of(o, src))
                    .unwrap_or("");
                acc.calls.push(CallSite {
                    from,
                    qualifier: CallQualifier::Attribute {
                        base: scope.to_string(),
                        name: name.to_string(),
                    },
                });
            }
            _ => {}
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "function_definition"
                    | "class_declaration"
                    | "anonymous_function_creation_expression"
            ) {
                stack.push(child);
            }
        }
    }
}

// ============================================================================
// Client-side HTTP call sites -> ENDPOINT nodes
// ============================================================================
//
// The OUTBOUND half of a cross-service hop (the inbound half is
// `scan_laravel_routes` / `scan_slim_routes` / `check_route_attrs`). Without it
// a Laravel or Symfony service calling another service is a dead end for
// `cross_stack_trace` in the outbound direction. Shapes covered:
//
//   Guzzle shorthand  `$this->client->get('/api/users/' . $id)`  verb <- name
//   Guzzle / Symfony  `$this->client->request('POST', '/api/users', ...)`
//                     — both libraries share `request(VERB, URL, ...)`, so the
//                     verb comes from the FIRST argument
//   async variants    `getAsync` / `requestAsync` — the `Async` suffix is
//                     stripped and the shape is otherwise identical
//   ext-curl          `curl_setopt($ch, CURLOPT_URL, '/api/users')` — GET
//                     unless the same body carries an explicit
//                     `CURLOPT_CUSTOMREQUEST, 'PUT'` literal (or CURLOPT_POST)
//
// `endpoint::url_to_path` is the ONLY precision gate that matters: the string
// must be a path (or an absolute URL whose path we keep) after the host is
// stripped, which is what keeps `$collection->get('user-1')` — the canonical
// PHP false positive, `->get(` being ubiquitous on collections, containers and
// config bags — out of the graph.

/// `Async`-suffix-stripped method name -> HTTP verb.
fn php_http_verb(name: &str) -> Option<&'static str> {
    match name {
        "get" => Some("GET"),
        "post" => Some("POST"),
        "put" => Some("PUT"),
        "patch" => Some("PATCH"),
        "delete" => Some("DELETE"),
        "head" => Some("HEAD"),
        "options" => Some("OPTIONS"),
        _ => None,
    }
}

/// The `arguments` child at index `i` (an `argument` wrapper node).
fn nth_arg<'a>(args: Option<TsNode<'a>>, i: usize) -> Option<TsNode<'a>> {
    let a = args?;
    let mut c = a.walk();
    a.named_children(&mut c).nth(i)
}

/// Unwrap an `argument` to the expression it carries. PHP named arguments
/// (`method: 'GET'`) put a `name` node first, so it is skipped.
fn arg_expr<'a>(arg: TsNode<'a>) -> Option<TsNode<'a>> {
    if arg.kind() != "argument" {
        return Some(arg);
    }
    let name_id = arg.child_by_field_name("name").map(|n| n.id());
    let mut c = arg.walk();
    arg.named_children(&mut c).find(|n| Some(n.id()) != name_id)
}

/// Inner text of a non-interpolating `string` node (its `string_content` /
/// `escape_sequence` children); falls back to quote-trimming for `''`.
fn php_string_inner(node: TsNode, src: &[u8]) -> String {
    let mut out = String::new();
    let mut c = node.walk();
    for part in node.named_children(&mut c) {
        out.push_str(text_of(part, src));
    }
    if out.is_empty() {
        return text_of(node, src)
            .trim_matches('\'')
            .trim_matches('"')
            .to_string();
    }
    out
}

/// `"…/users/$id"` -> `…/users/${…}`. Every non-literal child of the
/// `encapsed_string` becomes the `${…}` wildcard that
/// `normalise_http_path` (repo-graph-graph) collapses to `{}`, so an
/// interpolated client path pairs with route `/users/{id}`.
fn php_encapsed_template(node: TsNode, src: &[u8]) -> String {
    let mut out = String::new();
    let mut c = node.walk();
    for part in node.named_children(&mut c) {
        match part.kind() {
            "string_content" | "escape_sequence" => out.push_str(text_of(part, src)),
            _ => out.push_str("${…}"),
        }
    }
    out
}

/// `(raw_path, strong)` for an argument used as a URL.
///
/// A plain `'…'` literal is Strong. An `encapsed_string` and a `.`
/// concatenation (`'/api/users/' . $id`) both reconstruct with `${…}` in place
/// of every non-literal operand and are Medium — the same treatment java gives
/// `"/users/" + id`. Concatenation recurses so the left-associative chain
/// `'/a/' . $x . '/b'` rebuilds in order. At least one literal must survive, so
/// `$base . $path` (no path text at all) extracts nothing.
fn php_url_from_arg(arg: TsNode, src: &[u8]) -> Option<(String, bool)> {
    match arg.kind() {
        "string" => Some((php_string_inner(arg, src), true)),
        "encapsed_string" => Some((php_encapsed_template(arg, src), false)),
        "binary_expression" => {
            let op = arg.child_by_field_name("operator")?;
            if text_of(op, src) != "." {
                return None;
            }
            let mut out = String::new();
            let mut saw_literal = false;
            for side in [
                arg.child_by_field_name("left")?,
                arg.child_by_field_name("right")?,
            ] {
                match php_url_from_arg(side, src) {
                    Some((s, _)) => {
                        out.push_str(&s);
                        saw_literal = true;
                    }
                    None => out.push_str("${…}"),
                }
            }
            if saw_literal {
                Some((out, false))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// May a verb-named call on THIS receiver be an outbound HTTP call?
///
/// The mirror of `slim_receiver_is_bare_var`, and deliberately disjoint from
/// it: a member chain (`$this->client`, `self::$http`) is a field-held client,
/// while a bare `$app->get('/x', $h)` is Slim's ROUTE idiom. A bare variable
/// therefore only counts when `local_receiver_types` binds it to a class whose
/// name carries `Client` (Guzzle `Client`, Symfony `HttpClientInterface`,
/// `CurlHttpClient`). An unbound `$client->get('/x')` is a known miss — see the
/// follow-up note on the fixture.
fn php_client_receiver_ok(n: TsNode, src: &[u8], types: &HashMap<String, String>) -> bool {
    let Some(obj) = n.child_by_field_name("object") else {
        return false;
    };
    if obj.kind() != "variable_name" {
        return true;
    }
    types
        .get(text_of(obj, src))
        .is_some_and(|cls| cls.contains("Client"))
}

/// `(VERB, url argument)` for a member call that is an outbound HTTP request.
fn php_client_call_shape<'a>(
    n: TsNode<'a>,
    src: &'a [u8],
    types: &HashMap<String, String>,
) -> Option<(String, TsNode<'a>)> {
    let name = text_of(n.child_by_field_name("name")?, src);
    let base = name.strip_suffix("Async").unwrap_or(name);
    let args = n.child_by_field_name("arguments");

    if base == "request" {
        // Verb-first shape. Slim has no `->request(` route idiom and the verb
        // literal is itself the discriminator, so no receiver gate is needed.
        let verb_arg = arg_expr(nth_arg(args, 0)?)?;
        if verb_arg.kind() != "string" {
            return None;
        }
        let verb = php_string_inner(verb_arg, src).trim().to_ascii_uppercase();
        php_http_verb(&verb.to_ascii_lowercase())?;
        return Some((verb, arg_expr(nth_arg(args, 1)?)?));
    }

    let verb = php_http_verb(base)?;
    if !php_client_receiver_ok(n, src, types) {
        return None;
    }
    Some((verb.to_string(), arg_expr(nth_arg(args, 0)?)?))
}

/// The URL argument of `curl_setopt($ch, CURLOPT_URL, '…')`.
fn php_curl_url_arg<'a>(n: TsNode<'a>, src: &'a [u8]) -> Option<TsNode<'a>> {
    if text_of(n.child_by_field_name("function")?, src) != "curl_setopt" {
        return None;
    }
    let args = n.child_by_field_name("arguments");
    let opt = arg_expr(nth_arg(args, 1)?)?;
    if !text_of(opt, src).trim().ends_with("CURLOPT_URL") {
        return None;
    }
    arg_expr(nth_arg(args, 2)?)
}

/// ext-curl carries the verb on a SIBLING `curl_setopt`, so it is read from the
/// whole body once: an explicit `CURLOPT_CUSTOMREQUEST, 'PUT'` literal wins,
/// then `CURLOPT_POST*`, else the curl default GET.
fn php_curl_body_verb(body: TsNode, src: &[u8]) -> String {
    let text = text_of(body, src);
    const CUSTOM: &str = "CURLOPT_CUSTOMREQUEST";
    if let Some(i) = text.find(CUSTOM)
        && let Some((v, _)) = extract_first_string(&text[i + CUSTOM.len()..])
    {
        let v = v.trim().to_ascii_uppercase();
        if php_http_verb(&v.to_ascii_lowercase()).is_some() {
            return v;
        }
    }
    if text.contains("CURLOPT_POST") {
        return "POST".to_string();
    }
    "GET".to_string()
}

#[allow(clippy::too_many_arguments)]
fn push_php_endpoint(
    n: TsNode,
    url_arg: TsNode,
    src: &[u8],
    method: &str,
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let Some((raw, strong)) = php_url_from_arg(url_arg, src) else {
        return;
    };
    let Some(path) = endpoint::url_to_path(&raw) else {
        return;
    };
    let pos = n.start_position();
    let ep = ClientEndpoint {
        method: method.to_string(),
        path,
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: if strong {
            Confidence::Strong
        } else {
            Confidence::Medium
        },
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
}

/// Walk one function/method body for outbound HTTP call sites. Mirrors
/// `collect_calls_in`'s stack walk (and its nested-scope skips) so a call
/// inside a closure is attributed to that closure's own owner, not this one.
fn collect_client_endpoints_in(
    body: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    types: &HashMap<String, String>,
    acc: &mut Acc,
) {
    let mut curl_verb: Option<String> = None;
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "member_call_expression" | "nullsafe_member_call_expression" => {
                if let Some((method, url_arg)) = php_client_call_shape(n, src, types) {
                    push_php_endpoint(n, url_arg, src, &method, from, repo, file_rel, acc);
                }
            }
            "function_call_expression" => {
                if let Some(url_arg) = php_curl_url_arg(n, src) {
                    let verb = curl_verb
                        .get_or_insert_with(|| php_curl_body_verb(body, src))
                        .clone();
                    push_php_endpoint(n, url_arg, src, &verb, from, repo, file_rel, acc);
                }
            }
            _ => {}
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "function_definition"
                    | "class_declaration"
                    | "anonymous_function_creation_expression"
            ) {
                stack.push(child);
            }
        }
    }
}

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
    fn classes_and_methods() {
        let source = r#"<?php
namespace App\Services;

class UserService {
    public function getUser(string $id): User {
        return $this->repo->find($id);
    }

    private function validate(User $u): void {}
}
"#;
        let fp = parse_file(
            source,
            "src/Services/UserService.php",
            "App::Services",
            repo(),
        )
        .unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"validate"));
    }

    #[test]
    fn interfaces_and_enums() {
        let source = r#"<?php
namespace App;

interface Drawable {
    public function draw(): void;
}

enum Color {
    case Red;
    case Green;
}
"#;
        let fp = parse_file(source, "src/Types.php", "App", repo()).unwrap();
        assert_eq!(
            fp.nav
                .kind_by_id
                .values()
                .filter(|k| **k == node_kind::INTERFACE)
                .count(),
            1
        );
        assert_eq!(
            fp.nav
                .kind_by_id
                .values()
                .filter(|k| **k == node_kind::ENUM)
                .count(),
            1
        );
    }

    #[test]
    fn use_imports() {
        let source = r#"<?php
use App\Models\User;
use Illuminate\Http\Request;
"#;
        let fp = parse_file(source, "src/Controller.php", "App", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
    }

    #[test]
    fn symfony_route_methods_list() {
        let source = r#"<?php
namespace App\Controller;

class UserController {
    #[Route('/users', methods: ['GET', 'POST'])]
    public function list() {}

    #[Route('/users/{id}', methods: ['PUT', 'DELETE'])]
    public function update() {}
}
"#;
        let fp = parse_file(source, "src/UserController.php", "App::Controller", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /users"));
        assert!(route_names.contains(&"POST /users"));
        assert!(route_names.contains(&"PUT /users/{id}"));
        assert!(route_names.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn symfony_route_no_methods_is_any() {
        let source = r#"<?php
class C {
    #[Route('/health')]
    public function health() {}
}
"#;
        let fp = parse_file(source, "src/C.php", "App", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"ANY /health"));
    }

    #[test]
    fn symfony_class_route_prefix_composes() {
        let source = r#"<?php
namespace App\Controller;

use Symfony\Component\Routing\Annotation\Route;

#[Route('/api/v1/users')]
class UserController {
    #[Route('/{id}', methods: ['GET'])]
    public function show(int $id) {}

    #[Route('', methods: ['POST'])]
    public function create() {}
}
"#;
        let fp = parse_file(source, "src/UserController.php", "App::Controller", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(
            route_names.contains(&"GET /api/v1/users/{id}"),
            "class prefix must compose onto the action template: {route_names:?}"
        );
        assert!(
            route_names.contains(&"POST /api/v1/users"),
            "an empty action template is the prefix itself: {route_names:?}"
        );
        // The uncomposed templates must be gone, not merely joined by the new ones.
        assert!(!route_names.contains(&"GET /{id}"), "{route_names:?}");
        assert!(!route_names.contains(&"POST "), "{route_names:?}");
        // The controller's own #[Route] still lands (unchanged behaviour).
        assert!(
            route_names.contains(&"ANY /api/v1/users"),
            "{route_names:?}"
        );
    }

    #[test]
    fn symfony_class_scan_does_not_duplicate_action_routes() {
        let source = r#"<?php
#[Route('/api/v1/users')]
class UserController {
    #[Route('/{id}', methods: ['GET'])]
    public function show(int $id) {}
}
"#;
        let fp = parse_file(source, "src/UserController.php", "App", repo()).unwrap();
        let class_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "App::UserController");
        let handled: Vec<NodeId> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .map(|e| e.to)
            .collect();
        // Exactly one HANDLED_BY points at the class: its OWN #[Route]. The old
        // text scan re-found the method attribute at class level and emitted a
        // second `<action route> -> UserController`.
        assert_eq!(
            handled.iter().filter(|t| **t == class_id).count(),
            1,
            "class must be claimed once, by its own #[Route]"
        );
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.category == edge_category::HANDLED_BY)
                .count(),
            2,
            "one class route + one action route"
        );
    }

    #[test]
    fn laravel_route_facade() {
        let source = r#"<?php
Route::get('/users', [UserController::class, 'index']);
Route::post('/users', [UserController::class, 'store']);
Route::put('/users/{id}', [UserController::class, 'update']);
Route::delete('/users/{id}', [UserController::class, 'destroy']);
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /users"));
        assert!(route_names.contains(&"POST /users"));
        assert!(route_names.contains(&"PUT /users/{id}"));
        assert!(route_names.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn laravel_route_resource() {
        let source = r#"<?php
Route::resource('/photos', PhotoController::class);
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.iter().any(|n| n.starts_with("GET /photos")));
        assert!(route_names.iter().any(|n| n.starts_with("POST /photos")));
        assert!(route_names.iter().any(|n| n.starts_with("PUT /photos")));
        assert!(route_names.iter().any(|n| n.starts_with("DELETE /photos")));
    }

    fn route_names(fp: &FileParse) -> Vec<String> {
        fp.nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.clone())
            .collect()
    }

    #[test]
    fn laravel_route_prefix_group_composes() {
        let source = r#"<?php
Route::prefix('api/v1')->group(function () {
    Route::get('/users/{id}', [UserController::class, 'show']);
    Route::post('/users', [UserController::class, 'store']);
});
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        let names = route_names(&fp);
        assert!(
            names.contains(&"GET /api/v1/users/{id}".to_string()),
            "got {names:?}"
        );
        assert!(
            names.contains(&"POST /api/v1/users".to_string()),
            "got {names:?}"
        );
        assert!(!names.contains(&"GET /users/{id}".to_string()));
    }

    #[test]
    fn laravel_route_group_array_prefix_composes() {
        let source = r#"<?php
Route::group(['prefix' => 'admin', 'middleware' => 'auth'], function () {
    Route::get('/stats', [StatsController::class, 'index']);
});
Route::middleware('auth')->prefix('api')->group(function () {
    Route::get('/me', [MeController::class, 'show']);
});
Route::group(['middleware' => 'auth'], function () {
    Route::get('/plain', [PlainController::class, 'index']);
});
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        let names = route_names(&fp);
        assert!(names.contains(&"GET /admin/stats".to_string()), "got {names:?}");
        assert!(names.contains(&"GET /api/me".to_string()), "got {names:?}");
        // A middleware-only group contributes no path segment.
        assert!(names.contains(&"GET /plain".to_string()), "got {names:?}");
    }

    #[test]
    fn laravel_route_outside_group_unaffected() {
        // Regression lock for the range arithmetic: a `}` inside a string
        // literal must not close the group early, and routes after the real
        // closing brace must keep their bare path. The nested group proves the
        // outer-to-inner fold.
        let source = r#"<?php
Route::prefix('api')->group(function () {
    Route::prefix('v2')->group(function () {
        Route::get('/users', [UserController::class, 'index']);
    });
    Route::get('/ping', function () { return '} not a brace'; });
});
Route::get('/health', [HealthController::class, 'index']);
$builder = Route::prefix('orphan');
Route::get('/after', [AfterController::class, 'index']);
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        let names = route_names(&fp);
        assert!(names.contains(&"GET /api/v2/users".to_string()), "got {names:?}");
        assert!(names.contains(&"GET /api/ping".to_string()), "got {names:?}");
        assert!(names.contains(&"GET /health".to_string()), "got {names:?}");
        assert!(names.contains(&"GET /after".to_string()), "got {names:?}");
        assert!(!names.contains(&"GET /api/health".to_string()), "got {names:?}");
        assert!(!names.contains(&"GET /orphan/after".to_string()), "got {names:?}");
    }

    #[test]
    fn laravel_route_handled_by_array_callable() {
        let source = r#"<?php
use App\Http\Controllers\UserController;

Route::get('/users', [UserController::class, 'index']);
Route::post('/users', [UserController::class, 'store']);
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        let handled: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .collect();
        assert_eq!(handled.len(), 2, "one HANDLED_BY ref per verb route");
        assert!(handled.iter().any(|r| matches!(
            &r.qualifier,
            CallQualifier::Attribute { base, name } if base == "UserController" && name == "index"
        )));
        assert!(handled.iter().any(|r| matches!(
            &r.qualifier,
            CallQualifier::Attribute { base, name } if base == "UserController" && name == "store"
        )));
    }

    #[test]
    fn laravel_route_handled_by_string_callable() {
        // Classic `'Controller@action'` string form.
        let source = r#"<?php
Route::get('/legacy', 'LegacyController@show');
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        let handled: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .collect();
        assert_eq!(handled.len(), 1);
        assert!(matches!(
            &handled[0].qualifier,
            CallQualifier::Attribute { base, name } if base == "LegacyController" && name == "show"
        ));
    }

    #[test]
    fn laravel_route_closure_handler_emits_no_ref() {
        // A closure handler has no controller action to bind to.
        let source = r#"<?php
Route::get('/ping', function () { return 'pong'; });
"#;
        let fp = parse_file(source, "routes/web.php", "routes::web", repo()).unwrap();
        assert!(
            fp.refs
                .iter()
                .all(|r| r.category != edge_category::HANDLED_BY),
            "closure handler must not emit a HANDLED_BY ref"
        );
    }

    // ========================================================================
    // Slim route extraction
    // ========================================================================

    #[test]
    fn slim_app_verb_routes_emit() {
        let source = r#"<?php
$app = AppFactory::create();
$app->get('/health', function ($req, $res) { return $res; });
$app->post('/users', UserHandler::class);
$app->put('/users/{id}', UserHandler::class);
$app->delete('/users/{id}', UserHandler::class);
"#;
        let fp = parse_file(source, "public/index.php", "public::index", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /health"));
        assert!(route_names.contains(&"POST /users"));
        assert!(route_names.contains(&"PUT /users/{id}"));
        assert!(route_names.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn slim_map_emits_one_route_per_method() {
        let source = r#"<?php
$app->map(['GET', 'POST', 'PUT'], '/users', UserHandler::class);
"#;
        let fp = parse_file(source, "public/index.php", "public::index", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /users"));
        assert!(route_names.contains(&"POST /users"));
        assert!(route_names.contains(&"PUT /users"));
    }

    #[test]
    fn slim_skips_non_path_first_arg() {
        // `$cache->get('cache-key')` is the canonical false-positive shape.
        let source = r#"<?php
class Svc {
    public function load(): mixed {
        return $this->cache->get('cache-key');
    }
}
"#;
        let fp = parse_file(source, "src/Svc.php", "App", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "non-`/` first arg must not emit a Slim route");
    }

    #[test]
    fn slim_does_not_match_method_name_suffix() {
        // `->getName(` must not match `->get(`. Verifies word-boundary on `(`.
        let source = r#"<?php
class Svc {
    public function display(): void {
        echo $user->getName();
        echo $user->getAvatar();
    }
}
"#;
        let fp = parse_file(source, "src/Svc.php", "App", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "method-name suffix must not match `->get(`");
    }

    #[test]
    fn this_calls() {
        let source = r#"<?php
class Service {
    public function handle(): void {
        $this->validate();
        $helper->process();
    }
    private function validate(): void {}
}
"#;
        let fp = parse_file(source, "src/Service.php", "App", repo()).unwrap();
        let self_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::SelfMethod(_)))
            .collect();
        assert_eq!(self_calls.len(), 1);
    }

    /// Attribute-qualified call bases, for the local-receiver tests below.
    fn attr_bases(fp: &FileParse) -> Vec<(String, String)> {
        fp.calls
            .iter()
            .filter_map(|c| match &c.qualifier {
                CallQualifier::Attribute { base, name } => Some((base.clone(), name.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn local_new_binding_rewrites_receiver() {
        let source = r#"<?php
namespace App\Http;

use App\Services\Greeter;

class HomeController {
    public function index() {
        $greeter = new Greeter();
        return $greeter->greet("world");
    }
}
"#;
        let fp = parse_file(source, "src/HomeController.php", "App::Http", repo()).unwrap();
        let bases = attr_bases(&fp);
        assert_eq!(
            bases,
            vec![("Greeter".to_string(), "greet".to_string())],
            "`$greeter = new Greeter()` must rebase the call onto the class"
        );
    }

    #[test]
    fn typed_parameter_binds_receiver() {
        let source = r#"<?php
namespace App\Http;

function f(\App\Services\Greeter $g, string $name) {
    return $g->greet($name);
}
"#;
        let fp = parse_file(source, "src/f.php", "App::Http", repo()).unwrap();
        let bases = attr_bases(&fp);
        assert_eq!(
            bases,
            vec![("Greeter".to_string(), "greet".to_string())],
            "a class-typed parameter must bind the receiver; `string` must not"
        );
    }

    #[test]
    fn unbound_variable_receiver_is_unchanged() {
        let source = r#"<?php
class Service {
    public function handle(): void {
        $helper->process();
    }
}
"#;
        let fp = parse_file(source, "src/Service.php", "App", repo()).unwrap();
        let bases = attr_bases(&fp);
        assert_eq!(
            bases,
            vec![("$helper".to_string(), "process".to_string())],
            "an unbound receiver keeps the raw `$var` fallback"
        );
    }

    // ========================================================================
    // Client HTTP call sites -> ENDPOINT (A4.10)
    // ========================================================================

    /// Every ENDPOINT node name in a parse, sorted.
    fn endpoint_names(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ENDPOINT))
            .map(|(_, n)| n.clone())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn guzzle_client_get_emits_endpoint() {
        let source = r#"<?php
class ApiClient {
    private $client;
    public function fetchUser($id) {
        return $this->client->get('/api/users/' . $id);
    }
}
"#;
        let fp = parse_file(source, "src/ApiClient.php", "App", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec!["GET /api/users/${…}".to_string()],
            "`'/api/users/' . $id` folds the non-literal tail to a wildcard segment"
        );
        // and the enclosing method -> ENDPOINT CALLS edge exists
        let ep_id = endpoint::endpoint_id(repo(), "GET", "/api/users/${…}");
        assert!(
            fp.edges
                .iter()
                .any(|e| e.to == ep_id && e.category == edge_category::CALLS),
            "the enclosing method must CALL the endpoint"
        );
    }

    #[test]
    fn guzzle_request_verb_first_arg_emits_endpoint() {
        // Guzzle AND Symfony HttpClient share `request(VERB, URL, ...)`.
        let source = r#"<?php
class ApiClient {
    private $client;
    public function createUser($body) {
        return $this->client->request('POST', '/api/users', ['json' => $body]);
    }
    public function fetchOne($id) {
        return $this->http->requestAsync('GET', "/api/users/$id");
    }
}
"#;
        let fp = parse_file(source, "src/ApiClient.php", "App", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /api/users/${…}".to_string(),
                "POST /api/users".to_string()
            ],
            "verb comes from the first argument; `Async` is stripped; \"$id\" interpolates"
        );
    }

    #[test]
    fn php_client_calls_emit_no_route() {
        // The Slim/Laravel SOURCE scanners must not fire on a client file:
        // `$this->client->get('/api/users')` is outbound, not a route.
        let source = r#"<?php
class ApiClient {
    private $client;
    public function fetchUser($id) {
        return $this->client->get('/api/users/' . $id);
    }
    public function createUser($body) {
        return $this->client->post('/api/users', $body);
    }
}
"#;
        let fp = parse_file(source, "src/ApiClient.php", "App", repo()).unwrap();
        let routes: Vec<&String> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n)
            .collect();
        assert!(
            routes.is_empty(),
            "a member-chain receiver is a client, not a Slim router: {routes:?}"
        );
        assert_eq!(endpoint_names(&fp).len(), 2);
    }

    #[test]
    fn php_non_url_string_arg_is_dropped() {
        // `->get(` is ubiquitous in PHP (collections, containers, config bags);
        // `url_to_path` is the only gate, so this is the load-bearing negative.
        let source = r#"<?php
class Cache {
    private $collection;
    public function cached($id) {
        return $this->collection->get('user-' . $id);
    }
    public function opt() {
        return $this->config->get('app.name');
    }
}
"#;
        let fp = parse_file(source, "src/Cache.php", "App", repo()).unwrap();
        assert!(
            endpoint_names(&fp).is_empty(),
            "a non-path string argument must not become an ENDPOINT"
        );
    }

    #[test]
    fn bare_variable_receiver_needs_a_client_binding() {
        // `$app->get('/x', $h)` is Slim's ROUTE idiom — the ENDPOINT half must
        // stay off it, while a `new Client()` binding opts the same shape in.
        let source = r#"<?php
function boot() {
    $app = AppFactory::create();
    $app->get('/health', UserHandler::class);
}
function fetch() {
    $client = new \GuzzleHttp\Client();
    return $client->get('/api/users');
}
"#;
        let fp = parse_file(source, "src/boot.php", "App", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec!["GET /api/users".to_string()],
            "only the Client-typed receiver yields an ENDPOINT"
        );
    }

    #[test]
    fn curl_setopt_url_emits_endpoint_with_custom_verb() {
        let source = r#"<?php
function push($id) {
    $ch = curl_init();
    curl_setopt($ch, CURLOPT_URL, 'https://users-svc/api/users');
    curl_setopt($ch, CURLOPT_CUSTOMREQUEST, 'PUT');
    return curl_exec($ch);
}
"#;
        let fp = parse_file(source, "src/push.php", "App", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec!["PUT /api/users".to_string()],
            "host stripped by url_to_path; verb upgraded by the sibling CUSTOMREQUEST"
        );
    }
}
