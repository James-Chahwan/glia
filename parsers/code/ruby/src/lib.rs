use glia_code_domain::data_entity;
use glia_code_domain::endpoint::{
    ClientEndpoint, abs_path, join_path, push_client_endpoint, url_to_path,
};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
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

    let mut acc = Acc {
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
    acc.nav
        .record(module_id, module_simple, module_qname, node_kind::MODULE, None);

    visit_body(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    flush_ivar_types(&mut acc);

    if is_rails_routes_file(file_rel_path) {
        scan_rails_routes(root, src, file_rel_path, module_id, repo, &mut acc);
    } else {
        scan_sinatra_routes(root, src, module_id, repo, &mut acc);
    }

    if acc.endpoint_hits > 0 {
        eprintln!(
            "[ruby-http-client] {} endpoints in {}",
            acc.endpoint_hits, file_rel_path
        );
    }
    if acc.ar_models > 0 {
        eprintln!(
            "[orm-ar] models={} table_cells={} in {}",
            acc.ar_models, acc.ar_table_cells, file_rel_path
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
    /// The file MODULE's qname, set once by `parse_file`. Every `require` /
    /// `require_relative` is the file's import wherever it sits (file level,
    /// class body, module body), so [`collect_require`] records it from here,
    /// never from the enclosing CLASS / PACKAGE qname, which no MODULE carries
    /// and `graph/src/imports.rs` would drop (CB.16, the LA.40 class).
    module_qname: String,
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
    /// Dedups the ENDPOINT node per `(method, path)` within a file.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Client HTTP call sites emitted in this file (drives the fired_on marker).
    endpoint_hits: usize,
    /// ActiveRecord model declarations seen in this file (`[orm-ar]` marker).
    ar_models: usize,
    /// Of those, the ones whose `self.table_name = "…"` put a table cell on
    /// the entity (`[orm-ar]` marker).
    ar_table_cells: usize,
    /// LA.23b — constructor-typed instance variables per owning CLASS:
    /// `"@repo"` -> `Some("UserRepo")`, or `None` once two assignments in the
    /// file disagree. Flushed into `nav.field_types` by [`flush_ivar_types`]
    /// after the whole file is visited, so a class reopened later in the same
    /// file still sees every writer before anything is recorded.
    ivar_types:
        std::collections::HashMap<NodeId, std::collections::HashMap<String, Option<String>>>,
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
                collect_require(child, src, acc);
                collect_call(child, src, parent_id, false, acc);
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
        cells: Vec::new(),
    });
    acc.nav.record(id, name, &qname, node_kind::CLASS, Some(parent_id));

    emit_ar_model_entity(node, name, src, id, repo, acc);

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
        cells: Vec::new(),
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
        cells: Vec::new(),
    });
    acc.nav.record(id, name, &qname, kind, Some(parent_id));

    // LA.23b: `@x` inside an instance method (`def m`) is an instance
    // variable of the receiver. Inside `def self.m` it is the CLASS object's
    // own ivar, a different variable that shares the spelling, so singleton
    // methods neither type ivars nor emit ivar call sites.
    let instance_method = node.kind() == "method";
    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, repo, instance_method, acc);
        let hits = collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
        acc.endpoint_hits += hits;
        // Only a CLASS owns instance variables: a `module Foo` method's ivars
        // belong to whatever class mixes it in, and a top-level `def` has no
        // class at all.
        if instance_method && acc.nav.kind_by_id.get(&parent_id) == Some(&node_kind::CLASS) {
            collect_ivar_types(node, body, src, parent_id, acc);
        }
    }
}

// ============================================================================
// Receiver types for `@ivar.m()` (LA.23b)
// ============================================================================
//
// `@repo.find(id)` is emitted as `ComplexReceiver { receiver: "@repo", .. }`
// and the ivar's constructor type is recorded under the same `"@repo"` key in
// `CodeNav::field_types`, so the graph crate's receiver-type pass (A6.2a)
// binds `find` on `UserRepo`. Three writers type an ivar, in any instance
// method of a class (memoisation is idiomatic anywhere, not only in
// `initialize`):
//
//   @repo = UserRepo.new            @repo = Repos::UserRepo.new  (-> UserRepo)
//   @log ||= AuditLog.new
//   def initialize(repo: UserRepo.new) / (repo = UserRepo.new); @repo = repo
//
// Anything else (`@repo = build_repo`, `@repo = nil`) is deliberately untyped
// and does not count as a writer. An ivar two writers give different types is
// recorded as nothing.

/// Record the constructor-typed ivar assignments of one instance method of
/// `class_id` into `acc.ivar_types`. Walks the method body, blocks included,
/// but not nested `def` / `class` / `module` / `class << self` bodies.
fn collect_ivar_types(method: TsNode, body: TsNode, src: &[u8], class_id: NodeId, acc: &mut Acc) {
    let param_types = param_default_types(method, src);
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if let Some((field, ty)) = ivar_assignment_type(n, src, &param_types) {
            note_ivar_type(acc, class_id, field, ty);
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "method" | "singleton_method" | "class" | "module" | "singleton_class"
            ) {
                stack.push(child);
            }
        }
    }
}

/// `(ivar, type)` when `n` is `@x = X.new(...)`, `@x ||= X.new(...)`, or
/// `@x = p` / `@x ||= p` for a parameter `p` whose default is `X.new(...)`.
fn ivar_assignment_type(
    n: TsNode,
    src: &[u8],
    param_types: &std::collections::HashMap<String, String>,
) -> Option<(String, String)> {
    match n.kind() {
        "assignment" => {}
        "operator_assignment" => {
            let op = n.child_by_field_name("operator")?;
            if text_of(op, src) != "||=" {
                return None;
            }
        }
        _ => return None,
    }
    let left = n.child_by_field_name("left")?;
    if left.kind() != "instance_variable" {
        return None;
    }
    let right = n.child_by_field_name("right")?;
    let ty = match right.kind() {
        "identifier" => param_types.get(text_of(right, src)).cloned(),
        _ => ruby_new_type(right, src),
    }?;
    Some((text_of(left, src).to_string(), ty))
}

/// Parameter name -> constructor type, for the keyword (`repo: UserRepo.new`)
/// and optional (`repo = UserRepo.new`) parameters of `method` whose default
/// is a constructor call.
fn param_default_types(method: TsNode, src: &[u8]) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(params) = method.child_by_field_name("parameters") else {
        return out;
    };
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        if !matches!(p.kind(), "keyword_parameter" | "optional_parameter") {
            continue;
        }
        let (Some(name), Some(value)) = (
            p.child_by_field_name("name"),
            p.child_by_field_name("value"),
        ) else {
            continue;
        };
        if let Some(ty) = ruby_new_type(value, src) {
            out.insert(text_of(name, src).to_string(), ty);
        }
    }
    out
}

/// The class a constructor call builds: `X.new(...)` -> `X`, and
/// `A::B::X.new` / `::X.new` -> `X` (the last `scope_resolution` segment,
/// since the graph binds a type by its bare name). Anything else -> None.
fn ruby_new_type(rhs: TsNode, src: &[u8]) -> Option<String> {
    if rhs.kind() != "call" {
        return None;
    }
    let method = rhs.child_by_field_name("method")?;
    if text_of(method, src) != "new" {
        return None;
    }
    let recv = rhs.child_by_field_name("receiver")?;
    let name = match recv.kind() {
        "constant" => recv,
        "scope_resolution" => recv.child_by_field_name("name")?,
        _ => return None,
    };
    let name = text_of(name, src);
    (!name.is_empty()).then(|| name.to_string())
}

/// First writer types the ivar; a writer with a different type makes it
/// ambiguous for good (`None`), whatever comes after.
fn note_ivar_type(acc: &mut Acc, class_id: NodeId, field: String, ty: String) {
    let slot = acc.ivar_types.entry(class_id).or_default().entry(field);
    match slot {
        std::collections::hash_map::Entry::Vacant(v) => {
            v.insert(Some(ty));
        }
        std::collections::hash_map::Entry::Occupied(mut o) => {
            if o.get().as_deref() != Some(ty.as_str()) {
                o.insert(None);
            }
        }
    }
}

/// Write every unambiguous ivar type into `nav.field_types` (A6.2a's carrier).
fn flush_ivar_types(acc: &mut Acc) {
    for (owner, fields) in std::mem::take(&mut acc.ivar_types) {
        for (field, ty) in fields {
            if let Some(ty) = ty {
                acc.nav.record_field_type(owner, &field, &ty);
            }
        }
    }
}

/// Record a `require` / `require_relative` call as an import of the file
/// MODULE (`acc.module_qname`), whichever body it sits in. The path is kept
/// verbatim, as written.
fn collect_require(node: TsNode, src: &[u8], acc: &mut Acc) {
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
                from_module: acc.module_qname.clone(),
                target: ImportTarget::Module {
                    path: path.to_string(),
                    alias: None,
                },
                line: line_at(node),
            });
        }
    }
}

/// Push the CallSite for one `call` node. `ivars` is true only inside an
/// instance method, where an `@x` receiver is an instance variable whose
/// type `collect_ivar_types` may know (LA.23b).
fn collect_call(node: TsNode, src: &[u8], from: NodeId, ivars: bool, acc: &mut Acc) {
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
                line: line_at(node),
            });
        } else if recv.kind() == "identifier" || recv.kind() == "constant" {
            acc.calls.push(CallSite {
                from,
                qualifier: CallQualifier::Attribute {
                    base: recv_text.to_string(),
                    name: method_name.to_string(),
                },
                line: line_at(node),
            });
        } else if ivars && recv.kind() == "instance_variable" {
            // `@repo.find(id)`: the receiver text is the `field_types` key
            // (`"@repo"`), which A6.2a's `receiver_field` passes through
            // as-is. ComplexReceiver, not Attribute: it goes straight to the
            // receiver-type pass instead of the import / unique-global
            // lookups an Attribute base is tried against first.
            acc.calls.push(CallSite {
                from,
                qualifier: CallQualifier::ComplexReceiver {
                    receiver: recv_text.to_string(),
                    name: method_name.to_string(),
                },
                line: line_at(node),
            });
        }
    } else {
        acc.calls.push(CallSite {
            from,
            qualifier: CallQualifier::Bare(method_name.to_string()),
            line: line_at(node),
        });
    }
}

fn collect_calls_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    ivars: bool,
    acc: &mut Acc,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call" {
            collect_call(n, src, from, ivars, acc);
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
//
// The model DECLARATION (`class User < ApplicationRecord`) mints the same
// entity plus a DEFINES edge from the model CLASS, so a model no in-repo code
// queries is still in the graph and joinable from a migration or another
// service. Identity follows A13.1's ORM rule (`code_domain::data_entity`): the
// entity is keyed on the MODEL constant at both sites, so a query in any file
// lands on the declaration's node with no cross-file pre-pass. A declared
// `self.table_name = "…"` rides a table cell on the declaration only; Rails'
// default table is the plural of the constant, which `DbResolver`'s fold joins.

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

    let (qname, entity_id) = ar_entity(model, repo);
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
            cells: Vec::new(),
        });
    }
}

/// The model-keyed DATA_ENTITY qname and id for an ActiveRecord model constant.
/// The one construction site, shared by the query site and the declaration
/// site so both land on the same node.
fn ar_entity(model: &str, repo: RepoId) -> (String, NodeId) {
    let qname = format!("data_entity:sql:{model}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
    (qname, id)
}

/// Base classes whose direct subclasses are ActiveRecord models.
const AR_BASE_CLASSES: &[&str] = &["ApplicationRecord", "ActiveRecord::Base"];

/// What an ActiveRecord model's class body declares about its table.
#[derive(Default)]
struct ArClassDecls {
    /// The string literal of `self.table_name = "…"`, when present.
    table_name: Option<String>,
    /// `self.abstract_class = true`: an abstract base with no table.
    is_abstract: bool,
}

/// If `class` declares an ActiveRecord model (`class X < ApplicationRecord` /
/// `class X < ActiveRecord::Base`), emit its model-keyed DATA_ENTITY, a DEFINES
/// edge from `class_id` to it, and — only for a `self.table_name = "…"`
/// override — a table cell built by `code_domain::data_entity::table_cell`.
///
/// `class_name` is the declared name as written; a namespaced
/// `class Admin::User` keys on its last segment, the constant a query site
/// in the same namespace writes. Abstract bases (`ApplicationRecord` itself,
/// or any class setting `self.abstract_class = true`) own no table and mint
/// nothing.
fn emit_ar_model_entity(
    class: TsNode,
    class_name: &str,
    src: &[u8],
    class_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(superclass) = class.child_by_field_name("superclass") else {
        return;
    };
    let Some(base) = superclass.named_child(0) else {
        return;
    };
    let base = text_of(base, src).trim_start_matches("::");
    if !AR_BASE_CLASSES.contains(&base) {
        return;
    }
    let model = class_name.rsplit("::").next().unwrap_or(class_name);
    if model.is_empty() || NON_MODEL_CONSTANTS.contains(&model) {
        return;
    }
    let decls = class
        .child_by_field_name("body")
        .map(|body| ar_class_decls(body, src))
        .unwrap_or_default();
    if decls.is_abstract {
        return;
    }

    let (qname, entity_id) = ar_entity(model, repo);
    let table_cell = decls
        .table_name
        .as_deref()
        .map(|table| data_entity::table_cell(table, data_entity::orm::ACTIVERECORD));
    if table_cell.is_some() {
        acc.ar_table_cells += 1;
    }
    acc.ar_models += 1;

    if acc.data_entities.insert(entity_id) {
        acc.nodes.push(Node {
            id: entity_id,
            repo,
            confidence: Confidence::Strong,
            cells: table_cell.into_iter().collect(),
        });
        acc.nav
            .record(entity_id, model, &qname, node_kind::DATA_ENTITY, Some(class_id));
    } else if let Some(existing) = acc.nodes.iter_mut().find(|n| n.id == entity_id) {
        // A query earlier in this file already minted the entity: the
        // declaration is the authoritative site, so it lifts the confidence
        // and carries the table cell onto that same node.
        existing.confidence = Confidence::Strong;
        existing.cells.extend(table_cell);
    }
    acc.edges.push(Edge {
        from: class_id,
        to: entity_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
}

/// Scan an ActiveRecord class body's direct statements for
/// `self.table_name = "…"` and `self.abstract_class = true`.
fn ar_class_decls(body: TsNode, src: &[u8]) -> ArClassDecls {
    let mut decls = ArClassDecls::default();
    let mut cursor = body.walk();
    for stmt in body.named_children(&mut cursor) {
        if stmt.kind() != "assignment" {
            continue;
        }
        let (Some(left), Some(right)) = (
            stmt.child_by_field_name("left"),
            stmt.child_by_field_name("right"),
        ) else {
            continue;
        };
        if left.kind() != "call" {
            continue;
        }
        let Some(recv) = left.child_by_field_name("receiver") else {
            continue;
        };
        if recv.kind() != "self" {
            continue;
        }
        let Some(method) = left.child_by_field_name("method") else {
            continue;
        };
        match text_of(method, src) {
            "table_name" => {
                if let Some(table) = plain_string_literal(right, src) {
                    decls.table_name = Some(table);
                }
            }
            "abstract_class" => decls.is_abstract = right.kind() == "true",
            _ => {}
        }
    }
    decls
}

/// The text of a `string` node with no `#{…}` interpolation, or `None` for
/// any other node, an interpolated string, or a blank literal.
fn plain_string_literal(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut out = String::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "string_content" | "escape_sequence" => out.push_str(text_of(child, src)),
            _ => return None,
        }
    }
    let trimmed = out.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ============================================================================
// Client HTTP calls (Net::HTTP / Faraday / RestClient / HTTParty) -> ENDPOINT
// ============================================================================
//
// A Rails/Sinatra service calling another service was invisible to
// `HttpStackResolver`: ruby never emitted an ENDPOINT, so `blast_radius` and
// `cross_stack_trace` stopped dead at the ruby boundary. We mint the SAME
// shared ENDPOINT shape every other client-side parser emits (the helper lives
// in code-domain, so path handling cannot drift per-language).
//
// PRECISION. The receiver rule here is the loosest of any language — a Faraday
// connection is just a local (`conn.post(...)`, `@client.get(...)`), so we
// cannot key on the receiver's name. `url_to_path` is the ONLY gate: it returns
// None for anything that is not an absolute URL or a `/`-rooted path, which is
// what keeps `@cache.get("user:42")` and `params.get(:id)` out. Never emit
// before it returns Some.

/// HTTP verbs usable as a ruby client method name (lower-case, as written).
const RUBY_HTTP_VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

/// Constant receivers that unambiguously name an HTTP client library.
/// `Net::HTTP` arrives as a `scope_resolution` whose text is exactly that.
const HTTP_CLIENT_RECEIVERS: &[&str] = &["Net::HTTP", "RestClient", "HTTParty", "Faraday"];

/// Map a ruby client method name onto an upper-case HTTP verb.
/// `Net::HTTP` spells two of them differently (`get_response`, `post_form`).
fn ruby_http_verb(method: &str) -> Option<String> {
    match method {
        "get_response" => Some("GET".to_string()),
        "post_form" => Some("POST".to_string()),
        m if RUBY_HTTP_VERBS.contains(&m) => Some(m.to_ascii_uppercase()),
        _ => None,
    }
}

/// Reconstruct a ruby `string` node, replacing every `#{expr}` interpolation
/// with `${…}` so it normalises like a TS template path (`normalise_http_path`
/// collapses any segment containing `${` to `{}`, so `/users/${…}` matches
/// route `/users/{id}`). Returns `(text, had_interpolation)`.
fn ruby_string_path(string_node: TsNode, src: &[u8]) -> (String, bool) {
    let mut out = String::new();
    let mut interpolated = false;
    let mut cursor = string_node.walk();
    for child in string_node.named_children(&mut cursor) {
        match child.kind() {
            "string_content" | "escape_sequence" => out.push_str(text_of(child, src)),
            "interpolation" => {
                out.push_str("${…}");
                interpolated = true;
            }
            _ => {}
        }
    }
    (out, interpolated)
}

/// First `string` node in document order at or under `n`. Makes a wrapping
/// `URI(...)` / `URI.parse(...)` call transparent, which is how `Net::HTTP`
/// is always written.
fn first_string_descendant<'a>(n: TsNode<'a>, depth: usize) -> Option<TsNode<'a>> {
    if n.kind() == "string" {
        return Some(n);
    }
    if depth == 0 {
        return None;
    }
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        if let Some(found) = first_string_descendant(child, depth - 1) {
            return Some(found);
        }
    }
    None
}

/// `(VERB, raw_url, interpolated)` for a `call` node that is a client HTTP
/// call, else None. The URL is the first string literal anywhere inside the
/// first argument. `url_to_path` is applied by the caller.
fn client_call_candidate(call: TsNode, src: &[u8]) -> Option<(String, String, bool)> {
    // A receiver-less `get '/users' do … end` is a Sinatra ROUTE, not a client
    // call — requiring a receiver keeps the two scanners from colliding.
    let recv = call.child_by_field_name("receiver")?;
    let recv_ok = match recv.kind() {
        "constant" | "scope_resolution" => HTTP_CLIENT_RECEIVERS.contains(&text_of(recv, src)),
        // A Faraday/RestClient connection held in a local or an ivar
        // (`conn`, `@client`). Loose by necessity; `url_to_path` is the gate.
        "identifier" | "instance_variable" => true,
        _ => false,
    };
    if !recv_ok {
        return None;
    }
    let method = call.child_by_field_name("method")?;
    let verb = ruby_http_verb(text_of(method, src))?;
    let args = call.child_by_field_name("arguments")?;
    let first_arg = args.named_child(0)?;
    let string_node = first_string_descendant(first_arg, 4)?;
    let (raw, interpolated) = ruby_string_path(string_node, src);
    Some((verb, raw, interpolated))
}

/// Outbound HTTP call sites in a method body become shared ENDPOINT nodes
/// (+ a CALLS edge from the enclosing method) so `HttpStackResolver` can pair
/// them with a server ROUTE. Returns the number of call sites emitted.
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
        if n.kind() == "call"
            && let Some((method, raw, interpolated)) = client_call_candidate(n, src)
            && let Some(path) = url_to_path(&raw)
        {
            let pos = n.start_position();
            // An interpolated path is Medium (a segment we could not resolve);
            // a plain literal is Strong. Same rule as swift/scala.
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
            if !matches!(child.kind(), "method" | "singleton_method" | "class" | "module") {
                stack.push(child);
            }
        }
    }
    hits
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
// `namespace :api do ... end` and `scope :v1 do ... end` compose a path prefix
// onto every route declared inside them (`/api/v1/users/:id`); `scope path:`
// overrides the positional segment and `scope module:` / `scope as:` add no
// path at all. A `namespace` also scopes the controller MODULE
// (`Api::V1::UsersController`), which the HANDLED_BY ref deliberately does not
// model — the handler spec is still taken verbatim from `to:`.
//
// Routes are emitted in shape B — `<METHOD> <path>` qname + Text
// ROUTE_METHOD cell — the resolver compat shape that HttpStackResolver
// accepts uniformly across parser-java/csharp/php/rust/python/ruby.

fn is_rails_routes_file(rel_path: &str) -> bool {
    rel_path.ends_with("routes.rb") || rel_path.ends_with("/routes.rb")
}

/// Running tally for the `[ruby-routes]` fired-on marker: how many routes were
/// emitted with a non-empty scope prefix, and how many `namespace` / `scope`
/// blocks actually contributed a path segment.
#[derive(Default)]
struct RailsScopeStats {
    routes: usize,
    scopes: usize,
}

fn scan_rails_routes(
    root: TsNode,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut stats = RailsScopeStats::default();
    walk_rails(root, src, "", module_id, repo, acc, &mut stats);
    if stats.routes > 0 {
        eprintln!(
            "[ruby-routes] composed {} rails routes under {} scopes in {file_rel}",
            stats.routes, stats.scopes
        );
    }
}

/// Recursive routes.rb walk that carries the enclosing `namespace` / `scope`
/// path prefix down to every emit site, so `namespace :api do scope :v1 do
/// get "/users/:id" end end` yields `/api/v1/users/:id` rather than the bare
/// `/users/:id` the old prefix-less explicit-stack walk produced.
///
/// A `call` whose method is `namespace` / `scope` is a scope former: it
/// recurses into its block with the extended prefix and is deliberately NOT
/// also offered to `try_emit_rails_route` (it declares no route of its own,
/// and passing it on would double-count). Every other node recurses with the
/// prefix unchanged, which is what keeps a sibling route declared outside the
/// blocks un-prefixed.
fn walk_rails(
    n: TsNode,
    src: &[u8],
    prefix: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
    stats: &mut RailsScopeStats,
) {
    if n.kind() == "call" {
        let method = n
            .child_by_field_name("method")
            .map(|m| text_of(m, src))
            .unwrap_or("");
        if matches!(method, "namespace" | "scope") {
            let seg = rails_scope_segment(n, src);
            let inner = if seg.is_empty() {
                // `scope module: "admin"` / `scope as: :v1` scope the controller
                // module or the route helper name, never the path.
                prefix.to_string()
            } else {
                stats.scopes += 1;
                join_path(prefix, &seg)
            };
            if let Some(block) = rails_block_child(n) {
                walk_rails(block, src, &inner, module_id, repo, acc, stats);
            }
            return;
        }
        let before = acc.nodes.len();
        try_emit_rails_route(n, src, prefix, module_id, repo, acc);
        if !prefix.is_empty() {
            stats.routes += acc.nodes.len() - before;
        }
    }
    let mut cursor = n.walk();
    for c in n.named_children(&mut cursor) {
        walk_rails(c, src, prefix, module_id, repo, acc, stats);
    }
}

/// The `do ... end` / `{ ... }` body of a scope-forming call.
fn rails_block_child(call: TsNode) -> Option<TsNode> {
    let mut cursor = call.walk();
    call.named_children(&mut cursor)
        .find(|c| matches!(c.kind(), "do_block" | "block"))
}

/// The path segment a `namespace` / `scope` block contributes, slash-trimmed
/// and possibly empty.
///
/// `namespace :api` / `scope :v1` / `scope "/v1"` take the first positional
/// symbol or string; an explicit `path:` keyword wins over it (Rails lets
/// `namespace :api, path: "v2"` mount the module at a different path); and
/// `module:` / `as:` / `defaults:` alone contribute nothing.
fn rails_scope_segment(call: TsNode, src: &[u8]) -> String {
    let Some(args) = call.child_by_field_name("arguments") else {
        return String::new();
    };
    let mut cursor = args.walk();
    let mut positional: Option<String> = None;
    let mut path_kw: Option<String> = None;
    for arg in args.named_children(&mut cursor) {
        match arg.kind() {
            "pair" if is_path_pair(arg, src) => {
                if let Some(v) = arg.child_by_field_name("value") {
                    path_kw = Some(scope_literal(v, src));
                }
            }
            "string" | "simple_symbol" if positional.is_none() => {
                positional = Some(scope_literal(arg, src));
            }
            _ => {}
        }
    }
    path_kw
        .or(positional)
        .unwrap_or_default()
        .trim()
        .trim_matches('/')
        .to_string()
}

/// True for a `path: "/v1"` (or `:path => "/v1"`) keyword argument — the Rails
/// option that overrides the positional segment.
fn is_path_pair(pair: TsNode, src: &[u8]) -> bool {
    pair.child_by_field_name("key")
        .map(|k| text_of(k, src).trim_matches(|c| c == ':' || c == ' ') == "path")
        .unwrap_or(false)
}

/// A scope argument literal: `"/v1"` → `v1`, `:v1` → `v1`. Anything that is
/// not a plain literal (a constant, a variable) yields the empty segment.
fn scope_literal(n: TsNode, src: &[u8]) -> String {
    match n.kind() {
        "string" => string_inner(n, src),
        "simple_symbol" => text_of(n, src).trim_start_matches(':').trim().to_string(),
        _ => String::new(),
    }
}

fn try_emit_rails_route(
    call: TsNode,
    src: &[u8],
    prefix: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
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
            // Under a namespace the root IS the namespace path; with no prefix
            // `join_path("", "/")` is a pass-through and `abs_path` keeps "/".
            let path = abs_path(&join_path(prefix, "/"));
            emit_rails_route("GET", &path, h, module_id, repo, line_at(call), acc);
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
            let path = abs_path(&join_path(prefix, &format!("/{name}")));
            emit_rails_route("ANY", &path, None, module_id, repo, line_at(call), acc);
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
    let path = abs_path(&join_path(prefix, &path));
    emit_rails_route(verb, &path, handler, module_id, repo, line_at(call), acc);
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
    emit_rails_route(verb, path, None, module_id, repo, line_at(call), acc);
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

/// `line` is the route call's 0-based row: the site of its HANDLED_BY ref
/// (LC.3b).
fn emit_rails_route(
    method: &str,
    path: &str,
    handler: Option<(String, String)>,
    module_id: NodeId,
    repo: RepoId,
    line: u32,
    acc: &mut Acc,
) {
    let path = abs_path(path);
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
            line,
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
            payload: CellPayload::Json(glia_doc::position_json(root, file_rel)),
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
            payload: CellPayload::Json(glia_doc::position_json(node, file_rel)),
        },
    ];
    if let Some(doc) = glia_doc::leading_doc(node, src) {
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

    /// (from_module, path) for every import, in source order.
    fn import_pairs(fp: &FileParse) -> Vec<(String, String)> {
        fp.imports
            .iter()
            .map(|i| match &i.target {
                ImportTarget::Module { path, .. } => (i.from_module.clone(), path.clone()),
                other => (i.from_module.clone(), format!("{other:?}")),
            })
            .collect()
    }

    #[test]
    fn require_inside_module_is_the_files() {
        let source = "module Shop\n  require_relative 'pricing'\n\n  def self.go; end\nend\n";
        let fp = parse_file(source, "cart.rb", "cart", repo()).unwrap();
        assert_eq!(
            import_pairs(&fp),
            vec![("cart".to_string(), "pricing".to_string())]
        );
        assert_eq!(fp.imports[0].line, 1);
    }

    #[test]
    fn require_inside_class_is_the_files() {
        let source = "module Shop\n  class Cart\n    require 'ledger'\n\n    def total(x)\n      x\n    end\n  end\nend\n";
        let fp = parse_file(source, "cart.rb", "cart", repo()).unwrap();
        assert_eq!(
            import_pairs(&fp),
            vec![("cart".to_string(), "ledger".to_string())]
        );
        assert_eq!(fp.imports[0].line, 2);
    }

    #[test]
    fn top_level_require_unchanged() {
        // The fixtures/ruby-rails-imports shape: from the module, path verbatim.
        let source = "require_relative \"util\"\nrequire 'lib/helpers/auth'\n\ndef run\n  shared_util\nend\n";
        let fp = parse_file(source, "main.rb", "main", repo()).unwrap();
        assert_eq!(
            import_pairs(&fp),
            vec![
                ("main".to_string(), "util".to_string()),
                ("main".to_string(), "lib/helpers/auth".to_string()),
            ]
        );
        assert_eq!(fp.imports[0].line, 0);
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
    fn rails_namespace_composes() {
        // `namespace :api` + `scope :v1` both contribute a path segment to the
        // verb route declared inside them.
        let source = r#"
Rails.application.routes.draw do
  namespace :api do
    scope :v1 do
      get '/users/:id', to: 'users#show'
    end
  end
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(
            fp.nodes
                .iter()
                .any(|n| n.id == route_id("GET", "/api/v1/users/:id")),
            "namespace+scope must compose onto the verb route"
        );
        assert!(
            !fp.nodes.iter().any(|n| n.id == route_id("GET", "/users/:id")),
            "the un-prefixed route must not also be emitted"
        );
        // The HANDLED_BY ref still hangs off the (renamed) route node.
        assert!(has_handled_by(&fp, "UsersController", "show"));
        let show_ref = fp
            .refs
            .iter()
            .find(|r| r.category == edge_category::HANDLED_BY)
            .expect("show HANDLED_BY ref");
        assert_eq!(show_ref.from, route_id("GET", "/api/v1/users/:id"));
    }

    #[test]
    fn rails_namespace_resources_composes() {
        // `resources` inside the same nesting inherits the composed prefix.
        let source = r#"
Rails.application.routes.draw do
  namespace :api do
    scope '/v1' do
      resources :orders
    end
  end
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(
            fp.nodes
                .iter()
                .any(|n| n.id == route_id("ANY", "/api/v1/orders")),
            "resources must inherit the namespace/scope prefix"
        );
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("ANY", "/orders")));
    }

    #[test]
    fn rails_route_outside_namespace_unaffected() {
        // Regression lock for the recursion: leaving a scope block must restore
        // the outer prefix, and a non-path scope option contributes nothing.
        let source = r#"
Rails.application.routes.draw do
  namespace :api do
    get '/users', to: 'users#index'
  end

  scope module: 'admin' do
    get '/reports', to: 'reports#index'
  end

  get '/health', to: 'health#index'
  root 'home#index'
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/api/users")));
        // `scope module:` scopes the controller module, not the path.
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/reports")));
        // Sibling declared after the blocks keeps its bare path.
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/health")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/")));
    }

    #[test]
    fn rails_namespaced_root_is_the_namespace_path() {
        // `root` inside a namespace is that namespace's index, not "/".
        let source = r#"
Rails.application.routes.draw do
  namespace :admin do
    root 'dashboard#index'
  end
end
"#;
        let fp = parse_file(source, "config/routes.rb", "config::routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/admin")));
        assert!(has_handled_by(&fp, "DashboardController", "index"));
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
    // ActiveRecord model declarations (A13.13)
    // ========================================================================

    fn class_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, qname)
    }

    /// The table recorded on `id`'s node, read back through the A13.1 reader.
    fn table_on(fp: &FileParse, id: NodeId) -> Option<String> {
        let node = fp.nodes.iter().find(|n| n.id == id)?;
        data_entity::table_of(&node.cells)
    }

    fn data_entity_count(fp: &FileParse) -> usize {
        fp.nav
            .kind_by_id
            .values()
            .filter(|k| **k == node_kind::DATA_ENTITY)
            .count()
    }

    #[test]
    fn ar_table_name_override_rides_a_table_cell() {
        // The entity stays keyed on the MODEL (A13.1); the declared table
        // rides the declaration-site cell for DbResolver to join on.
        let source = r#"
class LegacyUser < ApplicationRecord
  self.table_name = "app_users"
end
"#;
        let fp = parse_file(
            source,
            "app/models/legacy_user.rb",
            "app::models::legacy_user",
            repo(),
        )
        .unwrap();
        let entity_id = data_entity_id("LegacyUser");
        assert_eq!(table_on(&fp, entity_id), Some("app_users".to_string()));
        let node = fp.nodes.iter().find(|n| n.id == entity_id).unwrap();
        let CellPayload::Json(raw) = &node.cells[0].payload else {
            panic!("table cell must be a Json payload");
        };
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v["orm"], data_entity::orm::ACTIVERECORD);
        assert_eq!(
            fp.nav.name_by_id.get(&entity_id).map(String::as_str),
            Some("LegacyUser")
        );
    }

    #[test]
    fn ar_model_declaration_emits_entity_without_a_query() {
        // No query site anywhere: the declaration alone puts the model in the
        // graph, CLASS -DEFINES-> DATA_ENTITY, and with no table_name override
        // it carries no table cell (the plural default is DbResolver's fold).
        let source = r#"
class User < ApplicationRecord
  has_many :posts
end
"#;
        let fp = parse_file(source, "app/models/user.rb", "app::models::user", repo()).unwrap();
        let entity_id = data_entity_id("User");
        assert!(fp.nodes.iter().any(|n| n.id == entity_id));
        assert_eq!(table_on(&fp, entity_id), None);
        assert!(
            fp.edges.iter().any(|e| e.from == class_id("app::models::user::User")
                && e.to == entity_id
                && e.category == edge_category::DEFINES),
            "expected User CLASS -DEFINES-> its DATA_ENTITY"
        );
        assert_eq!(data_entity_count(&fp), 1);
    }

    #[test]
    fn ar_active_record_base_and_namespaced_models_key_on_the_constant() {
        // `ActiveRecord::Base` (pre-Rails-5 apps) and `::ApplicationRecord`
        // are AR bases too; `class Admin::Account` keys on `Account`, the
        // constant a query inside `module Admin` writes.
        let source = r#"
class Admin::Account < ActiveRecord::Base
  self.table_name = 'admin_accounts'
end

class Invoice < ::ApplicationRecord
end
"#;
        let fp = parse_file(source, "app/models/admin.rb", "app::models::admin", repo()).unwrap();
        assert_eq!(
            table_on(&fp, data_entity_id("Account")),
            Some("admin_accounts".to_string())
        );
        assert!(fp.nodes.iter().any(|n| n.id == data_entity_id("Invoice")));
        assert_eq!(data_entity_count(&fp), 2);
    }

    #[test]
    fn ar_abstract_bases_and_plain_classes_mint_no_entity() {
        // ApplicationRecord itself, a custom abstract base, a non-AR subclass
        // and a plain class own no table: no entity, no DEFINES to one.
        let source = r#"
class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end

class TenantRecord < ApplicationRecord
  self.abstract_class = true
  self.table_name = "never_used"
end

class Mailer < ActionMailer::Base
end

class Report
end
"#;
        let fp = parse_file(
            source,
            "app/models/application_record.rb",
            "app::models::application_record",
            repo(),
        )
        .unwrap();
        assert_eq!(data_entity_count(&fp), 0, "abstract/non-AR classes mint no entity");
        assert!(
            !fp.edges.iter().any(|e| e.category == edge_category::DEFINES
                && fp.nav.kind_by_id.get(&e.to) == Some(&node_kind::DATA_ENTITY)),
        );
    }

    #[test]
    fn ar_interpolated_table_name_gives_no_cell() {
        // An interpolated table is not a literal; the model entity still
        // exists, keyed on the constant, but carries no table cell.
        let source = r#"
class Shard < ApplicationRecord
  self.table_name = "shard_#{ENV['N']}"
end
"#;
        let fp = parse_file(source, "app/models/shard.rb", "app::models::shard", repo()).unwrap();
        let entity_id = data_entity_id("Shard");
        assert!(fp.nodes.iter().any(|n| n.id == entity_id));
        assert_eq!(table_on(&fp, entity_id), None);
    }

    #[test]
    fn ar_declaration_and_query_share_one_entity() {
        // A query site before the declaration in the same file mints the
        // entity first; the declaration lands on that SAME node and adds its
        // table cell. A query after the declaration adds no second node.
        let source = r#"
class Audit
  def run
    LegacyUser.where(active: true)
  end
end

class LegacyUser < ApplicationRecord
  self.table_name = "app_users"

  def self.recent
    LegacyUser.where(recent: true)
  end
end
"#;
        let fp = parse_file(source, "app/models/mixed.rb", "app::models::mixed", repo()).unwrap();
        let entity_id = data_entity_id("LegacyUser");
        let matching: Vec<_> = fp.nodes.iter().filter(|n| n.id == entity_id).collect();
        assert_eq!(matching.len(), 1, "one entity node per model per file");
        assert_eq!(matching[0].confidence, Confidence::Strong);
        assert_eq!(table_on(&fp, entity_id), Some("app_users".to_string()));
        assert_eq!(data_entity_count(&fp), 1);
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.to == entity_id && e.category == edge_category::ACCESSES_DATA)
                .count(),
            2,
            "both accessors reach the one entity"
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

    // ---- client HTTP calls -> ENDPOINT (A4.9) ----------------------------

    fn endpoint_names(fp: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ENDPOINT)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).cloned())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn net_http_get_emits_endpoint() {
        let source = r#"
class ApiClient
  def fetch_user(id)
    Net::HTTP.get(URI("http://users-svc/api/users/#{id}"))
  end
end
"#;
        let fp = parse_file(source, "client/api_client.rb", "client::api_client", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec!["GET /api/users/${\u{2026}}".to_string()],
            "host stripped, interpolation normalised"
        );
        let ep_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            "endpoint:GET:/api/users/${\u{2026}}",
        );
        let from_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "client::api_client::ApiClient::fetch_user",
        );
        assert!(
            fp.edges.iter().any(|e| e.from == from_id
                && e.to == ep_id
                && e.category == edge_category::CALLS),
            "enclosing method -> ENDPOINT CALLS edge"
        );
    }

    #[test]
    fn faraday_conn_get_emits_endpoint() {
        let source = r#"
class ApiClient
  def list_users
    conn = Faraday.new
    conn.get('/api/users')
  end

  def create_user(body)
    @client.post("/api/users", body)
  end
end
"#;
        let fp = parse_file(source, "client/api_client.rb", "client::api_client", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec!["GET /api/users".to_string(), "POST /api/users".to_string()],
            "local and ivar Faraday connections both emit"
        );
    }

    #[test]
    fn ruby_client_calls_emit_no_route() {
        // The ruby parser carries two ROUTE scanners (rails + sinatra). Neither
        // may fire on a pure client file, or the graph gains a phantom server.
        let source = r#"
require 'net/http'

class ApiClient
  def fetch_user(id)
    Net::HTTP.get(URI("http://users-svc/api/users/#{id}"))
  end

  def create_user(body)
    conn = Faraday.new
    conn.post('/api/users', body)
  end
end
"#;
        let fp = parse_file(source, "client/api_client.rb", "client::api_client", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "a client file must not mint a ROUTE");
        assert_eq!(endpoint_names(&fp).len(), 2);
    }

    #[test]
    fn ruby_non_url_string_arg_is_dropped() {
        // `url_to_path` is the only precision gate on the loose receiver rule.
        let source = r#"
class ApiClient
  def cached(id)
    @cache.get("user:#{id}")
  end

  def setting
    config.get('database.host')
  end

  def lookup(id)
    params.get(id)
  end
end
"#;
        let fp = parse_file(source, "client/api_client.rb", "client::api_client", repo()).unwrap();
        assert!(
            endpoint_names(&fp).is_empty(),
            "non-URL string args must emit nothing, got {:?}",
            endpoint_names(&fp)
        );
    }

    // ------------------------------------------------------------------
    // LA.23b — `@ivar.m()` call sites and constructor-typed ivars
    // ------------------------------------------------------------------

    fn ivar_types_of(fp: &FileParse, class_qname: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .nav
            .field_types
            .get(&class_id(class_qname))
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    fn ivar_call_sites(fp: &FileParse) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .calls
            .iter()
            .filter_map(|c| match &c.qualifier {
                CallQualifier::ComplexReceiver { receiver, name } => {
                    Some((receiver.clone(), name.clone()))
                }
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn ivar_receiver_emits_complex_receiver_call_site() {
        let source = r#"
class UserService
  def get(id)
    @repo.find(id)
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        let get = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "svc::UserService::get");
        let site = fp
            .calls
            .iter()
            .find(|c| matches!(&c.qualifier, CallQualifier::ComplexReceiver { .. }))
            .expect("an @ivar receiver must push a CallSite");
        assert_eq!(site.from, get);
        assert_eq!(
            site.qualifier,
            CallQualifier::ComplexReceiver { receiver: "@repo".into(), name: "find".into() }
        );
        // No constructor writer: the call site exists, but nothing is typed.
        assert!(ivar_types_of(&fp, "svc::UserService").is_empty());
    }

    #[test]
    fn ivar_assigned_from_new_records_its_type() {
        let source = r#"
class UserService
  def initialize
    @repo = UserRepo.new
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert_eq!(
            ivar_types_of(&fp, "svc::UserService"),
            vec![("@repo".to_string(), "UserRepo".to_string())]
        );
    }

    #[test]
    fn memoised_ivar_records_its_type_in_any_method() {
        let source = r#"
class UserService
  def audit(x)
    @log ||= AuditLog.new
    @log.write(x)
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert_eq!(
            ivar_types_of(&fp, "svc::UserService"),
            vec![("@log".to_string(), "AuditLog".to_string())]
        );
        assert_eq!(ivar_call_sites(&fp), vec![("@log".to_string(), "write".to_string())]);
    }

    #[test]
    fn namespaced_constructor_records_the_last_segment() {
        let source = r#"
class UserService
  def initialize
    @repo = Repos::UserRepo.new(db)
    @cache = ::Cache.new
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert_eq!(
            ivar_types_of(&fp, "svc::UserService"),
            vec![
                ("@cache".to_string(), "Cache".to_string()),
                ("@repo".to_string(), "UserRepo".to_string()),
            ]
        );
    }

    #[test]
    fn keyword_and_optional_defaults_type_the_ivar_they_feed() {
        let source = r#"
class UserService
  def initialize(repo: UserRepo.new, log = AuditLog.new, name: "x")
    @repo = repo
    @log = log
    @name = name
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert_eq!(
            ivar_types_of(&fp, "svc::UserService"),
            vec![
                ("@log".to_string(), "AuditLog".to_string()),
                ("@repo".to_string(), "UserRepo".to_string()),
            ]
        );
    }

    #[test]
    fn conflicting_ivar_types_record_nothing() {
        // Within one method, across methods, and across a reopened class.
        let source = r#"
class UserService
  def initialize
    @repo = UserRepo.new
    @log = AuditLog.new
  end

  def reset
    @repo = CachedRepo.new
    @log = AuditLog.new
  end
end

class UserService
  def swap
    @log ||= NullLog.new
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert!(
            ivar_types_of(&fp, "svc::UserService").is_empty(),
            "got {:?}",
            ivar_types_of(&fp, "svc::UserService")
        );
    }

    #[test]
    fn untyped_writers_neither_type_nor_conflict() {
        let source = r#"
class UserService
  def initialize
    @repo = UserRepo.new
  end

  def reset
    @repo = nil
    @other = build_repo
    @sum += Counter.new
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert_eq!(
            ivar_types_of(&fp, "svc::UserService"),
            vec![("@repo".to_string(), "UserRepo".to_string())]
        );
    }

    #[test]
    fn module_and_top_level_methods_record_nothing() {
        let source = r#"
module Auditable
  def audit(x)
    @log ||= AuditLog.new
    @log.write(x)
  end
end

def helper
  @repo = UserRepo.new
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert!(fp.nav.field_types.is_empty(), "got {:?}", fp.nav.field_types);
        // The call site is still visible (unresolved diagnostics), not dropped.
        assert_eq!(ivar_call_sites(&fp), vec![("@log".to_string(), "write".to_string())]);
    }

    #[test]
    fn singleton_methods_neither_type_ivars_nor_emit_ivar_calls() {
        // `@client` in `def self.x` is the class object's ivar, not an
        // instance field of UserService.
        let source = r#"
class UserService
  def self.client
    @client ||= HttpClient.new
    @client.fetch
  end

  def run
    @client.fetch
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert!(fp.nav.field_types.is_empty(), "got {:?}", fp.nav.field_types);
        let run = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "svc::UserService::run");
        let sites: Vec<NodeId> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::ComplexReceiver { .. }))
            .map(|c| c.from)
            .collect();
        assert_eq!(sites, vec![run]);
    }

    #[test]
    fn nested_class_ivars_type_the_nested_class_only() {
        let source = r#"
class Outer
  def initialize
    @repo = OuterRepo.new
  end

  class Inner
    def initialize
      @repo = InnerRepo.new
    end
  end
end
"#;
        let fp = parse_file(source, "svc.rb", "svc", repo()).unwrap();
        assert_eq!(
            ivar_types_of(&fp, "svc::Outer"),
            vec![("@repo".to_string(), "OuterRepo".to_string())]
        );
        assert_eq!(
            ivar_types_of(&fp, "svc::Outer::Inner"),
            vec![("@repo".to_string(), "InnerRepo".to_string())]
        );
    }
}
