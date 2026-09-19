use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use tree_sitter::{Node as TsNode, Parser};

use repo_graph_code_domain::data_entity;
use repo_graph_code_domain::di_stats::{self, DiShape};
use repo_graph_code_domain::endpoint::{self, ClientEndpoint, push_client_endpoint};
use repo_graph_code_domain::line_of;
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

    // Namespaces, `use` maps and Eloquent model classes are read up front, so a
    // query site resolves its receiver whatever the declaration order.
    let mut acc = Acc {
        eloquent: Eloquent::prescan(root, src),
        module_qname: module_qname.to_string(),
        ..Acc::default()
    };

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

    // LB.7b: a top-level type outside a braced namespace hangs off the file's
    // DIRECTORY, not the file module (the MODULE node above keeps
    // `module_qname`: imports, TESTS pairing and the local-module index read
    // file-module qnames). Functions and `use` keep the file scope.
    let scope = type_scope(module_qname);
    visit_children(
        root,
        src,
        file_rel_path,
        module_qname,
        scope,
        module_id,
        repo,
        &mut acc,
    );
    if acc.dir_scoped_types > 0 && qname_debug() {
        eprintln!(
            "[qname] php: {} top-level types scoped to {scope} (file stem dropped) file={file_rel_path}",
            acc.dir_scoped_types
        );
    }
    if acc.scoped_uses > 0 && bind_debug_enabled() {
        eprintln!(
            "[php-use] {} braced-namespace uses bound to the file module {module_qname} file={file_rel_path}",
            acc.scoped_uses
        );
    }

    scan_laravel_routes(source, file_rel_path, module_id, repo, &mut acc);
    scan_slim_routes(source, module_id, repo, &mut acc);

    if !acc.endpoint_seen.is_empty() {
        eprintln!(
            "[php-http-client] {} endpoints in {file_rel_path}",
            acc.endpoint_seen.len()
        );
    }
    if acc.eloquent.models + acc.eloquent.queries > 0 {
        eprintln!(
            "[orm-eloquent] models={} table_cells={} queries={} in {file_rel_path}",
            acc.eloquent.models, acc.eloquent.table_cells, acc.eloquent.queries
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

/// LB.7b: a PHP top-level type outside a braced `namespace X { }` belongs to
/// its directory (LB.2's rule), not its file, so drop the file-stem segment the
/// engine's `path_to_qname` puts last: `src::Billing::Invoice` ->
/// `src::Billing`, and a file at the repo root (`index`) -> `""`. The directory,
/// not the declared namespace, is the scope on purpose: Laravel and Symfony put
/// every application under `App`, so two apps of one monorepo would otherwise
/// share every controller NodeId; PSR-4 makes the directory mirror the
/// namespace anyway, and PHP forbids two same-named classes in one namespace.
///
/// The braced form keeps `<Ns>::<Type>`: it exists to hold SEVERAL namespaces
/// in one file, and directory scope would merge their same-named classes.
///
/// The class `Invoice` of `Invoice.php` therefore shares its qname with the file
/// MODULE (different kind, different NodeId); `MergedGraph::pick_primary` ranks
/// the declaration over the container, so qname lookups land on the type.
fn type_scope(module_qname: &str) -> &str {
    module_qname
        .rsplit_once("::")
        .map_or("", |(dir, _stem)| dir)
}

/// `scope::name`, or the bare `name` for the empty (repo-root) scope.
fn scoped(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}::{name}")
    }
}

/// `GLIA_QNAME_DEBUG=1` turns on the per-file `[qname] php:` marker, read once.
/// Off by default: it would print for every PHP file of a build.
///   `GLIA_QNAME_DEBUG=1 glia analyze <repo> 2>&1 | grep '\[qname\] php:'`
fn qname_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG
        .get_or_init(|| std::env::var("GLIA_QNAME_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0"))
}

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    calls: Vec<CallSite>,
    refs: Vec<UnresolvedRef>,
    nav: CodeNav,
    /// LB.7b: top-level types minted under the directory scope (outside a
    /// braced namespace) — the `[qname] php:` marker's count.
    dir_scoped_types: usize,
    /// LA.40b: the file MODULE's qname — every `use` is recorded with it as
    /// `from_module`, whichever namespace form holds the statement.
    module_qname: String,
    /// LA.40b: `use` statements found inside a braced `namespace X { }` body
    /// (bound to the file module, not the namespace) — the `[php-use]` count.
    scoped_uses: usize,
    /// ENDPOINT ids already minted in THIS file — `push_client_endpoint` dedups
    /// the node through it while still pushing one CALLS edge per call site.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Eloquent model / query-site state for this file (`[orm-eloquent]`).
    eloquent: Eloquent,
}

/// `parent_qname` scopes functions; `type_scope` scopes classes / interfaces /
/// enums. At the file root they differ (the file module vs its directory,
/// LB.7b); inside a braced namespace body both are the namespace. A `use`
/// statement always belongs to the file module (`acc.module_qname`, LA.40b).
#[allow(clippy::too_many_arguments)]
fn visit_children(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    type_scope: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let dir_scoped = parent_qname != type_scope;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let minted = match child.kind() {
            "namespace_definition" => {
                visit_namespace(child, src, file_rel, parent_qname, parent_id, repo, acc);
                false
            }
            "class_declaration" => {
                visit_class(child, src, file_rel, type_scope, parent_id, repo, acc)
            }
            "interface_declaration" => {
                visit_interface(child, src, file_rel, type_scope, parent_id, repo, acc)
            }
            "enum_declaration" => {
                visit_enum(child, src, file_rel, type_scope, parent_id, repo, acc)
            }
            "function_definition" => {
                visit_function(child, src, file_rel, parent_qname, parent_id, repo, acc);
                false
            }
            "namespace_use_declaration" => {
                // Below the `program` root means inside a braced namespace
                // body: the use still binds for the whole file.
                if node.kind() != "program" {
                    acc.scoped_uses += 1;
                }
                collect_use(child, src, acc);
                false
            }
            _ => false,
        };
        if minted && dir_scoped {
            acc.dir_scoped_types += 1;
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
        cells: Vec::new(),
    });
    acc.nav
        .record(ns_id, simple, &qname, node_kind::PACKAGE, Some(parent_id));

    // Braced form: the body's types stay namespace-scoped (`<Ns>::<Type>`),
    // so the namespace is both the member and the type scope.
    if let Some(body) = node.child_by_field_name("body") {
        visit_children(body, src, file_rel, &qname, &qname, ns_id, repo, acc);
    }
}

/// `scope` is the type scope (the file's directory, or a braced namespace —
/// LB.7b). Returns whether a node was minted (a nameless declaration is not).
fn visit_class(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> bool {
    let Some(name_node) = node.child_by_field_name("name") else {
        return false;
    };
    let name = text_of(name_node, src);
    let qname = scoped(scope, name);
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::CLASS, Some(parent_id));
    if acc.eloquent.model_classes.contains(&node.id()) {
        emit_eloquent_model(node, src, name, id, repo, acc);
    }

    // Symfony composes a controller's class-level `#[Route('/prefix')]` onto
    // every action template, so the prefix must be known before the body walk.
    let class_prefix = class_route_prefix(node, src);
    let is_di = is_php_di_class(name, node, src);
    let mut composed = 0usize;
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "method_declaration" {
                composed +=
                    visit_method(child, src, file_rel, &qname, id, repo, &class_prefix, acc);
                if is_di && is_constructor(child, src) {
                    // `parent_id` is the file MODULE (semicolon namespace, the
                    // PSR-4 norm) or the namespace PACKAGE (brace form); the
                    // symbol table indexes both, so either scopes the lookup.
                    emit_ctor_injects(child, src, id, parent_id, acc);
                }
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
    true
}

/// `scope` is the type scope (the file's directory, or a braced namespace —
/// LB.7b). Returns whether a node was minted (a nameless declaration is not).
fn visit_interface(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> bool {
    let Some(name_node) = node.child_by_field_name("name") else {
        return false;
    };
    let name = text_of(name_node, src);
    let qname = scoped(scope, name);
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::INTERFACE, Some(parent_id));
    true
}

/// `scope` is the type scope (the file's directory, or a braced namespace —
/// LB.7b). Returns whether a node was minted (a nameless declaration is not).
fn visit_enum(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> bool {
    let Some(name_node) = node.child_by_field_name("name") else {
        return false;
    };
    let name = text_of(name_node, src);
    let qname = scoped(scope, name);
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::ENUM, Some(parent_id));
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

    let types = local_receiver_types(node, src);
    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, repo, acc, &types);
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::METHOD, Some(parent_id));

    let types = local_receiver_types(node, src);
    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, repo, acc, &types);
        collect_client_endpoints_in(body, src, id, repo, file_rel, &types, acc);
    }

    check_route_attrs(node, src, id, repo, class_prefix, acc)
}

/// Is this class a container-managed service whose constructor type-hints are
/// dependencies? PHP has no DI annotation on the constructor itself (Symfony
/// autowiring and Laravel's container both resolve every type-hint), so the
/// class-level gate is the whole precision story: a conventional service-role
/// name suffix, or a Symfony autoconfiguration attribute declared ON the class.
/// A value object (`Money`, `Email`) matches neither and emits nothing.
fn is_php_di_class(name: &str, class_node: TsNode, src: &[u8]) -> bool {
    const DI_SUFFIXES: [&str; 12] = [
        "Controller",
        "Service",
        "Repository",
        "Handler",
        "Manager",
        "Provider",
        "Factory",
        "Middleware",
        "Command",
        "Subscriber",
        "Listener",
        "Job",
    ];
    const DI_ATTRIBUTE_PREFIXES: [&str; 6] = [
        "AsController",
        "AsCommand",
        "AsEventListener",
        "AsMessageHandler",
        "Autoconfigure",
        "AsService",
    ];
    if DI_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    // The class's own `attributes` field, never the body: a method-level
    // `#[AsEventListener]` does not make its class a service.
    own_attributes(class_node, src)
        .iter()
        .any(|(attr, _)| DI_ATTRIBUTE_PREFIXES.iter().any(|p| attr.starts_with(p)))
}

/// `__construct`, compared case-insensitively (PHP method names are).
fn is_constructor(method: TsNode, src: &[u8]) -> bool {
    method
        .child_by_field_name("name")
        .is_some_and(|n| text_of(n, src).eq_ignore_ascii_case("__construct"))
}

/// One INJECTS `UnresolvedRef` per distinct class-typed constructor parameter,
/// from the consumer class to the dependency's bare type name. Covers plain
/// type-hinted parameters (Laravel) and PHP 8 promoted properties
/// (`private readonly UserRepository $users`, Symfony). Scalars, builtins and
/// union / intersection types go through [`class_type_name`] and emit nothing;
/// `variadic_parameter` is excluded — a variadic is a list, not one service.
/// The graph crate binds the bare name to the uniquely-named class/interface.
fn emit_ctor_injects(ctor: TsNode, src: &[u8], class_id: NodeId, module_id: NodeId, acc: &mut Acc) {
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return;
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        if !matches!(
            p.kind(),
            "simple_parameter" | "property_promotion_parameter"
        ) {
            continue;
        }
        let Some(ty) = p.child_by_field_name("type") else {
            continue;
        };
        let Some(name) = class_type_name(text_of(ty, src)) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        acc.refs.push(UnresolvedRef {
            from: class_id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(name),
            category: edge_category::INJECTS,
            line: line_at(ty),
        });
        di_stats::record(DiShape::PhpCtor);
    }
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
                        line: line_of(source, search_from + rel),
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
        cells: Vec::new(),
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

/// Record one `use` statement as imported by the FILE module. PHP scopes a
/// `use` inside a braced `namespace X { }` to that block, but glia's import
/// bindings are per file, and the graph resolves `from_module` against MODULE
/// nodes only: the namespace PACKAGE would drop the statement (and every call
/// binding through it). Two blocks of one file binding the same name: the later
/// `use` wins, in source order (LA.40b).
fn collect_use(node: TsNode, src: &[u8], acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let path = text.trim_start_matches("use ").trim_end_matches(';').trim();

    if let Some(last_bs) = path.rfind('\\') {
        let module_part = &path[..last_bs];
        let name = &path[last_bs + 1..];
        acc.imports.push(ImportStmt {
            from_module: acc.module_qname.clone(),
            target: ImportTarget::Symbol {
                module: module_part.replace('\\', "::"),
                name: name.to_string(),
                alias: None,
                level: 0,
            },
            line: line_at(node),
        });
    } else {
        acc.imports.push(ImportStmt {
            from_module: acc.module_qname.clone(),
            target: ImportTarget::Module {
                path: path.replace('\\', "::"),
                alias: None,
            },
            line: line_at(node),
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
    repo: RepoId,
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
                        line: line_at(n),
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
                        line: line_at(n),
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
                        line: line_at(n),
                    });
                } else {
                    acc.calls.push(CallSite {
                        from,
                        qualifier: CallQualifier::Attribute {
                            base: obj.to_string(),
                            name: name.to_string(),
                        },
                        line: line_at(n),
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
                    line: line_at(n),
                });
                eloquent_query_site(n, src, from, repo, acc);
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
// Laravel Eloquent -> DATA_ENTITY (A13.14)
// ============================================================================
//
// Identity follows A13.1's ORM rule (`code_domain::data_entity`): a model is
// keyed on its CLASS name, `data_entity:sql:<Model>`, because that is the one
// token every query site in any file can name (`User::where(...)`). The table
// a model DECLARES (`protected $table = 'app_users'`) rides a table cell,
// emitted only at the declaration site; the graph builder stacks it onto the
// one node the query sites share, and `DbResolver` joins on it.
//
// DECLARATION. A non-abstract class whose base's last `\` segment is `Model`
// (`extends Model`, `extends \Illuminate\Database\Eloquent\Model`), or whose
// base resolves through the file's `use` map to another Eloquent base (the
// default Laravel `User extends Authenticatable`, pivots), is a model: it
// DEFINES its entity. No table cell without a `$table` literal — Laravel's
// snake_case-plural default is the resolver's fold, never hand-rolled here.
//
// QUERY SITE. A static builder call on a model (`User::where|find|all|...`)
// emits ACCESSES_DATA from the enclosing method to the model's entity. The
// receiver must resolve by PHP's own name rules (`use` alias, leading `\`,
// else the current namespace) into a `\Models\` namespace — the Laravel
// convention, which keeps `Carbon::create` and `Str::of` out — or name a
// same-file model class. An UNIMPORTED receiver that only reaches `\Models\`
// through the implicit current namespace must also not be a facade name.
// `DB::table('x')` names its table directly, so it is table-keyed.
//
// Namespaces are tracked per scope (semicolon form runs to the next
// `namespace` statement, brace form is its body), so a multi-namespace file
// resolves each site against its own `use` map.

/// Base classes a model may extend, beyond any `…\Model`, as FQCNs.
const ELOQUENT_BASES: &[&str] = &[
    "Illuminate\\Database\\Eloquent\\Model",
    "Illuminate\\Foundation\\Auth\\User",
    "Illuminate\\Database\\Eloquent\\Relations\\Pivot",
    "Illuminate\\Database\\Eloquent\\Relations\\MorphPivot",
];

/// Static Eloquent entry points that run (or start) a query on the model's
/// table. Compared case-insensitively: PHP method names are. Excludes the
/// statics that never touch the table (`factory`, `observe`, `make`, `boot`).
const ELOQUENT_QUERY_METHODS: &[&str] = &[
    "where", "whereIn", "whereNotIn", "whereNull", "whereNotNull", "whereBetween", "whereHas",
    "whereKey", "orWhere", "firstWhere", "find", "findOrFail", "findMany", "findOrNew", "first",
    "firstOrFail", "firstOrCreate", "firstOrNew", "updateOrCreate", "all", "get", "create",
    "forceCreate", "insert", "upsert", "destroy", "query", "with", "withCount", "withTrashed",
    "onlyTrashed", "has", "doesntHave", "select", "orderBy", "latest", "oldest", "paginate",
    "simplePaginate", "cursorPaginate", "count", "exists", "pluck", "chunk", "cursor", "sum",
    "max", "min", "avg",
];

/// Laravel facades: a query-shaped name on one of these (`Session::all()`,
/// `Schema::create(...)`) is never a model query.
const LARAVEL_FACADES: &[&str] = &[
    "Route", "DB", "Cache", "Log", "Auth", "Config", "Storage", "Http", "Mail", "Queue", "Event",
    "Gate", "Session", "Schema", "Validator",
];

/// Receivers of `::table('x')` that open a query on a named table.
const DB_TABLE_RECEIVERS: &[&str] = &[
    "Illuminate\\Support\\Facades\\DB",
    "DB",
    "Illuminate\\Database\\Capsule\\Manager",
];

/// One PHP namespace scope: its byte range, name and class `use` map.
struct PhpScope {
    start: usize,
    end: usize,
    /// `App\Http`, or empty for the global namespace.
    namespace: String,
    /// Lowercased alias -> FQCN without a leading `\`.
    uses: HashMap<String, String>,
    /// Lowercased simple names of the Eloquent models declared in this scope.
    models: HashSet<String>,
}

impl PhpScope {
    fn new(start: usize, end: usize, namespace: String) -> Self {
        Self {
            start,
            end,
            namespace,
            uses: HashMap::new(),
            models: HashSet::new(),
        }
    }

    /// Resolve a class reference as PHP does: a leading `\` is absolute, the
    /// first segment of anything else goes through the `use` map, and an
    /// unimported name lives in the current namespace.
    fn resolve(&self, written: &str) -> String {
        let w = written.trim();
        if let Some(abs) = w.strip_prefix('\\') {
            return abs.to_string();
        }
        if let Some(rel) = w.strip_prefix("namespace\\") {
            return self.qualify(rel);
        }
        let (first, rest) = match w.split_once('\\') {
            Some((f, r)) => (f, Some(r)),
            None => (w, None),
        };
        match (self.uses.get(&first.to_ascii_lowercase()), rest) {
            (Some(fq), Some(r)) => format!("{fq}\\{r}"),
            (Some(fq), None) => fq.clone(),
            (None, _) => self.qualify(w),
        }
    }

    /// True when `written` names its class explicitly — absolute, qualified,
    /// or imported — rather than falling into the current namespace.
    fn is_explicit(&self, written: &str) -> bool {
        let w = written.trim();
        w.contains('\\') || self.uses.contains_key(&w.to_ascii_lowercase())
    }

    fn qualify(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}\\{name}", self.namespace)
        }
    }

    /// Record every class alias one `namespace_use_declaration` introduces.
    /// Function / const imports (`use function …`) name no class and are skipped.
    fn add_uses(&mut self, decl: TsNode, src: &[u8]) {
        if decl.child_by_field_name("type").is_some() {
            return;
        }
        let mut cursor = decl.walk();
        let kids: Vec<TsNode> = decl.named_children(&mut cursor).collect();
        // Group form `use App\Models\{User, Post as P};` puts the shared prefix
        // in a `namespace_name` beside the `namespace_use_group` body.
        let prefix = kids
            .iter()
            .find(|k| k.kind() == "namespace_name")
            .map(|p| text_of(*p, src).trim().trim_start_matches('\\').to_string());
        for kid in &kids {
            match kid.kind() {
                "namespace_use_clause" => self.add_clause(*kid, src, None),
                "namespace_use_group" => {
                    let mut gc = kid.walk();
                    for clause in kid.named_children(&mut gc) {
                        if clause.kind() == "namespace_use_clause" {
                            self.add_clause(clause, src, prefix.as_deref());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn add_clause(&mut self, clause: TsNode, src: &[u8], prefix: Option<&str>) {
        if clause.child_by_field_name("type").is_some() {
            return;
        }
        let alias = clause.child_by_field_name("alias");
        let alias_id = alias.map(|a| a.id());
        let mut cursor = clause.walk();
        let Some(target) = clause
            .named_children(&mut cursor)
            .find(|c| matches!(c.kind(), "name" | "qualified_name") && Some(c.id()) != alias_id)
        else {
            return;
        };
        let path = text_of(target, src).trim().trim_start_matches('\\');
        let fqcn = match prefix {
            Some(p) if !p.is_empty() => format!("{p}\\{path}"),
            _ => path.to_string(),
        };
        let key = match alias {
            Some(a) => text_of(a, src).trim().to_string(),
            None => fqcn.rsplit('\\').next().unwrap_or(&fqcn).to_string(),
        };
        if !key.is_empty() && !fqcn.is_empty() {
            self.uses.insert(key.to_ascii_lowercase(), fqcn);
        }
    }
}

/// Per-file Eloquent state: the prescanned scopes and model classes, the
/// entities / access edges already pushed, and the `[orm-eloquent]` counters.
#[derive(Default)]
struct Eloquent {
    scopes: Vec<PhpScope>,
    /// tree-sitter ids of the `class_declaration`s that are Eloquent models.
    model_classes: HashSet<usize>,
    /// DATA_ENTITY ids pushed from this file, so each is pushed once.
    entities: HashSet<NodeId>,
    /// `(from, entity)` ACCESSES_DATA pairs pushed, one edge per pair.
    access_edges: HashSet<(NodeId, NodeId)>,
    models: usize,
    table_cells: usize,
    queries: usize,
}

impl Eloquent {
    /// Walk the file's top level (and brace-form namespace bodies) once for
    /// namespaces, `use` maps and model classes. Classes are classified after
    /// every `use` is known, so declaration order does not matter.
    fn prescan(root: TsNode, src: &[u8]) -> Self {
        let mut ctx = Self::default();
        ctx.scopes.push(PhpScope::new(0, usize::MAX, String::new()));
        let mut current = 0usize;
        let mut classes: Vec<(TsNode, usize)> = Vec::new();
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            match child.kind() {
                "namespace_definition" => {
                    let ns = child
                        .child_by_field_name("name")
                        .map(|n| text_of(n, src).trim().to_string())
                        .unwrap_or_default();
                    if let Some(body) = child.child_by_field_name("body") {
                        let idx = ctx.scopes.len();
                        ctx.scopes
                            .push(PhpScope::new(body.start_byte(), body.end_byte(), ns));
                        let mut bc = body.walk();
                        for item in body.named_children(&mut bc) {
                            ctx.prescan_item(item, src, idx, &mut classes);
                        }
                    } else {
                        // Semicolon form: the scope runs to the next `namespace`.
                        if current != 0 {
                            ctx.scopes[current].end = child.start_byte();
                        }
                        current = ctx.scopes.len();
                        ctx.scopes
                            .push(PhpScope::new(child.start_byte(), usize::MAX, ns));
                    }
                }
                _ => ctx.prescan_item(child, src, current, &mut classes),
            }
        }
        for (class, idx) in classes {
            let Some(scope) = ctx.scopes.get(idx) else {
                continue;
            };
            if !is_eloquent_model(class, src, scope) {
                continue;
            }
            let name = class
                .child_by_field_name("name")
                .map(|n| text_of(n, src).to_ascii_lowercase());
            ctx.model_classes.insert(class.id());
            if let (Some(name), Some(scope)) = (name, ctx.scopes.get_mut(idx)) {
                scope.models.insert(name);
            }
        }
        ctx
    }

    fn prescan_item<'a>(
        &mut self,
        item: TsNode<'a>,
        src: &[u8],
        idx: usize,
        classes: &mut Vec<(TsNode<'a>, usize)>,
    ) {
        match item.kind() {
            "namespace_use_declaration" => {
                if let Some(scope) = self.scopes.get_mut(idx) {
                    scope.add_uses(item, src);
                }
            }
            "class_declaration" => classes.push((item, idx)),
            _ => {}
        }
    }

    /// The innermost scope holding byte `pos`: brace bodies and later
    /// semicolon scopes are pushed after the global one, so the last hit wins.
    fn scope_at(&self, pos: usize) -> Option<&PhpScope> {
        self.scopes
            .iter()
            .rev()
            .find(|s| s.start <= pos && pos < s.end)
    }
}

/// A non-abstract class extending `…\Model` or a use-resolved Eloquent base.
fn is_eloquent_model(class: TsNode, src: &[u8], scope: &PhpScope) -> bool {
    let mut cursor = class.walk();
    let kids: Vec<TsNode> = class.named_children(&mut cursor).collect();
    if kids.iter().any(|k| k.kind() == "abstract_modifier") {
        return false; // a shared base model maps no table of its own
    }
    let Some(base) = kids.iter().find(|k| k.kind() == "base_clause") else {
        return false;
    };
    let mut bc = base.walk();
    base.named_children(&mut bc).any(|b| {
        let written = text_of(b, src).trim();
        let last = written.rsplit('\\').next().unwrap_or(written);
        let resolved = scope.resolve(written);
        last.eq_ignore_ascii_case("Model")
            || ELOQUENT_BASES.iter().any(|f| f.eq_ignore_ascii_case(&resolved))
    })
}

/// A plain (non-interpolating) string literal's text, else `None`.
fn php_literal_string(node: TsNode, src: &[u8]) -> Option<String> {
    match node.kind() {
        "string" => Some(php_string_inner(node, src)),
        "encapsed_string" => {
            let mut c = node.walk();
            let plain = node
                .named_children(&mut c)
                .all(|p| matches!(p.kind(), "string_content" | "escape_sequence"));
            plain.then(|| php_encapsed_template(node, src))
        }
        _ => None,
    }
}

/// The literal of the model's instance `$table` property, if it declares one.
fn eloquent_table_property(class: TsNode, src: &[u8]) -> Option<String> {
    let body = class.child_by_field_name("body")?;
    let mut cursor = body.walk();
    for decl in body.named_children(&mut cursor) {
        if decl.kind() != "property_declaration" {
            continue;
        }
        let mut dc = decl.walk();
        let kids: Vec<TsNode> = decl.named_children(&mut dc).collect();
        if kids.iter().any(|k| k.kind() == "static_modifier") {
            continue;
        }
        for el in kids.iter().filter(|k| k.kind() == "property_element") {
            let is_table = el
                .child_by_field_name("name")
                .is_some_and(|n| text_of(n, src) == "$table");
            if !is_table {
                continue;
            }
            let table = el
                .child_by_field_name("default_value")
                .and_then(|v| php_literal_string(v, src))?;
            let table = table.trim();
            return (!table.is_empty()).then(|| table.to_string());
        }
    }
    None
}

/// The model-keyed (or `DB::table`-keyed) DATA_ENTITY qname and id.
fn eloquent_entity(name: &str, repo: RepoId) -> (String, NodeId) {
    let qname = format!("data_entity:sql:{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
    (qname, id)
}

/// Declaration site: the model's entity, the class DEFINES edge, and the
/// `$table` override as a table cell. A query site earlier in the same file
/// may already have pushed the node, so the cell then joins that node.
fn emit_eloquent_model(
    class: TsNode,
    src: &[u8],
    name: &str,
    class_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let (qname, entity_id) = eloquent_entity(name, repo);
    let cell = eloquent_table_property(class, src)
        .map(|table| data_entity::table_cell(&table, data_entity::orm::ELOQUENT));
    acc.eloquent.models += 1;
    if cell.is_some() {
        acc.eloquent.table_cells += 1;
    }
    if acc.eloquent.entities.insert(entity_id) {
        acc.nodes.push(Node {
            id: entity_id,
            repo,
            confidence: Confidence::Strong,
            cells: cell.into_iter().collect(),
        });
    } else if let Some(existing) = acc.nodes.iter_mut().find(|n| n.id == entity_id) {
        existing.cells.extend(cell);
    }
    acc.nav
        .record(entity_id, name, &qname, node_kind::DATA_ENTITY, Some(class_id));
    acc.edges.push(Edge {
        from: class_id,
        to: entity_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
}

/// Query site: a `scoped_call_expression` inside the method / function `from`.
fn eloquent_query_site(call: TsNode, src: &[u8], from: NodeId, repo: RepoId, acc: &mut Acc) {
    let Some(receiver) = call.child_by_field_name("scope") else {
        return;
    };
    if !matches!(receiver.kind(), "name" | "qualified_name" | "relative_name") {
        return; // `static::` / `$class::` / expressions name no class
    }
    let Some(method) = call.child_by_field_name("name").filter(|n| n.kind() == "name") else {
        return;
    };
    let method = text_of(method, src);
    let written = text_of(receiver, src).trim();
    let Some(scope) = acc.eloquent.scope_at(call.start_byte()) else {
        return;
    };
    let target = if method.eq_ignore_ascii_case("table") {
        db_table_target(call, src, written, scope)
    } else if ELOQUENT_QUERY_METHODS
        .iter()
        .any(|m| m.eq_ignore_ascii_case(method))
    {
        eloquent_model_target(written, scope)
    } else {
        None
    };
    let Some(name) = target else {
        return;
    };
    let (qname, entity_id) = eloquent_entity(&name, repo);
    acc.eloquent.queries += 1;
    if acc.eloquent.entities.insert(entity_id) {
        acc.nodes.push(Node {
            id: entity_id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        acc.nav
            .record(entity_id, &name, &qname, node_kind::DATA_ENTITY, Some(from));
    }
    if acc.eloquent.access_edges.insert((from, entity_id)) {
        acc.edges.push(Edge {
            from,
            to: entity_id,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
    }
}

/// The model a static receiver names, when it passes the precision gate.
fn eloquent_model_target(written: &str, scope: &PhpScope) -> Option<String> {
    let fqcn = scope.resolve(written);
    let (namespace, model) = fqcn.rsplit_once('\\').unwrap_or(("", fqcn.as_str()));
    if model.is_empty() {
        return None;
    }
    let in_models_ns = format!("\\{namespace}\\").contains("\\Models\\");
    let facade = LARAVEL_FACADES.iter().any(|f| f.eq_ignore_ascii_case(model));
    if in_models_ns && (scope.is_explicit(written) || !facade) {
        return Some(model.to_string());
    }
    let same_file_model =
        !scope.is_explicit(written) && scope.models.contains(&written.to_ascii_lowercase());
    same_file_model.then(|| written.to_string())
}

/// `DB::table('users as u')` -> `users`, when the receiver is the DB facade.
fn db_table_target(call: TsNode, src: &[u8], written: &str, scope: &PhpScope) -> Option<String> {
    let fqcn = scope.resolve(written);
    if !DB_TABLE_RECEIVERS.iter().any(|r| r.eq_ignore_ascii_case(&fqcn)) {
        return None;
    }
    let arg = nth_arg(call.child_by_field_name("arguments"), 0).and_then(arg_expr)?;
    let literal = php_literal_string(arg, src)?;
    let table = literal.split_whitespace().next()?;
    let name_shaped = table.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    name_shaped.then(|| table.to_string())
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

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
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
            "App::Services::UserService",
            repo(),
        )
        .unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"validate"));
    }

    /// Every qname of one node kind in a parse, sorted.
    fn qnames_of(fp: &FileParse, kind: repo_graph_core::NodeKindId) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == kind)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    /// LB.7b, over the `php-dir-qnames` fixture's three sources: a top-level
    /// type under the statement-form `namespace X;` or in a namespace-less file
    /// is directory-scoped (no doubled file stem); the braced `namespace X { }`
    /// form keeps its namespace scope; top-level functions keep the file scope.
    #[test]
    fn types_are_dir_scoped_outside_braced_namespaces() {
        // Statement form: the declarations are root siblings of the
        // `namespace_definition`, so they hang off the file MODULE.
        let invoice = r#"<?php

namespace App\Billing;

class Invoice
{
    public function total()
    {
        return $this->round();
    }

    private function round()
    {
        return 1;
    }
}

interface Payable
{
    public function pay();
}
"#;
        let module = "src::Billing::Invoice";
        let fp = parse_file(invoice, "src/Billing/Invoice.php", module, repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::CLASS),
            vec!["src::Billing::Invoice"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::INTERFACE),
            vec!["src::Billing::Payable"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec![
                "src::Billing::Invoice::round",
                "src::Billing::Invoice::total"
            ]
        );
        assert!(
            !fp.nav
                .qname_by_id
                .values()
                .any(|q| q.contains("Invoice::Invoice") || q.contains("Invoice::Payable")),
            "no qname doubles the file stem: {:?}",
            fp.nav.qname_by_id.values().collect::<Vec<_>>()
        );
        // The MODULE keeps the full file qname, so the public class shares it
        // under a different kind and NodeId, and the module still DEFINES it.
        assert_eq!(qnames_of(&fp, node_kind::MODULE), vec![module]);
        assert_eq!(qnames_of(&fp, node_kind::PACKAGE), vec!["App::Billing"]);
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module);
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, module);
        assert_ne!(module_id, class_id);
        assert_eq!(fp.nav.parent_of.get(&class_id), Some(&module_id));
        assert!(fp.edges.iter().any(|e| e.from == module_id
            && e.to == class_id
            && e.category == edge_category::DEFINES));

        // Namespace-less file: the class is directory-scoped, the top-level
        // function keeps the file scope.
        let legacy = r#"<?php

class LegacyThing
{
    public function go()
    {
        return legacy_helper();
    }
}

function legacy_helper()
{
    return 1;
}
"#;
        let module = "src::Legacy::LegacyThing";
        let fp = parse_file(legacy, "src/Legacy/LegacyThing.php", module, repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::CLASS),
            vec!["src::Legacy::LegacyThing"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec!["src::Legacy::LegacyThing::go"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::FUNCTION),
            vec!["src::Legacy::LegacyThing::legacy_helper"]
        );

        // Braced form: unchanged, the namespace scopes the type and is its
        // nav parent.
        let report = r#"<?php

namespace App\Braced {
    class Report
    {
        public function build()
        {
            return $this->sum();
        }

        private function sum()
        {
            return 2;
        }
    }
}
"#;
        let fp = parse_file(
            report,
            "src/Braced/Report.php",
            "src::Braced::Report",
            repo(),
        )
        .unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::CLASS),
            vec!["App::Braced::Report"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec!["App::Braced::Report::build", "App::Braced::Report::sum"]
        );
        let class_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "App::Braced::Report");
        let pkg_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::PACKAGE, "App::Braced");
        assert_eq!(fp.nav.parent_of.get(&class_id), Some(&pkg_id));

        // A file at the repo root has an empty directory scope: the bare name.
        let fp = parse_file(
            "<?php\nenum Kernel { case Boot; }\n",
            "Kernel.php",
            "Kernel",
            repo(),
        )
        .unwrap();
        assert_eq!(qnames_of(&fp, node_kind::ENUM), vec!["Kernel"]);
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
        let fp = parse_file(source, "src/Types.php", "App::Types", repo()).unwrap();
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
        let fp = parse_file(source, "src/Controller.php", "App::Controller", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
        assert!(
            fp.imports
                .iter()
                .all(|i| i.from_module == "App::Controller"),
            "statement-form use belongs to the file module: {:?}",
            fp.imports
        );
    }

    /// LA.40b: a `use` inside a braced namespace body is recorded with the FILE
    /// module as from_module — the namespace PACKAGE is no MODULE, so the graph
    /// would drop the statement.
    #[test]
    fn use_inside_braced_namespace_belongs_to_the_file() {
        let source = r#"<?php
namespace App\Http\Controllers {
    use App\Models\User;
    class C {}
}
"#;
        let module = "app::Http::Controllers::UserController";
        let fp = parse_file(
            source,
            "app/Http/Controllers/UserController.php",
            module,
            repo(),
        )
        .unwrap();
        assert_eq!(
            fp.imports,
            vec![ImportStmt {
                from_module: module.to_string(),
                target: ImportTarget::Symbol {
                    module: "App::Models".to_string(),
                    name: "User".to_string(),
                    alias: None,
                    level: 0,
                },
                // LC.3b: the `use` statement's 0-based row.
                line: 2,
            }]
        );
    }

    #[test]
    fn two_braced_blocks_in_one_file_both_bind_to_the_file() {
        let source = r#"<?php
namespace App\One {
    use App\Models\User;
    class A {}
}
namespace App\Two {
    use App\Models\Post;
    use Vendor;
    class B {}
}
"#;
        let module = "src::Mixed";
        let fp = parse_file(source, "src/Mixed.php", module, repo()).unwrap();
        assert_eq!(fp.imports.len(), 3, "{:?}", fp.imports);
        assert!(
            fp.imports.iter().all(|i| i.from_module == module),
            "every braced block's use binds to the file: {:?}",
            fp.imports
        );
        let names: Vec<&str> = fp
            .imports
            .iter()
            .map(|i| match &i.target {
                ImportTarget::Symbol { name, .. } => name.as_str(),
                ImportTarget::Module { path, .. } => path.as_str(),
            })
            .collect();
        assert_eq!(names, ["User", "Post", "Vendor"]);
        // The types stay namespace-scoped (LB.7b's braced rule is untouched).
        let qnames: Vec<&str> = fp.nav.qname_by_id.values().map(String::as_str).collect();
        assert!(qnames.contains(&"App::One::A"), "{qnames:?}");
        assert!(qnames.contains(&"App::Two::B"), "{qnames:?}");
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
        let fp = parse_file(
            source,
            "src/UserController.php",
            "App::Controller::UserController",
            repo(),
        )
        .unwrap();
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
        let fp = parse_file(source, "src/C.php", "App::C", repo()).unwrap();
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
        let fp = parse_file(
            source,
            "src/UserController.php",
            "App::Controller::UserController",
            repo(),
        )
        .unwrap();
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
        let fp = parse_file(
            source,
            "src/UserController.php",
            "App::UserController",
            repo(),
        )
        .unwrap();
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
        let fp = parse_file(source, "src/Svc.php", "App::Svc", repo()).unwrap();
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
        let fp = parse_file(source, "src/Svc.php", "App::Svc", repo()).unwrap();
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
        let fp = parse_file(source, "src/Service.php", "App::Service", repo()).unwrap();
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
        let fp = parse_file(
            source,
            "src/HomeController.php",
            "App::Http::HomeController",
            repo(),
        )
        .unwrap();
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
        let fp = parse_file(source, "src/f.php", "App::Http::f", repo()).unwrap();
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
        let fp = parse_file(source, "src/Service.php", "App::Service", repo()).unwrap();
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
        let fp = parse_file(source, "src/ApiClient.php", "App::ApiClient", repo()).unwrap();
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
        let fp = parse_file(source, "src/ApiClient.php", "App::ApiClient", repo()).unwrap();
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
        let fp = parse_file(source, "src/ApiClient.php", "App::ApiClient", repo()).unwrap();
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
        let fp = parse_file(source, "src/Cache.php", "App::Cache", repo()).unwrap();
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
        let fp = parse_file(source, "src/boot.php", "App::boot", repo()).unwrap();
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
        let fp = parse_file(source, "src/push.php", "App::push", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec!["PUT /api/users".to_string()],
            "host stripped by url_to_path; verb upgraded by the sibling CUSTOMREQUEST"
        );
    }

    // ========================================================================
    // Constructor injection (A7.5)
    // ========================================================================

    /// Every INJECTS ref's bare target name, sorted.
    fn injects_targets(fp: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn symfony_promoted_ctor_property_emits_injects_ref() {
        // Semicolon namespace (the PSR-4 norm): the class hangs off the file
        // MODULE, so that is the ref's scope. `int` and `?string` are builtins;
        // the nullable, fully-qualified logger is a class.
        let source = r#"<?php
namespace App\Controller;

use App\Repository\UserRepository;

class UserController
{
    public function __construct(
        private readonly UserRepository $users,
        private int $pageSize = 20,
        protected ?string $locale = null,
        private ?\Psr\Log\LoggerInterface $logger = null,
    ) {}

    public function show(int $id): array { return $this->users->find($id); }
}
"#;
        let module = "src::Controller::UserController";
        let fp = parse_file(source, "src/Controller/UserController.php", module, repo()).unwrap();
        assert_eq!(
            injects_targets(&fp),
            vec!["LoggerInterface", "UserRepository"]
        );
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module);
        // LB.7b: the class is directory-scoped, so it shares the file
        // MODULE's qname (no doubled `UserController::UserController`).
        let class_qname = "src::Controller::UserController";
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, class_qname);
        for r in fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
        {
            assert_eq!(
                r.from, class_id,
                "the consumer CLASS injects, not __construct"
            );
            assert_eq!(r.from_module, module_id);
        }
    }

    #[test]
    fn laravel_plain_ctor_type_hint_emits_injects_ref() {
        // Brace namespace: the class hangs off the namespace PACKAGE. Plain
        // (non-promoted) type hints; a repeated type is deduped, a union and a
        // variadic emit nothing, `__CONSTRUCT` still matches (PHP method names
        // are case-insensitive) and a non-constructor method's params do not count.
        let source = r#"<?php
namespace App\Jobs {
    class SendInvoiceJob
    {
        protected $mailer;

        public function __CONSTRUCT(\App\Services\Mailer $mailer, Mailer $again, Foo|Bar $either, Listener ...$rest)
        {
            $this->mailer = $mailer;
        }

        public function handle(InvoiceRepository $invoices): void {}
    }
}
"#;
        let fp = parse_file(
            source,
            "app/Jobs/SendInvoiceJob.php",
            "app::Jobs::SendInvoiceJob",
            repo(),
        )
        .unwrap();
        assert_eq!(injects_targets(&fp), vec!["Mailer"]);
        let pkg_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::PACKAGE, "App::Jobs");
        assert!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::INJECTS)
                .all(|r| r.from_module == pkg_id),
            "a brace-namespace class scopes its refs to the namespace PACKAGE"
        );
    }

    #[test]
    fn value_object_ctor_emits_no_injects() {
        // No service-role suffix and no class-level autoconfiguration
        // attribute: a value object's constructor is data, not dependencies.
        // A METHOD-level attribute must not open the gate for its class.
        let source = r#"<?php
namespace App\Domain;

class Money
{
    public function __construct(private Currency $c, private int $amount) {}
}

class Audit
{
    public function __construct(private Clock $clock) {}

    #[AsEventListener(event: 'kernel.request')]
    public function onRequest(): void {}
}
"#;
        let fp = parse_file(source, "src/Domain/Money.php", "src::Domain::Money", repo()).unwrap();
        assert!(
            injects_targets(&fp).is_empty(),
            "got {:?}",
            injects_targets(&fp)
        );
    }

    #[test]
    fn symfony_class_attribute_opens_the_di_gate() {
        // `SyncUsers` has no service suffix; a fully-qualified `#[AsCommand]`
        // declared ON the class, inside a grouped attribute list, autoconfigures it.
        let source = r#"<?php
namespace App\Console;

#[Deprecated, \Symfony\Component\Console\Attribute\AsCommand(name: 'app:sync-users')]
final class SyncUsers
{
    public function __construct(private UserRepository $users) {}
}
"#;
        let fp = parse_file(
            source,
            "src/Console/SyncUsers.php",
            "src::Console::SyncUsers",
            repo(),
        )
        .unwrap();
        assert_eq!(injects_targets(&fp), vec!["UserRepository"]);
    }

    // ========================================================================
    // Eloquent models and query sites (A13.14)
    // ========================================================================

    fn entity_id(name: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            &format!("data_entity:sql:{name}"),
        )
    }

    fn entity_names(fp: &FileParse) -> Vec<String> {
        let mut names: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::DATA_ENTITY)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).cloned())
            .collect();
        names.sort();
        names
    }

    fn has_edge(fp: &FileParse, from: NodeId, to: NodeId, category: u32) -> bool {
        fp.edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category.0 == category)
    }

    fn method_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, qname)
    }

    #[test]
    fn eloquent_table_property_used_as_entity_key() {
        // The declared table is the entity's JOIN key (DbResolver reads it back
        // through `table_of`); the node itself stays model-keyed so query sites
        // in other files converge on it.
        let source = r#"<?php
namespace App\Models;

use Illuminate\Database\Eloquent\Model;

class User extends Model
{
    protected $table = 'app_users';
}
"#;
        let fp = parse_file(source, "app/Models/User.php", "app::Models::User", repo()).unwrap();
        assert_eq!(entity_names(&fp), vec!["data_entity:sql:User"]);
        let entity = fp
            .nodes
            .iter()
            .find(|n| n.id == entity_id("User"))
            .expect("model-keyed entity node");
        assert_eq!(
            data_entity::table_of(&entity.cells),
            Some("app_users".to_string())
        );
        let CellPayload::Json(raw) = &entity.cells[0].payload else {
            panic!("table cell must be Json");
        };
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v["orm"], data_entity::orm::ELOQUENT);
        let class_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::CLASS,
            "app::Models::User",
        );
        assert!(has_edge(&fp, class_id, entity_id("User"), edge_category::DEFINES.0));
        assert!(
            !fp.nodes.iter().any(|n| n.id == entity_id("app_users")),
            "no table-keyed second id"
        );
    }

    #[test]
    fn eloquent_model_without_table_falls_back_to_class_name() {
        // No `$table`: no cell, so the join key is the qname tail (the class
        // name), folded by DbResolver — Laravel's plural is never hand-rolled.
        // `static $table` is not Eloquent's property and does not count.
        let source = r#"<?php
namespace App\Models;

class Post extends \Illuminate\Database\Eloquent\Model
{
    protected static $table = 'ignored';
    protected $fillable = ['title'];
}
"#;
        let fp = parse_file(source, "app/Models/Post.php", "app::Models::Post", repo()).unwrap();
        assert_eq!(entity_names(&fp), vec!["data_entity:sql:Post"]);
        let entity = fp.nodes.iter().find(|n| n.id == entity_id("Post")).unwrap();
        assert!(entity.cells.is_empty(), "got {:?}", entity.cells);
        assert_eq!(fp.nav.name_by_id.get(&entity_id("Post")).map(String::as_str), Some("Post"));
    }

    #[test]
    fn plain_php_class_emits_no_entity() {
        // A value object, a non-Eloquent base, and an ABSTRACT shared base model
        // (which maps no table of its own) all emit nothing.
        let source = r#"<?php
namespace App\Models;

use Illuminate\Database\Eloquent\Model;

class Money {}
class LoginForm extends FormModelBase {}
abstract class BaseModel extends Model {}
"#;
        let fp = parse_file(source, "app/Models/Misc.php", "app::Models::Misc", repo()).unwrap();
        assert!(entity_names(&fp).is_empty(), "got {:?}", entity_names(&fp));
        assert!(!fp.edges.iter().any(|e| e.category == edge_category::ACCESSES_DATA));
    }

    #[test]
    fn eloquent_auth_user_base_resolves_through_use_alias() {
        // Laravel's default `User extends Authenticatable`, where the alias names
        // `Illuminate\Foundation\Auth\User` — no `Model` segment is written.
        let source = r#"<?php
namespace App\Models;

use Illuminate\Foundation\Auth\User as Authenticatable;

class User extends Authenticatable
{
    protected $table = "members";
}
"#;
        let fp = parse_file(source, "app/Models/User.php", "app::Models::User", repo()).unwrap();
        assert_eq!(entity_names(&fp), vec!["data_entity:sql:User"]);
        let entity = fp.nodes.iter().find(|n| n.id == entity_id("User")).unwrap();
        assert_eq!(data_entity::table_of(&entity.cells), Some("members".to_string()));
    }

    #[test]
    fn eloquent_static_query_emits_accesses_data_from_the_enclosing_method() {
        // Plain import, aliased import, grouped import, absolute name, and a
        // query inside a closure all resolve into `\Models\` and anchor on the
        // enclosing method. Two queries on one model in one method: one edge.
        let source = r#"<?php
namespace App\Http;

use App\Models\User;
use App\Models\Billing\Invoice as Bill;
use App\Models\{Post, Tag as Label};

class ReportController
{
    public function active() {
        $n = User::where('active', 1)->count();
        return User::query()->latest()->get();
    }
    public function billing() { return Bill::findOrFail(1); }
    public function posts() {
        return collect([1])->map(function ($id) { return Post::find($id); });
    }
    public function labels() { return Label::all(); }
    public function orders() { return \App\Models\Order::paginate(10); }
}
"#;
        let fp = parse_file(
            source,
            "app/Http/ReportController.php",
            "app::Http::ReportController",
            repo(),
        )
        .unwrap();
        assert_eq!(
            entity_names(&fp),
            vec![
                "data_entity:sql:Invoice",
                "data_entity:sql:Order",
                "data_entity:sql:Post",
                "data_entity:sql:Tag",
                "data_entity:sql:User",
            ]
        );
        let m = |name: &str| method_id(&format!("app::Http::ReportController::{name}"));
        let ad = edge_category::ACCESSES_DATA.0;
        assert!(has_edge(&fp, m("active"), entity_id("User"), ad));
        assert!(has_edge(&fp, m("billing"), entity_id("Invoice"), ad));
        assert!(has_edge(&fp, m("posts"), entity_id("Post"), ad));
        assert!(has_edge(&fp, m("labels"), entity_id("Tag"), ad));
        assert!(has_edge(&fp, m("orders"), entity_id("Order"), ad));
        let access_edges = fp.edges.iter().filter(|e| e.category.0 == ad).count();
        assert_eq!(access_edges, 5, "one edge per (method, entity)");
        // Query sites carry no table cell: only the declaration site knows it.
        assert!(fp
            .nodes
            .iter()
            .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::DATA_ENTITY))
            .all(|n| n.cells.is_empty()));
        // The ordinary CALLS site is still pushed for the static call.
        assert!(fp.calls.iter().any(|c| matches!(
            &c.qualifier,
            CallQualifier::Attribute { base, name } if base == "User" && name == "where"
        )));
    }

    #[test]
    fn eloquent_query_precision_denies_facades_and_non_models() {
        // Query-shaped names on a non-`\Models\` class (Carbon), on imported
        // facades, on an UNIMPORTED facade name that only the implicit current
        // namespace would put in `\Models\`, on `static::` / `$cls::`, and a
        // non-query static on a real model (`factory`) all emit nothing.
        let source = r#"<?php
namespace App\Models;

use Carbon\Carbon;
use Illuminate\Support\Facades\Session;
use Illuminate\Support\Facades\Schema;

class Maintenance
{
    public function run($cls) {
        Carbon::create(2024, 1, 1);
        Session::all();
        Schema::create('users', fn ($t) => null);
        Cache::get('k');
        Event::with('x');
        static::find(1);
        $cls::find(1);
        Account::factory();
    }
}
"#;
        let fp = parse_file(source, "app/Models/Maintenance.php", "app::Models::Maintenance", repo())
            .unwrap();
        assert!(entity_names(&fp).is_empty(), "got {:?}", entity_names(&fp));
    }

    #[test]
    fn eloquent_same_file_model_outside_models_namespace() {
        // A model declared in the same file resolves even outside `\Models\`,
        // and the declaration's table cell joins the node the query pushed
        // first (the query class is declared before the model).
        let source = r#"<?php
namespace App;

use Illuminate\Database\Eloquent\Model;

class Reports
{
    public function recent() { return Flight::latest()->get(); }
}

class Flight extends Model
{
    protected $table = 'flights_v2';
}
"#;
        let fp = parse_file(source, "app/Reports.php", "app::Reports", repo()).unwrap();
        assert_eq!(entity_names(&fp), vec!["data_entity:sql:Flight"]);
        let nodes: Vec<&Node> = fp.nodes.iter().filter(|n| n.id == entity_id("Flight")).collect();
        assert_eq!(nodes.len(), 1, "one node per entity per file");
        assert_eq!(data_entity::table_of(&nodes[0].cells), Some("flights_v2".to_string()));
        assert!(has_edge(
            &fp,
            method_id("app::Reports::recent"),
            entity_id("Flight"),
            edge_category::ACCESSES_DATA.0
        ));
    }

    #[test]
    fn db_table_query_is_table_keyed() {
        // `DB::table('x')` names the table itself; an alias clause is dropped,
        // a non-literal argument and a non-DB `table()` receiver emit nothing.
        let source = r#"<?php
namespace App\Services;

use Illuminate\Support\Facades\DB;

class AuditService
{
    public function log() { DB::table('audit_log as a')->insert(['e' => 1]); }
    public function dynamic($t) { return DB::table($t)->get(); }
    public function other() { return Html::table('users'); }
}
"#;
        let fp = parse_file(source, "app/Services/AuditService.php", "app::Services::AuditService", repo())
            .unwrap();
        assert_eq!(entity_names(&fp), vec!["data_entity:sql:audit_log"]);
        assert!(has_edge(
            &fp,
            method_id("app::Services::AuditService::log"),
            entity_id("audit_log"),
            edge_category::ACCESSES_DATA.0
        ));
    }

    #[test]
    fn eloquent_scopes_follow_each_namespace_block() {
        // Brace-form namespaces: each block resolves against its OWN `use` map,
        // so the second block's `User` (a non-Models import) emits nothing.
        let source = r#"<?php
namespace App\Http {
    use App\Models\User;
    class A { public function a() { return User::all(); } }
}
namespace App\Legacy {
    use App\Legacy\User;
    class B { public function b() { return User::all(); } }
}
"#;
        let fp = parse_file(source, "app/Multi.php", "app::Multi", repo()).unwrap();
        assert_eq!(entity_names(&fp), vec!["data_entity:sql:User"]);
        let ad: Vec<&Edge> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::ACCESSES_DATA)
            .collect();
        assert_eq!(ad.len(), 1);
        assert_eq!(ad[0].from, method_id("App::Http::A::a"));
    }
}
