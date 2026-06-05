//! repo-graph-parser-python — tree-sitter Python → `repo_graph_core` types.
//!
//! Single-file scan: emit Module/Class/Function/Method nodes with Code/Doc/
//! Position cells, intra-file `defines` and `calls` edges. Cross-file refs
//! (imports, bare-name or attribute calls that bind to another module) are
//! recorded as `ImportStmt` / `CallSite` for the graph crate's cross-file
//! resolver.
//!
//! All code-domain primitives (constants, `FileParse`, `CodeNav`,
//! `ImportStmt`, `CallSite`, `ParseError`) live in `repo-graph-code-domain`
//! and are re-exported from this crate for convenience.

use std::collections::HashMap;

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};

/// Parse one Python source file.
///
/// `module_qname` is the dotted module path in `::` form (`myapp::users`).
/// `file_rel_path` is the repo-relative path stored in position cells.
pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();

    let mut acc = Acc::default();
    let root = tree.root_node();

    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    acc.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&root, src, file_rel_path),
    });
    let module_simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    acc.nav
        .record(module_id, module_simple, module_qname, node_kind::MODULE, None);

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "class_definition" => {
                visit_class(child, src, file_rel_path, module_qname, module_id, repo, &mut acc);
            }
            "function_definition" => {
                visit_function(
                    child, src, file_rel_path, module_qname, module_id, None, repo, &mut acc,
                    &[],
                );
            }
            "decorated_definition" => {
                visit_decorated_top(
                    child, src, file_rel_path, module_qname, module_id, repo, &mut acc,
                );
            }
            "import_statement" => collect_import(child, src, module_qname, &mut acc),
            "import_from_statement" => collect_import_from(child, src, module_qname, &mut acc),
            "expression_statement" => {
                // Top-level calls — record them with module as source.
                collect_calls_in(child, src, module_id, None, &mut acc);
                // Django-style path('/x', view) registrations in urls.py scan.
                scan_django_routes(child, src, repo, &mut acc);
                // glia v5 G19 — module-level constant assignments
                // (`MAX_RETRIES = …`). Only UPPERCASE names.
                collect_state_vars(
                    child, src, file_rel_path, module_qname, module_id, repo, &mut acc,
                );
            }
            "assignment" => {
                // urlpatterns = [ path(...), re_path(...) ] lives here too.
                scan_django_routes(child, src, repo, &mut acc);
            }
            _ => {}
        }
    }

    resolve_intra_file(acc, repo)
}

/// Unwrap a top-level `decorated_definition` into its inner def + decorator
/// list, then dispatch. v0.4.11a R-python — needed so Flask/FastAPI handlers
/// (which are always decorated) emit both their function node and the
/// associated Route nodes.
fn visit_decorated_top(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let (decos, inner) = split_decorated(n);
    let Some(inner) = inner else { return };
    match inner.kind() {
        "function_definition" => {
            visit_function(
                inner, src, file_rel, module_qname, module_id, None, repo, acc, &decos,
            );
        }
        "class_definition" => {
            // Class decorators are rare route surface in Py frameworks; skip
            // route extraction here but still visit so nodes/methods emit.
            visit_class(inner, src, file_rel, module_qname, module_id, repo, acc);
        }
        _ => {}
    }
}

fn split_decorated<'a>(n: TsNode<'a>) -> (Vec<TsNode<'a>>, Option<TsNode<'a>>) {
    let mut decos = Vec::new();
    let mut inner = None;
    let mut cursor = n.walk();
    for c in n.named_children(&mut cursor) {
        match c.kind() {
            "decorator" => decos.push(c),
            "function_definition" | "class_definition" => inner = Some(c),
            _ => {}
        }
    }
    (decos, inner)
}

// ============================================================================
// Internal accumulator
// ============================================================================

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    unresolved: Vec<UnresolvedCall>,
    /// v0.4.13a — type-annotation USES refs (cross-file resolvable).
    refs: Vec<UnresolvedRef>,
    /// module-level functions: bare name → node id
    module_functions: HashMap<String, NodeId>,
    /// v0.4.13a — module-level classes: bare name → class node id. Needed for
    /// intra-file super() resolution where the base class is in the same file.
    module_classes: HashMap<String, NodeId>,
    /// class methods: (class id, method name) → method node id
    class_methods: HashMap<(NodeId, String), NodeId>,
    /// v0.4.13a — per-class base-class simple names, in declaration order.
    /// Populated from `class Foo(Bar, Baz):`. Used to resolve super() calls.
    class_bases: HashMap<NodeId, Vec<String>>,
    /// v0.4.13 — per-class attribute names already emitted as ATTRIBUTE nodes.
    /// Dedupe set so `self.x` in `__init__` + `x: int = ...` class-level both
    /// observing the same attribute produce one node, one HAS_ATTRIBUTE edge.
    class_attrs: HashMap<NodeId, std::collections::HashSet<String>>,
    /// v0.4.13b — method ids with `@property` decorator. Read as `self.x`
    /// (valid attribute access) rather than `self.x()`. Lets composition-path
    /// synth filter method→class hops to only syntactically valid reads.
    properties: std::collections::HashSet<NodeId>,
    nav: CodeNav,
}

struct UnresolvedCall {
    from: NodeId,
    enclosing_class: Option<NodeId>,
    qualifier: CallQualifier,
}

// ============================================================================
// Visitors
// ============================================================================

fn visit_class(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = child_text(n, "name", src) else {
        return;
    };
    let class_qname = format!("{module_qname}::{name}");
    let class_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, &class_qname);
    acc.nodes.push(Node {
        id: class_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&n, src, file_rel),
    });
    acc.edges.push(Edge {
        from: module_id,
        to: class_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.module_classes.insert(name.to_string(), class_id);
    acc.nav
        .record(class_id, name, &class_qname, node_kind::CLASS, Some(module_id));

    // v0.4.13a — record base class simple names for super() resolution.
    // `class Foo(Bar, pkg.Baz, metaclass=Meta):` → record ["Bar", "Baz"].
    // Attributes (`pkg.Baz`) keep the trailing name; keyword args skipped.
    //
    // v0.4.13 — also emit INHERITS_FROM UnresolvedRef per base so the graph
    // crate's cross-file resolver can wire `Class → base_class` edges. Intra-
    // file matches are additionally handled here to keep resolution eager.
    if let Some(bases_list) = n.child_by_field_name("superclasses") {
        let mut bc = bases_list.walk();
        let mut out = Vec::new();
        for arg in bases_list.named_children(&mut bc) {
            let base_name = match arg.kind() {
                "identifier" => Some(text(arg, src).to_string()),
                "attribute" => arg
                    .child_by_field_name("attribute")
                    .map(|a| text(a, src).to_string()),
                _ => None,
            };
            if let Some(base) = base_name {
                acc.refs.push(UnresolvedRef {
                    from: class_id,
                    from_module: module_id,
                    qualifier: CallQualifier::Bare(base.clone()),
                    category: edge_category::INHERITS_FROM,
                });
                out.push(base);
            }
        }
        if !out.is_empty() {
            acc.class_bases.insert(class_id, out);
        }
    }

    let Some(body) = n.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "function_definition" => {
                visit_method(
                    member, src, file_rel, &class_qname, class_id, module_id, repo, acc, &[],
                );
            }
            "decorated_definition" => {
                let (decos, inner) = split_decorated(member);
                if let Some(inner) = inner
                    && inner.kind() == "function_definition"
                {
                    visit_method(
                        inner,
                        src,
                        file_rel,
                        &class_qname,
                        class_id,
                        module_id,
                        repo,
                        acc,
                        &decos,
                    );
                }
            }
            // v0.4.13 — class-level attribute declarations.
            // `x = ...` → expression_statement > assignment > left: identifier
            // `x: T = ...` / `x: T` → same shape, left is identifier with type
            "expression_statement" => {
                let mut ec = member.walk();
                for child in member.named_children(&mut ec) {
                    if matches!(child.kind(), "assignment") {
                        if let Some(lhs) = child.child_by_field_name("left")
                            && lhs.kind() == "identifier"
                        {
                            let attr_name = text(lhs, src);
                            emit_class_attribute(
                                class_id,
                                &class_qname,
                                attr_name,
                                file_rel,
                                module_id,
                                repo,
                                acc,
                            );
                            // v0.4.13b — typed class attribute `x: T = …` / `x: T`.
                            // Emit USES refs from the attribute node to each
                            // class name in the annotation; enables cross-file
                            // type surfacing for typed attrs even without RHS
                            // inference.
                            if !(attr_name.starts_with("__") && attr_name.ends_with("__")) {
                                let attr_qname = format!("{class_qname}::{attr_name}");
                                let attr_id = NodeId::from_parts(
                                    GRAPH_TYPE,
                                    repo,
                                    node_kind::ATTRIBUTE,
                                    &attr_qname,
                                );
                                if let Some(ty) = child.child_by_field_name("type") {
                                    collect_attr_type_ref(ty, src, attr_id, module_id, acc);
                                }
                                // v0.4.13b — RHS constructor inference for
                                // class-level `x = Target(...)`.
                                if let Some(rhs) = child.child_by_field_name("right") {
                                    emit_rhs_constructor_refs(
                                        rhs, src, attr_id, module_id, acc,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// v0.4.13 — create an ATTRIBUTE node + HAS_ATTRIBUTE edge for a class
/// attribute, deduped per class. Called from both the class-body walker (for
/// class-level declarations) and from the `self.<attr> = …` scanner in method
/// bodies. Single entry point keeps the dedupe in one place.
fn emit_class_attribute(
    class_id: NodeId,
    class_qname: &str,
    attr_name: &str,
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // Skip Python-internal names — `__init__`, `__slots__`, etc. are not
    // compositional attributes; they're language machinery.
    if attr_name.starts_with("__") && attr_name.ends_with("__") {
        return;
    }
    let set = acc.class_attrs.entry(class_id).or_default();
    if !set.insert(attr_name.to_string()) {
        return; // already emitted for this class
    }
    let attr_qname = format!("{class_qname}::{attr_name}");
    let attr_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ATTRIBUTE, &attr_qname);
    // Minimal cells — position points to the class (attribute spans vary); a
    // single POSITION cell is enough for projection/activation. No Code cell
    // since the attribute body is the class body.
    let pos_json = format!(
        "{{\"file\":\"{}\"}}",
        file_rel.replace('\\', "\\\\").replace('"', "\\\""),
    );
    acc.nodes.push(Node {
        id: attr_id,
        repo,
        confidence: Confidence::Weak,
        cells: vec![Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(pos_json),
        }],
    });
    acc.edges.push(Edge {
        from: class_id,
        to: attr_id,
        category: edge_category::HAS_ATTRIBUTE,
        confidence: Confidence::Strong,
    });
    acc.nav.record(
        attr_id,
        attr_name,
        &attr_qname,
        node_kind::ATTRIBUTE,
        Some(class_id),
    );
    // Silence unused warning for module_id — attribute targets don't need
    // cross-file resolution (they're always local to their class).
    let _ = module_id;
}

/// v0.4.13 — walk a method body scanning for every `self.<attr>` access
/// (assignment LHS or plain read), and for each, emit ATTRIBUTE + HAS_ATTRIBUTE
/// via `emit_class_attribute`. Dedupe is handled downstream.
///
/// Scanning reads, not just assignments, catches two important patterns:
///   1. `__init__`-style: `self.opts = SchemaOpts(meta)` — assignment.
///   2. **Metaclass-attached attributes**: marshmallow's `Schema.opts` is set
///      via `klass.opts = ...` inside `SchemaMeta.__new__` (line 112 of
///      schema.py), never through `self.`. But every Schema method reads
///      `self.opts.X` — so scanning reads surfaces the attribute on Schema.
/// The read-based signal matches what any reader (human or LLM) infers:
/// if a class's methods read `self.foo`, the class has an attribute `foo`.
fn collect_self_attr_assignments(
    body: TsNode,
    src: &[u8],
    class_id: NodeId,
    class_qname: &str,
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        // Don't descend into nested function/class bodies — `self` there is a
        // different class's self. Method bodies get their own call.
        if matches!(node.kind(), "function_definition" | "class_definition") {
            continue;
        }
        // Any `self.<attr>` — assignment LHS, read in an expression, call
        // receiver — is represented as an `attribute` node in the AST with
        // `object` = `self` identifier, `attribute` = the attr name.
        if node.kind() == "attribute"
            && let Some(obj) = node.child_by_field_name("object")
            && obj.kind() == "identifier"
            && text(obj, src) == "self"
            && let Some(attr) = node.child_by_field_name("attribute")
        {
            let attr_name = text(attr, src);
            emit_class_attribute(
                class_id, class_qname, attr_name, file_rel, module_id, repo, acc,
            );
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            stack.push(child);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_method(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    class_qname: &str,
    class_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
    decorators: &[TsNode],
) {
    let Some(name) = child_text(n, "name", src) else {
        return;
    };
    let method_qname = format!("{class_qname}::{name}");
    let method_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &method_qname);
    acc.nodes.push(Node {
        id: method_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&n, src, file_rel),
    });
    acc.edges.push(Edge {
        from: class_id,
        to: method_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.class_methods
        .insert((class_id, name.to_string()), method_id);
    acc.nav
        .record(method_id, name, &method_qname, node_kind::METHOD, Some(class_id));

    for deco in decorators {
        check_route_decorator(*deco, src, method_id, repo, acc);
        // v0.4.13b — `@property` marks the method as attribute-style.
        let raw = text(*deco, src);
        let body = raw.trim_start_matches('@').trim();
        if body == "property" {
            acc.properties.insert(method_id);
        }
    }

    // v0.4.13a — emit USES refs for each type name referenced in parameter
    // annotations and the return-type annotation. Enables cross-file class
    // surfacing via PPR on the model's own annotations.
    collect_type_refs(n, src, method_id, module_id, acc);

    // v0.4.13 — RETURNS_TYPE edge for explicit `def m(self) -> T:` annotations.
    // Keyed off the method/function node so BFS can jump `method → return class`
    // when composing access paths.
    collect_return_type_ref(n, src, method_id, module_id, acc);

    if let Some(body) = n.child_by_field_name("body") {
        collect_calls_in(body, src, method_id, Some(class_id), acc);
        // v0.4.13 — scan for `self.<attr> = …` assignments that define class
        // attributes via instance-side `__init__`-style initialisation.
        collect_self_attr_assignments(
            body, src, class_id, class_qname, file_rel, module_id, repo, acc,
        );
        // v0.4.13b — RHS constructor inference: `self.<attr> = Target(...)`
        // emits USES ref from ATTRIBUTE to Target so PPR can surface the
        // concrete type when the attribute is activated.
        collect_self_attr_rhs_types(
            body, src, class_qname, module_id, repo, acc,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_function(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    parent_func_id: Option<NodeId>,
    repo: RepoId,
    acc: &mut Acc,
    decorators: &[TsNode],
) {
    let Some(name) = child_text(n, "name", src) else {
        return;
    };
    let func_qname = format!("{module_qname}::{name}");
    let func_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &func_qname);
    acc.nodes.push(Node {
        id: func_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&n, src, file_rel),
    });
    let parent = parent_func_id.unwrap_or(module_id);
    acc.edges.push(Edge {
        from: parent,
        to: func_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    // Only top-level functions go in the module symbol table — nested ones
    // aren't reachable by bare name from module scope.
    if parent_func_id.is_none() {
        acc.module_functions
            .insert(name.to_string(), func_id);
    }
    acc.nav
        .record(func_id, name, &func_qname, node_kind::FUNCTION, Some(parent));

    for deco in decorators {
        check_route_decorator(*deco, src, func_id, repo, acc);
    }

    // v0.4.13a — USES refs from parameter/return type annotations.
    collect_type_refs(n, src, func_id, module_id, acc);

    // v0.4.13 — RETURNS_TYPE edge for explicit `def f() -> T:` annotations.
    collect_return_type_ref(n, src, func_id, module_id, acc);

    if let Some(body) = n.child_by_field_name("body") {
        collect_calls_in(body, src, func_id, None, acc);
        // Nested defs inside the body — visited recursively.
        let mut cursor = body.walk();
        for member in body.named_children(&mut cursor) {
            match member.kind() {
                "function_definition" => visit_function(
                    member, src, file_rel, &func_qname, module_id, Some(func_id), repo, acc, &[],
                ),
                "decorated_definition" => {
                    let (decos, inner) = split_decorated(member);
                    if let Some(inner) = inner
                        && inner.kind() == "function_definition"
                    {
                        visit_function(
                            inner, src, file_rel, &func_qname, module_id, Some(func_id), repo,
                            acc, &decos,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

// ============================================================================
// State variables (glia v5 G19)
// ============================================================================
//
// Module-level constants — `MAX_RETRIES = 3`, `DEFAULTS = {...}`. tree-sitter
// represents these as a top-level `expression_statement` wrapping an
// `assignment` whose `left` is a bare `identifier`. We emit one STATE_VAR per
// UPPERCASE name (screaming-snake convention); lowercase names are skipped to
// avoid noise from ordinary module-scope locals.
//
// Noise gate: an UPPERCASE name with no leading doc whose RHS is a single
// literal primitive (number / string / bool / None) is skipped. Documented
// constants and non-trivial initialisers (calls, lists, dicts, tuples) are
// kept. Module-level vars rarely carry docstrings, so leading_doc/None is the
// common case — the literal-primitive test does the real filtering.

#[allow(clippy::too_many_arguments)]
fn collect_state_vars(
    stmt: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = stmt.walk();
    for child in stmt.named_children(&mut cursor) {
        if child.kind() != "assignment" {
            continue;
        }
        let Some(lhs) = child.child_by_field_name("left") else {
            continue;
        };
        if lhs.kind() != "identifier" {
            continue;
        }
        let name = text(lhs, src);
        // UPPERCASE-only (screaming snake): at least one letter, no lowercase.
        // The naming convention is itself the intent signal — a SCREAMING_SNAKE
        // name is a declared constant regardless of whether its RHS is a bare
        // literal, so it bypasses the literal-primitive noise gate that applies
        // to languages without a constant-naming convention (e.g. Go). Python
        // module-level assignments have no docstring path, so there is no doc
        // signal to consult; `state_var_is_noise` only filters the degenerate
        // bare-annotation (`X: int`) case here.
        if !is_screaming_snake(name) {
            continue;
        }
        if state_var_is_noise(child) {
            continue;
        }

        let qname = format!("{module_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: build_cells(&stmt, src, file_rel),
        });
        acc.nav
            .record(id, name, &qname, node_kind::STATE_VAR, Some(module_id));
        acc.edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
        });
    }
}

/// True if `s` is screaming-snake-case: contains at least one ASCII letter and
/// no lowercase letters (digits / underscores allowed). `MAX_RETRIES`, `PI`,
/// `_X2` all qualify; `config`, `MixedCase` do not.
fn is_screaming_snake(s: &str) -> bool {
    let mut saw_alpha = false;
    for c in s.chars() {
        if c.is_ascii_lowercase() {
            return false;
        }
        if c.is_ascii_uppercase() {
            saw_alpha = true;
        }
    }
    saw_alpha
}

/// Noise gate for a module-level UPPERCASE assignment. The SCREAMING_SNAKE
/// naming convention is the declared-constant signal, so a constant with a
/// literal-primitive RHS is still meaningful and is kept. The only thing
/// filtered here is the degenerate bare annotation (`X: int` with no value),
/// which declares nothing concrete. Python module-level assignments have no
/// docstring path, so there is no doc signal to consult.
fn state_var_is_noise(assignment: TsNode) -> bool {
    // No `right` field → bare annotation without a value; trivial.
    assignment.child_by_field_name("right").is_none()
}

// ============================================================================
// Imports
// ============================================================================

fn collect_import(n: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    // `import a, b.c as d` — children are dotted_name or aliased_import.
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        match child.kind() {
            "dotted_name" => {
                let path = text(child, src).to_string();
                acc.imports.push(ImportStmt {
                    from_module: from_module.to_string(),
                    target: ImportTarget::Module { path, alias: None },
                });
            }
            "aliased_import" => {
                let Some(name_n) = child.child_by_field_name("name") else {
                    continue;
                };
                let Some(alias_n) = child.child_by_field_name("alias") else {
                    continue;
                };
                acc.imports.push(ImportStmt {
                    from_module: from_module.to_string(),
                    target: ImportTarget::Module {
                        path: text(name_n, src).to_string(),
                        alias: Some(text(alias_n, src).to_string()),
                    },
                });
            }
            _ => {}
        }
    }
}

fn collect_import_from(n: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    // Fields: module_name (dotted_name | relative_import) + name children.
    let (module, level) = match n.child_by_field_name("module_name") {
        Some(m) if m.kind() == "dotted_name" => (text(m, src).to_string(), 0),
        Some(m) if m.kind() == "relative_import" => parse_relative_import(m, src),
        Some(_) | None => (String::new(), 0),
    };

    // Imported names are the `name` field (can be multi). Walk named children
    // after the module_name and treat dotted_name / aliased_import as items.
    let mut cursor = n.walk();
    let mut saw_module = false;
    for child in n.named_children(&mut cursor) {
        if !saw_module {
            // Skip the module_name / relative_import slot.
            if matches!(child.kind(), "dotted_name" | "relative_import")
                && n.child_by_field_name("module_name").map(|m| m.id()) == Some(child.id())
            {
                saw_module = true;
                continue;
            }
        }
        match child.kind() {
            "dotted_name" => {
                acc.imports.push(ImportStmt {
                    from_module: from_module.to_string(),
                    target: ImportTarget::Symbol {
                        module: module.clone(),
                        name: text(child, src).to_string(),
                        alias: None,
                        level,
                    },
                });
            }
            "aliased_import" => {
                let Some(name_n) = child.child_by_field_name("name") else {
                    continue;
                };
                let alias = child
                    .child_by_field_name("alias")
                    .map(|a| text(a, src).to_string());
                acc.imports.push(ImportStmt {
                    from_module: from_module.to_string(),
                    target: ImportTarget::Symbol {
                        module: module.clone(),
                        name: text(name_n, src).to_string(),
                        alias,
                        level,
                    },
                });
            }
            _ => {}
        }
    }
}

fn parse_relative_import(n: TsNode, src: &[u8]) -> (String, u32) {
    // `.` * level + optional dotted_name.
    let raw = text(n, src);
    let level = raw.chars().take_while(|c| *c == '.').count() as u32;
    let module = raw.trim_start_matches('.').to_string();
    (module, level)
}

// ============================================================================
// Call collection
// ============================================================================

fn collect_calls_in(
    n: TsNode,
    src: &[u8],
    from: NodeId,
    enclosing_class: Option<NodeId>,
    acc: &mut Acc,
) {
    let mut stack = vec![n];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        // Don't descend into nested function/class bodies — they have their
        // own from-node and are walked separately.
        if matches!(kind, "function_definition" | "class_definition") {
            continue;
        }
        if kind == "call"
            && let Some(q) = extract_call_qualifier(node, src)
        {
            acc.unresolved.push(UnresolvedCall {
                from,
                enclosing_class,
                qualifier: q,
            });
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn extract_call_qualifier(call: TsNode, src: &[u8]) -> Option<CallQualifier> {
    let func = call.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => Some(CallQualifier::Bare(text(func, src).to_string())),
        "attribute" => {
            let object = func.child_by_field_name("object")?;
            let attr = func.child_by_field_name("attribute")?;
            let name = text(attr, src).to_string();
            if object.kind() == "identifier" {
                let base = text(object, src).to_string();
                if base == "self" {
                    Some(CallQualifier::SelfMethod(name))
                } else {
                    Some(CallQualifier::Attribute { base, name })
                }
            } else if object.kind() == "call" && is_super_call(object, src) {
                // v0.4.13a — `super().method()` reaches here because the
                // receiver is a `call` node, not an identifier. Classify it
                // so resolve_intra_file can walk the enclosing class's base
                // list instead of dropping this into ComplexReceiver.
                Some(CallQualifier::SuperMethod(name))
            } else {
                // Chained / complex receivers — keep the raw text.
                Some(CallQualifier::ComplexReceiver {
                    receiver: text(object, src).to_string(),
                    name,
                })
            }
        }
        _ => None,
    }
}

/// True for the `super()` or `super(Class, self)` call-expression shape —
/// i.e. the receiver of a `super().m()` attribute chain.
fn is_super_call(call_node: TsNode, src: &[u8]) -> bool {
    call_node
        .child_by_field_name("function")
        .map(|f| f.kind() == "identifier" && text(f, src) == "super")
        .unwrap_or(false)
}

/// v0.4.13a — walk a function/method definition's parameters and return-type
/// annotations, emit a USES `UnresolvedRef` for each class-like identifier
/// referenced in those types. Enables PPR to reach class nodes named only in
/// type annotations (e.g. `def _bind(self, s: Schema)` surfaces `Schema`).
///
/// Scope: only bare identifiers and the trailing name of `pkg.Class`
/// attributes. Does not attempt to unpack generics like `List[Field]` into
/// `List` + `Field` separately — tree-sitter-python represents those as a
/// `subscript` node containing identifiers, and walking descendants covers
/// both. Keyword-argument defaults and string annotations ("Schema") are
/// skipped (no parse of string contents).
fn collect_type_refs(def: TsNode, src: &[u8], from: NodeId, module_id: NodeId, acc: &mut Acc) {
    // Parameter annotations: walk `parameters` for `typed_parameter` /
    // `typed_default_parameter`, read their `type` field.
    if let Some(params) = def.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for p in params.named_children(&mut cursor) {
            match p.kind() {
                "typed_parameter" | "typed_default_parameter" => {
                    if let Some(ty) = p.child_by_field_name("type") {
                        emit_type_idents(ty, src, from, module_id, acc);
                    }
                }
                _ => {}
            }
        }
    }
    // Return type: `def foo() -> Ret:`.
    if let Some(ret) = def.child_by_field_name("return_type") {
        emit_type_idents(ret, src, from, module_id, acc);
    }
}

/// Walk a type-annotation subtree, collect every identifier (including the
/// trailing `.name` of attribute access), dedupe within this call, emit USES
/// refs. Skips Python built-ins and common typing constructors so we don't
/// clog the unresolved list with `str`/`int`/`Optional`/`List`.
fn emit_type_idents(ty: TsNode, src: &[u8], from: NodeId, module_id: NodeId, acc: &mut Acc) {
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut stack = vec![ty];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "identifier" => {
                let s = text(n, src);
                if !is_type_noise(s) && seen.insert(s.to_string()) {
                    acc.refs.push(UnresolvedRef {
                        from,
                        from_module: module_id,
                        qualifier: CallQualifier::Bare(s.to_string()),
                        category: edge_category::USES,
                    });
                }
            }
            "attribute" => {
                // `pkg.Class` → only the trailing name is the type.
                if let Some(attr) = n.child_by_field_name("attribute") {
                    let s = text(attr, src);
                    if !is_type_noise(s) && seen.insert(s.to_string()) {
                        acc.refs.push(UnresolvedRef {
                            from,
                            from_module: module_id,
                            qualifier: CallQualifier::Bare(s.to_string()),
                            category: edge_category::USES,
                        });
                    }
                }
            }
            _ => {
                let mut c = n.walk();
                for child in n.named_children(&mut c) {
                    stack.push(child);
                }
            }
        }
    }
}

/// v0.4.13 — if `def … -> Ret:` has an explicit return-type annotation, emit
/// a RETURNS_TYPE `UnresolvedRef` per class-like identifier in the annotation.
/// Parallel to the USES emission in `collect_type_refs`, but tagged with the
/// composition-edge category so BFS can walk `method → return_class` without
/// collapsing it into generic semantic references.
///
/// Only fires on explicit annotations — `@property` walkers like
/// `Field.root` that lack a `-> Schema` annotation don't produce edges here.
/// Those rely on docstring fallback in the A+ cell renderer (step 3).
fn collect_return_type_ref(
    def: TsNode,
    src: &[u8],
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let Some(ret) = def.child_by_field_name("return_type") else {
        return;
    };
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut stack = vec![ret];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "identifier" => {
                let s = text(n, src);
                if !is_type_noise(s) && seen.insert(s.to_string()) {
                    acc.refs.push(UnresolvedRef {
                        from,
                        from_module: module_id,
                        qualifier: CallQualifier::Bare(s.to_string()),
                        category: edge_category::RETURNS_TYPE,
                    });
                }
            }
            "attribute" => {
                if let Some(attr) = n.child_by_field_name("attribute") {
                    let s = text(attr, src);
                    if !is_type_noise(s) && seen.insert(s.to_string()) {
                        acc.refs.push(UnresolvedRef {
                            from,
                            from_module: module_id,
                            qualifier: CallQualifier::Bare(s.to_string()),
                            category: edge_category::RETURNS_TYPE,
                        });
                    }
                }
            }
            _ => {
                let mut c = n.walk();
                for child in n.named_children(&mut c) {
                    stack.push(child);
                }
            }
        }
    }
}

/// v0.4.13b — walk a `type` annotation subtree (from an annotated assignment
/// like `x: T = …`) and emit USES refs from the ATTRIBUTE node for each
/// project-class identifier found. Same traversal shape as
/// `collect_return_type_ref` but tagged `USES` (attrs aren't callable, so
/// RETURNS_TYPE doesn't fit without a composition.rs behavior change).
fn collect_attr_type_ref(
    ty: TsNode,
    src: &[u8],
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut stack = vec![ty];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "identifier" => {
                let s = text(n, src);
                if !is_type_noise(s) && seen.insert(s.to_string()) {
                    acc.refs.push(UnresolvedRef {
                        from,
                        from_module: module_id,
                        qualifier: CallQualifier::Bare(s.to_string()),
                        category: edge_category::USES,
                    });
                }
            }
            "attribute" => {
                if let Some(attr) = n.child_by_field_name("attribute") {
                    let s = text(attr, src);
                    if !is_type_noise(s) && seen.insert(s.to_string()) {
                        acc.refs.push(UnresolvedRef {
                            from,
                            from_module: module_id,
                            qualifier: CallQualifier::Bare(s.to_string()),
                            category: edge_category::USES,
                        });
                    }
                }
            }
            _ => {
                let mut c = n.walk();
                for child in n.named_children(&mut c) {
                    stack.push(child);
                }
            }
        }
    }
}

/// v0.4.13b — RHS constructor inference. Walks method bodies looking for
/// `self.<attr> = Target(...)` or `self.<attr> = mod.Target(...)` and emits a
/// USES ref from the ATTRIBUTE node to the callee name. Enables PPR to
/// surface concrete types for attributes initialised via constructor calls.
fn collect_self_attr_rhs_types(
    body: TsNode,
    src: &[u8],
    class_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "function_definition" | "class_definition") {
            continue;
        }
        if node.kind() == "assignment"
            && let Some(lhs) = node.child_by_field_name("left")
            && lhs.kind() == "attribute"
            && let Some(obj) = lhs.child_by_field_name("object")
            && obj.kind() == "identifier"
            && text(obj, src) == "self"
            && let Some(attr) = lhs.child_by_field_name("attribute")
            && let Some(rhs) = node.child_by_field_name("right")
        {
            let attr_name = text(attr, src);
            if !(attr_name.starts_with("__") && attr_name.ends_with("__")) {
                let attr_qname = format!("{class_qname}::{attr_name}");
                let attr_id = NodeId::from_parts(
                    GRAPH_TYPE,
                    repo,
                    node_kind::ATTRIBUTE,
                    &attr_qname,
                );
                emit_rhs_constructor_refs(rhs, src, attr_id, module_id, acc);
            }
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            stack.push(child);
        }
    }
}

/// Extract the callee class name from an RHS expression and emit a USES ref.
/// Handles `Target(...)` (identifier callee) and `mod.Target(...)` (attribute
/// callee, takes the final segment). Does not recurse into call args — only
/// the immediate callee is relevant for attribute type inference.
fn emit_rhs_constructor_refs(
    rhs: TsNode,
    src: &[u8],
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    if rhs.kind() != "call" {
        return;
    }
    let Some(func) = rhs.child_by_field_name("function") else {
        return;
    };
    let name = match func.kind() {
        "identifier" => text(func, src),
        "attribute" => {
            let Some(attr) = func.child_by_field_name("attribute") else {
                return;
            };
            text(attr, src)
        }
        _ => return,
    };
    if is_type_noise(name) {
        return;
    }
    acc.refs.push(UnresolvedRef {
        from,
        from_module: module_id,
        qualifier: CallQualifier::Bare(name.to_string()),
        category: edge_category::USES,
    });
}

/// Tokens we don't want to churn the unresolved-refs list with. Python
/// builtins and common `typing` sugar that rarely point at project classes.
fn is_type_noise(s: &str) -> bool {
    matches!(
        s,
        "str" | "bytes" | "int" | "float" | "bool" | "None" | "object"
            | "list" | "dict" | "tuple" | "set" | "frozenset"
            | "Any" | "Optional" | "Union" | "List" | "Dict" | "Tuple" | "Set"
            | "Callable" | "Iterable" | "Iterator" | "Generator" | "AsyncIterable"
            | "AsyncIterator" | "AsyncGenerator" | "Awaitable" | "Coroutine"
            | "Sequence" | "Mapping" | "MutableMapping" | "MutableSequence"
            | "Type" | "ClassVar" | "Final" | "Literal" | "Self"
    )
}

// ============================================================================
// Intra-file resolution
// ============================================================================

fn resolve_intra_file(mut acc: Acc, _repo: RepoId) -> Result<FileParse, ParseError> {
    let mut out = FileParse {
        nodes: std::mem::take(&mut acc.nodes),
        edges: std::mem::take(&mut acc.edges),
        imports: std::mem::take(&mut acc.imports),
        calls: Vec::new(),
        refs: std::mem::take(&mut acc.refs),
        nav: std::mem::take(&mut acc.nav),
        properties: std::mem::take(&mut acc.properties),
    };
    for uc in acc.unresolved {
        let resolved: Option<NodeId> = match &uc.qualifier {
            CallQualifier::Bare(name) => acc.module_functions.get(name).copied(),
            CallQualifier::SelfMethod(name) => uc
                .enclosing_class
                .and_then(|cid| acc.class_methods.get(&(cid, name.clone())).copied()),
            // v0.4.13a — `super().m()`: walk the enclosing class's recorded
            // base names, look each up in the local module's classes, and
            // return the first match with a method of the given name. If the
            // base class is imported from another file, this misses and we
            // fall through to the cross-file CallSite path.
            CallQualifier::SuperMethod(name) => uc.enclosing_class.and_then(|cid| {
                acc.class_bases.get(&cid).and_then(|bases| {
                    bases.iter().find_map(|base_name| {
                        acc.module_classes
                            .get(base_name)
                            .and_then(|base_id| {
                                acc.class_methods.get(&(*base_id, name.clone())).copied()
                            })
                    })
                })
            }),
            _ => None,
        };
        match resolved {
            Some(to) => out.edges.push(Edge {
                from: uc.from,
                to,
                category: edge_category::CALLS,
                confidence: Confidence::Strong,
            }),
            None => out.calls.push(CallSite {
                from: uc.from,
                qualifier: uc.qualifier,
            }),
        }
    }
    Ok(out)
}

// ============================================================================
// Cell building
// ============================================================================

fn build_cells(n: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    let code = Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Text(slice(n, src).to_string()),
    };
    let pos = Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(position_json(n, file_rel)),
    };
    let mut cells = vec![code, pos];
    if let Some(doc) = extract_docstring(n, src) {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    cells
}

fn position_json(n: &TsNode, file_rel: &str) -> String {
    let start = n.start_position();
    let end = n.end_position();
    format!(
        "{{\"file\":\"{}\",\"start_line\":{},\"end_line\":{}}}",
        file_rel.replace('\\', "\\\\").replace('"', "\\\""),
        start.row,
        end.row
    )
}

/// Returns the module/class/function docstring if present.
fn extract_docstring(n: &TsNode, src: &[u8]) -> Option<String> {
    let body = match n.kind() {
        "module" => *n,
        _ => n.child_by_field_name("body")?,
    };
    let mut cursor = body.walk();
    let first = body.named_children(&mut cursor).next()?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let mut inner_cursor = first.walk();
    let string_node = first.named_children(&mut inner_cursor).next()?;
    if string_node.kind() != "string" {
        return None;
    }
    let raw = text(string_node, src);
    Some(strip_string_quotes(raw))
}

fn strip_string_quotes(s: &str) -> String {
    const PREFIXES: [char; 8] = ['r', 'R', 'b', 'B', 'u', 'U', 'f', 'F'];
    let t = s.trim_start_matches(PREFIXES);
    let stripped = if t.len() >= 6
        && ((t.starts_with("\"\"\"") && t.ends_with("\"\"\""))
            || (t.starts_with("'''") && t.ends_with("'''")))
    {
        &t[3..t.len() - 3]
    } else if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"'))
            || (t.starts_with('\'') && t.ends_with('\'')))
    {
        &t[1..t.len() - 1]
    } else {
        t
    };
    stripped.to_string()
}

// ============================================================================
// Route extraction — Flask / FastAPI / Django (v0.4.11a R-python)
// ============================================================================
//
// Flask / FastAPI use decorators on function/method handlers:
//   @app.route('/path', methods=['GET','POST'])   (Flask)
//   @app.get('/path')                              (Flask 2+, FastAPI)
//   @router.post('/path')                          (FastAPI)
//   @blueprint.route('/path')                      (Flask)
//
// Django uses `path('/url', view)` / `re_path(...)` inside a `urlpatterns`
// list in `urls.py`. Method defaults to ANY because Django method dispatch
// happens inside the view function, not the URL declaration.

fn check_route_decorator(
    deco: TsNode,
    src: &[u8],
    handler_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // Decorator text starts with '@'. Its `.call` form gives us the function
    // expression + argument list.
    let raw = text(deco, src);
    let body = raw.trim_start_matches('@').trim();
    let Some(paren) = body.find('(') else {
        return;
    };
    let head = &body[..paren];
    // Verb is the trailing attribute: `app.get` → "get"; `app.route` → "route".
    let verb = head.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let Some(methods) = route_methods_for(&verb, &body[paren..]) else {
        return;
    };
    let args = &body[paren + 1..];
    let Some(path) = first_string_literal(args) else {
        return;
    };
    for m in methods {
        emit_route(m, &path, handler_id, repo, acc);
    }
}

/// Returns the HTTP methods a Python decorator maps to, or None if not a
/// route decorator. The inputs are the trailing attribute (`get`, `route`,
/// `websocket`…) and the full arg-list slice starting at `(`.
fn route_methods_for(verb: &str, args: &str) -> Option<Vec<&'static str>> {
    match verb {
        "get" => Some(vec!["GET"]),
        "post" => Some(vec!["POST"]),
        "put" => Some(vec!["PUT"]),
        "delete" => Some(vec!["DELETE"]),
        "patch" => Some(vec!["PATCH"]),
        "head" => Some(vec!["HEAD"]),
        "options" => Some(vec!["OPTIONS"]),
        "route" => Some(flask_route_methods(args)),
        _ => None,
    }
}

/// Extract the `methods=[...]` kwarg from a Flask-style `@app.route(...)`.
/// Defaults to `["GET"]` when absent.
fn flask_route_methods(args: &str) -> Vec<&'static str> {
    let Some(idx) = args.find("methods") else {
        return vec!["GET"];
    };
    let rest = &args[idx + "methods".len()..];
    let Some(lb) = rest.find('[') else {
        return vec!["GET"];
    };
    let Some(rb) = rest[lb..].find(']') else {
        return vec!["GET"];
    };
    let list = &rest[lb + 1..lb + rb];
    let mut out = Vec::new();
    for part in list.split(',') {
        let t = part.trim().trim_matches('\'').trim_matches('"').trim();
        let verb = match t.to_ascii_uppercase().as_str() {
            "GET" => "GET",
            "POST" => "POST",
            "PUT" => "PUT",
            "DELETE" => "DELETE",
            "PATCH" => "PATCH",
            "HEAD" => "HEAD",
            "OPTIONS" => "OPTIONS",
            _ => continue,
        };
        out.push(verb);
    }
    if out.is_empty() {
        out.push("GET");
    }
    out
}

/// Django `urls.py` scan — finds `path('/url', view)` / `re_path(r'/url', …)`
/// / `url(r'/url', …)` calls inside the node and emits one Route per path.
/// Method is ANY because Django views dispatch internally.
fn scan_django_routes(root: TsNode, src: &[u8], repo: RepoId, acc: &mut Acc) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "call"
            && let Some(func) = n.child_by_field_name("function")
        {
            let name = text(func, src);
            let is_django =
                matches!(name, "path" | "re_path" | "url") || name.ends_with(".path");
            if is_django
                && let Some(args) = n.child_by_field_name("arguments")
            {
                let arg_text = text(args, src);
                if let Some(path) = first_string_literal(&arg_text[1..]) {
                    emit_route_no_handler("ANY", &path, repo, acc);
                }
            }
        }
        let mut cursor = n.walk();
        for c in n.named_children(&mut cursor) {
            stack.push(c);
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

fn emit_route_no_handler(method: &str, path: &str, repo: RepoId, acc: &mut Acc) {
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

fn first_string_literal(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\'' || b == b'"' {
            let quote = b;
            // Skip leading r/b/u/f string prefixes captured earlier — s has
            // already been sliced past the `(`, so we can match the opener.
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != quote {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                j += 1;
            }
            if j >= bytes.len() {
                return None;
            }
            let lit = std::str::from_utf8(&bytes[i + 1..j]).ok()?.to_string();
            if lit.is_empty() || lit.len() > 256 {
                return None;
            }
            return Some(lit);
        }
        // Skip common prefix chars before a quote (r''/b""/rb'' — up to 2
        // char prefix). If `b` is alphanumeric or '_' we just keep walking.
        i += 1;
    }
    None
}

// ============================================================================
// Tree-sitter helpers
// ============================================================================

fn slice<'a>(n: &TsNode, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[n.byte_range()]).unwrap_or("")
}

fn text<'a>(n: TsNode, src: &'a [u8]) -> &'a str {
    slice(&n, src)
}

fn child_text<'a>(n: TsNode, field: &str, src: &'a [u8]) -> Option<&'a str> {
    n.child_by_field_name(field).map(|c| text(c, src))
}

// ============================================================================
// Call-arg extraction for synth_callsite_argflow
// ============================================================================

/// Surface info for one Python `call` expression — enough to reason about
/// usage-typed polymorphism downstream without widening `CallSite` (which
/// every language parser would need to track in lockstep).
#[derive(Debug, Clone)]
pub struct CallArgInfo {
    pub callee_simple_name: String,
    pub receiver_text: String,
    pub args: Vec<String>,
    pub start_line: usize,
}

/// Re-parse `source` and return one `CallArgInfo` per `call` node.
pub fn extract_calls_with_args(source: &str) -> Vec<CallArgInfo> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
    if parser.set_language(&lang).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut out: Vec<CallArgInfo> = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == "call"
            && let Some(info) = collect_call_arg_info(n, src)
        {
            out.push(info);
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    out
}

fn collect_call_arg_info(call: TsNode, src: &[u8]) -> Option<CallArgInfo> {
    let func = call.child_by_field_name("function")?;
    let (receiver_text, callee_simple_name) = match func.kind() {
        "identifier" => (String::new(), text(func, src).to_string()),
        "attribute" => {
            let object = func.child_by_field_name("object")?;
            let attr = func.child_by_field_name("attribute")?;
            (text(object, src).to_string(), text(attr, src).to_string())
        }
        _ => return None,
    };
    let mut args: Vec<String> = Vec::new();
    if let Some(arglist) = call.child_by_field_name("arguments") {
        let mut cursor = arglist.walk();
        for child in arglist.named_children(&mut cursor) {
            if child.kind() == "keyword_argument" {
                continue;
            }
            args.push(text(child, src).to_string());
        }
    }
    Some(CallArgInfo {
        callee_simple_name,
        receiver_text,
        args,
        start_line: call.start_position().row + 1,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::EdgeCategoryId;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://py_smoke")
    }

    fn has_edge(parse: &FileParse, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> bool {
        parse
            .edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == cat)
    }

    #[test]
    fn parses_helpers_module_with_two_functions() {
        let src = "def hash_password(password):\n    return _inner(password)\n\n\ndef _inner(p):\n    return p.encode()\n";
        let parse = parse_file(src, "myapp/helpers.py", "myapp::helpers", repo()).unwrap();

        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "myapp::helpers");
        let hash_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "myapp::helpers::hash_password",
        );
        let inner_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "myapp::helpers::_inner",
        );

        assert!(parse.nodes.iter().any(|n| n.id == module_id));
        assert!(parse.nodes.iter().any(|n| n.id == hash_id));
        assert!(parse.nodes.iter().any(|n| n.id == inner_id));

        assert!(has_edge(&parse, module_id, hash_id, edge_category::DEFINES));
        assert!(has_edge(&parse, module_id, inner_id, edge_category::DEFINES));

        // Intra-file bare call: hash_password → _inner
        assert!(
            has_edge(&parse, hash_id, inner_id, edge_category::CALLS),
            "expected intra-file bare call to resolve, got calls edges: {:?}",
            parse
                .edges
                .iter()
                .filter(|e| e.category == edge_category::CALLS)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn uppercase_module_constant_emits_state_var_lowercase_does_not() {
        let src = "MAX_RETRIES = 3\nconfig = {}\n";
        let parse = parse_file(src, "myapp/settings.py", "myapp::settings", repo()).unwrap();

        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "myapp::settings");

        // UPPERCASE constant → STATE_VAR with DEFINES edge from module.
        let max_retries = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::STATE_VAR,
            "myapp::settings::MAX_RETRIES",
        );
        assert!(parse.nodes.iter().any(|n| n.id == max_retries));
        assert!(has_edge(&parse, module_id, max_retries, edge_category::DEFINES));

        // lowercase name → skipped entirely.
        let config = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::STATE_VAR,
            "myapp::settings::config",
        );
        assert!(!parse.nodes.iter().any(|n| n.id == config));
    }

    #[test]
    fn parses_users_class_with_self_call() {
        let src = "from .helpers import hash_password\n\n\nclass User:\n    def login(self, password):\n        return hash_password(password)\n\n    def save(self):\n        self.login(\"x\")\n";
        let parse = parse_file(src, "myapp/users.py", "myapp::users", repo()).unwrap();

        let class_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::CLASS,
            "myapp::users::User",
        );
        let login_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "myapp::users::User::login",
        );
        let save_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "myapp::users::User::save",
        );

        assert!(parse.nodes.iter().any(|n| n.id == class_id));
        assert!(parse.nodes.iter().any(|n| n.id == login_id));
        assert!(parse.nodes.iter().any(|n| n.id == save_id));

        assert!(has_edge(&parse, class_id, login_id, edge_category::DEFINES));
        assert!(has_edge(&parse, class_id, save_id, edge_category::DEFINES));

        // self.login() inside save — intra-class self call resolves.
        assert!(
            has_edge(&parse, save_id, login_id, edge_category::CALLS),
            "expected self.login call to resolve to User::login"
        );

        // hash_password(...) inside login — cross-file, stays unresolved.
        assert!(
            parse
                .calls
                .iter()
                .any(|c| c.from == login_id
                    && matches!(&c.qualifier, CallQualifier::Bare(n) if n == "hash_password")),
            "expected hash_password call to be unresolved, got: {:?}",
            parse.calls
        );

        // Relative import record.
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Symbol { module, name, level, .. }
                if module == "helpers" && name == "hash_password" && *level == 1
        )));
    }

    #[test]
    fn parses_auth_with_absolute_and_submodule_imports() {
        let src = "from myapp.users import User\nfrom myapp import helpers\n\n\ndef do_login():\n    u = User()\n    u.login(\"x\")\n    helpers.hash_password(\"x\")\n";
        let parse = parse_file(src, "myapp/auth.py", "myapp::auth", repo()).unwrap();

        let do_login_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "myapp::auth::do_login",
        );
        assert!(parse.nodes.iter().any(|n| n.id == do_login_id));

        // Two import records.
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Symbol { module, name, level, .. }
                if module == "myapp.users" && name == "User" && *level == 0
        )));
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Symbol { module, name, level, .. }
                if module == "myapp" && name == "helpers" && *level == 0
        )));

        // Three call sites, all cross-file at the v0.4.2 layer.
        let mut quals: Vec<&CallQualifier> = parse
            .calls
            .iter()
            .filter(|c| c.from == do_login_id)
            .map(|c| &c.qualifier)
            .collect();
        quals.sort_by_key(|q| format!("{q:?}"));
        assert_eq!(quals.len(), 3, "unexpected call sites: {quals:?}");
        // User() — bare call (constructor)
        assert!(quals.iter().any(|q| matches!(q, CallQualifier::Bare(n) if n == "User")));
        // u.login("x") — Attribute. v0.4.3 disambiguates "u is a local var → drop"
        // from "helpers is an imported name → resolve" using the import table.
        assert!(quals.iter().any(
            |q| matches!(q, CallQualifier::Attribute { base, name } if base == "u" && name == "login")
        ));
        // helpers.hash_password("x") — Attribute
        assert!(quals.iter().any(
            |q| matches!(q, CallQualifier::Attribute { base, name } if base == "helpers" && name == "hash_password")
        ));
    }

    #[test]
    fn module_node_has_code_and_position_cells() {
        let src = "def f(): pass\n";
        let parse = parse_file(src, "foo.py", "foo", repo()).unwrap();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "foo");
        let m = parse.nodes.iter().find(|n| n.id == module_id).unwrap();
        assert!(m.cells.iter().any(|c| c.kind == cell_type::CODE));
        assert!(m.cells.iter().any(|c| c.kind == cell_type::POSITION));
    }

    #[test]
    fn docstring_becomes_doc_cell() {
        let src = "\"\"\"hello world\"\"\"\n\ndef f():\n    \"\"\"inner doc\"\"\"\n    return 1\n";
        let parse = parse_file(src, "foo.py", "foo", repo()).unwrap();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "foo");
        let func_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "foo::f");
        let m = parse.nodes.iter().find(|n| n.id == module_id).unwrap();
        let f = parse.nodes.iter().find(|n| n.id == func_id).unwrap();
        assert!(
            m.cells.iter().any(|c| c.kind == cell_type::DOC
                && matches!(&c.payload, CellPayload::Text(t) if t == "hello world")),
            "module doc cell missing"
        );
        assert!(
            f.cells.iter().any(|c| c.kind == cell_type::DOC
                && matches!(&c.payload, CellPayload::Text(t) if t == "inner doc")),
            "function doc cell missing"
        );
    }

    #[test]
    fn syntax_error_produces_partial_graph() {
        // tree-sitter recovers — we still get the valid top-level def.
        let src = "def ok(): pass\n\nthis is !!! not valid python\n";
        let parse = parse_file(src, "broken.py", "broken", repo()).unwrap();
        let ok_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "broken::ok");
        assert!(parse.nodes.iter().any(|n| n.id == ok_id));
    }

    // ----- v0.4.11a R-python: route extraction -----

    fn route_id(method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            &format!("{method} {path}"),
        )
    }

    #[test]
    fn flask_app_get_decorator_emits_route() {
        let src = "from flask import Flask\napp = Flask(__name__)\n\n@app.get('/users')\ndef list_users():\n    return []\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        let handler = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "app::list_users",
        );
        let rid = route_id("GET", "/users");
        assert!(parse.nodes.iter().any(|n| n.id == rid), "missing Route");
        assert!(parse.nodes.iter().any(|n| n.id == handler));
        assert!(
            has_edge(&parse, rid, handler, edge_category::HANDLED_BY),
            "missing HANDLED_BY edge"
        );
    }

    #[test]
    fn flask_route_with_methods_kwarg_emits_multiple() {
        let src = "from flask import Flask\napp = Flask(__name__)\n\n@app.route('/users', methods=['GET','POST'])\ndef users():\n    return []\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        assert!(parse.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(parse.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
    }

    #[test]
    fn flask_route_without_methods_defaults_to_get() {
        let src = "@app.route('/ping')\ndef ping():\n    return 'pong'\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        assert!(parse.nodes.iter().any(|n| n.id == route_id("GET", "/ping")));
    }

    #[test]
    fn fastapi_router_post_decorator_emits_route() {
        let src = "from fastapi import APIRouter\nrouter = APIRouter()\n\n@router.post('/items')\nasync def create_item(item: dict):\n    return item\n";
        let parse = parse_file(src, "routes.py", "routes", repo()).unwrap();
        let handler = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "routes::create_item",
        );
        let rid = route_id("POST", "/items");
        assert!(parse.nodes.iter().any(|n| n.id == rid));
        assert!(has_edge(&parse, rid, handler, edge_category::HANDLED_BY));
    }

    #[test]
    fn django_path_call_emits_route_without_handler() {
        let src = "from django.urls import path, re_path\nfrom . import views\n\nurlpatterns = [\n    path('users/', views.user_list),\n    re_path(r'^admin/', views.admin),\n]\n";
        let parse = parse_file(src, "urls.py", "urls", repo()).unwrap();
        assert!(parse.nodes.iter().any(|n| n.id == route_id("ANY", "users/")));
        assert!(parse.nodes.iter().any(|n| n.id == route_id("ANY", "^admin/")));
    }

    #[test]
    fn class_method_decorator_emits_route() {
        let src = "class Api:\n    @staticmethod\n    @app.get('/ok')\n    def ok():\n        return 'ok'\n";
        let parse = parse_file(src, "api.py", "api", repo()).unwrap();
        let handler = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "api::Api::ok",
        );
        let rid = route_id("GET", "/ok");
        assert!(parse.nodes.iter().any(|n| n.id == handler), "method missing");
        assert!(parse.nodes.iter().any(|n| n.id == rid), "route missing");
        assert!(has_edge(&parse, rid, handler, edge_category::HANDLED_BY));
    }

    #[test]
    fn non_route_decorator_is_ignored() {
        let src = "@functools.lru_cache(maxsize=128)\ndef compute(x):\n    return x\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let has_any_route = parse
            .nodes
            .iter()
            .any(|n| matches!(parse.nav.kind_by_id.get(&n.id).copied(), Some(k) if k == node_kind::ROUTE));
        assert!(!has_any_route, "non-route decorator shouldn't emit a Route");
    }

    // v0.4.13a — super() calls route through the parent class, intra-file.
    #[test]
    fn super_call_resolves_to_parent_method_intra_file() {
        let src = "class Base:\n    def hook(self):\n        return 1\n\n\nclass Child(Base):\n    def hook(self):\n        return super().hook() + 1\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let base_hook = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "m::Base::hook");
        let child_hook =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "m::Child::hook");
        assert!(
            has_edge(&parse, child_hook, base_hook, edge_category::CALLS),
            "expected super().hook() to resolve to Base::hook, got calls: {:?}",
            parse
                .edges
                .iter()
                .filter(|e| e.category == edge_category::CALLS)
                .collect::<Vec<_>>()
        );
    }

    // v0.4.13a — parameter type annotation emits a USES ref (Bare qualifier)
    // for later cross-file class resolution.
    #[test]
    fn param_type_annotation_emits_uses_ref() {
        let src = "class Schema:\n    pass\n\n\nclass Field:\n    def bind(self, schema: Schema) -> None:\n        pass\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let bind_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "m::Field::bind");
        let has_schema_ref = parse.refs.iter().any(|r| {
            r.from == bind_id
                && r.category == edge_category::USES
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Schema")
        });
        assert!(
            has_schema_ref,
            "expected Schema USES ref from Field::bind, got refs: {:?}",
            parse.refs
        );
        // Return type `None` is in the noise filter — should NOT emit a ref.
        let has_none_ref = parse
            .refs
            .iter()
            .any(|r| matches!(&r.qualifier, CallQualifier::Bare(n) if n == "None"));
        assert!(!has_none_ref, "`None` return annotation shouldn't emit a ref");
    }

    // v0.4.13 — INHERITS_FROM refs for every base class in `class Foo(Bar, Baz)`.
    #[test]
    fn class_base_emits_inherits_from_ref() {
        let src = "class Schema: pass\n\nclass TimeSchema(Schema):\n    pass\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let child_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "m::TimeSchema");
        let has_ref = parse.refs.iter().any(|r| {
            r.from == child_id
                && r.category == edge_category::INHERITS_FROM
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Schema")
        });
        assert!(
            has_ref,
            "expected INHERITS_FROM ref TimeSchema→Schema, got refs: {:?}",
            parse.refs
        );
    }

    // v0.4.13 — HAS_ATTRIBUTE edges from class-level, __init__ self-assignments,
    // AND plain self-reads (catches metaclass-attached attrs like Schema.opts).
    #[test]
    fn class_attribute_emits_has_attribute_edge() {
        // Four shapes: class-level, class-level annotated, instance self.assign,
        // plus a plain read `self.opts.X` (no assignment ever on `self.opts`).
        let src = "class Schema:\n    name = 'x'\n    meta: dict = {}\n    def __init__(self):\n        self.exclude = None\n        self.exclude = 'dedupe'\n    def render(self):\n        return self.opts.render_module\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "m::Schema");

        for attr in ["name", "meta", "exclude", "opts"] {
            let attr_id = NodeId::from_parts(
                GRAPH_TYPE,
                repo(),
                node_kind::ATTRIBUTE,
                &format!("m::Schema::{attr}"),
            );
            assert!(
                parse.nodes.iter().any(|n| n.id == attr_id),
                "missing ATTRIBUTE node for {attr}"
            );
            assert!(
                has_edge(&parse, class_id, attr_id, edge_category::HAS_ATTRIBUTE),
                "missing HAS_ATTRIBUTE edge for {attr}"
            );
        }

        // `self.exclude = 'dedupe'` is the second self-assign; dedupe must fire.
        let exclude_count = parse
            .nodes
            .iter()
            .filter(|n| parse.nav.qname_by_id.get(&n.id).map(|s| s.as_str()) == Some("m::Schema::exclude"))
            .count();
        assert_eq!(exclude_count, 1, "expected exactly one ATTRIBUTE node for exclude (dedupe)");

        // Dunder names are skipped.
        let dunder_count = parse
            .nodes
            .iter()
            .filter(|n| {
                parse.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::ATTRIBUTE)
                    && parse
                        .nav
                        .qname_by_id
                        .get(&n.id)
                        .map(|s| s.contains("__"))
                        .unwrap_or(false)
            })
            .count();
        assert_eq!(dunder_count, 0, "dunder attributes should be skipped");
    }

    // v0.4.13 — RETURNS_TYPE ref from explicit return-type annotation.
    #[test]
    fn return_type_annotation_emits_returns_type_ref() {
        let src = "class Schema: pass\n\nclass Field:\n    def ensure(self) -> Schema:\n        return self\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let ensure_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "m::Field::ensure");
        let has_ref = parse.refs.iter().any(|r| {
            r.from == ensure_id
                && r.category == edge_category::RETURNS_TYPE
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Schema")
        });
        assert!(
            has_ref,
            "expected RETURNS_TYPE ref Field::ensure→Schema, got refs: {:?}",
            parse.refs
        );
    }

    // v0.4.13b — USES ref from class-level typed attribute `x: T = ...`.
    #[test]
    fn class_attr_type_annotation_emits_uses_ref() {
        let src = "class Schema: pass\n\nclass Field:\n    parent: Schema = None\n    name: str = ''\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let parent_attr = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "m::Field::parent",
        );
        let has_schema_ref = parse.refs.iter().any(|r| {
            r.from == parent_attr
                && r.category == edge_category::USES
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Schema")
        });
        assert!(
            has_schema_ref,
            "expected USES ref Field::parent→Schema, got refs: {:?}",
            parse.refs
        );
        // `name: str` — `str` is type-noise, should NOT emit a ref.
        let name_attr = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "m::Field::name",
        );
        let has_str_ref = parse.refs.iter().any(|r| {
            r.from == name_attr
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "str")
        });
        assert!(
            !has_str_ref,
            "str should be filtered as type-noise, got refs: {:?}",
            parse.refs
        );
    }

    #[test]
    fn property_decorator_marks_method_as_property() {
        let src = r#"
class Field:
    @property
    def root(self):
        return self._root

    def from_dict(self):
        return self._root

    @classmethod
    def not_a_property(cls):
        return None
"#;
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let root = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "m::Field::root");
        let from_dict = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::METHOD, "m::Field::from_dict",
        );
        let not_prop = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::METHOD, "m::Field::not_a_property",
        );
        assert!(parse.properties.contains(&root), "expected @property root");
        assert!(!parse.properties.contains(&from_dict), "from_dict is not a property");
        assert!(!parse.properties.contains(&not_prop), "classmethod is not a property");
    }

    #[test]
    fn self_attr_rhs_constructor_emits_uses_ref() {
        let src = r#"
class Target:
    pass

class mod:
    class QualTarget:
        pass

class Field:
    module_attr = Target()

    def __init__(self):
        self._t = Target()
        self._q = mod.QualTarget()
        self._s = "string"
        self._n = dict()
"#;
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();

        // self._t = Target() → USES Target
        let t_attr = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "m::Field::_t",
        );
        assert!(
            parse.refs.iter().any(|r| {
                r.from == t_attr
                    && r.category == edge_category::USES
                    && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Target")
            }),
            "expected USES ref Field::_t→Target, got refs: {:?}",
            parse.refs
        );

        // self._q = mod.QualTarget() → USES QualTarget (final segment)
        let q_attr = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "m::Field::_q",
        );
        assert!(
            parse.refs.iter().any(|r| {
                r.from == q_attr
                    && r.category == edge_category::USES
                    && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "QualTarget")
            }),
            "expected USES ref Field::_q→QualTarget, got refs: {:?}",
            parse.refs
        );

        // self._n = dict() → dict is type-noise, NO ref
        let n_attr = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "m::Field::_n",
        );
        assert!(
            !parse.refs.iter().any(|r| {
                r.from == n_attr
                    && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "dict")
            }),
            "dict should be filtered as type-noise, got refs: {:?}",
            parse.refs
        );

        // Class-level: module_attr = Target() → USES Target
        let ma_attr = NodeId::from_parts(
            GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "m::Field::module_attr",
        );
        assert!(
            parse.refs.iter().any(|r| {
                r.from == ma_attr
                    && r.category == edge_category::USES
                    && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Target")
            }),
            "expected USES ref Field::module_attr→Target, got refs: {:?}",
            parse.refs
        );
    }
}
