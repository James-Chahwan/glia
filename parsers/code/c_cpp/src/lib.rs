use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser, Tree};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};

/// Which tree-sitter grammar a C/C++ file goes to (LB.10a). `.c` is C;
/// `.cc` / `.cpp` / `.cxx` / `.hpp` / `.hh` / `.hxx` are C++; a `.h` can be
/// either, so it is decided per file from the parse itself
/// ([`Dialect::Header`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    C,
    Cpp,
    /// Parsed with the C++ grammar (close to a superset of C); only when that
    /// tree has errors is it re-parsed with C, and the tree with fewer
    /// ERROR-covered bytes wins (C++ on ties). Bytes, not node counts: one C
    /// ERROR node can swallow a whole `class {...}` while a macro-heavy C++
    /// header produces several small ones.
    Header,
}

impl Dialect {
    /// The dialect of a file by its extension; anything that is not a C or
    /// C++ source / header extension (`.h`, no extension) is a
    /// [`Dialect::Header`].
    pub fn from_path(path: &str) -> Self {
        match Path::new(path).extension().and_then(|e| e.to_str()) {
            Some("c") => Dialect::C,
            Some("cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx") => Dialect::Cpp,
            _ => Dialect::Header,
        }
    }
}

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    dialect: Dialect,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let parsed = parse_tree(source, dialect)?;
    if dialect == Dialect::Header && qname_debug() {
        // LB.10a debug marker:
        //   `GLIA_QNAME_DEBUG=1 glia analyze <repo> 2>&1 | grep '\[qname\] c_cpp:'`
        let chosen = if parsed.dialect == Dialect::C { "c" } else { "cpp" };
        let c_bytes = parsed.c_error_bytes.map_or_else(|| "-".to_string(), |b| b.to_string());
        eprintln!(
            "[qname] c_cpp: header parsed as {chosen} (cpp_error_bytes={}, c_error_bytes={c_bytes}) file={file_rel_path}",
            parsed.cpp_error_bytes
        );
    }
    let tree = parsed.tree;
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc::default();

    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    acc.push_node(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: file_cells(&root, src, file_rel_path),
    });
    acc.nav.record(
        module_id,
        &module_name(file_rel_path, module_qname),
        module_qname,
        node_kind::MODULE,
        None,
    );

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

/// A MODULE's nav name is its file stem (LB.10a): every C/C++ MODULE is named
/// by its full file name (`src::Widget.h`), and the stem (`Widget`) is the
/// name `bare_module_qname` reads the bare path (`src::Widget`) back from.
/// A path with no stem keeps the qname's last segment.
fn module_name(file_rel_path: &str, module_qname: &str) -> String {
    match Path::new(file_rel_path).file_stem() {
        Some(stem) if !stem.is_empty() => stem.to_string_lossy().into_owned(),
        _ => module_qname.rsplit("::").next().unwrap_or(module_qname).to_string(),
    }
}

/// A parsed file and the grammar that parsed it.
struct Parsed {
    tree: Tree,
    /// [`Dialect::C`] or [`Dialect::Cpp`], never [`Dialect::Header`].
    dialect: Dialect,
    /// [`error_weight`] of the C++ tree when a header's re-parse needed it;
    /// 0 when not measured (a `.c` or C++ file, or a header whose C++ tree has
    /// no error).
    cpp_error_bytes: usize,
    /// [`error_weight`] of the C tree, when one was parsed.
    c_error_bytes: Option<usize>,
}

fn parse_with(source: &str, lang: tree_sitter::Language) -> Result<Tree, ParseError> {
    let mut parser = Parser::new();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    parser.parse(source, None).ok_or(ParseError::NoTree)
}

/// Parse `source` in `dialect`; a [`Dialect::Header`] is resolved to the
/// grammar whose tree has fewer error bytes (C++ unless C is strictly better).
fn parse_tree(source: &str, dialect: Dialect) -> Result<Parsed, ParseError> {
    match dialect {
        Dialect::C => Ok(Parsed {
            tree: parse_with(source, tree_sitter_c::LANGUAGE.into())?,
            dialect: Dialect::C,
            cpp_error_bytes: 0,
            c_error_bytes: None,
        }),
        Dialect::Cpp | Dialect::Header => {
            let cpp = parse_with(source, tree_sitter_cpp::LANGUAGE.into())?;
            if dialect == Dialect::Cpp || !cpp.root_node().has_error() {
                return Ok(Parsed { tree: cpp, dialect: Dialect::Cpp, cpp_error_bytes: 0, c_error_bytes: None });
            }
            let cpp_error_bytes = error_weight(cpp.root_node());
            let c = parse_with(source, tree_sitter_c::LANGUAGE.into())?;
            let c_error_bytes = error_weight(c.root_node());
            let (tree, dialect) = if c_error_bytes < cpp_error_bytes {
                (c, Dialect::C)
            } else {
                (cpp, Dialect::Cpp)
            };
            Ok(Parsed { tree, dialect, cpp_error_bytes, c_error_bytes: Some(c_error_bytes) })
        }
    }
}

/// Bytes covered by the outermost ERROR nodes of the tree under `root`, plus
/// one per MISSING node. An iterative cursor walk that never descends into an
/// ERROR node (no double counting) nor into an error-free subtree.
fn error_weight(root: TsNode) -> usize {
    let mut weight = 0usize;
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        let descend = if node.is_error() {
            weight += node.end_byte().saturating_sub(node.start_byte());
            false
        } else {
            if node.is_missing() {
                weight += 1;
            }
            node.has_error()
        };
        if descend && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return weight;
            }
        }
    }
}

/// `GLIA_QNAME_DEBUG=1` turns on the per-header `[qname] c_cpp:` marker, read
/// once. Off by default: it would print for every header of a build.
fn qname_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("GLIA_QNAME_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
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
    /// LB.10a: id -> index in `nodes` of every node emitted so far. Lookup
    /// only, never iterated.
    emitted: HashMap<NodeId, usize>,
}

impl Acc {
    /// Push `node` unless its id was emitted before in this file, in which
    /// case its cells are appended to the first node (`merge_parses`'
    /// duplicate rule, applied inside one file). True on the first emission:
    /// only then does the caller add the DEFINES / CONTAINS edge and the nav
    /// record, so a function defined in both branches of an `#ifdef`, a
    /// reopened namespace or an overload set is one node with one edge.
    fn push_node(&mut self, node: Node) -> bool {
        if let Some(&idx) = self.emitted.get(&node.id) {
            if let Some(first) = self.nodes.get_mut(idx) {
                first.cells.extend(node.cells);
            }
            return false;
        }
        self.emitted.insert(node.id, self.nodes.len());
        self.nodes.push(node);
        true
    }
}

/// The preprocessor conditionals whose children are walked (LB.10a). Include
/// guards and platform switches wrap most headers, and tree-sitter does not
/// evaluate them, so every branch is walked.
fn is_preproc_block(kind: &str) -> bool {
    matches!(
        kind,
        "preproc_ifdef" | "preproc_if" | "preproc_else" | "preproc_elif" | "preproc_elifdef"
    )
}

/// The node of a preprocessor conditional to walk: the node itself (its
/// `alternative` is a child and reached the same way), or for a literal
/// `#if 0`, the one dead branch, only its `alternative`.
fn live_branch<'t>(node: TsNode<'t>, src: &[u8]) -> Option<TsNode<'t>> {
    let dead = node.kind() == "preproc_if"
        && node
            .child_by_field_name("condition")
            .is_some_and(|c| text_of(c, src).trim() == "0");
    if dead {
        node.child_by_field_name("alternative")
    } else {
        Some(node)
    }
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
        visit_one(child, src, file_rel, parent_qname, parent_id, repo, acc);
    }
}

fn visit_one(
    child: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    match child.kind() {
        "preproc_include" => collect_include(child, src, parent_qname, acc),
        "function_definition" => {
            visit_function(child, src, file_rel, parent_qname, parent_id, repo, acc);
        }
        "struct_specifier" | "class_specifier" => {
            visit_type(child, src, file_rel, parent_qname, parent_id, repo, acc);
        }
        "enum_specifier" => {
            visit_enum(child, src, file_rel, parent_qname, parent_id, repo, acc);
        }
        "namespace_definition" => {
            visit_namespace(child, src, file_rel, parent_qname, parent_id, repo, acc);
        }
        "declaration" => {
            // A declaration whose type is a class / struct body
            // (`struct P { int x; } p;`); a bodiless one declares nothing.
            let mut c2 = child.walk();
            for gc in child.named_children(&mut c2) {
                if gc.kind() == "struct_specifier" || gc.kind() == "class_specifier" {
                    visit_type(gc, src, file_rel, parent_qname, parent_id, repo, acc);
                }
            }
        }
        kind if is_preproc_block(kind) => {
            if let Some(branch) = live_branch(child, src) {
                visit_children(branch, src, file_rel, parent_qname, parent_id, repo, acc);
            }
        }
        _ => {}
    }
}

fn visit_namespace(
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
    let ns_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE, &qname);

    let first = acc.push_node(Node {
        id: ns_id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: parent_id,
            to: ns_id,
            category: edge_category::CONTAINS,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(ns_id, name, &qname, node_kind::PACKAGE, Some(parent_id));
    }

    if let Some(body) = node.child_by_field_name("body") {
        visit_children(body, src, file_rel, &qname, ns_id, repo, acc);
    }
}

fn visit_type(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // LB.10a: a bodiless specifier (`class Gadget;`, the `struct point` of
    // `struct point *point_new(int);`) is a forward declaration or a use of
    // the type, never the type itself.
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let kind = if node.kind() == "class_specifier" {
        node_kind::CLASS
    } else {
        node_kind::STRUCT
    };
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);

    let first = acc.push_node(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav.record(id, name, &qname, kind, Some(parent_id));
    }

    visit_members(body, src, file_rel, &qname, id, repo, acc);
}

/// The inline methods of a class / struct body, including those inside its
/// preprocessor conditionals (LB.10a).
fn visit_members(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    class_qname: &str,
    class_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                visit_method(child, src, file_rel, class_qname, class_id, repo, acc);
            }
            kind if is_preproc_block(kind) => {
                if let Some(branch) = live_branch(child, src) {
                    visit_members(branch, src, file_rel, class_qname, class_id, repo, acc);
                }
            }
            _ => {}
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
    // LB.10a: `enum class E : int;` declares no enumerators, no type.
    if node.child_by_field_name("body").is_none() {
        return;
    }
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENUM, &qname);

    let first = acc.push_node(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, name, &qname, node_kind::ENUM, Some(parent_id));
    }
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
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return;
    };
    let name = extract_func_name(declarator, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    let first = acc.push_node(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, &name, &qname, node_kind::FUNCTION, Some(parent_id));
    }

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
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
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return;
    };
    let name = extract_func_name(declarator, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);

    let first = acc.push_node(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, &name, &qname, node_kind::METHOD, Some(parent_id));
    }

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
    }
}

fn extract_func_name(declarator: TsNode, src: &[u8]) -> String {
    // Walk down through function_declarator → pointer_declarator → etc. to find identifier
    let mut node = declarator;
    loop {
        if let Some(decl) = node.child_by_field_name("declarator") {
            node = decl;
        } else if matches!(
            node.kind(),
            "identifier" | "field_identifier" | "qualified_identifier" | "destructor_name"
        ) {
            return text_of(node, src).to_string();
        } else {
            return text_of(node, src).split('(').next().unwrap_or("").trim().to_string();
        }
    }
}

fn collect_include(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    // #include "local.h" — local includes only
    let Some(path_node) = node.child_by_field_name("path") else {
        return;
    };
    let path = text_of(path_node, src);
    if path.starts_with('"') {
        let cleaned = path.trim_matches('"');
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Module {
                path: cleaned.to_string(),
                alias: None,
            },
            line: line_at(node),
        });
    }
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some(func) = n.child_by_field_name("function")
        {
            let qualifier = classify_call(func, src);
            acc.calls.push(CallSite { from, qualifier, line: line_at(n) });
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(child.kind(), "function_definition" | "lambda_expression") {
                stack.push(child);
            }
        }
    }
}

fn classify_call(func_node: TsNode, src: &[u8]) -> CallQualifier {
    match func_node.kind() {
        "identifier" => CallQualifier::Bare(text_of(func_node, src).to_string()),
        "field_expression" => {
            let obj = func_node
                .child_by_field_name("argument")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            let field = func_node
                .child_by_field_name("field")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            if obj == "this" {
                CallQualifier::SelfMethod(field.to_string())
            } else {
                CallQualifier::Attribute {
                    base: obj.to_string(),
                    name: field.to_string(),
                }
            }
        }
        "qualified_identifier" => {
            let text = text_of(func_node, src);
            if let Some(pos) = text.rfind("::") {
                CallQualifier::Attribute {
                    base: text[..pos].to_string(),
                    name: text[pos + 2..].to_string(),
                }
            } else {
                CallQualifier::Bare(text.to_string())
            }
        }
        _ => CallQualifier::ComplexReceiver {
            receiver: text_of(func_node, src).to_string(),
            name: String::new(),
        },
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
    fn structs_and_functions_c() {
        let source = r#"
#include "header.h"

struct Point {
    int x;
    int y;
};

int add(int a, int b) {
    return a + b;
}
"#;
        let fp = parse_file(source, "src/math.c", "src::math.c", Dialect::C, repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::STRUCT).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 1);
        assert_eq!(fp.imports.len(), 1);
    }

    #[test]
    fn classes_and_methods_cpp() {
        let source = r#"
class UserService {
public:
    void getUser(int id) {
        this->validate(id);
    }
    void validate(int id) {}
};
"#;
        let fp = parse_file(source, "src/service.cpp", "src::service.cpp", Dialect::Cpp, repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 2);
    }

    #[test]
    fn namespaces() {
        let source = r#"
namespace app {
    struct Config {};
    void init() {}
}
"#;
        let fp = parse_file(source, "src/app.cpp", "src::app.cpp", Dialect::Cpp, repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::PACKAGE).count(), 1);
    }
    fn count(fp: &FileParse, kind: repo_graph_core::NodeKindId) -> usize {
        fp.nav.kind_by_id.values().filter(|k| **k == kind).count()
    }

    fn id_named(fp: &FileParse, kind: repo_graph_core::NodeKindId, name: &str) -> Option<NodeId> {
        fp.nav
            .kind_by_id
            .iter()
            .find(|(id, k)| **k == kind && fp.nav.name_by_id.get(id).map(String::as_str) == Some(name))
            .map(|(id, _)| *id)
    }

    fn code_cells(fp: &FileParse, id: NodeId) -> Vec<String> {
        fp.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| &n.cells)
            .filter(|c| c.kind == cell_type::CODE)
            .filter_map(|c| match &c.payload {
                CellPayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// LB.10a: a `.h` goes to the C++ grammar; only when C parses it with
    /// fewer error bytes (`class` as an identifier) does C win.
    #[test]
    fn header_dialect_prefers_cpp_and_falls_back_to_c() {
        let cpp = "class W { public: int a() { return 1; } };\n";
        let parsed = parse_tree(cpp, Dialect::Header).unwrap();
        assert_eq!(parsed.dialect, Dialect::Cpp);
        assert_eq!(parsed.c_error_bytes, None, "a clean C++ tree never re-parses");
        let fp = parse_file(cpp, "src/W.h", "src::W.h", Dialect::Header, repo()).unwrap();
        assert_eq!(count(&fp, node_kind::CLASS), 1);
        assert_eq!(count(&fp, node_kind::METHOD), 1);
        assert_eq!(count(&fp, node_kind::FUNCTION), 0, "the C grammar misreads the class as a FUNCTION");

        // `template` is a C identifier and a C++ keyword. tree-sitter-cpp
        // reads `class` / `new` / `this` as identifiers without an error, so
        // it is the parameter name that makes the C++ tree fail.
        let c = "struct point { int x; };\nint point_format(const struct point *p, const char *template);\n";
        let parsed = parse_tree(c, Dialect::Header).unwrap();
        assert_eq!(parsed.dialect, Dialect::C);
        assert_eq!(parsed.c_error_bytes, Some(0));
        assert!(parsed.cpp_error_bytes > 0);
        let fp = parse_file(c, "c/point.h", "c::point.h", Dialect::Header, repo()).unwrap();
        assert_eq!(count(&fp, node_kind::STRUCT), 1);

        for (path, dialect) in [
            ("a.c", Dialect::C),
            ("a.cc", Dialect::Cpp),
            ("a.cpp", Dialect::Cpp),
            ("a.cxx", Dialect::Cpp),
            ("a.hpp", Dialect::Cpp),
            ("a.hh", Dialect::Cpp),
            ("a.hxx", Dialect::Cpp),
            ("a.h", Dialect::Header),
            ("src/dir.v2/a.h", Dialect::Header),
        ] {
            assert_eq!(Dialect::from_path(path), dialect, "{path}");
        }
    }

    /// Outermost ERROR bytes plus one per MISSING node; an error-free tree
    /// weighs 0.
    #[test]
    fn error_weight_counts_error_bytes() {
        let clean = parse_with("int f(void) { return 1; }\n", tree_sitter_c::LANGUAGE.into()).unwrap();
        assert_eq!(error_weight(clean.root_node()), 0);
        let broken = parse_with("int f(void) { return 1 }\n", tree_sitter_c::LANGUAGE.into()).unwrap();
        assert!(broken.root_node().has_error());
        assert!(error_weight(broken.root_node()) > 0);
    }

    /// LB.10a: include guards and `#ifdef` branches are walked; only a
    /// literal `#if 0` branch is dead; a function defined in two branches is
    /// one node, one DEFINES edge, one nav child, carrying both bodies.
    #[test]
    fn preprocessor_blocks_are_walked() {
        let guarded = "#ifndef X_H\n#define X_H\nclass X {\n public:\n  int a() { return 1; }\n#ifdef DEBUG\n  int dbg() { return 2; }\n#endif\n};\n#endif\n";
        let fp = parse_file(guarded, "src/X.h", "src::X.h", Dialect::Header, repo()).unwrap();
        assert_eq!(count(&fp, node_kind::CLASS), 1);
        assert_eq!(count(&fp, node_kind::METHOD), 2, "a method inside a class-body #ifdef is walked");

        let dead = "#if 0\nint dead(void){return 0;}\n#else\nint live(void){return 1;}\n#endif\n";
        let fp = parse_file(dead, "src/d.c", "src::d.c", Dialect::C, repo()).unwrap();
        assert!(id_named(&fp, node_kind::FUNCTION, "live").is_some());
        assert!(id_named(&fp, node_kind::FUNCTION, "dead").is_none());
        assert_eq!(count(&fp, node_kind::FUNCTION), 1);

        let twins = "#ifdef A\nint g(void){return 1;}\n#else\nint g(void){return 2;}\n#endif\n";
        let fp = parse_file(twins, "src/t.c", "src::t.c", Dialect::C, repo()).unwrap();
        let g = id_named(&fp, node_kind::FUNCTION, "g").expect("FUNCTION g");
        assert_eq!(count(&fp, node_kind::FUNCTION), 1);
        assert_eq!(fp.nodes.iter().filter(|n| n.id == g).count(), 1);
        assert_eq!(
            fp.edges.iter().filter(|e| e.to == g && e.category == edge_category::DEFINES).count(),
            1
        );
        let module = fp.nodes[0].id;
        assert_eq!(fp.nav.children_of[&module].iter().filter(|c| **c == g).count(), 1);
        assert_eq!(code_cells(&fp, g).len(), 2, "both branches' bodies stack on the one node");

        // `#include` inside a guard still records the import; nested
        // `#if` / `#elif` branches are all walked.
        let nested = "#ifndef Y_H\n#define Y_H\n#include \"z.h\"\n#if defined(WIN)\nint w(void);\nint win(void){return 1;}\n#elif defined(MAC)\nint mac(void){return 2;}\n#endif\n#endif\n";
        let fp = parse_file(nested, "src/y.h", "src::y.h", Dialect::Header, repo()).unwrap();
        assert_eq!(fp.imports.len(), 1);
        assert!(id_named(&fp, node_kind::FUNCTION, "win").is_some());
        assert!(id_named(&fp, node_kind::FUNCTION, "mac").is_some());
    }

    /// LB.10a: a bodiless class / struct / enum specifier mints nothing; the
    /// definition is the one type node.
    #[test]
    fn forward_declarations_mint_no_type() {
        let source = "class Gadget;\nstruct point;\nstruct point *make();\nenum class Mode : int;\nstruct point { int x; };\n";
        let fp = parse_file(source, "src/f.cpp", "src::f.cpp", Dialect::Cpp, repo()).unwrap();
        assert_eq!(count(&fp, node_kind::CLASS), 0);
        assert_eq!(count(&fp, node_kind::ENUM), 0);
        assert_eq!(count(&fp, node_kind::STRUCT), 1);
        let point = id_named(&fp, node_kind::STRUCT, "point").expect("STRUCT point");
        let code = code_cells(&fp, point);
        assert_eq!(code.len(), 1);
        assert!(code[0].starts_with("struct point {"), "{code:?}");
        // One DEFINES per emitted node: none dangle towards a stub.
        let defines = fp.edges.iter().filter(|e| e.category == edge_category::DEFINES).count();
        assert_eq!(defines, 1);
    }

    /// Overloads share a qname: one node, one DEFINES edge.
    #[test]
    fn overloads_fold_to_one_node() {
        let source = "int f(int a) { return a; }\ndouble f(double a) { return a; }\n";
        let fp = parse_file(source, "src/o.cpp", "src::o.cpp", Dialect::Cpp, repo()).unwrap();
        let f = id_named(&fp, node_kind::FUNCTION, "f").expect("FUNCTION f");
        assert_eq!(fp.edges.iter().filter(|e| e.to == f).count(), 1);
        assert_eq!(code_cells(&fp, f).len(), 2);
    }

    /// LB.10a: the MODULE is named by its file name, its nav name is the
    /// stem, so `bare_module_qname` reads back `src::Widget`.
    #[test]
    fn module_nav_name_is_the_stem() {
        let fp = parse_file("class Widget {};\n", "src/Widget.h", "src::Widget.h", Dialect::Header, repo()).unwrap();
        let module = fp.nodes[0].id;
        assert_eq!(fp.nav.name_by_id[&module], "Widget");
        assert_eq!(fp.nav.qname_by_id[&module], "src::Widget.h");
        assert_eq!(
            repo_graph_code_domain::bare_module_qname("src::Widget.h", &fp.nav.name_by_id[&module]).as_deref(),
            Some("src::Widget")
        );
        assert_eq!(module_name("", "src::x.c"), "x.c");
    }
}
