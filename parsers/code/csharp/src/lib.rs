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

    visit_children(root, src, file_rel_path, module_qname, module_id, module_id, repo, &mut acc);

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
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "using_directive" => collect_using(child, src, parent_qname, acc),
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                visit_namespace(child, src, file_rel, parent_qname, parent_id, module_id, repo, acc);
            }
            "class_declaration" | "struct_declaration" | "interface_declaration"
            | "enum_declaration" | "record_declaration" | "record_struct_declaration" => {
                visit_type_decl(child, src, file_rel, parent_qname, parent_id, module_id, repo, acc);
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
    module_id: NodeId,
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
        visit_children(node, src, file_rel, &qname, ns_id, module_id, repo, acc);
    } else if let Some(body) = node.child_by_field_name("body") {
        visit_children(body, src, file_rel, &qname, ns_id, module_id, repo, acc);
    }
}

fn visit_type_decl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    module_id: NodeId,
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
            emit_heritage_ref(raw, category, id, module_id, acc);
        }
    }

    // Pattern E (DI): is this a class that receives injected dependencies?
    // Gate on a DI signal (controller/service-shaped name or DI attribute) so a
    // plain data class with a constructor doesn't emit spurious INJECTS edges.
    let is_di = is_di_class(name, text_of(node, src));

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "method_declaration" | "constructor_declaration" => {
                visit_method(child, src, file_rel, &qname, id, repo, acc);
                if is_di && child.kind() == "constructor_declaration" {
                    emit_ctor_injects(child, src, id, module_id, repo, acc);
                }
            }
            "field_declaration" => {
                visit_field_decl(child, src, file_rel, &qname, id, repo, acc);
            }
            "class_declaration" | "struct_declaration" | "interface_declaration"
            | "enum_declaration" | "record_declaration" | "record_struct_declaration" => {
                visit_type_decl(child, src, file_rel, &qname, id, module_id, repo, acc);
            }
            _ => {}
        }
    }

    check_route_attrs(node, src, id, repo, acc);
}

/// Pattern E gate: does this type look like a DI consumer? ASP.NET controllers
/// and services receive dependencies via constructor injection. We recognise
/// them by a conventional name suffix or a DI-registration attribute, which
/// avoids flagging plain data/DTO classes that merely happen to have a ctor.
fn is_di_class(name: &str, node_text: &str) -> bool {
    const DI_SUFFIXES: [&str; 9] = [
        "Controller",
        "Service",
        "Repository",
        "Handler",
        "Manager",
        "Provider",
        "Factory",
        "Middleware",
        "Worker",
    ];
    if DI_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    // Attribute-based signals (search only the leading attribute region — the
    // class body is irrelevant and could contain unrelated text).
    let head = &node_text[..node_text.find('{').unwrap_or(node_text.len())];
    head.contains("[ApiController]")
        || head.contains("[Controller]")
        || head.contains("[Service")
        || head.contains("[Injectable")
}

/// Pattern E: for each constructor parameter whose type is a class/interface
/// (not a primitive), push an INJECTS `UnresolvedRef` from the consumer class
/// to that dependency type. The graph resolver binds the bare type name to the
/// uniquely-named class/interface node and forms the edge.
fn emit_ctor_injects(
    ctor: TsNode,
    src: &[u8],
    class_id: NodeId,
    module_id: NodeId,
    _repo: RepoId,
    acc: &mut Acc,
) {
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        if param.kind() != "parameter" {
            continue;
        }
        let Some(type_node) = param.child_by_field_name("type") else {
            continue;
        };
        let Some(type_name) = injectable_type_name(type_node, src) else {
            continue;
        };
        acc.refs.push(UnresolvedRef {
            from: class_id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(type_name),
            category: edge_category::INJECTS,
        });
    }
}

/// Extract the bare dependency type name from a parameter `type` node, or
/// `None` if it's a primitive/built-in that isn't a DI target. Strips generic
/// arguments and namespace qualifiers (`Shop.Services.IUserService<T>` →
/// `IUserService`).
fn injectable_type_name(type_node: TsNode, src: &[u8]) -> Option<String> {
    // `predefined_type` covers int/string/bool/... — never a DI dependency.
    if type_node.kind() == "predefined_type" {
        return None;
    }
    let raw = text_of(type_node, src);
    let base = raw.split('<').next().unwrap_or(raw).trim();
    let simple = base.rsplit('.').next().unwrap_or(base).trim();
    let simple = simple.trim_end_matches('?'); // nullable reference type
    if simple.is_empty() || is_primitive_type(simple) {
        return None;
    }
    Some(simple.to_string())
}

/// C# built-in / value types that are never resolved as injected services.
fn is_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "int" | "uint" | "long" | "ulong" | "short" | "ushort"
            | "byte" | "sbyte" | "float" | "double" | "decimal"
            | "bool" | "char" | "string" | "object" | "void"
            | "Int32" | "Int64" | "UInt32" | "UInt64" | "Boolean"
            | "String" | "Char" | "Byte" | "Double" | "Single"
            | "Decimal" | "Object" | "DateTime" | "TimeSpan" | "Guid"
    )
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
/// Emits an `UnresolvedRef` (not a name-derived `Edge`) so `resolve_refs` binds
/// the bare base-type name to the uniquely-named class/interface node across the
/// repo (INHERITS_FROM for a base class, IMPLEMENTS for an interface). External
/// bases (e.g. `ControllerBase`) simply stay unresolved, which is fine.
fn emit_heritage_ref(
    raw: &str,
    category: repo_graph_core::EdgeCategoryId,
    from_id: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let base = raw.split('<').next().unwrap_or(raw).trim();
    let simple = base.rsplit('.').next().unwrap_or(base).trim();
    if simple.is_empty() {
        return;
    }
    acc.refs.push(UnresolvedRef {
        from: from_id,
        from_module: module_id,
        qualifier: CallQualifier::Bare(simple.to_string()),
        category,
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
            // No receiver: an unqualified call `Load()` inside a method is an
            // implicit `this.Load()` — a method of the enclosing type (or an
            // inherited one). C# has no module-level free functions, so a bare
            // invocation never resolves against module top-level symbols;
            // classify it as `SelfMethod` so `resolve_calls` binds it against
            // the enclosing class's methods (`class_methods[<enclosing class>]`).
            // Mirrors the Java parser, which faces the same no-free-functions
            // shape. (A genuinely bare static-imported call — `using static` —
            // simply falls through to unresolved, same as before.)
            "identifier" => CallQualifier::SelfMethod(text_of(func, src).to_string()),
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
        // G12.5: `class X : Base, IFoo` → INHERITS_FROM(Base) + IMPLEMENTS(IFoo),
        // both emitted as `UnresolvedRef`s (Bare base-type name) so resolve_refs
        // binds them to the uniquely-named class/interface — not name-derived
        // phantom edges.
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

        // Heritage is now REFs, not name-derived edges.
        let implements: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::IMPLEMENTS)
            .collect();
        assert_eq!(implements.len(), 1);
        assert_eq!(
            implements[0].qualifier,
            CallQualifier::Bare("IFoo".to_string())
        );
        let inherits: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INHERITS_FROM)
            .collect();
        assert_eq!(inherits.len(), 1);
        assert_eq!(
            inherits[0].qualifier,
            CallQualifier::Bare("Base".to_string())
        );

        // No phantom name-derived heritage edges remain.
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.category == edge_category::IMPLEMENTS
                    || e.category == edge_category::INHERITS_FROM)
                .count(),
            0
        );
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
    fn di_constructor_injects() {
        // Pattern E: an ASP.NET controller whose constructor takes an
        // interface-typed dependency emits an INJECTS UnresolvedRef with the
        // bare service type name; the primitive `int` param is skipped.
        let source = r#"
using Shop.Services;

namespace Shop.Controllers
{
    [ApiController]
    public class UsersController : ControllerBase
    {
        private readonly IUserService _userService;

        public UsersController(IUserService userService, int page)
        {
            _userService = userService;
        }
    }
}
"#;
        let fp = parse_file(source, "Controllers/UsersController.cs", "Shop::Controllers", repo()).unwrap();
        let injects: Vec<&UnresolvedRef> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .collect();
        assert_eq!(injects.len(), 1, "exactly one INJECTS ref (primitive skipped)");
        assert_eq!(
            injects[0].qualifier,
            CallQualifier::Bare("IUserService".to_string())
        );
    }

    #[test]
    fn di_gate_skips_plain_data_class() {
        // A plain data class with a class-typed ctor param must NOT emit INJECTS.
        let source = r#"
namespace Shop.Models
{
    public class Order
    {
        public Order(Customer customer) {}
    }
}
"#;
        let fp = parse_file(source, "Models/Order.cs", "Shop::Models", repo()).unwrap();
        assert_eq!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::INJECTS)
                .count(),
            0
        );
    }

    #[test]
    fn calls_selfmethod_and_attribute() {
        // Pattern C (CALLS): mirrors the csharp-aspnet fixture.
        //  - a bare same-class call `Load(id)` must be `SelfMethod("Load")` so
        //    resolve_calls binds it against the enclosing class's methods
        //    (a plain `Bare` would never resolve — C# has no free functions);
        //  - a field-qualified call `_userService.GetById(id)` is
        //    `Attribute { base: "_userService", name: "GetById" }` (instance
        //    dispatch — the graph resolves it only when the field's declared
        //    type is a locally-bound class);
        //  - an explicit `this.Helper()` is `SelfMethod("Helper")`.
        let source = r#"
namespace Shop.Services
{
    public class UserService
    {
        public User GetById(int id)
        {
            return Load(id);
        }

        private User Load(int id)
        {
            this.Helper();
            return new User();
        }
    }

    public class UsersController
    {
        private readonly IUserService _userService;
        public User GetUser(int id)
        {
            return _userService.GetById(id);
        }
    }
}
"#;
        let fp = parse_file(source, "Services.cs", "Shop::Services", repo()).unwrap();

        // bare `Load(id)` -> SelfMethod("Load")
        assert!(
            fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::SelfMethod("Load".to_string())),
            "bare same-class call must be SelfMethod(\"Load\"), got: {:?}",
            fp.calls
        );
        // and must NOT be emitted as Bare (would not resolve against class methods)
        assert!(
            !fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::Bare("Load".to_string())),
            "bare intra-class call must be SelfMethod, not Bare"
        );
        // `this.Helper()` -> SelfMethod("Helper")
        assert!(
            fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::SelfMethod("Helper".to_string())),
            "this.Helper() must be SelfMethod(\"Helper\")"
        );
        // `_userService.GetById(id)` -> Attribute { base: "_userService", name: "GetById" }
        assert!(
            fp.calls.iter().any(|c| c.qualifier
                == CallQualifier::Attribute {
                    base: "_userService".to_string(),
                    name: "GetById".to_string(),
                }),
            "field-qualified call must be Attribute{{_userService, GetById}}, got: {:?}",
            fp.calls
        );
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
