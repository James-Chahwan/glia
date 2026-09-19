use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser, Tree};

pub use glia_code_domain::{
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
    let (fp, stats) = parse_tree_into(&parsed.tree, source, file_rel_path, module_qname, repo);
    if qname_debug() && let Some(line) = stats.marker(file_rel_path) {
        // LB.10b debug marker:
        //   `GLIA_QNAME_DEBUG=1 glia analyze <repo> 2>&1 | grep '\[qname\] c_cpp: .* header types scoped'`
        eprintln!("{line}");
    }
    Ok(fp)
}

/// Walk a parsed tree into a [`FileParse`], with the counts of the LB.10b
/// identity rules for the marker.
fn parse_tree_into(
    tree: &Tree,
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> (FileParse, IdentityStats) {
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc {
        dir: module_qname.rsplit_once("::").map_or("", |(d, _)| d).to_string(),
        is_header: is_header_path(file_rel_path),
        ..Acc::default()
    };

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

    let scope = Scope { parent_qname: module_qname, parent_id: module_id, ns_path: "" };
    visit_children(root, src, file_rel_path, &scope, repo, &mut acc);

    let fp = FileParse {
        nodes: acc.nodes,
        edges: acc.edges,
        imports: acc.imports,
        calls: acc.calls,
        refs: acc.refs,
        nav: acc.nav,
        properties: Default::default(),
    };
    (fp, acc.stats)
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
    /// LB.10b: the file MODULE's directory (its qname minus the last
    /// segment, "" at the repo root), the scope of a header's global types.
    dir: String,
    /// LB.10b: the file is a header by extension ([`is_header_path`]).
    is_header: bool,
    /// LB.10b: the class / struct types this file defines, by C++ name
    /// (namespace path + name, `shop::Cart`) -> (id, qname). The first
    /// definition wins. Lookup only, never iterated.
    local_types: HashMap<String, (NodeId, String)>,
    /// LB.10b: the counts behind the `[qname] c_cpp:` identity marker.
    stats: IdentityStats,
}

/// Where a declaration sits (LB.10b): its nav parent (the file MODULE or a
/// namespace PACKAGE) and the named namespaces around it.
struct Scope<'s> {
    parent_qname: &'s str,
    parent_id: NodeId,
    /// The enclosing named namespaces, `::`-joined (`a::b`); "" is the
    /// global namespace.
    ns_path: &'s str,
}

/// `a::b`, or `b` alone when `a` is empty.
fn join(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        format!("{a}::{b}")
    }
}

/// A header by extension (LB.10b). Only the extension decides, never the
/// grammar LB.10a picked for a `.h`.
fn is_header_path(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some("h" | "hh" | "hpp" | "hxx")
    )
}

/// LB.10b: the qname of a class / struct / enum named `name` in `scope`.
/// A header's type is the same type in every translation unit that includes
/// it (C++'s one-definition rule), so it takes its C++ name: the namespace
/// path (`shop::Cart`), or in the global namespace the header's directory
/// (`src::Widget`, LB.2's monorepo rule). A source file's type is visible
/// only in that translation unit and keeps the file scope
/// (`src::Widget.cpp::Local`).
fn type_qname(acc: &Acc, scope: &Scope, name: &str) -> String {
    if !acc.is_header {
        join(scope.parent_qname, name)
    } else if !scope.ns_path.is_empty() {
        join(scope.ns_path, name)
    } else {
        join(&acc.dir, name)
    }
}

/// The namespaces C++ searches for a qualified definition's scope, innermost
/// first: `a::b` -> `a::b`, `a`, "" (the global namespace).
fn ns_prefixes(ns_path: &str) -> impl Iterator<Item = &str> {
    let mut next = Some(ns_path);
    std::iter::from_fn(move || {
        let cur = next?;
        next = if cur.is_empty() {
            None
        } else {
            Some(cur.rsplit_once("::").map_or("", |(outer, _)| outer))
        };
        Some(cur)
    })
}

/// LB.10b: what one file's type / out-of-line identity rules did.
#[derive(Debug, Default, PartialEq, Eq)]
struct IdentityStats {
    /// Header types named by their namespace path.
    header_ns: usize,
    /// Header types named by their directory (global namespace).
    header_dir: usize,
    /// Source-file types that kept the file scope.
    file_local: usize,
    /// Out-of-line member definitions bound to a class this file defines.
    bound: usize,
    /// Out-of-line member definitions emitted at the header rule's qname for
    /// LB.10c to bind.
    provisional: usize,
}

impl IdentityStats {
    /// `[qname] c_cpp: H header types scoped (N namespace, D directory), L
    /// file-local types kept, O out-of-line members (B bound in-file, P
    /// provisional) file=<file>`, for a file with at least one type or
    /// out-of-line member.
    fn marker(&self, file_rel: &str) -> Option<String> {
        let header = self.header_ns + self.header_dir;
        let out_of_line = self.bound + self.provisional;
        (header + self.file_local + out_of_line > 0).then(|| {
            format!(
                "[qname] c_cpp: {header} header types scoped ({} namespace, {} directory), \
                 {} file-local types kept, {out_of_line} out-of-line members \
                 ({} bound in-file, {} provisional) file={file_rel}",
                self.header_ns, self.header_dir, self.file_local, self.bound, self.provisional
            )
        })
    }

    /// Count one type emitted in `scope` of a file that is (or is not) a
    /// header.
    fn count_type(&mut self, is_header: bool, scope: &Scope) {
        if !is_header {
            self.file_local += 1;
        } else if scope.ns_path.is_empty() {
            self.header_dir += 1;
        } else {
            self.header_ns += 1;
        }
    }
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
    scope: &Scope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit_one(child, src, file_rel, scope, repo, acc);
    }
}

fn visit_one(
    child: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    repo: RepoId,
    acc: &mut Acc,
) {
    match child.kind() {
        "preproc_include" => collect_include(child, src, scope.parent_qname, acc),
        "function_definition" => {
            visit_function(child, src, file_rel, scope, repo, acc);
        }
        "struct_specifier" | "class_specifier" => {
            visit_type(child, src, file_rel, scope, repo, acc);
        }
        "enum_specifier" => {
            visit_enum(child, src, file_rel, scope, repo, acc);
        }
        "namespace_definition" => {
            visit_namespace(child, src, file_rel, scope, repo, acc);
        }
        "declaration" => {
            // A declaration whose type is a class / struct body
            // (`struct P { int x; } p;`); a bodiless one declares nothing.
            let mut c2 = child.walk();
            for gc in child.named_children(&mut c2) {
                if gc.kind() == "struct_specifier" || gc.kind() == "class_specifier" {
                    visit_type(gc, src, file_rel, scope, repo, acc);
                }
            }
        }
        kind if is_preproc_block(kind) => {
            if let Some(branch) = live_branch(child, src) {
                visit_children(branch, src, file_rel, scope, repo, acc);
            }
        }
        _ => {}
    }
}

/// A named namespace block is a PACKAGE under its parent (the file MODULE
/// or the enclosing block), as before LB.10b; its contents see the namespace
/// path extended by the block's name (`a::b` for `namespace a::b`, one
/// segment for an `inline namespace v1`). An anonymous namespace is not
/// walked.
fn visit_namespace(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{}::{name}", scope.parent_qname);
    let ns_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::PACKAGE, &qname);

    let first = acc.push_node(Node {
        id: ns_id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: scope.parent_id,
            to: ns_id,
            category: edge_category::CONTAINS,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(ns_id, name, &qname, node_kind::PACKAGE, Some(scope.parent_id));
    }

    if let Some(body) = node.child_by_field_name("body") {
        let ns_path = join(scope.ns_path, name);
        let inner = Scope { parent_qname: &qname, parent_id: ns_id, ns_path: &ns_path };
        visit_children(body, src, file_rel, &inner, repo, acc);
    }
}

fn visit_type(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
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
    let qname = type_qname(acc, scope, name);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);

    let first = acc.push_node(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: scope.parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav.record(id, name, &qname, kind, Some(scope.parent_id));
        acc.stats.count_type(acc.is_header, scope);
    }
    acc.local_types
        .entry(join(scope.ns_path, name))
        .or_insert_with(|| (id, qname.clone()));

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
    scope: &Scope,
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
    let qname = type_qname(acc, scope, name);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENUM, &qname);

    let first = acc.push_node(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: scope.parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, name, &qname, node_kind::ENUM, Some(scope.parent_id));
        acc.stats.count_type(acc.is_header, scope);
    }
}

/// A function definition at file or namespace scope. An unqualified name is
/// a free FUNCTION in the file scope (`src::a.cpp::f`); a qualified one
/// (`Q::m`) is an out-of-line member definition ([`visit_out_of_line`]).
fn visit_function(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
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
    if let Some((q, m)) = name.rsplit_once("::")
        && !q.is_empty()
        && !m.is_empty()
    {
        visit_out_of_line(node, src, file_rel, scope, (q, m), repo, acc);
        return;
    }
    let qname = format!("{}::{name}", scope.parent_qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    let first = acc.push_node(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    if first {
        acc.edges.push(Edge {
            from: scope.parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, &name, &qname, node_kind::FUNCTION, Some(scope.parent_id));
    }

    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, id, acc);
    }
}

/// LB.10b: an out-of-line member definition `Q::m`.
///
/// C++ looks the qualifier up from the definition's own namespace outward, so
/// when this file defines the type `Q` names (in the current namespace or an
/// enclosing one) the member binds to it here: METHOD `<type qname>::m`, nav
/// name `m`, nav parent and DEFINES from the type, and a `this->x()` in its
/// body resolves through the type at graph build.
///
/// Otherwise the class lives in another file, which the parser cannot see
/// (nor tell a class from a namespace, nor CLASS from STRUCT): the member is
/// a provisional METHOD at the header rule's qname (the namespace path +
/// `Q`, or the directory + `Q` when that is a single segment), nav name
/// `Q::m` and HEAD's nav parent and DEFINES from the lexical scope. That qname
/// is already the class's member qname whenever the class sits in the
/// definition's directory (global namespace) or namespace; LB.10c binds or
/// renames the rest at graph build.
fn visit_out_of_line(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    (q, m): (&str, &str),
    repo: RepoId,
    acc: &mut Acc,
) {
    let local = ns_prefixes(scope.ns_path).find_map(|p| acc.local_types.get(&join(p, q)).cloned());
    let (qname, nav_name, parent_id) = match local {
        Some((type_id, type_qname)) => {
            acc.stats.bound += 1;
            (format!("{type_qname}::{m}"), m.to_string(), type_id)
        }
        None => {
            acc.stats.provisional += 1;
            let full = join(scope.ns_path, q);
            let owner = if full.contains("::") { full } else { join(&acc.dir, &full) };
            (format!("{owner}::{m}"), format!("{q}::{m}"), scope.parent_id)
        }
    };
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
            .record(id, &nav_name, &qname, node_kind::METHOD, Some(parent_id));
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
        } else if node.kind() == "reference_declarator"
            && let Some(inner) = node.named_child(0)
        {
            // LB.10b: tree-sitter-cpp gives `T& f()` / `T&& f()` no
            // `declarator` field; the declarator is its one named child.
            node = inner;
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
    fn count(fp: &FileParse, kind: glia_core::NodeKindId) -> usize {
        fp.nav.kind_by_id.values().filter(|k| **k == kind).count()
    }

    fn id_named(fp: &FileParse, kind: glia_core::NodeKindId, name: &str) -> Option<NodeId> {
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
            glia_code_domain::bare_module_qname("src::Widget.h", &fp.nav.name_by_id[&module]).as_deref(),
            Some("src::Widget")
        );
        assert_eq!(module_name("", "src::x.c"), "x.c");
    }

    /// The id of the `kind` node at `qname`, when the parse recorded it.
    fn node_at(fp: &FileParse, kind: glia_core::NodeKindId, qname: &str) -> Option<NodeId> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        (fp.nav.kind_by_id.get(&id) == Some(&kind) && fp.nav.qname_by_id.get(&id).map(String::as_str) == Some(qname))
            .then_some(id)
    }

    fn all_qnames(fp: &FileParse) -> Vec<&str> {
        let mut out: Vec<&str> = fp.nav.qname_by_id.values().map(String::as_str).collect();
        out.sort_unstable();
        out
    }

    fn defines(fp: &FileParse, from: NodeId, to: NodeId) -> usize {
        fp.edges
            .iter()
            .filter(|e| e.from == from && e.to == to && e.category == edge_category::DEFINES)
            .count()
    }

    /// LB.10b: a header's type is named by its C++ name - the namespace path,
    /// or the header's directory in the global namespace - never by the
    /// header file; its nav parent (MODULE / PACKAGE) does not move.
    #[test]
    fn header_types_take_their_cpp_name() {
        let widget = "#ifndef W_H\n#define W_H\nclass Widget {\n public:\n  void run();\n  int helper() { return 1; }\n};\nstruct Point { int x; };\nenum Color { Red };\n#endif\n";
        let fp = parse_file(widget, "src/Widget.h", "src::Widget.h", Dialect::Header, repo()).unwrap();
        let module = fp.nodes[0].id;
        let class = node_at(&fp, node_kind::CLASS, "src::Widget").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        let helper = node_at(&fp, node_kind::METHOD, "src::Widget::helper").expect("METHOD src::Widget::helper");
        let point = node_at(&fp, node_kind::STRUCT, "src::Point").expect("STRUCT src::Point");
        assert!(node_at(&fp, node_kind::ENUM, "src::Color").is_some(), "{:?}", all_qnames(&fp));
        assert_eq!(fp.nav.parent_of[&class], module);
        assert_eq!(fp.nav.parent_of[&point], module);
        assert_eq!(fp.nav.parent_of[&helper], class);
        assert_eq!(fp.nav.name_by_id[&class], "Widget");
        assert_eq!(defines(&fp, module, class), 1);
        assert_eq!(defines(&fp, class, helper), 1);
        assert!(all_qnames(&fp).iter().all(|q| !q.starts_with("src::Widget.h::")), "{:?}", all_qnames(&fp));

        let cart = "#pragma once\nnamespace shop {\nclass Cart {\n public:\n  int total();\n  int tax() { return 1; }\n};\n}\n";
        let fp = parse_file(cart, "include/shop/cart.hpp", "include::shop::cart.hpp", Dialect::Cpp, repo()).unwrap();
        let class = node_at(&fp, node_kind::CLASS, "shop::Cart").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert!(node_at(&fp, node_kind::METHOD, "shop::Cart::tax").is_some());
        let package = node_at(&fp, node_kind::PACKAGE, "include::shop::cart.hpp::shop").expect("PACKAGE keeps its shape");
        assert_eq!(fp.nav.parent_of[&class], package);
        assert_eq!(defines(&fp, package, class), 1);

        // A nested namespace specifier is the whole path; an inline
        // namespace is one segment; a nested block extends the path.
        let nested = "namespace a::b { struct S {}; }\nnamespace lib { inline namespace v1 { class T {}; } namespace detail { enum E { X }; } }\n";
        let fp = parse_file(nested, "inc/n.hh", "inc::n.hh", Dialect::Cpp, repo()).unwrap();
        for (kind, q) in [
            (node_kind::STRUCT, "a::b::S"),
            (node_kind::CLASS, "lib::v1::T"),
            (node_kind::ENUM, "lib::detail::E"),
            (node_kind::PACKAGE, "inc::n.hh::a::b"),
            (node_kind::PACKAGE, "inc::n.hh::lib::v1"),
        ] {
            assert!(node_at(&fp, kind, q).is_some(), "{q} missing: {:?}", all_qnames(&fp));
        }

        // A root-level header: the directory is empty.
        let fp = parse_file("class W {};\n", "W.h", "W.h", Dialect::Header, repo()).unwrap();
        assert!(node_at(&fp, node_kind::CLASS, "W").is_some(), "{:?}", all_qnames(&fp));
        // A C header's struct follows the same rule.
        let fp = parse_file("struct point { int x; };\n", "c/point.h", "c::point.h", Dialect::Header, repo()).unwrap();
        assert!(node_at(&fp, node_kind::STRUCT, "c::point").is_some(), "{:?}", all_qnames(&fp));
    }

    /// LB.10b: a source file's types, namespace blocks and free functions
    /// keep the file scope.
    #[test]
    fn source_types_stay_file_local() {
        let source = "class Local { public: int b() { return 4; } };\nnamespace n { class K {}; }\nint f() { return 1; }\nstruct S { int x; };\n";
        let fp = parse_file(source, "src/Widget.cpp", "src::Widget.cpp", Dialect::Cpp, repo()).unwrap();
        for (kind, q) in [
            (node_kind::CLASS, "src::Widget.cpp::Local"),
            (node_kind::METHOD, "src::Widget.cpp::Local::b"),
            (node_kind::CLASS, "src::Widget.cpp::n::K"),
            (node_kind::PACKAGE, "src::Widget.cpp::n"),
            (node_kind::FUNCTION, "src::Widget.cpp::f"),
            (node_kind::STRUCT, "src::Widget.cpp::S"),
        ] {
            assert!(node_at(&fp, kind, q).is_some(), "{q} missing: {:?}", all_qnames(&fp));
        }
        // A C file too; a header's free function keeps its file scope.
        let fp = parse_file("struct p { int x; };\nint g(void) { return 0; }\n", "c/p.c", "c::p.c", Dialect::C, repo()).unwrap();
        assert!(node_at(&fp, node_kind::STRUCT, "c::p.c::p").is_some(), "{:?}", all_qnames(&fp));
        let fp = parse_file("static inline int sq(int x) { return x * x; }\n", "src/a.h", "src::a.h", Dialect::Header, repo()).unwrap();
        assert!(node_at(&fp, node_kind::FUNCTION, "src::a.h::sq").is_some(), "{:?}", all_qnames(&fp));
    }

    /// LB.10b: an out-of-line member binds to a class this file defines
    /// (looked up from its namespace outward); any other is a provisional
    /// METHOD at the header rule's qname under the lexical scope.
    #[test]
    fn out_of_line_members() {
        let source = "\
#include \"Widget.h\"
class Local {
 public:
  int a();
  int b() { return 4; }
  const int& ref();
};
int Local::a() { return this->b(); }
const int& Local::ref() { static int x = 1; return x; }
void Widget::run() { this->helper(); }
namespace shop { int Cart::total() { return this->tax(); } }
void shop::init() {}
namespace n { struct K { int f(); }; }
namespace n { namespace m { int K::f() { return 0; } } }
int& g() { static int y; return y; }
";
        let fp = parse_file(source, "src/Widget.cpp", "src::Widget.cpp", Dialect::Cpp, repo()).unwrap();
        let module = fp.nodes[0].id;
        let local = node_at(&fp, node_kind::CLASS, "src::Widget.cpp::Local").expect("CLASS Local");

        // In-file: bound to the class.
        let a = node_at(&fp, node_kind::METHOD, "src::Widget.cpp::Local::a").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&a], "a");
        assert_eq!(fp.nav.parent_of[&a], local);
        assert_eq!(defines(&fp, local, a), 1);
        assert_eq!(defines(&fp, module, a), 0);
        let calls: Vec<&CallSite> = fp.calls.iter().filter(|c| c.from == a).collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].qualifier, CallQualifier::SelfMethod("b".to_string()));
        let r = node_at(&fp, node_kind::METHOD, "src::Widget.cpp::Local::ref").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&r], "ref", "a reference_declarator loses its `& `");
        assert_eq!(fp.nav.parent_of[&r], local);
        // Looked up outward: `K::f` inside `n::m` binds to `n::K`.
        let k = node_at(&fp, node_kind::STRUCT, "src::Widget.cpp::n::K").expect("STRUCT n::K");
        let f = node_at(&fp, node_kind::METHOD, "src::Widget.cpp::n::K::f").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&f], k);

        // Provisional: the header rule's qname, the lexical parent.
        let run = node_at(&fp, node_kind::METHOD, "src::Widget::run").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&run], "Widget::run");
        assert_eq!(fp.nav.parent_of[&run], module);
        assert_eq!(defines(&fp, module, run), 1);
        let package = node_at(&fp, node_kind::PACKAGE, "src::Widget.cpp::shop").expect("PACKAGE shop");
        let total = node_at(&fp, node_kind::METHOD, "shop::Cart::total").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&total], "Cart::total");
        assert_eq!(fp.nav.parent_of[&total], package);
        assert_eq!(fp.calls.iter().filter(|c| c.from == total).count(), 1);
        let init = node_at(&fp, node_kind::METHOD, "src::shop::init").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&init], "shop::init");

        // Free functions stay FUNCTIONs in the file scope, `& ` dropped.
        let g = node_at(&fp, node_kind::FUNCTION, "src::Widget.cpp::g").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&g], "g");
        assert_eq!(count(&fp, node_kind::FUNCTION), 1, "{:?}", all_qnames(&fp));

        // The marker's counts: Local, n::K file-local; a, ref, K::f bound;
        // run, total, init provisional.
        let tree = parse_tree(source, Dialect::Cpp).unwrap().tree;
        let (_, stats) = parse_tree_into(&tree, source, "src/Widget.cpp", "src::Widget.cpp", repo());
        assert_eq!(stats, IdentityStats { file_local: 2, bound: 3, provisional: 3, ..IdentityStats::default() });
        assert_eq!(
            stats.marker("src/Widget.cpp").as_deref(),
            Some(
                "[qname] c_cpp: 0 header types scoped (0 namespace, 0 directory), 2 file-local types kept, \
                 6 out-of-line members (3 bound in-file, 3 provisional) file=src/Widget.cpp"
            )
        );
        assert_eq!(IdentityStats::default().marker("x.c"), None);
    }

    /// The outward namespace search order and the header extensions.
    #[test]
    fn scope_helpers() {
        assert_eq!(ns_prefixes("a::b::c").collect::<Vec<_>>(), ["a::b::c", "a::b", "a", ""]);
        assert_eq!(ns_prefixes("").collect::<Vec<_>>(), [""]);
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("a", "x"), "a::x");
        for (path, header) in [
            ("a.h", true),
            ("a.hh", true),
            ("a.hpp", true),
            ("a.hxx", true),
            ("a.c", false),
            ("a.cc", false),
            ("a.cpp", false),
            ("a.cxx", false),
            ("dir.h/a.cpp", false),
        ] {
            assert_eq!(is_header_path(path), header, "{path}");
        }
    }
}
