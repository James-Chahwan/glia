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
    let lang: tree_sitter::Language = tree_sitter_ruby::LANGUAGE.into();
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

    visit_body(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);

    if is_rails_routes_file(file_rel_path) {
        scan_rails_routes(root, src, module_id, repo, &mut acc);
    } else {
        scan_sinatra_routes(root, src, module_id, repo, &mut acc);
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
    /// DATA_ENTITY node ids already emitted this file (dedup, one node per model).
    data_entities: std::collections::HashSet<NodeId>,
    /// (accessor, entity) pairs already linked (dedup repeated queries).
    access_edges: std::collections::HashSet<(NodeId, NodeId)>,
}

fn visit_body(
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
            "class" => visit_class(child, src, file_rel, parent_qname, parent_id, repo, acc),
            "module" => visit_module(child, src, file_rel, parent_qname, parent_id, repo, acc),
            "method" | "singleton_method" => {
                visit_method(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "call" => {
                collect_require(child, src, parent_qname, acc);
                collect_call(child, src, parent_id, acc);
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
    acc.nav.record(id, name, &qname, node_kind::CLASS, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        visit_body(body, src, file_rel, &qname, id, repo, acc);
    }
}

fn visit_module(
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
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::CONTAINS,
        confidence: Confidence::Strong,
    });
    acc.nav
        .record(id, name, &qname, node_kind::PACKAGE, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        visit_body(body, src, file_rel, &qname, id, repo, acc);
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
    // A top-level `def` (parent is the file MODULE) is a free function, not a
    // class method — match the FUNCTION convention Python/Go/TS use for
    // module-level defs. Defs inside a class/module (PACKAGE) stay METHOD.
    let kind = if acc.nav.kind_by_id.get(&parent_id) == Some(&node_kind::MODULE) {
        node_kind::FUNCTION
    } else {
        node_kind::METHOD
    };
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

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, repo, acc);
    }
}

fn collect_require(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let method_name = node
        .child_by_field_name("method")
        .map(|n| text_of(n, src))
        .unwrap_or("");
    if method_name != "require" && method_name != "require_relative" {
        return;
    }
    let Some(args) = node.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if arg.kind() == "string" {
            let raw = text_of(arg, src);
            let path = raw.trim_matches(|c| c == '\'' || c == '"');
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Module {
                    path: path.to_string(),
                    alias: None,
                },
            });
        }
    }
}

fn collect_call(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let method_name = node
        .child_by_field_name("method")
        .map(|n| text_of(n, src))
        .unwrap_or("");
    if method_name.is_empty() {
        return;
    }
    if let Some(recv) = node.child_by_field_name("receiver") {
        let recv_text = text_of(recv, src);
        if recv_text == "self" {
            acc.calls.push(CallSite {
                from,
                qualifier: CallQualifier::SelfMethod(method_name.to_string()),
            });
        } else if recv.kind() == "identifier" || recv.kind() == "constant" {
            acc.calls.push(CallSite {
                from,
                qualifier: CallQualifier::Attribute {
                    base: recv_text.to_string(),
                    name: method_name.to_string(),
                },
            });
        }
    } else {
        acc.calls.push(CallSite {
            from,
            qualifier: CallQualifier::Bare(method_name.to_string()),
        });
    }
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, repo: RepoId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call" {
            collect_call(n, src, from, acc);
            try_emit_accesses_data(n, src, from, repo, acc);
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(child.kind(), "method" | "singleton_method" | "class" | "module") {
                stack.push(child);
            }
        }
    }
}

// ============================================================================
// ActiveRecord data-access extraction (ACCESSES_DATA)
// ============================================================================
//
// A query issued against an ActiveRecord model constant — `User.where(...)`,
// `Post.find(id)`, `Account.find_by(...)` — is a data access that the raw-SQL /
// mongoose scanners in the shared `data_entities` extractor never see (there is
// no SQL string, no ORM table decl). We surface it here: mint a DATA_ENTITY for
// the model and an ACCESSES_DATA edge from the enclosing accessor method to it.
//
// Precision comes from two gates together: the receiver must be a bare
// `constant` (models are constants) that is not a well-known stdlib/framework
// namespace, and the method must be an AR query entry point. This keeps
// `Time.now` / `Math.sqrt` / `JSON.parse` from minting spurious entities.

/// AR query entry points invoked on a model constant. Kept to finders / query
/// builders that unambiguously read or write the backing table.
const AR_QUERY_METHODS: &[&str] = &[
    "where",
    "find",
    "find_by",
    "find_by!",
    "find_each",
    "find_or_create_by",
    "find_or_initialize_by",
    "all",
    "first",
    "last",
    "pluck",
    "count",
    "exists?",
    "create",
    "create!",
    "update_all",
    "delete_all",
    "destroy_all",
];

/// Constants that are Ruby/Rails namespaces or stdlib classes, never AR models.
const NON_MODEL_CONSTANTS: &[&str] = &[
    "Rails",
    "ActiveRecord",
    "ApplicationRecord",
    "Time",
    "Date",
    "DateTime",
    "Math",
    "File",
    "Dir",
    "Kernel",
    "JSON",
    "Logger",
    "ENV",
    "String",
    "Array",
    "Hash",
    "Integer",
    "Float",
    "Struct",
    "Set",
    "Range",
];

/// If `call` is an ActiveRecord query on a model constant, emit a DATA_ENTITY
/// for the model (once per file) and an ACCESSES_DATA edge from `from` (the
/// enclosing accessor) to it.
fn try_emit_accesses_data(call: TsNode, src: &[u8], from: NodeId, repo: RepoId, acc: &mut Acc) {
    let Some(recv) = call.child_by_field_name("receiver") else {
        return;
    };
    if recv.kind() != "constant" {
        return;
    }
    let model = text_of(recv, src);
    if model.is_empty() || NON_MODEL_CONSTANTS.contains(&model) {
        return;
    }
    let method = call
        .child_by_field_name("method")
        .map(|n| text_of(n, src))
        .unwrap_or("");
    if !AR_QUERY_METHODS.contains(&method) {
        return;
    }

    let qname = format!("data_entity:sql:{model}");
    let entity_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
    if acc.data_entities.insert(entity_id) {
        acc.nodes.push(Node {
            id: entity_id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        acc.nav
            .record(entity_id, model, &qname, node_kind::DATA_ENTITY, Some(from));
    }
    if acc.access_edges.insert((from, entity_id)) {
        acc.edges.push(Edge {
            from,
            to: entity_id,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Medium,
        });
    }
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ============================================================================
// Rails route extraction (v0.4.11a R-ruby)
// ============================================================================
//
// Gated to `config/routes.rb` (or any file named routes.rb) to avoid
// false-positives on arbitrary `get`/`post` method calls elsewhere in the
// codebase. Inside the Rails router DSL we match:
//
//   get/post/put/patch/delete/match '/path'[, to: 'ctrl#act']
//   root 'ctrl#index'
//   resources :users      → emits ANY /users
//   resource :profile     → emits ANY /profile
//
// Routes are emitted in shape B — `<METHOD> <path>` qname + Text
// ROUTE_METHOD cell — the resolver compat shape that HttpStackResolver
// accepts uniformly across parser-java/csharp/php/rust/python/ruby.

fn is_rails_routes_file(rel_path: &str) -> bool {
    rel_path.ends_with("routes.rb") || rel_path.ends_with("/routes.rb")
}

fn scan_rails_routes(root: TsNode, src: &[u8], module_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "call" {
            try_emit_rails_route(n, src, module_id, repo, acc);
        }
        let mut cursor = n.walk();
        for c in n.named_children(&mut cursor) {
            stack.push(c);
        }
    }
}

fn try_emit_rails_route(call: TsNode, src: &[u8], module_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let method = call
        .child_by_field_name("method")
        .map(|n| text_of(n, src))
        .unwrap_or("");
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };

    let verb = match method {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "patch" => "PATCH",
        "delete" => "DELETE",
        "match" => "ANY",
        "root" => {
            // `root "home#index"` — first arg is the handler string; path "/".
            let mut cursor = args.walk();
            let handler = args
                .named_children(&mut cursor)
                .next()
                .filter(|n| n.kind() == "string")
                .map(|n| string_inner(n, src));
            let h = handler.as_deref().and_then(parse_handler);
            emit_rails_route("GET", "/", h, module_id, repo, acc);
            return;
        }
        "resources" | "resource" => {
            // `resources :users` — convention-mapped, no explicit handler string.
            let mut cursor = args.walk();
            let Some(first) = args.named_children(&mut cursor).next() else {
                return;
            };
            let name = text_of(first, src).trim_start_matches(':').trim();
            if name.is_empty() {
                return;
            }
            let path = format!("/{name}");
            emit_rails_route("ANY", &path, None, module_id, repo, acc);
            return;
        }
        _ => return,
    };

    // get/post/put/patch/delete/match: `<verb> "/path"[, to: "ctrl#act"]`
    // or the hash-rocket form `<verb> "/path" => "ctrl#act"`.
    let mut cursor = args.walk();
    let Some(first) = args.named_children(&mut cursor).next() else {
        return;
    };

    let (path, handler_spec): (Option<String>, Option<String>) = if first.kind() == "pair" {
        // Hash-rocket: key is the path string, value is the handler string.
        let path = first
            .child_by_field_name("key")
            .filter(|n| n.kind() == "string")
            .map(|n| string_inner(n, src));
        let handler = first
            .child_by_field_name("value")
            .filter(|n| n.kind() == "string")
            .map(|n| string_inner(n, src));
        (path, handler)
    } else if first.kind() == "string" {
        // Path string, optional trailing `to: "ctrl#act"` pair.
        (Some(string_inner(first, src)), find_to_handler(args, src))
    } else {
        return;
    };

    let Some(path) = path else { return };
    if path.is_empty() {
        return;
    }
    let handler = handler_spec.as_deref().and_then(parse_handler);
    emit_rails_route(verb, &path, handler, module_id, repo, acc);
}

/// Trim the surrounding quotes off a tree-sitter `string` node's text.
fn string_inner(n: TsNode, src: &[u8]) -> String {
    text_of(n, src)
        .trim_matches(|c| c == '\'' || c == '"')
        .to_string()
}

/// Scan an argument list for a `to: "ctrl#act"` (or `:to => "..."`) pair and
/// return the handler string value.
fn find_to_handler(args: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if arg.kind() != "pair" {
            continue;
        }
        let key_ok = arg
            .child_by_field_name("key")
            .map(|k| text_of(k, src).trim_matches(|c| c == ':' || c == ' ') == "to")
            .unwrap_or(false);
        if !key_ok {
            continue;
        }
        if let Some(val) = arg.child_by_field_name("value").filter(|n| n.kind() == "string") {
            return Some(string_inner(val, src));
        }
    }
    None
}

/// Parse a Rails handler spec `"controller#action"` into the controller class
/// name (`Attribute.base`) and action method name (`Attribute.name`). Applies
/// the Rails camelize + `Controller` suffix convention, e.g. `admin/users` →
/// `Admin::UsersController`.
fn parse_handler(spec: &str) -> Option<(String, String)> {
    let (controller, action) = spec.split_once('#')?;
    let controller = controller.trim();
    let action = action.trim();
    if controller.is_empty() || action.is_empty() {
        return None;
    }
    Some((controller_class_name(controller), action.to_string()))
}

fn controller_class_name(controller: &str) -> String {
    let parts: Vec<String> = controller.split('/').map(camelize_segment).collect();
    format!("{}Controller", parts.join("::"))
}

fn camelize_segment(seg: &str) -> String {
    seg.split('_')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

// ============================================================================
// Sinatra route extraction (any .rb file except routes.rb)
// ============================================================================
//
// Sinatra DSL: `get '/path' do ... end`, `post '/users' do ... end`. Same
// verbs as Rails, but always block-based and registered at top-level (classic
// app) or inside a `class App < Sinatra::Base` body (modular).
//
// Discriminating from arbitrary Ruby calls is a real concern — `cache.get(key)`
// and `get(:symbol)` share the syntactic shape. Three filters together:
//   1. No `receiver` field (DSL call, not method-on-object)
//   2. First arg is a string literal beginning with `/`
//   3. A trailing `do_block` / `block` exists (handler body)
//
// `namespace '/api' do ... end` (sinatra-namespace gem) prefix tracking is
// skipped — consistent with the existing Rails scanner which doesn't track
// `scope` / `namespace` either. Routes emit at their literal path.

const SINATRA_VERBS: &[(&str, &str)] = &[
    ("get", "GET"),
    ("post", "POST"),
    ("put", "PUT"),
    ("patch", "PATCH"),
    ("delete", "DELETE"),
    ("head", "HEAD"),
    ("options", "OPTIONS"),
    ("link", "LINK"),
    ("unlink", "UNLINK"),
];

fn scan_sinatra_routes(root: TsNode, src: &[u8], module_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "call" {
            try_emit_sinatra_route(n, src, module_id, repo, acc);
        }
        let mut cursor = n.walk();
        for c in n.named_children(&mut cursor) {
            stack.push(c);
        }
    }
}

fn try_emit_sinatra_route(call: TsNode, src: &[u8], module_id: NodeId, repo: RepoId, acc: &mut Acc) {
    // Filter 1: no explicit receiver (DSL call, not `obj.get(...)`).
    if call.child_by_field_name("receiver").is_some() {
        return;
    }
    let method = call
        .child_by_field_name("method")
        .map(|n| text_of(n, src))
        .unwrap_or("");
    let Some(verb) = SINATRA_VERBS
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, v)| *v)
    else {
        return;
    };
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let Some(first) = args.named_children(&mut cursor).next() else {
        return;
    };
    // Filter 2: first arg is a string literal starting with `/`.
    if first.kind() != "string" {
        return;
    }
    let raw = text_of(first, src);
    let path = raw.trim_matches(|c| c == '\'' || c == '"');
    if !path.starts_with('/') {
        return;
    }
    // Filter 3: a trailing block (handler) is present. Tree-sitter Ruby
    // surfaces this either as a `block:` field on the call or as a sibling
    // `do_block` / `block` named child immediately after the call.
    if !call_has_block(call) {
        return;
    }

    // Sinatra handlers are inline blocks, not named — no HANDLED_BY handler.
    emit_rails_route(verb, path, None, module_id, repo, acc);
}

fn call_has_block(call: TsNode) -> bool {
    if call.child_by_field_name("block").is_some() {
        return true;
    }
    // Fallback for grammars that attach the block as the last named child
    // rather than via a labelled field.
    let count = call.named_child_count();
    if count == 0 {
        return false;
    }
    let last = call.named_child((count - 1) as u32);
    matches!(last.map(|n| n.kind()), Some("do_block") | Some("block"))
}

fn emit_rails_route(
    method: &str,
    path: &str,
    handler: Option<(String, String)>,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
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

    // Route -> handler (HANDLED_BY). The handler is `controller#action`; the
    // graph's resolve_refs binds `Attribute { base, name }` against the class
    // method map (falling back to a unique global method named `action`).
    if let Some((base, name)) = handler {
        acc.refs.push(UnresolvedRef {
            from: route_id,
            from_module: module_id,
            qualifier: CallQualifier::Attribute { base, name },
            category: edge_category::HANDLED_BY,
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    #[test]
    fn classes_and_methods() {
        let source = r#"
class User
  def initialize(name)
    @name = name
  end

  def greet
    "Hello #{@name}"
  end
end
"#;
        let fp = parse_file(source, "app/models/user.rb", "app::models::user", repo()).unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"User"));
        assert!(names.contains(&"initialize"));
        assert!(names.contains(&"greet"));
    }

    #[test]
    fn modules() {
        let source = r#"
module Auth
  class Token
    def verify; end
  end
end
"#;
        let fp = parse_file(source, "lib/auth.rb", "lib::auth", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::PACKAGE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
    }

    #[test]
    fn require_imports() {
        let source = r#"
require 'json'
require_relative '../helpers/auth'
"#;
        let fp = parse_file(source, "app/service.rb", "app::service", repo()).unwrap();
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
    fn rails_verb_routes_emit() {
        let source = r#"
Rails.application.routes.draw do
  get '/users', to: 'users#index'
  post '/users', to: 'users#create'
  put '/users/:id', to: 'users#update'
  delete '/users/:id', to: 'users#destroy'
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("PUT", "/users/:id")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("DELETE", "/users/:id")));
    }

    fn has_handled_by(fp: &FileParse, base: &str, name: &str) -> bool {
        fp.refs.iter().any(|r| {
            r.category == edge_category::HANDLED_BY
                && matches!(
                    &r.qualifier,
                    CallQualifier::Attribute { base: b, name: n } if b == base && n == name
                )
        })
    }

    #[test]
    fn rails_hash_rocket_routes_emit_handled_by() {
        // The `get "path" => "ctrl#act"` form: route node + HANDLED_BY ref.
        let source = r#"
Rails.application.routes.draw do
  get "users" => "users#index"
  get "users/:id" => "users#show"
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(
            fp.nodes
                .iter()
                .any(|n| n.id == route_id("GET", "/users/:id"))
        );
        // HANDLED_BY refs to UsersController#index / #show.
        assert!(has_handled_by(&fp, "UsersController", "index"));
        assert!(has_handled_by(&fp, "UsersController", "show"));
        // The ref's `from` is the ROUTE node id.
        let index_ref = fp
            .refs
            .iter()
            .find(|r| {
                r.category == edge_category::HANDLED_BY
                    && matches!(&r.qualifier, CallQualifier::Attribute { name, .. } if name == "index")
            })
            .expect("index HANDLED_BY ref");
        assert_eq!(index_ref.from, route_id("GET", "/users"));
    }

    #[test]
    fn rails_to_option_route_emits_handled_by() {
        // The `get "/path", to: "ctrl#act"` form.
        let source = r#"
Rails.application.routes.draw do
  get "/users", to: "users#index"
  post "/admin/reports", to: "admin/reports#create"
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(has_handled_by(&fp, "UsersController", "index"));
        // Namespaced controller camelizes to Admin::ReportsController.
        assert!(has_handled_by(&fp, "Admin::ReportsController", "create"));
    }

    #[test]
    fn rails_root_emits_handled_by() {
        let source = r#"
Rails.application.routes.draw do
  root "home#index"
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/")));
        assert!(has_handled_by(&fp, "HomeController", "index"));
    }

    #[test]
    fn rails_resources_emit_no_handled_by() {
        // `resources` has no explicit handler string — route only, no ref.
        let source = r#"
Rails.application.routes.draw do
  resources :posts
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/posts")));
        assert!(
            !fp.refs
                .iter()
                .any(|r| r.category == edge_category::HANDLED_BY)
        );
    }

    #[test]
    fn rails_resources_and_root_emit() {
        let source = r#"
Rails.application.routes.draw do
  resources :posts
  resource :profile
  root 'home#index'
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/posts")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/profile")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/")));
    }

    #[test]
    fn routes_not_extracted_outside_routes_file() {
        // `get` used as hash accessor / method name elsewhere shouldn't emit routes.
        let source = r#"
class UsersController
  def get(key)
    @cache.get(key)
  end
end
"#;
        let fp = parse_file(source, "app/controllers/users.rb", "app::controllers::users", repo())
            .unwrap();
        let has_route = fp
            .nav
            .kind_by_id
            .values()
            .any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "non-routes.rb file should not emit ROUTE nodes");
    }

    // ========================================================================
    // ActiveRecord data-access extraction (ACCESSES_DATA)
    // ========================================================================

    fn data_entity_id(model: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            &format!("data_entity:sql:{model}"),
        )
    }

    #[test]
    fn activerecord_query_emits_accesses_data() {
        // The substrate-gap fixture: a controller action querying a model.
        let source = r#"
class ReportsController < ApplicationController
  def active
    @users = User.where(active: true)
    render json: @users
  end
end
"#;
        let fp = parse_file(
            source,
            "app/controllers/reports_controller.rb",
            "app::controllers::reports_controller",
            repo(),
        )
        .unwrap();

        // DATA_ENTITY node for the User model.
        let entity_id = data_entity_id("User");
        assert!(
            fp.nodes.iter().any(|n| n.id == entity_id),
            "expected a DATA_ENTITY node for the User model"
        );
        assert_eq!(
            fp.nav.kind_by_id.get(&entity_id).copied(),
            Some(node_kind::DATA_ENTITY)
        );

        // ACCESSES_DATA edge from the `active` accessor method to that entity.
        let method_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "app::controllers::reports_controller::ReportsController::active",
        );
        assert!(
            fp.edges.iter().any(|e| e.from == method_id
                && e.to == entity_id
                && e.category == edge_category::ACCESSES_DATA),
            "expected ACCESSES_DATA edge from `active` to the User entity"
        );
    }

    #[test]
    fn activerecord_query_deduped_per_model() {
        // Two queries against the same model → one DATA_ENTITY, one edge.
        let source = r#"
class UsersController < ApplicationController
  def index
    @active = User.where(active: true)
    @all = User.all
  end
end
"#;
        let fp = parse_file(
            source,
            "app/controllers/users_controller.rb",
            "app::controllers::users_controller",
            repo(),
        )
        .unwrap();
        let entity_id = data_entity_id("User");
        assert_eq!(
            fp.nodes.iter().filter(|n| n.id == entity_id).count(),
            1,
            "DATA_ENTITY node should be emitted once per model"
        );
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.to == entity_id && e.category == edge_category::ACCESSES_DATA)
                .count(),
            1,
            "one accessor → one ACCESSES_DATA edge even across repeated queries"
        );
    }

    #[test]
    fn non_model_constant_calls_do_not_emit_accesses_data() {
        // `Time.now`, `JSON.parse`, `Math.sqrt` are constant calls but not AR
        // queries — no DATA_ENTITY / ACCESSES_DATA should be minted.
        let source = r#"
class Report
  def build
    t = Time.now
    payload = JSON.parse(body)
    Math.sqrt(4)
  end
end
"#;
        let fp = parse_file(source, "app/models/report.rb", "app::models::report", repo()).unwrap();
        assert!(
            !fp.nav
                .kind_by_id
                .values()
                .any(|k| *k == node_kind::DATA_ENTITY),
            "stdlib/namespace constant calls must not mint DATA_ENTITY nodes"
        );
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.category == edge_category::ACCESSES_DATA),
            "no ACCESSES_DATA edges for non-model constant calls"
        );
    }

    // ========================================================================
    // Sinatra route extraction
    // ========================================================================

    #[test]
    fn sinatra_classic_top_level_routes_emit() {
        let source = r#"
require 'sinatra'

get '/health' do
  'ok'
end

post '/users' do
  'created'
end

delete '/users/:id' do
  'gone'
end
"#;
        let fp = parse_file(source, "app.rb", "app", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/health")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(
            fp.nodes
                .iter()
                .any(|n| n.id == route_id("DELETE", "/users/:id"))
        );
    }

    #[test]
    fn sinatra_modular_routes_inside_class_emit() {
        let source = r#"
require 'sinatra/base'

class App < Sinatra::Base
  get '/ping' do
    'pong'
  end

  put '/items/:id' do
    'updated'
  end
end
"#;
        let fp = parse_file(source, "lib/app.rb", "lib::app", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/ping")));
        assert!(
            fp.nodes
                .iter()
                .any(|n| n.id == route_id("PUT", "/items/:id"))
        );
    }

    #[test]
    fn sinatra_skips_call_with_explicit_receiver() {
        // `cache.get('/users')` looks like a Sinatra route syntactically but
        // has a receiver; the no-receiver filter must skip it.
        let source = r#"
def lookup
  cache.get('/users') do |row|
    row.name
  end
end
"#;
        let fp = parse_file(source, "lib/svc.rb", "lib::svc", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "calls with receivers must not emit Sinatra routes");
    }

    #[test]
    fn sinatra_skips_non_path_string_first_arg() {
        // `get('some-key') do ... end` — string arg, no leading slash → not a route.
        let source = r#"
get 'some-key' do
  'value'
end
"#;
        let fp = parse_file(source, "lib/store.rb", "lib::store", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "non-`/` first arg must not emit a Sinatra route");
    }

    #[test]
    fn sinatra_skips_symbol_first_arg() {
        // `get :user_id` — symbol, not a path string. Common in DSLs.
        let source = r#"
get :user_id do
  42
end
"#;
        let fp = parse_file(source, "lib/dsl.rb", "lib::dsl", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "symbol first arg must not emit a Sinatra route");
    }

    #[test]
    fn sinatra_skips_call_without_block() {
        // `get '/path'` with no trailing block — can't be a Sinatra registration
        // (always block-based). Could be e.g. a call to a helper that returns
        // the GET response for a path. Suppressing avoids false positives.
        let source = r#"
def fetch
  get '/path'
end
"#;
        let fp = parse_file(source, "lib/client.rb", "lib::client", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "call without trailing block must not emit a route");
    }
}
