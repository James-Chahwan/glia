use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser, Tree};

use glia_code_domain::NavFact;
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
    /// [`Dialect::Header`]. CB.1: `.inl` / `.ipp` / `.tpp` hold C++ template
    /// and inline implementation code, so they take the C++ grammar; the
    /// C-vs-C++ auto-detect (LB.10a) stays for `.h` alone.
    pub fn from_path(path: &str) -> Self {
        match Path::new(path).extension().and_then(|e| e.to_str()) {
            Some("c") => Dialect::C,
            Some("cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "inl" | "ipp" | "tpp") => {
                Dialect::Cpp
            }
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

    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    let mut acc = Acc::new(module_id, module_qname, is_header_path(file_rel_path));
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

    let scope = Scope {
        parent_qname: module_qname,
        parent_id: module_id,
        ns_path: "",
        anon: false,
        extern_c: false,
    };
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
    /// CB.19: the file MODULE. An `#include` and every [`NavFact`] belong
    /// to the file, whatever namespace or function they sit in.
    module_id: NodeId,
    module_qname: String,
    /// CB.19: the type parameter names of the enclosing template
    /// declarations, outermost first. A type spelled by one of them is
    /// unknown ([`simple_type`] gives `""`).
    template_params: Vec<String>,
}

impl Acc {
    fn new(module_id: NodeId, module_qname: &str, is_header: bool) -> Self {
        Acc {
            nodes: Vec::new(),
            edges: Vec::new(),
            imports: Vec::new(),
            calls: Vec::new(),
            refs: Vec::new(),
            nav: CodeNav::default(),
            emitted: HashMap::new(),
            dir: module_qname.rsplit_once("::").map_or("", |(d, _)| d).to_string(),
            is_header,
            local_types: HashMap::new(),
            stats: IdentityStats::default(),
            module_id,
            module_qname: module_qname.to_string(),
            template_params: Vec::new(),
        }
    }

    /// CB.19: record `fact` on the file MODULE (CB.6's `record_fact`, which
    /// drops a repeat: an `#ifdef` walked twice records each fact once).
    /// True when the fact is new.
    fn fact(&mut self, fact: NavFact) -> bool {
        let before = self.nav.nav_facts.get(&self.module_id).map_or(0, Vec::len);
        self.nav.record_fact(self.module_id, fact);
        self.nav.nav_facts.get(&self.module_id).map_or(0, Vec::len) > before
    }
}

/// Where a declaration sits (LB.10b): its nav parent (the file MODULE or a
/// namespace PACKAGE) and the named namespaces around it.
#[derive(Clone, Copy)]
struct Scope<'s> {
    parent_qname: &'s str,
    parent_id: NodeId,
    /// The enclosing named namespaces, `::`-joined (`a::b`); "" is the
    /// global namespace.
    ns_path: &'s str,
    /// CB.19: inside an anonymous namespace: every function declared or
    /// defined here has internal linkage.
    anon: bool,
    /// CB.19: inside an `extern "C"` block or declaration.
    extern_c: bool,
}

/// CB.19: the class / struct / union a nested declaration sits in.
#[derive(Clone, Copy)]
struct Outer<'o> {
    qname: &'o str,
    id: NodeId,
    /// Its C++ name, namespace path included (`shop::Cart`): the
    /// `Acc::local_types` key its nested types extend (`shop::Cart::Line`).
    key: &'o str,
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
/// grammar LB.10a picked for a `.h`. CB.1: `.inl` / `.ipp` / `.tpp` are
/// `#include`d like a header, so a type they declare is the same type in every
/// includer and takes the header rule.
fn is_header_path(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some("h" | "hh" | "hpp" | "hxx" | "inl" | "ipp" | "tpp")
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
    /// CB.19: types nested in a class / struct / union (`shop::Cart::Line`);
    /// not in the three counts above, which count outermost types.
    nested: usize,
    /// CB.19: template declarations walked.
    templates: usize,
    /// CB.19: unions (STRUCTs).
    unions: usize,
    /// CB.19: functions / types defined, and prototypes declared, inside an
    /// anonymous namespace.
    anon_ns: usize,
    /// CB.19: the same inside an `extern "C"` block or declaration.
    extern_c: usize,
    /// CB.19: file / namespace-scope function prototypes recorded
    /// (`DeclaresFn`, or `InternalLinkage` for a `static` one).
    prototypes: usize,
    /// CB.19: `using namespace` / `using a::b` facts recorded.
    usings: usize,
}

impl IdentityStats {
    /// `[qname] c_cpp: H header types scoped (N namespace, D directory), L
    /// file-local types kept, O out-of-line members (B bound in-file, P
    /// provisional) nested=.. templates=.. unions=.. anon_ns=.. extern_c=..
    /// prototypes=.. usings=.. file=<file>`, for a file with any count.
    fn marker(&self, file_rel: &str) -> Option<String> {
        let header = self.header_ns + self.header_dir;
        let out_of_line = self.bound + self.provisional;
        (*self != IdentityStats::default()).then(|| {
            format!(
                "[qname] c_cpp: {header} header types scoped ({} namespace, {} directory), \
                 {} file-local types kept, {out_of_line} out-of-line members \
                 ({} bound in-file, {} provisional) nested={} templates={} unions={} \
                 anon_ns={} extern_c={} prototypes={} usings={} file={file_rel}",
                self.header_ns,
                self.header_dir,
                self.file_local,
                self.bound,
                self.provisional,
                self.nested,
                self.templates,
                self.unions,
                self.anon_ns,
                self.extern_c,
                self.prototypes,
                self.usings
            )
        })
    }

    /// CB.19: count one entity declared in `scope`'s anonymous namespace /
    /// `extern "C"` block, if any.
    fn count_linkage(&mut self, scope: &Scope) {
        self.anon_ns += usize::from(scope.anon);
        self.extern_c += usize::from(scope.extern_c);
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
        // CB.19 (C3): an `#include` inside `namespace shop { .. }` is still
        // the file's: textual inclusion, from the file MODULE.
        "preproc_include" => collect_include(child, src, acc),
        "function_definition" => {
            visit_function(child, src, file_rel, scope, repo, acc);
        }
        "struct_specifier" | "class_specifier" | "union_specifier" => {
            visit_type(child, src, file_rel, scope, None, repo, acc);
        }
        "enum_specifier" => {
            visit_enum(child, src, file_rel, scope, None, repo, acc);
        }
        "namespace_definition" => {
            visit_namespace(child, src, file_rel, scope, repo, acc);
        }
        "linkage_specification" => {
            visit_linkage(child, src, file_rel, scope, repo, acc);
        }
        "template_declaration" => {
            visit_template(child, src, file_rel, scope, None, repo, acc);
        }
        "using_declaration" => record_using(child, src, scope.ns_path, acc),
        "declaration" => {
            // A declaration whose type is a class / struct / union body
            // (`struct P { int x; } p;`); a bodiless one declares nothing.
            let mut c2 = child.walk();
            for gc in child.named_children(&mut c2) {
                if matches!(gc.kind(), "struct_specifier" | "class_specifier" | "union_specifier") {
                    visit_type(gc, src, file_rel, scope, None, repo, acc);
                }
            }
            record_prototypes(child, src, scope, acc);
        }
        kind if is_preproc_block(kind) => {
            if let Some(branch) = live_branch(child, src) {
                visit_children(branch, src, file_rel, scope, repo, acc);
            }
        }
        _ => {}
    }
}

/// CB.19 (C1): `extern "C" { .. }` / `extern "C" int f() {..}`. Linkage
/// changes nothing about C++ naming, so the contents keep the enclosing
/// scope: its `body` is a `declaration_list` walked like the enclosing one,
/// or the one `function_definition` / `declaration` it applies to.
fn visit_linkage(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let is_c = node
        .child_by_field_name("value")
        .is_some_and(|v| text_of(v, src).trim_matches('"') == "C");
    let inner = Scope { extern_c: scope.extern_c || is_c, ..*scope };
    if body.kind() == "declaration_list" {
        visit_children(body, src, file_rel, &inner, repo, acc);
    } else {
        visit_one(body, src, file_rel, &inner, repo, acc);
    }
}

/// CB.19 (C1): a template declaration is its inner declaration, named
/// without template arguments (`template <class T> class Box` is
/// `shop::Box`), in the same scope. At file / namespace scope (`outer` =
/// `None`) the inner class / struct / union, function, declaration or
/// nested template goes through [`visit_one`]; inside a class body
/// (`outer` = the class) a member template goes through [`visit_member`].
/// The template's type parameters are unknown types while it is walked.
fn visit_template(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    outer: Option<Outer>,
    repo: RepoId,
    acc: &mut Acc,
) {
    let depth = acc.template_params.len();
    if let Some(params) = node.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for p in params.named_children(&mut cursor) {
            if let Some(name) = template_param_name(p, src) {
                acc.template_params.push(name.to_string());
            }
        }
    }
    acc.stats.templates += 1;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match (outer, child.kind()) {
            (_, "template_parameter_list") => {}
            (Some(o), _) => visit_member(child, src, file_rel, scope, o, repo, acc),
            (
                None,
                "class_specifier" | "struct_specifier" | "union_specifier" | "function_definition"
                | "declaration" | "template_declaration",
            ) => visit_one(child, src, file_rel, scope, repo, acc),
            _ => {}
        }
    }
    acc.template_params.truncate(depth);
}

/// The name a template type parameter introduces (`T` of `class T`,
/// `typename T = int`, `class... Ts`, `template <class> class C`); a
/// non-type parameter (`int N`) names no type.
fn template_param_name<'a>(p: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    if !matches!(
        p.kind(),
        "type_parameter_declaration"
            | "optional_type_parameter_declaration"
            | "variadic_type_parameter_declaration"
            | "template_template_parameter_declaration"
    ) {
        return None;
    }
    if let Some(name) = p.child_by_field_name("name") {
        return Some(text_of(name, src));
    }
    let mut cursor = p.walk();
    for c in p.named_children(&mut cursor) {
        if c.kind() == "type_identifier" {
            return Some(text_of(c, src));
        }
    }
    None
}

/// A named namespace block is a PACKAGE under its parent (the file MODULE
/// or the enclosing block), as before LB.10b; its contents see the namespace
/// path extended by the block's name (`a::b` for `namespace a::b`, one
/// segment for an `inline namespace v1`).
///
/// CB.19 (C1): an anonymous namespace has no name, so no PACKAGE: its body
/// is walked in the enclosing scope, its free functions keep the file-scope
/// qname of any source-file function (`src::cart.cpp::clamp`), and each
/// records `InternalLinkage` on the file MODULE.
fn visit_namespace(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        if let Some(body) = node.child_by_field_name("body") {
            let inner = Scope { anon: true, ..*scope };
            visit_children(body, src, file_rel, &inner, repo, acc);
        }
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
        let inner = Scope { parent_qname: &qname, parent_id: ns_id, ns_path: &ns_path, ..*scope };
        visit_children(body, src, file_rel, &inner, repo, acc);
    }
}

/// A class / struct / union definition. CB.19: a union is a STRUCT (C4); a
/// template specialisation's `Box<int>` is named `Box`, so it merges into
/// the primary template's node (cells stack, the overload rule); and a type
/// nested in `outer` is `<outer qname>::<name>` in every file, header or
/// not (a nested type is scoped by its outer type in every translation
/// unit), with the outer type as nav parent and DEFINES source.
fn visit_type(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    outer: Option<Outer>,
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
    let name = type_name(name_node, src);
    if name.is_empty() {
        return;
    }
    let kind = if node.kind() == "class_specifier" {
        node_kind::CLASS
    } else {
        node_kind::STRUCT
    };
    let (qname, parent_id, key) = match outer {
        Some(o) => (format!("{}::{name}", o.qname), o.id, join(o.key, &name)),
        None => (type_qname(acc, scope, &name), scope.parent_id, join(scope.ns_path, &name)),
    };
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
        acc.nav.record(id, &name, &qname, kind, Some(parent_id));
        if outer.is_some() {
            acc.stats.nested += 1;
        } else {
            acc.stats.count_type(acc.is_header, scope);
            acc.stats.count_linkage(scope);
        }
        acc.stats.unions += usize::from(node.kind() == "union_specifier");
    }
    acc.local_types
        .entry(key.clone())
        .or_insert_with(|| (id, qname.clone()));

    let me = Outer { qname: &qname, id, key: &key };
    visit_members(body, src, file_rel, scope, me, repo, acc);
}

/// The members of a class / struct / union body, including those inside
/// its preprocessor conditionals (LB.10a).
fn visit_members(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    owner: Outer,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        visit_member(child, src, file_rel, scope, owner, repo, acc);
    }
}

/// One member of `owner`'s body: an inline method, and (CB.19) a nested
/// class / struct / union / enum, a member template, and a field's declared
/// type for the receiver pass. A member declaration (`int add(int);`), a
/// friend and a class-scope `using` mint nothing.
fn visit_member(
    child: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    owner: Outer,
    repo: RepoId,
    acc: &mut Acc,
) {
    match child.kind() {
        "function_definition" => {
            visit_method(child, src, file_rel, scope.ns_path, owner, repo, acc);
        }
        "class_specifier" | "struct_specifier" | "union_specifier" => {
            visit_type(child, src, file_rel, scope, Some(owner), repo, acc);
        }
        "enum_specifier" => {
            visit_enum(child, src, file_rel, scope, Some(owner), repo, acc);
        }
        "template_declaration" => {
            visit_template(child, src, file_rel, scope, Some(owner), repo, acc);
        }
        "field_declaration" => visit_field(child, src, file_rel, scope, owner, repo, acc),
        kind if is_preproc_block(kind) => {
            if let Some(branch) = live_branch(child, src) {
                visit_members(branch, src, file_rel, scope, owner, repo, acc);
            }
        }
        _ => {}
    }
}

/// CB.19: a `field_declaration` in `owner`'s body. A type defined in its
/// `type` (`struct Line { .. };`, `struct Line { .. } line;`) is a nested
/// type; every declarator that names a data member (not a member function)
/// records its simple type as `owner`'s field type (`Box box;` ->
/// `box: Box`), which the receiver pass (graph/src/calls.rs) reads for
/// `box.get()`.
fn visit_field(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    owner: Outer,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(ty) = node.child_by_field_name("type") else {
        return;
    };
    match ty.kind() {
        "class_specifier" | "struct_specifier" | "union_specifier" => {
            visit_type(ty, src, file_rel, scope, Some(owner), repo, acc);
        }
        "enum_specifier" => visit_enum(ty, src, file_rel, scope, Some(owner), repo, acc),
        _ => {}
    }
    let Some(type_name) = simple_type(ty, src, acc) else {
        return;
    };
    let mut cursor = node.walk();
    for d in node.children_by_field_name("declarator", &mut cursor) {
        if let Some(field) = declared_name(d, src) {
            acc.nav.record_field_type(owner.id, field, &type_name);
        }
    }
}

fn visit_enum(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &Scope,
    outer: Option<Outer>,
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
    let name = type_name(name_node, src);
    if name.is_empty() {
        return;
    }
    let (qname, parent_id) = match outer {
        Some(o) => (format!("{}::{name}", o.qname), o.id),
        None => (type_qname(acc, scope, &name), scope.parent_id),
    };
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
            .record(id, &name, &qname, node_kind::ENUM, Some(parent_id));
        if outer.is_some() {
            acc.stats.nested += 1;
        } else {
            acc.stats.count_type(acc.is_header, scope);
            acc.stats.count_linkage(scope);
        }
    }
}

/// CB.19: a type's name as its node spells it: a specialisation's
/// `template_type` (`Box<int>`) reads its `name` field (`Box`); any other
/// name (`Line`, a qualified `Outer::Inner`) loses its template arguments.
fn type_name(name_node: TsNode, src: &[u8]) -> String {
    match name_node.kind() {
        "template_type" => name_node
            .child_by_field_name("name")
            .map_or_else(String::new, |n| text_of(n, src).to_string()),
        _ => strip_template_args(text_of(name_node, src)),
    }
}

/// CB.19: `name` without its template argument lists: `Box<T>::get` ->
/// `Box::get`, `max2<int>` -> `max2`, `Map<K, std::vector<V>>::at` ->
/// `Map::at`. An operator's own symbol is kept (`Box<T>::operator<` ->
/// `Box::operator<`); unbalanced brackets leave the name as written.
fn strip_template_args(name: &str) -> String {
    if !name.contains('<') {
        return name.to_string();
    }
    let (head, op) = match name.find("operator") {
        Some(at) => name.split_at(at),
        None => (name, ""),
    };
    let mut out = String::with_capacity(head.len());
    let mut depth = 0usize;
    for ch in head.chars() {
        match ch {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            '>' => return name.to_string(),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    if depth != 0 {
        return name.to_string();
    }
    out.push_str(op);
    out
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
    // CB.19: template arguments are not part of a name (`Box<T>::get`).
    let name = strip_template_args(&extract_func_name(declarator, src));
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
        acc.stats.count_linkage(scope);
    }
    // CB.19 (C6): a `static` free function, or one in an anonymous
    // namespace, is file-local: never the definition another file's
    // prototype names.
    if scope.anon || is_static(node, src) {
        acc.fact(NavFact::InternalLinkage { name: name.clone() });
    }

    visit_callable(node, src, id, scope.ns_path, acc);
}

/// CB.19: what a function / method definition records beyond its node:
/// the call sites of its body, the simple types of its parameters and
/// local declarations (`Acc::nav` local_types, for the receiver pass) and
/// the `using` directives in its body (approximated as file-wide facts).
fn visit_callable(node: TsNode, src: &[u8], fn_id: NodeId, ns_path: &str, acc: &mut Acc) {
    if let Some(declarator) = node.child_by_field_name("declarator") {
        record_params(declarator, src, fn_id, acc);
    }
    if let Some(body) = node.child_by_field_name("body") {
        collect_calls_in(body, src, fn_id, acc);
        collect_body_facts(body, src, fn_id, ns_path, acc);
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

    visit_callable(node, src, id, scope.ns_path, acc);
}

/// An inline method of `owner`; `ns_path` is the namespace around the
/// class (for the `using` facts of its body).
fn visit_method(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    ns_path: &str,
    owner: Outer,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(declarator) = node.child_by_field_name("declarator") else {
        return;
    };
    let name = strip_template_args(&extract_func_name(declarator, src));
    if name.is_empty() {
        return;
    }
    let (parent_qname, parent_id) = (owner.qname, owner.id);
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

    visit_callable(node, src, id, ns_path, acc);
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

/// `#include "local.h"` (local includes only), from the file MODULE
/// whatever scope it sits in (CB.19: `namespace shop { #include ".." }`).
fn collect_include(node: TsNode, src: &[u8], acc: &mut Acc) {
    let Some(path_node) = node.child_by_field_name("path") else {
        return;
    };
    let path = text_of(path_node, src);
    if path.starts_with('"') {
        let cleaned = path.trim_matches('"');
        acc.imports.push(ImportStmt {
            from_module: acc.module_qname.clone(),
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

/// CB.19: a `static` storage class on a definition / declaration.
fn is_static(node: TsNode, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    for c in node.named_children(&mut cursor) {
        if c.kind() == "storage_class_specifier" && text_of(c, src).trim() == "static" {
            return true;
        }
    }
    false
}

/// CB.19 (C6): the function prototypes of a file / namespace-scope
/// `declaration` (`int codec_encode(const char *);`), recorded on the file
/// MODULE: `DeclaresFn { ns, name }` for one whose definition may live in
/// another file, `InternalLinkage { name }` for a `static` one or one in an
/// anonymous namespace (its definition is in this translation unit). A
/// variable (`extern int x;`, a function pointer `int (*fp)(int);`) and a
/// qualified name declare no prototype. Never called for a class body, so a
/// member or friend declaration is not a prototype.
fn record_prototypes(decl: TsNode, src: &[u8], scope: &Scope, acc: &mut Acc) {
    let internal = scope.anon || is_static(decl, src);
    let mut cursor = decl.walk();
    for d in decl.children_by_field_name("declarator", &mut cursor) {
        let Some(name) = prototype_name(d, src) else {
            continue;
        };
        let name = strip_template_args(name);
        if name.is_empty() || name.contains("::") {
            continue;
        }
        let fact = if internal {
            NavFact::InternalLinkage { name }
        } else {
            NavFact::DeclaresFn { ns: scope.ns_path.to_string(), name }
        };
        if acc.fact(fact) {
            acc.stats.prototypes += 1;
            acc.stats.count_linkage(scope);
        }
    }
}

/// The function a declarator declares: through pointer / reference
/// declarators (`struct point *point_new(int)`) to a `function_declarator`
/// whose own declarator is a name. `None` for a variable or a function
/// pointer (`(*fp)(int)`: a parenthesised declarator).
fn prototype_name<'a>(declarator: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut node = declarator;
    loop {
        match node.kind() {
            "function_declarator" => {
                let name = node.child_by_field_name("declarator")?;
                return matches!(
                    name.kind(),
                    "identifier" | "field_identifier" | "operator_name" | "template_function"
                )
                .then(|| text_of(name, src));
            }
            "pointer_declarator" | "attributed_declarator" => {
                node = node.child_by_field_name("declarator").or_else(|| node.named_child(0))?;
            }
            "reference_declarator" => node = node.named_child(0)?,
            _ => return None,
        }
    }
}

/// CB.19 (C7): `using namespace a::b;` -> `UsingNamespace { within, ns:
/// "a::b" }`, `using a::b::f;` -> `UsingName { within, ns: "a::b", name:
/// "f" }`, on the file MODULE wherever they appear; `within` is the
/// namespace path they sit in. `using X = Y;` is an `alias_declaration`,
/// never seen here; `using enum E;` is skipped.
fn record_using(node: TsNode, src: &[u8], within: &str, acc: &mut Acc) {
    let mut cursor = node.walk();
    let mut is_ns = false;
    let mut target: Option<String> = None;
    for c in node.children(&mut cursor) {
        match c.kind() {
            "namespace" => is_ns = true,
            "enum" => return,
            "identifier" | "qualified_identifier" | "namespace_identifier" | "type_identifier" => {
                target = Some(text_of(c, src).split_whitespace().collect());
            }
            _ => {}
        }
    }
    let Some(target) = target else {
        return;
    };
    let within = within.to_string();
    let fact = if is_ns {
        let ns = target.trim_start_matches("::").to_string();
        if ns.is_empty() {
            return;
        }
        NavFact::UsingNamespace { within, ns }
    } else {
        let Some((ns, name)) = target.rsplit_once("::") else {
            return;
        };
        if name.is_empty() {
            return;
        }
        NavFact::UsingName {
            within,
            ns: ns.trim_start_matches("::").to_string(),
            name: name.to_string(),
        }
    };
    if acc.fact(fact) {
        acc.stats.usings += 1;
    }
}

/// CB.19 (C5): the parameters of a function / method definition, as locals
/// of `fn_id` with their simple types (`Cart c` -> `c: Cart`).
fn record_params(declarator: TsNode, src: &[u8], fn_id: NodeId, acc: &mut Acc) {
    let mut node = declarator;
    let fn_decl = loop {
        match node.kind() {
            "function_declarator" => break node,
            "reference_declarator" => match node.named_child(0) {
                Some(inner) => node = inner,
                None => return,
            },
            _ => match node.child_by_field_name("declarator") {
                Some(inner) => node = inner,
                None => return,
            },
        }
    };
    let Some(params) = fn_decl.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        record_param(p, src, fn_id, acc);
    }
}

/// One `parameter_declaration` (or its optional / variadic forms, or a
/// `catch` clause's) as a local of `fn_id`.
fn record_param(p: TsNode, src: &[u8], fn_id: NodeId, acc: &mut Acc) {
    if !matches!(
        p.kind(),
        "parameter_declaration" | "optional_parameter_declaration" | "variadic_parameter_declaration"
    ) {
        return;
    }
    let (Some(ty), Some(d)) = (p.child_by_field_name("type"), p.child_by_field_name("declarator"))
    else {
        return;
    };
    if let (Some(ty), Some(name)) = (simple_type(ty, src, acc), declared_name(d, src)) {
        acc.nav.record_local_type(fn_id, name, &ty);
    }
}

/// CB.19: the local declarations (`Cart c;`, `shop::Cart c(1);`, `auto c =
/// Cart();`, `Cart* p = new Cart();`), range-for variables and `catch`
/// parameters of a body, as locals of `fn_id`, and its `using` directives
/// as file facts. Nested function definitions and lambdas are skipped, as
/// [`collect_calls_in`] skips them.
fn collect_body_facts(body: TsNode, src: &[u8], fn_id: NodeId, ns_path: &str, acc: &mut Acc) {
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "declaration" => record_locals(n, src, fn_id, acc),
            "for_range_loop" => {
                let ty = n.child_by_field_name("type");
                let name = n.child_by_field_name("declarator").and_then(|d| declared_name(d, src));
                if let (Some(ty), Some(name)) = (ty, name) {
                    // `for (auto& x : xs)`: an element of unknown type.
                    let simple = if ty.kind() == "placeholder_type_specifier" {
                        Some(String::new())
                    } else {
                        simple_type(ty, src, acc)
                    };
                    if let Some(simple) = simple {
                        acc.nav.record_local_type(fn_id, name, &simple);
                    }
                }
            }
            "catch_clause" => {
                if let Some(params) = n.child_by_field_name("parameters") {
                    let mut cursor = params.walk();
                    for p in params.named_children(&mut cursor) {
                        record_param(p, src, fn_id, acc);
                    }
                }
            }
            "using_declaration" => record_using(n, src, ns_path, acc),
            _ => {}
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(child.kind(), "function_definition" | "lambda_expression") {
                stack.push(child);
            }
        }
    }
}

/// The locals one body `declaration` binds. An explicit type is shared by
/// every declarator (`Cart i = Cart(), j;`); an `auto` declarator takes the
/// type its initialiser names ([`init_type`]).
fn record_locals(decl: TsNode, src: &[u8], fn_id: NodeId, acc: &mut Acc) {
    let Some(ty) = decl.child_by_field_name("type") else {
        return;
    };
    let is_auto = ty.kind() == "placeholder_type_specifier";
    let declared = if is_auto { None } else { simple_type(ty, src, acc) };
    let mut cursor = decl.walk();
    for d in decl.children_by_field_name("declarator", &mut cursor) {
        let Some(name) = declared_name(d, src) else {
            continue;
        };
        let ty = if is_auto { init_type(d, src, acc) } else { declared.clone() };
        if let Some(ty) = ty {
            acc.nav.record_local_type(fn_id, name, &ty);
        }
    }
}

/// The type an `auto` declarator's initialiser names: `Cart()` /
/// `shop::Cart(1)` -> `Cart` (the callee's last segment, read as a
/// constructor), `new Cart()` / `Cart{}` -> `Cart`; any other initialiser
/// is a local of unknown type (`""`); no initialiser records nothing.
fn init_type(declarator: TsNode, src: &[u8], acc: &Acc) -> Option<String> {
    if declarator.kind() != "init_declarator" {
        return None;
    }
    let Some(value) = declarator.child_by_field_name("value") else {
        return Some(String::new());
    };
    let named = match value.kind() {
        "call_expression" => value.child_by_field_name("function").map(|f| callee_type(f, src, acc)),
        "new_expression" | "compound_literal_expression" => value
            .child_by_field_name("type")
            .and_then(|t| simple_type(t, src, acc)),
        _ => None,
    };
    Some(named.unwrap_or_default())
}

/// The type a constructor-shaped callee names: `Cart`, `shop::Cart`,
/// `Box<int>`; anything else (`obj.make`) is unknown (`""`).
fn callee_type(func: TsNode, src: &[u8], acc: &Acc) -> String {
    match func.kind() {
        "identifier" | "type_identifier" | "qualified_identifier" | "template_type" => {
            simple_type(func, src, acc).unwrap_or_default()
        }
        "template_function" => func
            .child_by_field_name("name")
            .map_or_else(String::new, |n| text_of(n, src).to_string()),
        _ => String::new(),
    }
}

/// CB.19: the simple type name a type specifier spells, for the receiver
/// pass: qualifiers (`const`, `struct`), `*` / `&` (in the declarator) and
/// template arguments dropped, the last `::` segment kept (`shop::Cart` ->
/// `Cart`, `std::unique_ptr<Cart>` -> `unique_ptr`: no container peeling).
/// `None` records nothing: a primitive (`int`, `unsigned long`, `size_t`),
/// `auto` alone, a bodiless use of an anonymous type. `Some("")` is a local
/// of unknown type: a template parameter, `decltype(..)`, a dependent
/// `typename T::x`.
fn simple_type(ty: TsNode, src: &[u8], acc: &Acc) -> Option<String> {
    match ty.kind() {
        "primitive_type" | "sized_type_specifier" | "placeholder_type_specifier" => None,
        "type_identifier" | "identifier" | "namespace_identifier" => {
            let name = text_of(ty, src).trim();
            if is_primitive_name(name) {
                None
            } else if acc.template_params.iter().any(|p| p == name) {
                Some(String::new())
            } else {
                Some(name.to_string())
            }
        }
        "qualified_identifier" | "template_type" | "struct_specifier" | "class_specifier"
        | "union_specifier" | "enum_specifier" => {
            simple_type(ty.child_by_field_name("name")?, src, acc)
        }
        _ => Some(String::new()),
    }
}

/// Builtin type names the C / C++ grammars can spell as a `type_identifier`.
fn is_primitive_name(name: &str) -> bool {
    matches!(
        name,
        "void" | "bool" | "char" | "short" | "int" | "long" | "float" | "double" | "signed"
            | "unsigned" | "size_t" | "ssize_t" | "ptrdiff_t" | "intptr_t" | "uintptr_t"
            | "wchar_t" | "char8_t" | "char16_t" | "char32_t" | "int8_t" | "int16_t" | "int32_t"
            | "int64_t" | "uint8_t" | "uint16_t" | "uint32_t" | "uint64_t" | "auto"
    )
}

/// The name a variable / field / parameter declarator binds (`c`, `*p`,
/// `&r`, `arr[4]`, `x = 1`); `None` for a function declarator (a member or
/// local function declaration), a qualified name (`int Foo::n = 0;`) or a
/// structured binding.
fn declared_name<'a>(declarator: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut node = declarator;
    loop {
        match node.kind() {
            "identifier" | "field_identifier" => return Some(text_of(node, src)),
            "init_declarator" | "pointer_declarator" | "array_declarator" | "attributed_declarator" => {
                node = node.child_by_field_name("declarator").or_else(|| node.named_child(0))?;
            }
            "reference_declarator" | "parenthesized_declarator" => node = node.named_child(0)?,
            _ => return None,
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
            ("a.inl", Dialect::Cpp),
            ("a.ipp", Dialect::Cpp),
            ("a.tpp", Dialect::Cpp),
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

    /// CB.1: `.inl` / `.ipp` / `.tpp` are included implementation files:
    /// parsed with the C++ grammar and named by the header rule, so a global
    /// type takes its directory (`src::P`), while the same type in a `.cpp`
    /// keeps its file scope. Their free functions keep the file scope, like a
    /// header's.
    #[test]
    fn included_implementation_files_take_the_header_rule() {
        let source = "struct P {};\ntemplate <typename T> inline T twice(T v) { return v * 2; }\n";
        for (file, module) in [("src/x.inl", "src::x.inl"), ("src/x.ipp", "src::x.ipp"), ("src/x.tpp", "src::x.tpp")] {
            let dialect = Dialect::from_path(file);
            assert_eq!(dialect, Dialect::Cpp, "{file}");
            let fp = parse_file(source, file, module, dialect, repo()).unwrap();
            assert!(node_at(&fp, node_kind::STRUCT, "src::P").is_some(), "{file}: {:?}", all_qnames(&fp));
            assert!(
                node_at(&fp, node_kind::FUNCTION, &format!("{module}::twice")).is_some(),
                "{file}: {:?}",
                all_qnames(&fp)
            );
        }
        let fp = parse_file(source, "src/x.cpp", "src::x.cpp", Dialect::from_path("src/x.cpp"), repo()).unwrap();
        assert!(node_at(&fp, node_kind::STRUCT, "src::x.cpp::P").is_some(), "{:?}", all_qnames(&fp));
        assert!(node_at(&fp, node_kind::STRUCT, "src::P").is_none(), "{:?}", all_qnames(&fp));
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
                 6 out-of-line members (3 bound in-file, 3 provisional) nested=0 templates=0 unions=0 \
                 anon_ns=0 extern_c=0 prototypes=0 usings=0 file=src/Widget.cpp"
            )
        );
        assert_eq!(IdentityStats::default().marker("x.c"), None);
    }

    // ---- CB.19 ---------------------------------------------------------

    /// The `[qname] c_cpp:` marker a parse of `source` prints.
    fn marker_of(source: &str, file: &str, module: &str) -> String {
        let tree = parse_tree(source, Dialect::from_path(file)).unwrap().tree;
        let (_, stats) = parse_tree_into(&tree, source, file, module, repo());
        stats.marker(file).unwrap_or_default()
    }

    /// The NavFacts a parse recorded on its file MODULE, in order.
    fn module_facts(fp: &FileParse) -> Vec<glia_code_domain::NavFact> {
        let module = fp.nodes[0].id;
        fp.nav.nav_facts.get(&module).cloned().unwrap_or_default()
    }

    fn locals(fp: &FileParse, scope: NodeId) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .nav
            .local_types
            .get(&scope)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    fn fields(fp: &FileParse, owner: NodeId) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .nav
            .field_types
            .get(&owner)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> =
            list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        out.sort();
        out
    }

    /// CB.19 (C1): `extern "C" { .. }` and `extern "C" int f() {..}` are
    /// transparent: their definitions keep the enclosing scope, and a
    /// prototype inside one is a `DeclaresFn` (the fixture's codec.h,
    /// whose block is split across `#ifdef __cplusplus`).
    #[test]
    fn extern_c_block_and_definition() {
        use glia_code_domain::NavFact;
        let source = "\
extern \"C\" {
int c_fn(void) { return 1; }
struct CS { int v; };
int c_proto(int);
}
extern \"C\" int legacy_entry(int v) { return c_fn(); }
extern \"C++\" { int cpp_fn() { return 2; } }
";
        let fp = parse_file(source, "src/a.cpp", "src::a.cpp", Dialect::Cpp, repo()).unwrap();
        let module = fp.nodes[0].id;
        for (kind, q) in [
            (node_kind::FUNCTION, "src::a.cpp::c_fn"),
            (node_kind::FUNCTION, "src::a.cpp::legacy_entry"),
            (node_kind::FUNCTION, "src::a.cpp::cpp_fn"),
            (node_kind::STRUCT, "src::a.cpp::CS"),
        ] {
            let id = node_at(&fp, kind, q).unwrap_or_else(|| panic!("{q} missing: {:?}", all_qnames(&fp)));
            assert_eq!(fp.nav.parent_of[&id], module, "{q}");
        }
        let legacy = node_at(&fp, node_kind::FUNCTION, "src::a.cpp::legacy_entry").unwrap();
        let calls: Vec<&CallQualifier> = fp.calls.iter().filter(|c| c.from == legacy).map(|c| &c.qualifier).collect();
        assert_eq!(calls, [&CallQualifier::Bare("c_fn".to_string())]);
        assert_eq!(module_facts(&fp), [NavFact::DeclaresFn { ns: String::new(), name: "c_proto".to_string() }]);
        assert!(
            marker_of(source, "src/a.cpp", "src::a.cpp").contains(" extern_c=4 prototypes=1 "),
            "{}",
            marker_of(source, "src/a.cpp", "src::a.cpp")
        );

        let codec = "#pragma once\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nint codec_encode(const char *in, char *out);\n#ifdef __cplusplus\n}\n#endif\n";
        let fp = parse_file(codec, "src/codec.h", "src::codec.h", Dialect::Header, repo()).unwrap();
        assert_eq!(
            module_facts(&fp),
            [NavFact::DeclaresFn { ns: String::new(), name: "codec_encode".to_string() }]
        );
        assert_eq!(count(&fp, node_kind::FUNCTION), 0, "a prototype mints no node");
        assert_eq!(
            marker_of(codec, "src/codec.h", "src::codec.h"),
            "[qname] c_cpp: 0 header types scoped (0 namespace, 0 directory), 0 file-local types kept, \
             0 out-of-line members (0 bound in-file, 0 provisional) nested=0 templates=0 unions=0 \
             anon_ns=0 extern_c=1 prototypes=1 usings=0 file=src/codec.h"
        );
    }

    /// CB.19 (C1 / C6): an anonymous namespace mints no PACKAGE; its
    /// functions keep the file scope (`src::cart.cpp::clamp`, or the
    /// enclosing named namespace's) and, like a `static` one, record
    /// `InternalLinkage` on the file MODULE.
    #[test]
    fn anonymous_namespace_functions_are_file_scoped_and_internal() {
        use glia_code_domain::NavFact;
        let source = "\
namespace {
int clamp(int v) { return v < 0 ? 0 : v; }
struct Hidden { int x; };
int later(int);
}
namespace shop { namespace { int inner() { return clamp(1); } } }
static int s() { return 0; }
int pub_fn() { return clamp(2); }
";
        let fp = parse_file(source, "src/cart.cpp", "src::cart.cpp", Dialect::Cpp, repo()).unwrap();
        let module = fp.nodes[0].id;
        let clamp = node_at(&fp, node_kind::FUNCTION, "src::cart.cpp::clamp").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&clamp], module);
        assert_eq!(defines(&fp, module, clamp), 1);
        assert!(node_at(&fp, node_kind::STRUCT, "src::cart.cpp::Hidden").is_some(), "{:?}", all_qnames(&fp));
        let shop = node_at(&fp, node_kind::PACKAGE, "src::cart.cpp::shop").expect("PACKAGE shop");
        let inner = node_at(&fp, node_kind::FUNCTION, "src::cart.cpp::shop::inner").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&inner], shop);
        assert_eq!(count(&fp, node_kind::PACKAGE), 1, "an anonymous namespace is no PACKAGE: {:?}", all_qnames(&fp));
        let internal = |n: &str| NavFact::InternalLinkage { name: n.to_string() };
        assert_eq!(module_facts(&fp), [internal("clamp"), internal("later"), internal("inner"), internal("s")]);

        // The fixture's cart.cpp: one anonymous-namespace function, one
        // extern "C" definition, two provisional out-of-line members.
        let cart = "#include \"cart.hpp\"\n\nnamespace {\nint clamp(int v) { return v < 0 ? 0 : v; }\n}\n\nnamespace shop {\nint Cart::add(int qty) {\n    return clamp(qty);\n}\n\nint Cart::Line::total() const { return qty; }\n}\n\nextern \"C\" int legacy_entry(int v) {\n    return clamp(v);\n}\n";
        assert_eq!(
            marker_of(cart, "src/cart.cpp", "src::cart.cpp"),
            "[qname] c_cpp: 0 header types scoped (0 namespace, 0 directory), 0 file-local types kept, \
             2 out-of-line members (0 bound in-file, 2 provisional) nested=0 templates=0 unions=0 \
             anon_ns=1 extern_c=1 prototypes=0 usings=0 file=src/cart.cpp"
        );
        let fp = parse_file(cart, "src/cart.cpp", "src::cart.cpp", Dialect::Cpp, repo()).unwrap();
        assert!(node_at(&fp, node_kind::FUNCTION, "src::cart.cpp::legacy_entry").is_some(), "{:?}", all_qnames(&fp));
        assert!(node_at(&fp, node_kind::METHOD, "shop::Cart::Line::total").is_some(), "{:?}", all_qnames(&fp));
    }

    /// CB.19 (C1): a template's inner declaration is the entity, named
    /// without template arguments: a class template, a member template, a
    /// function template, and an out-of-line member `Box<T>::peek` that
    /// binds to the in-file class. A template parameter is an unknown type.
    #[test]
    fn template_class_and_function() {
        let source = "\
namespace shop {
template <typename T>
class Box {
public:
    T get() const { return value; }
    template <class U> void each(U u) { u(); }
    T value;
    Box<T>* next;
};
template <class T> T max2(T a, T b) { return a > b ? a : b; }
template <class T> T Box<T>::peek() const { return value; }
}
";
        let fp = parse_file(source, "include/box.hpp", "include::box.hpp", Dialect::Cpp, repo()).unwrap();
        let class = node_at(&fp, node_kind::CLASS, "shop::Box").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&class], "Box");
        for q in ["shop::Box::get", "shop::Box::each", "shop::Box::peek"] {
            let m = node_at(&fp, node_kind::METHOD, q).unwrap_or_else(|| panic!("{q} missing: {:?}", all_qnames(&fp)));
            assert_eq!(fp.nav.parent_of[&m], class, "{q}");
        }
        let max2 = node_at(&fp, node_kind::FUNCTION, "include::box.hpp::shop::max2").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.name_by_id[&max2], "max2");
        assert!(all_qnames(&fp).iter().all(|q| !q.contains('<')), "{:?}", all_qnames(&fp));
        // `T value` is a template parameter: no field type; `Box<T>* next`
        // is `Box`. `U u` / `T a` are locals of unknown type.
        assert_eq!(fields(&fp, class), pairs(&[("next", "Box")]));
        let each = node_at(&fp, node_kind::METHOD, "shop::Box::each").unwrap();
        assert_eq!(locals(&fp, each), pairs(&[("u", "")]));
        assert_eq!(locals(&fp, max2), pairs(&[("a", ""), ("b", "")]));
        assert!(marker_of(source, "include/box.hpp", "include::box.hpp").contains(" templates=4 "));
        assert_eq!(strip_template_args("Map<K, std::vector<V>>::at"), "Map::at");
        assert_eq!(strip_template_args("Box<T>::operator<"), "Box::operator<");
        assert_eq!(strip_template_args("operator<<"), "operator<<");
        assert_eq!(strip_template_args("a<b"), "a<b");
    }

    /// CB.19 (C1): an explicit specialisation `Box<int>` is named `Box` and
    /// merges into the primary template's node: one CLASS, one DEFINES,
    /// both bodies as CODE cells; its members merge by name the same way.
    #[test]
    fn template_specialisation_merges() {
        let source = "\
template <class T> class Box { public: T get() const { return v; } T v; };
template <> class Box<int> { public: int get() const { return 0; } int extra() { return 1; } };
";
        let fp = parse_file(source, "src/box.h", "src::box.h", Dialect::Header, repo()).unwrap();
        let module = fp.nodes[0].id;
        let class = node_at(&fp, node_kind::CLASS, "src::Box").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(count(&fp, node_kind::CLASS), 1);
        assert_eq!(defines(&fp, module, class), 1);
        assert_eq!(code_cells(&fp, class).len(), 2, "the specialisation's body stacks on the primary");
        let get = node_at(&fp, node_kind::METHOD, "src::Box::get").expect("METHOD get");
        assert_eq!(code_cells(&fp, get).len(), 2);
        assert_eq!(defines(&fp, class, get), 1);
        assert!(node_at(&fp, node_kind::METHOD, "src::Box::extra").is_some(), "{:?}", all_qnames(&fp));
    }

    /// CB.19 (C4): a union is a STRUCT - at namespace scope, in a
    /// declaration, and nested in a struct; an anonymous union mints
    /// nothing.
    #[test]
    fn union_is_a_struct() {
        let source = "\
namespace shop { union Number { int i; float f; }; }
union Tagged { int a; } tagged;
struct S { union { int x; } anon; union Inner { int y; } in; };
";
        let fp = parse_file(source, "src/n.hpp", "src::n.hpp", Dialect::Cpp, repo()).unwrap();
        for q in ["shop::Number", "src::Tagged", "src::S", "src::S::Inner"] {
            assert!(node_at(&fp, node_kind::STRUCT, q).is_some(), "{q} missing: {:?}", all_qnames(&fp));
        }
        assert_eq!(count(&fp, node_kind::STRUCT), 4, "{:?}", all_qnames(&fp));
        assert_eq!(count(&fp, node_kind::CLASS), 0);
        let s = node_at(&fp, node_kind::STRUCT, "src::S").unwrap();
        assert_eq!(fields(&fp, s), pairs(&[("in", "Inner")]));
        assert!(marker_of(source, "src/n.hpp", "src::n.hpp").contains(" nested=1 templates=0 unions=3 "));
    }

    /// CB.19 (C1): a nested type is `<outer qname>::<name>` with the outer
    /// type as nav parent and DEFINES source, its members under it - in a
    /// header (the fixture's cart.hpp) and in a source file.
    #[test]
    fn nested_type_and_member() {
        let source = "#pragma once\nnamespace shop {\n#include \"detail.hpp\"\n\ntemplate <typename T>\nclass Box {\npublic:\n    T get() const { return value; }\n    T value;\n};\n\nunion Number {\n    int i;\n    float f;\n};\n\nclass Cart {\npublic:\n    struct Line {\n        int qty;\n        int total() const { return qty * 2; }\n        enum class Kind { A };\n    };\n    int add(int qty);\n    Line first;\n};\n}\n";
        let fp = parse_file(source, "src/cart.hpp", "src::cart.hpp", Dialect::Cpp, repo()).unwrap();
        let cart = node_at(&fp, node_kind::CLASS, "shop::Cart").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        let line = node_at(&fp, node_kind::STRUCT, "shop::Cart::Line").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&line], cart);
        assert_eq!(fp.nav.name_by_id[&line], "Line");
        assert_eq!(defines(&fp, cart, line), 1);
        let total = node_at(&fp, node_kind::METHOD, "shop::Cart::Line::total").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&total], line);
        assert_eq!(defines(&fp, line, total), 1);
        let kind = node_at(&fp, node_kind::ENUM, "shop::Cart::Line::Kind").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&kind], line);
        assert_eq!(fields(&fp, cart), pairs(&[("first", "Line")]));
        assert!(node_at(&fp, node_kind::METHOD, "shop::Box::get").is_some());
        assert_eq!(
            marker_of(source, "src/cart.hpp", "src::cart.hpp"),
            "[qname] c_cpp: 3 header types scoped (3 namespace, 0 directory), 0 file-local types kept, \
             0 out-of-line members (0 bound in-file, 0 provisional) nested=2 templates=1 unions=1 \
             anon_ns=0 extern_c=0 prototypes=0 usings=0 file=src/cart.hpp"
        );

        // A source file's nested type keeps its outermost type's file scope.
        let local = "class Outer { struct Inner { int f() { return 1; } }; };\n";
        let fp = parse_file(local, "src/a.cpp", "src::a.cpp", Dialect::Cpp, repo()).unwrap();
        let inner = node_at(&fp, node_kind::STRUCT, "src::a.cpp::Outer::Inner").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        let f = node_at(&fp, node_kind::METHOD, "src::a.cpp::Outer::Inner::f").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&f], inner);
    }

    /// CB.19: an out-of-line member of a nested type binds in-file through
    /// the nested type's C++ name (`shop::Cart::Line`), looked up from the
    /// definition's namespace outward.
    #[test]
    fn out_of_line_nested_member_binds_in_file() {
        let source = "\
namespace shop {
class Cart { public: struct Line { int total() const; int twice() const; }; };
int Cart::Line::total() const { return 1; }
}
int shop::Cart::Line::twice() const { return total() * 2; }
";
        let fp = parse_file(source, "src/cart.cpp", "src::cart.cpp", Dialect::Cpp, repo()).unwrap();
        let line = node_at(&fp, node_kind::STRUCT, "src::cart.cpp::shop::Cart::Line").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        for q in ["src::cart.cpp::shop::Cart::Line::total", "src::cart.cpp::shop::Cart::Line::twice"] {
            let m = node_at(&fp, node_kind::METHOD, q).unwrap_or_else(|| panic!("{q} missing: {:?}", all_qnames(&fp)));
            assert_eq!(fp.nav.parent_of[&m], line, "{q}");
            assert_eq!(defines(&fp, line, m), 1, "{q}");
        }
        assert_eq!(count(&fp, node_kind::FUNCTION), 0, "{:?}", all_qnames(&fp));
        assert!(marker_of(source, "src/cart.cpp", "src::cart.cpp").contains("(2 bound in-file, 0 provisional)"));

        // The same in a header: the nested type's C++ name is its qname.
        let fp = parse_file(source, "src/cart.hpp", "src::cart.hpp", Dialect::Cpp, repo()).unwrap();
        let line = node_at(&fp, node_kind::STRUCT, "shop::Cart::Line").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        let total = node_at(&fp, node_kind::METHOD, "shop::Cart::Line::total").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(fp.nav.parent_of[&total], line);
    }

    /// CB.19 (C3): an `#include` inside a namespace block is the file's:
    /// `from_module` is the file MODULE, never the namespace PACKAGE.
    #[test]
    fn include_inside_namespace_is_the_files() {
        let source = "#pragma once\nnamespace shop {\n#include \"detail.hpp\"\nnamespace inner {\n#include \"more.hpp\"\n}\n}\n#include \"top.hpp\"\n";
        let fp = parse_file(source, "src/cart.hpp", "src::cart.hpp", Dialect::Cpp, repo()).unwrap();
        let got: Vec<(&str, u32)> = fp.imports.iter().map(|i| (i.from_module.as_str(), i.line)).collect();
        assert_eq!(got, [("src::cart.hpp", 2), ("src::cart.hpp", 4), ("src::cart.hpp", 7)]);
    }

    /// CB.19 (C5): field types of a class body and the parameter / local
    /// types of every function body, as simple type names; primitives record
    /// nothing, an unreadable type records a local of unknown type (`""`).
    #[test]
    fn field_and_local_types_recorded() {
        let source = "\
class Box { public: int get() const { return 1; } };
class Cart {
  Box box;
  std::unique_ptr<Cart> owned, *second;
  const shop::Cart& ref;
  int n;
  int proto(int);
 public:
  int add(Box b, const Box& r, Box* p, int k) {
    Cart c;
    shop::Cart e(1);
    auto f = Cart();
    Cart* g = new Cart();
    auto h = new Box();
    auto m = items[0];
    int z = 0;
    for (Box x : list) {}
    for (auto& y : list) {}
    try {} catch (const Box& err) {}
    return box.get();
  }
};
int run(Cart c) { Cart d; return c.add(d); }
";
        let fp = parse_file(source, "src/run.cpp", "src::run.cpp", Dialect::Cpp, repo()).unwrap();
        let cart = node_at(&fp, node_kind::CLASS, "src::run.cpp::Cart").unwrap_or_else(|| panic!("{:?}", all_qnames(&fp)));
        assert_eq!(
            fields(&fp, cart),
            pairs(&[("box", "Box"), ("owned", "unique_ptr"), ("second", "unique_ptr"), ("ref", "Cart")])
        );
        let add = node_at(&fp, node_kind::METHOD, "src::run.cpp::Cart::add").unwrap();
        assert_eq!(
            locals(&fp, add),
            pairs(&[
                ("b", "Box"),
                ("r", "Box"),
                ("p", "Box"),
                ("c", "Cart"),
                ("e", "Cart"),
                ("f", "Cart"),
                ("g", "Cart"),
                ("h", "Box"),
                ("m", ""),
                ("x", "Box"),
                ("y", ""),
                ("err", "Box"),
            ])
        );
        let run = node_at(&fp, node_kind::FUNCTION, "src::run.cpp::run").unwrap();
        assert_eq!(locals(&fp, run), pairs(&[("c", "Cart"), ("d", "Cart")]));
        // The call sites the receiver pass reads them for.
        assert!(fp.calls.iter().any(|c| c.from == add
            && c.qualifier == CallQualifier::Attribute { base: "box".to_string(), name: "get".to_string() }));
    }

    /// CB.19 (C6 / C7): file / namespace-scope prototypes are `DeclaresFn`
    /// (a `static` one `InternalLinkage`), `using` directives and
    /// declarations are recorded wherever they sit, and a `static`
    /// definition is `InternalLinkage`; variables, function pointers,
    /// member and friend declarations and type aliases record nothing, and
    /// an `#ifdef` walked twice records each fact once.
    #[test]
    fn prototypes_usings_and_static_recorded() {
        use glia_code_domain::NavFact;
        let source = "\
int codec_encode(const char *in);
struct point *point_new(int);
static int s_proto(int);
int (*fp)(int);
extern int counter;
namespace shop { int ns_fn(int); using namespace detail; }
using namespace std;
using shop::Cart;
using Alias = shop::Cart;
static int helper(int x) { return x; }
int visible(int x) { using namespace inner::deep; return x; }
class K { int member(int); friend int peer(K&); };
#ifdef A
int twice(int);
#else
int twice(int);
#endif
";
        let fp = parse_file(source, "src/codec.hpp", "src::codec.hpp", Dialect::Cpp, repo()).unwrap();
        let declares = |ns: &str, n: &str| NavFact::DeclaresFn { ns: ns.to_string(), name: n.to_string() };
        let internal = |n: &str| NavFact::InternalLinkage { name: n.to_string() };
        assert_eq!(
            module_facts(&fp),
            [
                declares("", "codec_encode"),
                declares("", "point_new"),
                internal("s_proto"),
                declares("shop", "ns_fn"),
                NavFact::UsingNamespace { within: "shop".to_string(), ns: "detail".to_string() },
                NavFact::UsingNamespace { within: String::new(), ns: "std".to_string() },
                NavFact::UsingName { within: String::new(), ns: "shop".to_string(), name: "Cart".to_string() },
                internal("helper"),
                NavFact::UsingNamespace { within: String::new(), ns: "inner::deep".to_string() },
                declares("", "twice"),
            ]
        );
        assert!(
            marker_of(source, "src/codec.hpp", "src::codec.hpp").contains(" prototypes=5 usings=4 "),
            "{}",
            marker_of(source, "src/codec.hpp", "src::codec.hpp")
        );

        // A C file: a `static` definition is internal, an external one is
        // no fact at all.
        let c = "static int shift(int c) { return c + 1; }\nint codec_encode(const char *in) { return shift(in[0]); }\n";
        let fp = parse_file(c, "src/codec.c", "src::codec.c", Dialect::C, repo()).unwrap();
        assert_eq!(module_facts(&fp), [internal("shift")]);
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
            ("a.inl", true),
            ("a.ipp", true),
            ("a.tpp", true),
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
