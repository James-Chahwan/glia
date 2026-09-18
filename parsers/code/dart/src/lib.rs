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
            // LA.23e: declared / constructor-initialised field types, for
            // A6.2a's receiver-type pass.
            collect_dart_field_types(child, src, id, acc);
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
    // LA.23e: the METHOD this member declared, if any. Only a body under its
    // own METHOD emits call sites: a constructor / getter / setter signature
    // mints no node, and attributing its calls to `acc.nodes.last()` (the
    // previous member, or an ENDPOINT it pushed) would mint wrong CALLS edges.
    let mut own_method: Option<NodeId> = None;
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
            own_method = Some(id);
        }
        if child.kind() == "function_body" {
            // A body without its own METHOD keeps the pre-LA.23e source for
            // Pattern A endpoints (`acc.nodes.last()`), unchanged, and emits
            // no call sites.
            let (from, call_sites) = match own_method {
                Some(id) => (Some(id), true),
                None => (acc.nodes.last().map(|n| n.id), false),
            };
            if let Some(from) = from {
                collect_calls_in(child, src, from, call_sites, repo, file_rel, acc);
            }
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

/// Walk a body for Pattern A endpoints and, when `call_sites` is set, the
/// receiver call sites of every selector chain in it (LA.23e). Nested
/// closures and local functions are not entered.
fn collect_calls_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    call_sites: bool,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        // Pattern A: client HTTP call (`dio.get('/x')`) → ENDPOINT node so the
        // HttpStackResolver can pair it with a server ROUTE.
        try_detect_dart_endpoint(n, src, from, repo, file_rel, acc);
        if call_sites {
            push_selector_chain_calls(n, src, from, acc);
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

// ============================================================================
// LA.23e: receiver call sites over tree-sitter-dart's selector chains
// ============================================================================
//
// tree-sitter-dart 0.1.0 has no `selector_expression` / call node. A postfix
// expression is a primary followed by SIBLING `selector` nodes inside whatever
// node holds it (an `expression_statement`, `return_statement`, `argument`,
// `await_expression`, `initialized_identifier`, a `=>` `function_body`, ...):
//
//   repo.find(id)       identifier  selector(.find)  selector(argument_part)
//   this.repo.find(id)  this        selector(.repo)  selector(.find)  selector(argument_part)
//
// `this` is an ANONYMOUS node, so the scan walks every child, not only the
// named ones. One call site per `argument_part`:
//
//   f()          -> Bare(f)             this.m()    -> SelfMethod(m)
//   x.m()        -> Attribute{x, m}     x?.m()      -> Attribute{x, m}
//   this.f.m()   -> ComplexReceiver{"this.f", m}
//   a.b().c()    -> Attribute{a, b}, then ComplexReceiver{"a.b()", c}
//
// A ComplexReceiver's receiver is the primary's and the preceding selectors'
// texts, concatenated (so layout whitespace between chained selectors drops).
// `super.m()`, cascades (`..m()`), `new` / `const` constructions, a
// parenthesised primary (`(a).m()`) and the second call of `f()()` emit
// nothing here.

/// What one `selector` in a postfix chain does.
enum SelectorPart<'a> {
    /// `.name` / `?.name`: a member access.
    Member(&'a str),
    /// `(args)`: invokes what the chain has built so far.
    Call,
    /// `<T>`: generic arguments of the call that follows. Transparent.
    TypeArgs,
    /// `!`, `[i]`, anything else: the receiver is no longer a plain name.
    Opaque,
}

fn selector_part<'a>(sel: TsNode<'a>, src: &'a [u8]) -> SelectorPart<'a> {
    let Some(inner) = sel.named_child(0) else {
        return SelectorPart::Opaque; // the `!` null assertion has no named child
    };
    match inner.kind() {
        "argument_part" => SelectorPart::Call,
        "type_arguments" => SelectorPart::TypeArgs,
        "unconditional_assignable_selector" | "conditional_assignable_selector" => {
            // `.name` / `?.name`, not an index `[i]` / `?[i]` (whose named child
            // can be an identifier too): the leading token decides.
            let dotted = inner
                .child(0)
                .is_some_and(|t| matches!(t.kind(), "." | "?."));
            match inner.named_child(0) {
                Some(id) if dotted && id.kind() == "identifier" => {
                    SelectorPart::Member(text_of(id, src))
                }
                _ => SelectorPart::Opaque,
            }
        }
        _ => SelectorPart::Opaque,
    }
}

/// Emit the call sites of every `identifier` / `this` primary among `n`'s
/// children that is followed by one or more `selector` siblings.
fn push_selector_chain_calls(n: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut cursor = n.walk();
    let kids: Vec<TsNode> = n.children(&mut cursor).collect();
    let mut i = 0;
    while let Some(primary) = kids.get(i).copied() {
        if !matches!(primary.kind(), "identifier" | "this") {
            i += 1;
            continue;
        }
        let selectors: Vec<TsNode> = kids
            .iter()
            .skip(i + 1)
            .take_while(|k| k.kind() == "selector")
            .copied()
            .collect();
        i += 1 + selectors.len();
        for qualifier in chain_call_sites(primary, &selectors, src) {
            acc.calls.push(CallSite { from, qualifier });
        }
    }
}

/// The call sites of one primary + selector chain, in source order.
fn chain_call_sites(primary: TsNode, selectors: &[TsNode], src: &[u8]) -> Vec<CallQualifier> {
    let head = text_of(primary, src);
    let is_this = primary.kind() == "this";
    let mut out = Vec::new();
    // The member access awaiting its call: (selector index, name).
    let mut pending: Option<(usize, &str)> = None;
    // True while nothing but `<T>` follows the primary, so `f()` / `f<T>()`
    // calls the primary itself.
    let mut bare = true;
    for (j, sel) in selectors.iter().enumerate() {
        match selector_part(*sel, src) {
            SelectorPart::Member(name) => {
                pending = Some((j, name));
                bare = false;
            }
            SelectorPart::TypeArgs => {}
            SelectorPart::Opaque => {
                pending = None;
                bare = false;
            }
            SelectorPart::Call => {
                let site = match pending.take() {
                    // `this.m()` / `x.m()`: the member hangs off the primary.
                    Some((0, name)) if is_this => Some(CallQualifier::SelfMethod(name.to_string())),
                    Some((0, name)) => Some(CallQualifier::Attribute {
                        base: head.to_string(),
                        name: name.to_string(),
                    }),
                    // `this.f.m()`, `a.b().c()`, `x!.m()`: everything before
                    // the member is the receiver.
                    Some((at, name)) => {
                        let mut receiver = head.to_string();
                        for s in selectors.iter().take(at) {
                            receiver.push_str(text_of(*s, src));
                        }
                        Some(CallQualifier::ComplexReceiver {
                            receiver,
                            name: name.to_string(),
                        })
                    }
                    None if bare && !is_this => Some(CallQualifier::Bare(head.to_string())),
                    None => None,
                };
                out.extend(site);
                bare = false;
            }
        }
    }
    out
}

// ============================================================================
// LA.23e: declared field types -> CodeNav::field_types
// ============================================================================
//
// Read by A6.2a's receiver-type pass in the graph crate: `repo.find()` /
// `this.repo.find()` inside a method binds `find` on the type of the enclosing
// class's field `repo`. A field's type comes from its declaration (`final T x;`,
// `late T x;`, `T? x;`, `T x = ...;`, `static final T x = ...;`, `p.T x;`) or,
// untyped, from a constructor-call initialiser (`final x = T();`,
// `var x = T.named();`, `final x = p.T();`, `const T()`, `new T()`).
// Constructor field formals (`UserService(this.repo)`) add nothing: the
// field's own declaration carries the type.

/// Core-library types: a field of one of these never names a repo class.
const DART_BUILTIN_TYPES: &[&str] = &[
    "int", "double", "String", "bool", "num", "dynamic", "Object", "List", "Map", "Set", "Future",
    "Stream", "Iterable", "Function", "FutureOr", "Null", "Never",
];

/// What a field `declaration` says about its type.
enum DeclaredType {
    /// A named repo-class candidate (`final UserRepo repo;`).
    Named(String),
    /// No type written (`final x = ...`, `var x = ...`): the initialiser decides.
    Untyped,
    /// A builtin, `void`, function or record type: nothing to bind.
    Rejected,
}

/// The type written in a field `declaration`, read from the children before
/// its identifier list. tree-sitter-dart inlines the type as siblings:
/// `type_identifier` (twice with a `.` for an import prefix `p.T`), then an
/// optional `type_arguments` and `?`. The last `type_identifier` is the simple
/// name, so generics, prefixes and nullability drop.
fn dart_declared_type(decl: TsNode, src: &[u8]) -> DeclaredType {
    let mut name: Option<&str> = None;
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => name = Some(text_of(child, src)),
            "function_type" | "record_type" | "void_type" => return DeclaredType::Rejected,
            "initialized_identifier_list" | "static_final_declaration_list" => break,
            _ => {}
        }
    }
    match name {
        None => DeclaredType::Untyped,
        Some(t) if DART_BUILTIN_TYPES.contains(&t) => DeclaredType::Rejected,
        Some(t) => DeclaredType::Named(t.to_string()),
    }
}

/// A capitalised identifier (`UserRepo`, `_RepoImpl`): a class name by Dart
/// convention, as opposed to a function or an import prefix.
fn is_dart_class_name(name: &str) -> bool {
    name.trim_start_matches('_')
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
}

/// The class a field initialiser constructs, for an untyped field. `item` is
/// an `initialized_identifier` / `static_final_declaration`: the field's
/// `identifier`, `=`, then the initialiser expression inlined as siblings.
fn dart_initialiser_type(item: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = item.walk();
    let kids: Vec<TsNode> = item.named_children(&mut cursor).collect();
    // kids[0] is the field name; the initialiser starts at kids[1].
    let init = kids.get(1).copied()?;
    let name = match init.kind() {
        // `const T()` / `new T()` / `const p.T()`: the last capitalised type.
        "const_object_expression" | "new_expression" => {
            let mut c = init.walk();
            let last = init
                .named_children(&mut c)
                .filter(|k| k.kind() == "type_identifier")
                .map(|k| text_of(k, src))
                .filter(|t| is_dart_class_name(t))
                .last();
            last?.to_string()
        }
        // `T(...)`, `T<A>(...)`, `T.named(...)`, `p.T(...)`, then optional
        // cascades (`T()..init()` still yields the T).
        "identifier" => {
            let head = text_of(init, src);
            let rest: Vec<TsNode> = kids.iter().skip(2).copied().collect();
            let selectors: Vec<TsNode> = rest
                .iter()
                .take_while(|k| k.kind() == "selector")
                .copied()
                .collect();
            if rest
                .iter()
                .skip(selectors.len())
                .any(|k| k.kind() != "cascade_section")
            {
                return None;
            }
            let mut member: Option<&str> = None;
            let mut members = 0usize;
            let mut called = false;
            for sel in &selectors {
                if called {
                    return None; // `T().build()`: the result is something else
                }
                match selector_part(*sel, src) {
                    SelectorPart::Member(m) => {
                        member = Some(m);
                        members += 1;
                    }
                    SelectorPart::TypeArgs => {}
                    SelectorPart::Call => called = true,
                    SelectorPart::Opaque => return None,
                }
            }
            if !called || members > 1 {
                return None;
            }
            match member {
                // `T(...)` / `T<A>(...)`
                None if is_dart_class_name(head) => head.to_string(),
                // `T.named(...)`: a named constructor or static factory
                Some(_) if is_dart_class_name(head) => head.to_string(),
                // `p.T(...)`: an import-prefixed class
                Some(m) if is_dart_class_name(m) => m.to_string(),
                _ => return None,
            }
        }
        _ => return None,
    };
    (!DART_BUILTIN_TYPES.contains(&name.as_str())).then_some(name)
}

/// Record every typed or constructor-initialised field of the class whose
/// body is `class_body` into `acc.nav.field_types` under `class_id`.
fn collect_dart_field_types(class_body: TsNode, src: &[u8], class_id: NodeId, acc: &mut Acc) {
    let mut members = class_body.walk();
    for member in class_body.named_children(&mut members) {
        if member.kind() != "class_member" {
            continue;
        }
        let mut decls = member.walk();
        for decl in member.named_children(&mut decls) {
            if decl.kind() != "declaration" {
                continue;
            }
            let declared = dart_declared_type(decl, src);
            let mut lists = decl.walk();
            for list in decl.named_children(&mut lists) {
                let item_kind = match list.kind() {
                    "initialized_identifier_list" => "initialized_identifier",
                    "static_final_declaration_list" => "static_final_declaration",
                    _ => continue,
                };
                let mut items = list.walk();
                for item in list.named_children(&mut items) {
                    if item.kind() != item_kind {
                        continue;
                    }
                    let Some(field) = item.named_child(0).filter(|n| n.kind() == "identifier")
                    else {
                        continue;
                    };
                    let ty = match &declared {
                        DeclaredType::Named(t) => Some(t.clone()),
                        DeclaredType::Untyped => dart_initialiser_type(item, src),
                        DeclaredType::Rejected => None,
                    };
                    if let Some(t) = ty {
                        acc.nav.record_field_type(class_id, text_of(field, src), &t);
                    }
                }
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

    // ---- LA.23e: selector-chain call sites + field types --------------------

    fn method_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, qname)
    }

    /// The call qualifiers emitted from one method, sorted: `collect_calls_in`
    /// walks a body with a stack (last statement first), which is
    /// deterministic but not source order.
    fn calls_from(fp: &FileParse, qname: &str) -> Vec<CallQualifier> {
        let id = method_id(qname);
        let mut out: Vec<CallQualifier> = fp
            .calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| c.qualifier.clone())
            .collect();
        out.sort_by_key(|q| format!("{q:?}"));
        out
    }

    fn sorted(mut v: Vec<CallQualifier>) -> Vec<CallQualifier> {
        v.sort_by_key(|q| format!("{q:?}"));
        v
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    fn complex(receiver: &str, name: &str) -> CallQualifier {
        CallQualifier::ComplexReceiver {
            receiver: receiver.to_string(),
            name: name.to_string(),
        }
    }

    fn field_types(fp: &FileParse, class_qname: &str) -> std::collections::HashMap<String, String> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, class_qname);
        fp.nav.field_types.get(&id).cloned().unwrap_or_default()
    }

    /// The grammar shape the scanner is written against: tree-sitter-dart
    /// 0.1.0 has no call node; a call is a primary followed by sibling
    /// `selector`s, and `this` is an anonymous token.
    #[test]
    fn grammar_pins_the_sibling_selector_shape() {
        let source = "class A { void m() { repo.find(id); this.go(); } }\n";
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let src = source.as_bytes();
        let mut stmts = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "expression_statement" {
                stmts.push(n);
            }
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
        }
        stmts.sort_by_key(|n| n.start_byte());
        let kinds = |n: TsNode| -> Vec<(String, bool)> {
            let mut c = n.walk();
            n.children(&mut c)
                .map(|k| (k.kind().to_string(), k.is_named()))
                .collect()
        };
        assert_eq!(text_of(stmts[0], src), "repo.find(id);");
        assert_eq!(
            kinds(stmts[0]),
            vec![
                ("identifier".to_string(), true),
                ("selector".to_string(), true),
                ("selector".to_string(), true),
                (";".to_string(), false),
            ]
        );
        assert_eq!(kinds(stmts[1])[0], ("this".to_string(), false));
        assert!(!tree_sitter_dart::NODE_TYPES.contains("\"selector_expression\""));
    }

    #[test]
    fn selector_chains_emit_call_sites() {
        let source = r#"class A {
  void run() {
    x.m();
    this.m();
    this.f.m();
    f();
    a.b().c();
  }
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(
            calls_from(&fp, "lib::a::A::run"),
            sorted(vec![
                attr("x", "m"),
                CallQualifier::SelfMethod("m".to_string()),
                complex("this.f", "m"),
                CallQualifier::Bare("f".to_string()),
                attr("a", "b"),
                complex("a.b()", "c"),
            ])
        );
        // Within one chain the sites come out in call order.
        let id = method_id("lib::a::A::run");
        let chain: Vec<&CallQualifier> = fp
            .calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| &c.qualifier)
            .filter(|q| matches!(q, CallQualifier::Attribute { base, .. } if base == "a")
                || matches!(q, CallQualifier::ComplexReceiver { receiver, .. } if receiver == "a.b()"))
            .collect();
        assert_eq!(chain, vec![&attr("a", "b"), &complex("a.b()", "c")]);
    }

    #[test]
    fn selector_chains_in_other_positions_and_shapes() {
        let source = r#"class A {
  String get(int id) => repo.find(id);
  Future<void> run() async {
    final r = await _other.find(1);
    x?.m();
    y!.m();
    list[i].m();
    g(1);
    use(repo.find(2));
    f()();
    super.m();
    repo..find(3);
    (a).m();
    return this
        .repo
        .find(4);
  }
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(
            calls_from(&fp, "lib::a::A::get"),
            vec![attr("repo", "find")]
        );
        // `super.m()`, the cascade, `(a).m()` and the call on `f()`'s result
        // emit nothing. (A generic call `g<int>()` is not probed: this grammar
        // parses it as a relational expression, `g < int > ()`.)
        assert_eq!(
            calls_from(&fp, "lib::a::A::run"),
            sorted(vec![
                attr("_other", "find"),
                attr("x", "m"),
                complex("y!", "m"),
                complex("list[i]", "m"),
                CallQualifier::Bare("g".to_string()),
                CallQualifier::Bare("use".to_string()),
                attr("repo", "find"),
                CallQualifier::Bare("f".to_string()),
                complex("this.repo", "find"),
            ])
        );
    }

    /// A constructor / getter body mints no METHOD, so it emits no call site
    /// rather than lending its calls to the member before it.
    #[test]
    fn body_without_its_own_method_emits_no_call_site() {
        let source = r#"class A {
  void first() {}
  A(this.repo) {
    repo.init();
  }
  String get name => repo.name();
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert!(fp.calls.is_empty(), "{:?}", fp.calls);
    }

    #[test]
    fn typed_and_late_and_nullable_fields_record_their_type() {
        let source = r#"class S {
  final UserRepo repo;
  late UserRepo lateRepo;
  late final Cache cache;
  UserRepo? maybe;
  AuthApi a, b;
  final p.Remote remote;
  final Box<UserRepo> box;
  Mailer mailer = Mailer();
  static final Clock clock = Clock();
  S(this.repo);
}
"#;
        let fp = parse_file(source, "lib/s.dart", "lib::s", repo()).unwrap();
        let f = field_types(&fp, "lib::s::S");
        let want = [
            ("repo", "UserRepo"),
            ("lateRepo", "UserRepo"),
            ("cache", "Cache"),
            ("maybe", "UserRepo"),
            ("a", "AuthApi"),
            ("b", "AuthApi"),
            ("remote", "Remote"),
            ("box", "Box"),
            ("mailer", "Mailer"),
            ("clock", "Clock"),
        ];
        for (field, ty) in want {
            assert_eq!(f.get(field).map(String::as_str), Some(ty), "{field}: {f:?}");
        }
        assert_eq!(f.len(), want.len(), "{f:?}");
    }

    #[test]
    fn untyped_fields_take_the_constructed_class() {
        let source = r#"class S {
  final _other = UserRepo();
  var named = UserRepo.named(1);
  final generic = Holder<int>();
  final prefixed = p.Remote();
  final konst = const Settings();
  final fresh = new Pool();
  final cascaded = Bus()..start();
  final _impl = _RepoImpl();
  static const k = Registry();
  final fromCall = makeRepo();
  final chained = Factory().build();
  final lower = repo.create();
  final literal = 3;
}
"#;
        let fp = parse_file(source, "lib/s.dart", "lib::s", repo()).unwrap();
        let f = field_types(&fp, "lib::s::S");
        let want = [
            ("_other", "UserRepo"),
            ("named", "UserRepo"),
            ("generic", "Holder"),
            ("prefixed", "Remote"),
            ("konst", "Settings"),
            ("fresh", "Pool"),
            ("cascaded", "Bus"),
            ("_impl", "_RepoImpl"),
            ("k", "Registry"),
        ];
        for (field, ty) in want {
            assert_eq!(f.get(field).map(String::as_str), Some(ty), "{field}: {f:?}");
        }
        for untyped in ["fromCall", "chained", "lower", "literal"] {
            assert!(!f.contains_key(untyped), "{untyped}: {f:?}");
        }
    }

    #[test]
    fn builtin_function_and_record_types_are_rejected() {
        let source = r#"class S {
  final int count;
  String name = 'x';
  dynamic d;
  Object o = UserRepo();
  final List<UserRepo> repos = [];
  final Future<UserRepo> pending;
  final void Function(int) cb;
  final (int, String) rec;
  final list = List<int>.filled(3, 0);
  final done = Future.value(1);
}
"#;
        let fp = parse_file(source, "lib/s.dart", "lib::s", repo()).unwrap();
        assert!(
            field_types(&fp, "lib::s::S").is_empty(),
            "{:?}",
            fp.nav.field_types
        );
    }

    /// The LA.23e fixture's own file: both fields typed, all three call shapes
    /// emitted from the three methods.
    #[test]
    fn field_dispatch_fixture_shape() {
        let source = r#"import 'user_repo.dart';

class UserService {
  final UserRepo repo;
  final _other = UserRepo();

  UserService(this.repo);

  String get(int id) {
    return repo.find(id);
  }

  String other(int id) {
    return _other.find(id);
  }

  String viaThis(int id) {
    return this.repo.find(id);
  }
}
"#;
        let fp = parse_file(source, "lib/user_service.dart", "lib::user_service", repo()).unwrap();
        let f = field_types(&fp, "lib::user_service::UserService");
        assert_eq!(f.get("repo").map(String::as_str), Some("UserRepo"));
        assert_eq!(f.get("_other").map(String::as_str), Some("UserRepo"));
        assert_eq!(f.len(), 2);
        let q = "lib::user_service::UserService";
        assert_eq!(
            calls_from(&fp, &format!("{q}::get")),
            vec![attr("repo", "find")]
        );
        assert_eq!(
            calls_from(&fp, &format!("{q}::other")),
            vec![attr("_other", "find")]
        );
        assert_eq!(
            calls_from(&fp, &format!("{q}::viaThis")),
            vec![complex("this.repo", "find")]
        );
        // The untyped field's initialiser is a class-level expression, not a
        // method body: it emits no call site.
        assert_eq!(fp.calls.len(), 3, "{:?}", fp.calls);
    }
}
