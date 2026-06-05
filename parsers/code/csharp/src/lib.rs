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
    let lang: tree_sitter::Language = tree_sitter_c_sharp::LANGUAGE.into();
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

    visit_children(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);

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
            "using_directive" => collect_using(child, src, parent_qname, acc),
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                visit_namespace(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "class_declaration" | "struct_declaration" | "interface_declaration"
            | "enum_declaration" | "record_declaration" | "record_struct_declaration" => {
                visit_type_decl(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
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
    let qname = name.replace('.', "::");
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

    // File-scoped namespace has no body block — declarations are direct children.
    if node.kind() == "file_scoped_namespace_declaration" {
        visit_children(node, src, file_rel, &qname, ns_id, repo, acc);
    } else if let Some(body) = node.child_by_field_name("body") {
        visit_children(body, src, file_rel, &qname, ns_id, repo, acc);
    }
}

fn visit_type_decl(
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
    let kind = match node.kind() {
        "class_declaration" | "record_declaration" | "record_struct_declaration" => node_kind::CLASS,
        "struct_declaration" => node_kind::STRUCT,
        "interface_declaration" => node_kind::INTERFACE,
        "enum_declaration" => node_kind::ENUM,
        _ => return,
    };
    let qname = format!("{parent_qname}::{name}");
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

    // G12.5: base_list. C# does not syntactically distinguish base class from
    // interfaces, so heuristic: a name starting with `I` + uppercase letter is
    // an interface → IMPLEMENTS; otherwise → INHERITS_FROM. A single ambiguous
    // item defaults to INHERITS_FROM.
    let mut base_cursor = node.walk();
    if let Some(base_list) = node
        .children(&mut base_cursor)
        .find(|c| c.kind() == "base_list")
    {
        let mut bl_cursor = base_list.walk();
        for base in base_list.named_children(&mut bl_cursor) {
            // Skip primary-constructor argument lists; only type/base names.
            if base.kind() == "argument_list" {
                continue;
            }
            let raw = text_of(base, src);
            let category = if is_interface_name(raw) {
                edge_category::IMPLEMENTS
            } else {
                edge_category::INHERITS_FROM
            };
            emit_heritage_ref(raw, category, id, repo, acc);
        }
    }

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "method_declaration" | "constructor_declaration" => {
                visit_method(child, src, file_rel, &qname, id, repo, acc);
            }
            "field_declaration" => {
                visit_field_decl(child, src, file_rel, &qname, id, repo, acc);
            }
            "class_declaration" | "struct_declaration" | "interface_declaration"
            | "enum_declaration" | "record_declaration" | "record_struct_declaration" => {
                visit_type_decl(child, src, file_rel, &qname, id, repo, acc);
            }
            _ => {}
        }
    }

    check_route_attrs(node, src, id, repo, acc);
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
    });
    acc.nav
        .record(id, name, &qname, node_kind::METHOD, Some(parent_id));

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
    }

    check_route_attrs(node, src, id, repo, acc);
}

/// G12.5: heuristic — does this base-list name look like an interface?
/// C# convention: interfaces are `I` followed by an uppercase letter (IFoo).
fn is_interface_name(raw: &str) -> bool {
    // Take the trailing simple name, stripping generics + namespace qualifiers.
    let base = raw.split('<').next().unwrap_or(raw).trim();
    let simple = base.rsplit('.').next().unwrap_or(base).trim();
    let mut chars = simple.chars();
    matches!(chars.next(), Some('I'))
        && matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
}

/// G12.5: record an unresolved heritage reference from a class to a supertype.
fn emit_heritage_ref(
    raw: &str,
    category: repo_graph_core::EdgeCategoryId,
    from_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let base = raw.split('<').next().unwrap_or(raw).trim();
    let simple = base.rsplit('.').next().unwrap_or(base).trim();
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

/// G19: class-level constants / static fields. `const TYPE NAME = ...;` or
/// `static readonly TYPE NAME = ...;`. Emits a STATE_VAR node + DEFINES edge
/// per declarator. Noise gate: skip undocumented + literal-primitive fields.
fn visit_field_decl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let text = text_of(node, src);
    let is_const = text.contains("const");
    let is_static_readonly = text.contains("static") && text.contains("readonly");
    if !(is_const || is_static_readonly) {
        return;
    }
    let has_doc = repo_graph_doc::leading_doc(&node, src).is_some();

    // field_declaration → variable_declaration → variable_declarator(s).
    let mut fcursor = node.walk();
    for var_decl in node
        .named_children(&mut fcursor)
        .filter(|c| c.kind() == "variable_declaration")
    {
        let mut vcursor = var_decl.walk();
        for declarator in var_decl
            .named_children(&mut vcursor)
            .filter(|c| c.kind() == "variable_declarator")
        {
            let Some(name_node) = declarator.child_by_field_name("name") else {
                continue;
            };
            // Noise gate: undocumented + primitive-literal initializer → skip.
            if !has_doc {
                let mut dcursor = declarator.walk();
                let lit = declarator
                    .named_children(&mut dcursor)
                    .any(|c| is_primitive_literal(c.kind()));
                if lit {
                    continue;
                }
            }
            let name = text_of(name_node, src);
            let qname = format!("{parent_qname}::{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);
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
                .record(id, name, &qname, node_kind::STATE_VAR, Some(parent_id));
        }
    }
}

/// True for C# primitive/atom literal initializer node kinds.
fn is_primitive_literal(kind: &str) -> bool {
    matches!(
        kind,
        "integer_literal"
            | "real_literal"
            | "boolean_literal"
            | "character_literal"
            | "string_literal"
            | "verbatim_string_literal"
            | "raw_string_literal"
            | "null_literal"
    )
}

fn check_route_attrs(node: TsNode, src: &[u8], handler_id: NodeId, repo: RepoId, acc: &mut Acc) {
    let text = text_of(node, src);
    let aspnet = [
        ("[HttpGet", "GET"),
        ("[HttpPost", "POST"),
        ("[HttpPut", "PUT"),
        ("[HttpDelete", "DELETE"),
        ("[HttpPatch", "PATCH"),
        ("[HttpHead", "HEAD"),
        ("[HttpOptions", "OPTIONS"),
    ];
    for (prefix, method) in &aspnet {
        let mut search_from = 0;
        while let Some(rel) = text[search_from..].find(prefix) {
            let pos = search_from + rel;
            let after = &text[pos + prefix.len()..];
            let path = if after.starts_with("(\"") {
                extract_quoted(&after[1..])
            } else {
                Some("/".to_string())
            };
            if let Some(path) = path {
                emit_route(method, &path, handler_id, repo, acc);
            }
            search_from = pos + prefix.len();
        }
    }
    // [Route("/path")] — ASP.NET conventional routing, ANY method.
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find("[Route(\"") {
        let pos = search_from + rel;
        let after = &text[pos + "[Route(\"".len()..];
        if let Some(end) = after.find('"') {
            let path = &after[..end];
            emit_route("ANY", path, handler_id, repo, acc);
        }
        search_from = pos + "[Route(\"".len();
    }
    // Minimal API: app.MapGet("/path", ...)
    for method_name in &["MapGet", "MapPost", "MapPut", "MapDelete", "MapPatch", "MapHead", "MapOptions"] {
        let search = format!(".{method_name}(\"");
        let mut search_from = 0;
        while let Some(rel) = text[search_from..].find(&search) {
            let pos = search_from + rel;
            let after = &text[pos + search.len()..];
            if let Some(end) = after.find('"') {
                let path = &after[..end];
                let method = method_name.trim_start_matches("Map").to_uppercase();
                emit_route(&method, path, handler_id, repo, acc);
            }
            search_from = pos + search.len();
        }
    }
}

fn emit_route(method: &str, path: &str, handler_id: NodeId, repo: RepoId, acc: &mut Acc) {
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

fn extract_quoted(text: &str) -> Option<String> {
    let start = text.find('"')?;
    let rest = &text[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn collect_using(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let path = text
        .trim_start_matches("using ")
        .trim_start_matches("static ")
        .trim_start_matches("global ")
        .trim_end_matches(';')
        .trim();

    if path.contains('=') {
        return; // using alias directive — skip for now
    }

    if let Some(last_dot) = path.rfind('.') {
        let module_part = &path[..last_dot];
        let name = &path[last_dot + 1..];
        if name == "*" {
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Module {
                    path: module_part.replace('.', "::"),
                    alias: None,
                },
            });
        } else {
            acc.imports.push(ImportStmt {
                from_module: from_module.to_string(),
                target: ImportTarget::Symbol {
                    module: module_part.replace('.', "::"),
                    name: name.to_string(),
                    alias: None,
                    level: 0,
                },
            });
        }
    } else {
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Module {
                path: path.replace('.', "::"),
                alias: None,
            },
        });
    }
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "invocation_expression" {
            let qualifier = classify_invocation(n, src);
            acc.calls.push(CallSite { from, qualifier });
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "class_declaration"
                    | "lambda_expression"
                    | "local_function_statement"
                    | "anonymous_method_expression"
            ) {
                stack.push(child);
            }
        }
    }
}

fn classify_invocation(node: TsNode, src: &[u8]) -> CallQualifier {
    if let Some(func) = node.child_by_field_name("function") {
        match func.kind() {
            "identifier" => CallQualifier::Bare(text_of(func, src).to_string()),
            "member_access_expression" => {
                let obj = func
                    .child_by_field_name("expression")
                    .map(|n| text_of(n, src))
                    .unwrap_or("");
                let name = func
                    .child_by_field_name("name")
                    .map(|n| text_of(n, src))
                    .unwrap_or("");
                if obj == "this" {
                    CallQualifier::SelfMethod(name.to_string())
                } else if func
                    .child_by_field_name("expression")
                    .is_some_and(|v| v.kind() == "identifier")
                {
                    CallQualifier::Attribute {
                        base: obj.to_string(),
                        name: name.to_string(),
                    }
                } else {
                    CallQualifier::ComplexReceiver {
                        receiver: obj.to_string(),
                        name: name.to_string(),
                    }
                }
            }
            _ => CallQualifier::ComplexReceiver {
                receiver: text_of(func, src).to_string(),
                name: String::new(),
            },
        }
    } else {
        CallQualifier::Bare(String::new())
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
        let source = r#"
namespace MyApp.Services;

public class UserService {
    public User GetUser(string id) {
        return _db.Find(id);
    }

    private void Validate(User u) {}
}
"#;
        let fp = parse_file(source, "Services/UserService.cs", "MyApp::Services", repo()).unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"GetUser"));
        assert!(names.contains(&"Validate"));
    }

    #[test]
    fn structs_enums_interfaces() {
        let source = r#"
namespace MyApp;

public struct Point { public int X; public int Y; }
public enum Color { Red, Green, Blue }
public interface IDrawable { void Draw(); }
"#;
        let fp = parse_file(source, "Models.cs", "MyApp", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::STRUCT).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::ENUM).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
    }

    #[test]
    fn implements_and_state_var() {
        // G12.5: `class X : Base, IFoo` → INHERITS_FROM(Base) + IMPLEMENTS(IFoo).
        // G19: a documented `const int FEE = 250;` emits a STATE_VAR.
        let source = r#"
namespace MyApp;

public class X : Base, IFoo {
    /// <summary>The processing fee in cents.</summary>
    public const int FEE = 250;

    public const int RAW = 7;
}
"#;
        let fp = parse_file(source, "X.cs", "MyApp", repo()).unwrap();

        let state_vars: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::STATE_VAR)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert_eq!(state_vars, vec!["FEE"]);

        let implements = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::IMPLEMENTS)
            .count();
        assert_eq!(implements, 1);
        let inherits = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::INHERITS_FROM)
            .count();
        assert_eq!(inherits, 1);
    }

    #[test]
    fn using_imports() {
        let source = r#"
using System.Linq;
using MyApp.Models;
using static MyApp.Helpers.StringExtensions;
"#;
        let fp = parse_file(source, "App.cs", "MyApp", repo()).unwrap();
        assert_eq!(fp.imports.len(), 3);
    }

    #[test]
    fn aspnet_routes() {
        let source = r#"
namespace MyApp.Controllers;

public class UsersController {
    [HttpGet("/users")]
    public IActionResult List() { return Ok(); }

    [HttpPost("/users")]
    public IActionResult Create() { return Ok(); }
}
"#;
        let fp = parse_file(source, "Controllers/UsersController.cs", "MyApp::Controllers", repo()).unwrap();
        let routes: Vec<_> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /users"));
        assert!(routes.contains(&"POST /users"));
    }

    #[test]
    fn aspnet_full_methods_and_route_attr() {
        let source = r#"
[Route("/api/v1")]
public class ThingsController {
    [HttpGet("/things")]
    public IActionResult List() { return Ok(); }
    [HttpPut("/things/{id}")]
    public IActionResult Update() { return Ok(); }
    [HttpDelete("/things/{id}")]
    public IActionResult Destroy() { return Ok(); }
    [HttpHead("/things")]
    public IActionResult Head() { return Ok(); }
    [HttpOptions("/things")]
    public IActionResult Opts() { return Ok(); }
}
"#;
        let fp = parse_file(source, "Controllers/Things.cs", "MyApp", repo()).unwrap();
        let routes: Vec<_> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /things"));
        assert!(routes.contains(&"PUT /things/{id}"));
        assert!(routes.contains(&"DELETE /things/{id}"));
        assert!(routes.contains(&"HEAD /things"));
        assert!(routes.contains(&"OPTIONS /things"));
        assert!(routes.contains(&"ANY /api/v1"));
    }

    #[test]
    fn this_calls() {
        let source = r#"
namespace MyApp;

public class Service {
    public void Handle() {
        this.Validate();
        _helper.Process();
    }
    private void Validate() {}
}
"#;
        let fp = parse_file(source, "Service.cs", "MyApp", repo()).unwrap();
        let self_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::SelfMethod(_)))
            .collect();
        assert_eq!(self_calls.len(), 1);
    }
}
