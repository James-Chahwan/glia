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
    let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
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

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "import_declaration" => collect_import(child, src, module_qname, &mut acc),
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "record_declaration" => {
                visit_type_decl(child, src, file_rel_path, module_qname, module_id, repo, &mut acc);
            }
            _ => {}
        }
    }

    scan_ktor_routes(source, repo, &mut acc);
    scan_webflux_routes(source, repo, &mut acc);
    scan_javalin_routes(source, repo, &mut acc);

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

fn visit_type_decl(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let kind = match node.kind() {
        "class_declaration" | "record_declaration" => node_kind::CLASS,
        "interface_declaration" => node_kind::INTERFACE,
        "enum_declaration" => node_kind::ENUM,
        _ => return,
    };
    let qname = format!("{module_qname}::{name}");
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

    // G12.5: class heritage. `superclass` (extends) → INHERITS_FROM;
    // `interfaces` (super_interfaces → type_list) → IMPLEMENTS per interface.
    if let Some(superclass) = node.child_by_field_name("superclass") {
        let mut sc_cursor = superclass.walk();
        for sc in superclass.named_children(&mut sc_cursor) {
            emit_heritage_ref(text_of(sc, src), edge_category::INHERITS_FROM, id, repo, acc);
        }
    }
    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        // `interfaces` is a `super_interfaces` wrapping a `type_list`.
        let mut if_cursor = interfaces.walk();
        for type_list in interfaces.named_children(&mut if_cursor) {
            let mut tl_cursor = type_list.walk();
            for iface in type_list.named_children(&mut tl_cursor) {
                emit_heritage_ref(text_of(iface, src), edge_category::IMPLEMENTS, id, repo, acc);
            }
        }
    }

    // Walk body for methods + nested types.
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
            "class_declaration" | "interface_declaration" | "enum_declaration"
            | "record_declaration" => {
                visit_type_decl(child, src, file_rel, &qname, id, repo, acc);
            }
            _ => {}
        }
    }

    // Check for Spring/JAX-RS route annotations on the class.
    check_route_annotations(node, src, file_rel, id, repo, acc);
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

    // Check for route annotations on the method.
    check_route_annotations(node, src, file_rel, id, repo, acc);
}

/// G12.5: record an unresolved heritage reference (extends/implements) from a
/// class to a supertype name. The graph resolver turns the name into an edge;
/// here we record the directional edge intent with the given category.
fn emit_heritage_ref(
    raw: &str,
    category: repo_graph_core::EdgeCategoryId,
    from_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // Strip generic args (e.g. `Comparable<Foo>` → `Comparable`) and take the
    // trailing simple name (e.g. `pkg.Base` → `Base`).
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

/// G19: class-level constants / static fields. Emits a STATE_VAR node for each
/// declarator in a `static final` field, plus a DEFINES edge class→field.
/// Noise gate: skip when undocumented AND the initializer is a primitive literal.
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
    // Only class-level constants: must be both `static` and `final`.
    if !(text.contains("static") && text.contains("final")) {
        return;
    }
    let has_doc = repo_graph_doc::leading_doc(&node, src).is_some();

    let mut cursor = node.walk();
    for declarator in node.children_by_field_name("declarator", &mut cursor) {
        let Some(name_node) = declarator.child_by_field_name("name") else {
            continue;
        };
        // Noise gate: undocumented + literal-primitive initializer → skip.
        if !has_doc {
            if let Some(value) = declarator.child_by_field_name("value") {
                if is_primitive_literal(value.kind()) {
                    continue;
                }
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

/// True for Java primitive/atom literal initializer node kinds.
fn is_primitive_literal(kind: &str) -> bool {
    matches!(
        kind,
        "decimal_integer_literal"
            | "hex_integer_literal"
            | "octal_integer_literal"
            | "binary_integer_literal"
            | "decimal_floating_point_literal"
            | "hex_floating_point_literal"
            | "character_literal"
            | "string_literal"
            | "true"
            | "false"
            | "null_literal"
    )
}

fn check_route_annotations(
    node: TsNode,
    src: &[u8],
    _file_rel: &str,
    handler_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // Walk siblings/markers before this node looking for annotations.
    // In Java tree-sitter, annotations are modifiers on the declaration.
    let text = text_of(node, src);

    // Spring: @GetMapping("/path"), @PostMapping, @RequestMapping
    // Micronaut: @Get("/path"), @Post, @Put, @Delete, @Patch, @Head, @Options (no Mapping suffix)
    let patterns = [
        ("@GetMapping", "GET"),
        ("@PostMapping", "POST"),
        ("@PutMapping", "PUT"),
        ("@DeleteMapping", "DELETE"),
        ("@PatchMapping", "PATCH"),
        ("@Get(", "GET"),
        ("@Post(", "POST"),
        ("@Put(", "PUT"),
        ("@Delete(", "DELETE"),
        ("@Patch(", "PATCH"),
        ("@Head(", "HEAD"),
        ("@Options(", "OPTIONS"),
    ];
    for (prefix, method) in &patterns {
        if let Some(pos) = text.find(prefix)
            && let Some(path) = extract_annotation_string(&text[pos..])
        {
            emit_route(method, &path, handler_id, repo, acc);
        }
    }
    // Micronaut @Controller("/api") at class level — emit as ANY base route.
    if let Some(pos) = text.find("@Controller(")
        && let Some(path) = extract_annotation_string(&text[pos..])
    {
        emit_route("ANY", &path, handler_id, repo, acc);
    }
    // @RequestMapping with method param
    if let Some(pos) = text.find("@RequestMapping")
        && let Some(path) = extract_annotation_string(&text[pos..])
    {
        emit_route("ANY", &path, handler_id, repo, acc);
    }
    // JAX-RS: @Path("/path") + @GET/@POST
    if let Some(pos) = text.find("@Path")
        && let Some(path) = extract_annotation_string(&text[pos..])
    {
        let method = if text.contains("@GET") {
            "GET"
        } else if text.contains("@POST") {
            "POST"
        } else if text.contains("@PUT") {
            "PUT"
        } else if text.contains("@DELETE") {
            "DELETE"
        } else {
            "ANY"
        };
        emit_route(method, &path, handler_id, repo, acc);
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

fn scan_ktor_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Ktor (Kotlin): `get("/path") { ... }`, `post("/path") { ... }`, etc.
    // File is Kotlin (tree-sitter-java rejects most of it so we rely on text).
    let methods: &[(&str, &str)] = &[
        ("get(\"", "GET"),
        ("post(\"", "POST"),
        ("put(\"", "PUT"),
        ("patch(\"", "PATCH"),
        ("delete(\"", "DELETE"),
        ("head(\"", "HEAD"),
        ("options(\"", "OPTIONS"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            // Require the needle to be a word-start so we don't match e.g.
            // `forget("...")` or `setget("...")`.
            let word_start = pos == 0 || {
                let prev = source.as_bytes()[pos - 1];
                !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.')
            };
            let start = pos + needle.len();
            if !word_start {
                search_from = start;
                continue;
            }
            let bytes = source.as_bytes();
            let mut j = start;
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
            let path = &source[start..j];
            // Ktor DSL expects routes to start with `/`. This filters out many
            // false positives (e.g., `get("count")`) at zero cost.
            if !path.starts_with('/') {
                search_from = j + 1;
                continue;
            }
            // Look ahead for opening `{` — Ktor route DSL always opens a block.
            let after_paren = source[j + 1..]
                .find(|c: char| !c.is_whitespace() && c != ')')
                .map(|o| source.as_bytes()[j + 1 + o]);
            if after_paren != Some(b'{') {
                search_from = j + 1;
                continue;
            }
            let key = format!("{method} {path}");
            if seen.insert(key.clone()) {
                emit_ktor_route(method, path, repo, acc);
            }
            search_from = j + 1;
        }
    }
}

fn scan_webflux_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Spring WebFlux functional DSL: RouterFunctions.route().GET("/path", h).POST(...)
    let methods: &[(&str, &str)] = &[
        (".GET(\"", "GET"),
        (".POST(\"", "POST"),
        (".PUT(\"", "PUT"),
        (".PATCH(\"", "PATCH"),
        (".DELETE(\"", "DELETE"),
        (".HEAD(\"", "HEAD"),
        (".OPTIONS(\"", "OPTIONS"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let bytes = source.as_bytes();
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let start = pos + needle.len();
            let mut j = start;
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
            let path = &source[start..j];
            if !path.starts_with('/') {
                search_from = j + 1;
                continue;
            }
            let key = format!("{method} {path}");
            if seen.insert(key.clone()) {
                emit_ktor_route(method, path, repo, acc);
            }
            search_from = j + 1;
        }
    }
}

fn scan_javalin_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Javalin: `app.get("/path", handler)` / `app.post("/path", ctx -> {...})`.
    // Distinct from Ktor (top-level `get("/path") { ... }`): Javalin always has
    // a receiver (`app.` / `router.`) and never a trailing `{` block — both
    // ruled out by the Ktor scanner above. Discriminator from `Map.get("k")`
    // is the path-`/` first-arg filter.
    let methods: &[(&str, &str)] = &[
        (".get(\"", "GET"),
        (".post(\"", "POST"),
        (".put(\"", "PUT"),
        (".patch(\"", "PATCH"),
        (".delete(\"", "DELETE"),
        (".head(\"", "HEAD"),
        (".options(\"", "OPTIONS"),
    ];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let bytes = source.as_bytes();
    for (needle, method) in methods {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let start = pos + needle.len();
            let mut j = start;
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
            let path = &source[start..j];
            if !path.starts_with('/') {
                search_from = j + 1;
                continue;
            }
            // Must have a comma after the path (Javalin always takes a handler
            // as the second arg). Filters out single-arg `.get("/x")` fetcher
            // calls that happen to use a slash key.
            let after = source[j + 1..].trim_start();
            if !after.starts_with(',') {
                search_from = j + 1;
                continue;
            }
            let key = format!("{method} {path}");
            if seen.insert(key.clone()) {
                emit_ktor_route(method, path, repo, acc);
            }
            search_from = j + 1;
        }
    }
}

fn emit_ktor_route(method: &str, path: &str, repo: RepoId, acc: &mut Acc) {
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

fn extract_annotation_string(text: &str) -> Option<String> {
    let paren = text.find('(')?;
    let rest = &text[paren + 1..];
    // Find first quoted string: "..." or value = "..."
    let quote_start = rest.find('"')?;
    let after = &rest[quote_start + 1..];
    let quote_end = after.find('"')?;
    Some(after[..quote_end].to_string())
}

fn collect_import(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    // `import com.foo.bar.Baz;` or `import static com.foo.bar.Baz.method;`
    let text = text_of(node, src).trim().to_string();
    let path = text
        .trim_start_matches("import ")
        .trim_start_matches("static ")
        .trim_end_matches(';')
        .trim();

    if path.ends_with(".*") {
        // Wildcard import — module import
        let module_path = path.trim_end_matches(".*").replace('.', "::");
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Module {
                path: module_path,
                alias: None,
            },
        });
    } else if let Some(last_dot) = path.rfind('.') {
        let module_part = &path[..last_dot];
        let name = &path[last_dot + 1..];
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
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "method_invocation" {
            let qualifier = classify_method_invocation(n, src);
            acc.calls.push(CallSite { from, qualifier });
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "class_declaration"
                    | "lambda_expression"
                    | "method_declaration"
                    | "anonymous_class_body"
            ) {
                stack.push(child);
            }
        }
    }
}

fn classify_method_invocation(node: TsNode, src: &[u8]) -> CallQualifier {
    let name = node
        .child_by_field_name("name")
        .map(|n| text_of(n, src))
        .unwrap_or("");
    if let Some(obj) = node.child_by_field_name("object") {
        let obj_text = text_of(obj, src);
        if obj_text == "this" {
            CallQualifier::SelfMethod(name.to_string())
        } else if obj.kind() == "identifier" {
            CallQualifier::Attribute {
                base: obj_text.to_string(),
                name: name.to_string(),
            }
        } else {
            CallQualifier::ComplexReceiver {
                receiver: obj_text.to_string(),
                name: name.to_string(),
            }
        }
    } else {
        CallQualifier::Bare(name.to_string())
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
package com.example;

public class UserService {
    public User getUser(String id) {
        return db.find(id);
    }

    private void validate(User u) {}
}
"#;
        let fp = parse_file(source, "src/main/java/UserService.java", "com::example", repo()).unwrap();
        let names: Vec<&str> = fp.nav.name_by_id.values().map(|s| s.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"getUser"));
        assert!(names.contains(&"validate"));
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 2);
    }

    #[test]
    fn interfaces_and_enums() {
        let source = r#"
package com.example;

public interface Drawable {
    void draw();
}

public enum Color {
    RED, GREEN, BLUE;
}
"#;
        let fp = parse_file(source, "src/main/java/Types.java", "com::example", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::ENUM).count(), 1);
    }

    #[test]
    fn implements_and_state_var() {
        // G12.5: `implements IFoo` emits an IMPLEMENTS edge (class → interface).
        // G19: a documented `static final int FEE = 250;` emits a STATE_VAR.
        let source = r#"
package com.example;

public class X extends Base implements IFoo, IBar {
    /** The processing fee in cents. */
    public static final int FEE = 250;

    public static final int RAW = 7;
}
"#;
        let fp = parse_file(source, "src/main/java/X.java", "com::example", repo()).unwrap();

        // STATE_VAR: only the documented FEE survives the noise gate.
        let state_vars: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::STATE_VAR)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert_eq!(state_vars, vec!["FEE"]);

        // IMPLEMENTS edges: one per interface (IFoo, IBar).
        let implements = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::IMPLEMENTS)
            .count();
        assert_eq!(implements, 2);
        // extends Base → INHERITS_FROM.
        let inherits = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::INHERITS_FROM)
            .count();
        assert_eq!(inherits, 1);
    }

    #[test]
    fn imports() {
        let source = r#"
package com.example;

import com.example.models.User;
import java.util.*;
import static org.junit.Assert.assertEquals;
"#;
        let fp = parse_file(source, "src/main/java/App.java", "com::example", repo()).unwrap();
        assert_eq!(fp.imports.len(), 3);
    }

    #[test]
    fn spring_routes() {
        let source = r#"
package com.example;

public class UserController {
    @GetMapping("/users")
    public List<User> list() { return null; }

    @PostMapping("/users")
    public User create() { return null; }
}
"#;
        let fp = parse_file(source, "src/main/java/UserController.java", "com::example", repo()).unwrap();
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
    fn micronaut_routes() {
        let source = r#"
package com.example;

@Controller("/api")
public class ThingsController {
    @Get("/things")
    public Thing list() { return null; }

    @Post("/things")
    public Thing create() { return null; }

    @Put("/things/{id}")
    public Thing update() { return null; }

    @Delete("/things/{id}")
    public void destroy() {}
}
"#;
        let fp = parse_file(source, "ThingsController.java", "com::example", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /things"));
        assert!(routes.contains(&"PUT /things/{id}"));
        assert!(routes.contains(&"DELETE /things/{id}"));
        assert!(routes.contains(&"ANY /api"));
    }

    #[test]
    fn ktor_routes() {
        let source = r#"
fun Application.module() {
    routing {
        get("/users") {
            call.respond(listOf<String>())
        }
        post("/users") {
            call.respond("ok")
        }
        route("/admin") {
            delete("/users/{id}") { call.respond("ok") }
        }
    }
}
"#;
        let fp = parse_file(source, "Application.kt", "com::example", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /users"));
        assert!(routes.contains(&"POST /users"));
        assert!(routes.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn webflux_functional_routes() {
        let source = r#"
@Configuration
public class RouterConfig {
    @Bean
    public RouterFunction<ServerResponse> routes(UserHandler handler) {
        return RouterFunctions.route()
            .GET("/users", handler::list)
            .POST("/users", handler::create)
            .DELETE("/users/{id}", handler::destroy)
            .build();
    }
}
"#;
        let fp = parse_file(source, "RouterConfig.java", "com::example", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /users"));
        assert!(routes.contains(&"POST /users"));
        assert!(routes.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn javalin_routes() {
        let source = r#"
import io.javalin.Javalin;

public class App {
    public static void main(String[] args) {
        Javalin app = Javalin.create();
        app.get("/health", ctx -> ctx.result("ok"));
        app.post("/users", UserHandler::create);
        app.put("/users/{id}", UserHandler::update);
        app.delete("/users/{id}", UserHandler::destroy);
    }
}
"#;
        let fp = parse_file(source, "App.java", "com::example", repo()).unwrap();
        let routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(routes.contains(&"GET /health"));
        assert!(routes.contains(&"POST /users"));
        assert!(routes.contains(&"PUT /users/{id}"));
        assert!(routes.contains(&"DELETE /users/{id}"));
    }

    #[test]
    fn javalin_skips_map_get_with_path_key() {
        // `cache.get("/users")` shape: path-`/` filter alone would let it
        // through; the comma-after-path filter rejects it (single-arg call).
        let source = r#"
public class Svc {
    public String load() {
        return cache.get("/users");
    }
}
"#;
        let fp = parse_file(source, "Svc.java", "com::example", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "single-arg `.get(\"/key\")` must not emit a route");
    }

    #[test]
    fn javalin_skips_non_path_first_arg() {
        let source = r#"
public class Svc {
    public String load() {
        return cache.get("user-id", fallback);
    }
}
"#;
        let fp = parse_file(source, "Svc.java", "com::example", repo()).unwrap();
        let has_route = fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE);
        assert!(!has_route, "non-`/` first arg must not emit a route");
    }

    #[test]
    fn this_calls() {
        let source = r#"
package com.example;

public class Service {
    public void handle() {
        this.validate();
        helper.process();
    }
    private void validate() {}
}
"#;
        let fp = parse_file(source, "src/main/java/Service.java", "com::example", repo()).unwrap();
        let self_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::SelfMethod(_)))
            .collect();
        assert_eq!(self_calls.len(), 1);
        let attr_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::Attribute { .. }))
            .collect();
        assert_eq!(attr_calls.len(), 1);
    }
}
