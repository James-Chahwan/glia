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
            collect_calls_in(child, src, method_id, acc);
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

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
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

// ============================================================================
// Dart route extraction (v0.4.11a R-dart)
// ============================================================================
//
// Two framework surfaces covered via text scan (robust to tree-sitter-dart's
// no-field-name quirk):
//
//   go_router navigation:  GoRoute(path: '/users', ...)  → ANY /users
//   shelf / shelf_router:  router.get('/users', handler) → GET /users
//                          ..post('/x', h)  (cascade)    → POST /x
//
// Shape B ROUTE nodes (METHOD <path> qname + Text ROUTE_METHOD cell).

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
            emit_dart_route("ANY", &path, repo, acc, &mut seen);
        }
        idx = start;
    }

    // shelf-style `.get('/...' / .post('/...' / etc.
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let needle = format!(".{method}(");
        let mut idx = 0;
        while let Some(pos) = source[idx..].find(&needle) {
            let after = &source[idx + pos + needle.len()..];
            if let Some(path) = first_string_literal_dart(after)
                && path.starts_with('/')
            {
                let verb = method.to_ascii_uppercase();
                emit_dart_route(&verb, &path, repo, acc, &mut seen);
            }
            idx += pos + needle.len();
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

fn emit_dart_route(
    method: &str,
    path: &str,
    repo: RepoId,
    acc: &mut Acc,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    let key = (method.to_string(), path.to_string());
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
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/users/:id")));
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
}
