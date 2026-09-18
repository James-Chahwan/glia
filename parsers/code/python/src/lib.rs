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
use repo_graph_code_domain::di_stats::{self, DiShape};
use repo_graph_code_domain::endpoint::{
    self, ClientEndpoint, HitExtras, push_client_endpoint_with,
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

    // substrate-gap py-accesses-data — pre-pass: harvest `__tablename__` from
    // every model class so `session.query(User)` sites (which may appear before
    // *or* after the class in file order) can resolve `User` → its table.
    scan_model_tables(root, src, repo, &mut acc);

    // substrate-gap py-router-prefix — pre-pass: harvest APIRouter/Blueprint
    // prefixes so a decorator whose receiver carries one composes the real
    // path. Pre-pass for the same reason as above: routers are conventionally
    // assigned above their handlers, but this makes file order irrelevant.
    scan_router_prefixes(root, src, &mut acc);

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
                collect_calls_in(child, src, module_id, None, repo, file_rel_path, &mut acc);
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

    if acc.routes_composed > 0 {
        eprintln!(
            "[py-routes] composed {} routes under {} router prefixes in {}",
            acc.routes_composed,
            acc.router_prefixes.len(),
            file_rel_path
        );
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
    /// Pattern A — dedup for client-HTTP ENDPOINT nodes: one node per
    /// (method, path) even if the same endpoint is called from several sites.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// substrate-gap py-accesses-data — SQLAlchemy model class name → the
    /// `data_entity:sql:<table>` node id, harvested from `__tablename__ = "..."`.
    /// Lets a `session.query(User)` call inside a function anchor an
    /// `ACCESSES_DATA` edge to that data-entity node (function-anchored, not
    /// module-anchored — the data_entities extractor already emits the coarse
    /// module→entity edge, but not the accessor→entity one the graph needs).
    model_tables: HashMap<String, NodeId>,
    /// Dedup for function-anchored `ACCESSES_DATA` edges: one edge per
    /// (accessor fn, data-entity) even when a fn issues the query repeatedly.
    accesses_data_seen: std::collections::HashSet<(NodeId, NodeId)>,
    /// substrate-gap py-router-prefix — module-level router/blueprint receiver
    /// name → its path prefix (`router` → `/api/v1/users`), harvested by
    /// `scan_router_prefixes`. A receiver reassigned mid-file takes the last
    /// value, matching how `model_tables` behaves.
    router_prefixes: HashMap<String, String>,
    /// Count of routes whose path was composed from a receiver prefix. Drives
    /// the `[py-routes]` fired_on marker.
    routes_composed: usize,
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

    // A7.4 — FastAPI `Depends(...)` in parameter defaults, Annotated markers
    // and route-decorator `dependencies=[...]` (class-based views, dependency
    // classes' `__init__`).
    collect_depends_refs(n, decorators, src, method_id, module_id, acc);

    if let Some(body) = n.child_by_field_name("body") {
        collect_calls_in(body, src, method_id, Some(class_id), repo, file_rel, acc);
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

    // A7.4 — FastAPI `Depends(...)` in parameter defaults, Annotated markers
    // and route-decorator `dependencies=[...]`.
    collect_depends_refs(n, decorators, src, func_id, module_id, acc);

    if let Some(body) = n.child_by_field_name("body") {
        collect_calls_in(body, src, func_id, None, repo, file_rel, acc);
        // substrate-gap py-tests — a pytest `def test_x` that calls a bare
        // function emits a fn-level TESTS ref (test_add → add). The engine's
        // module→module TESTS post-pass stays; this adds the finer edge the
        // graph contract wants. Resolves via the same import-binding path as a
        // normal cross-file call, so only calls that bind to a real project fn
        // become edges (builtins/asserts fall through harmlessly).
        if is_pytest_test(name) {
            collect_test_targets(body, src, func_id, module_id, acc);
        }
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
    repo: RepoId,
    file_rel: &str,
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
        if kind == "call" {
            // Pattern A: client HTTP call (`requests.get('/x')`) → shared
            // ENDPOINT node so the HttpStackResolver can pair it with a server
            // ROUTE. This is *additional* to the normal call-qualifier record
            // below (which stays as a cross-file CallSite, harmless).
            try_detect_py_endpoint(node, src, from, repo, file_rel, acc);
            // substrate-gap py-accesses-data — `session.query(User)` inside a
            // function/method → ACCESSES_DATA edge anchored on `from` (the
            // enclosing accessor), not the module.
            try_detect_data_access(node, src, from, acc);
            if let Some(q) = extract_call_qualifier(node, src) {
                acc.unresolved.push(UnresolvedCall {
                    from,
                    enclosing_class,
                    qualifier: q,
                });
            }
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

// ============================================================================
// Data access (substrate-gap py-accesses-data)
// ============================================================================
//
// A SQLAlchemy model declares its table with `__tablename__ = "users"`; a query
// site names the model class (`session.query(User)`). The `data_entities`
// extractor already mints the `data_entity:sql:users` node + a coarse
// module→entity ACCESSES_DATA edge, but the graph contract wants the edge
// anchored on the *accessor function* (`find_users`), so PPR/impact can reach
// the data from the code path that touches it. We resolve `User` → its table
// (built in `scan_model_tables`) and emit that function-anchored edge here.

/// Pre-pass: walk top-level class definitions (bare or decorated), read each
/// one's `__tablename__ = "<table>"`, and record `class_name → data_entity node
/// id`. The node id matches the one the `data_entities` extractor mints, so the
/// edge we emit points at the shared entity node.
fn scan_model_tables(root: TsNode, src: &[u8], repo: RepoId, acc: &mut Acc) {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        let class_node = match child.kind() {
            "class_definition" => Some(child),
            "decorated_definition" => {
                split_decorated(child).1.filter(|i| i.kind() == "class_definition")
            }
            _ => None,
        };
        let Some(class_node) = class_node else { continue };
        let Some(class_name) = child_text(class_node, "name", src) else {
            continue;
        };
        let Some(table) = find_tablename(class_node, src) else {
            continue;
        };
        if table.is_empty() {
            continue;
        }
        let qname = format!("data_entity:sql:{table}");
        let entity_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
        acc.model_tables.insert(class_name.to_string(), entity_id);
    }
}

/// Read `__tablename__ = "<table>"` from a class body, if present. tree-sitter
/// shape: `expression_statement > assignment` with `left` = the `__tablename__`
/// identifier and `right` = a string literal.
fn find_tablename(class_node: TsNode, src: &[u8]) -> Option<String> {
    let body = class_node.child_by_field_name("body")?;
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if member.kind() != "expression_statement" {
            continue;
        }
        let mut inner = member.walk();
        for stmt in member.named_children(&mut inner) {
            if stmt.kind() == "assignment"
                && let Some(lhs) = stmt.child_by_field_name("left")
                && lhs.kind() == "identifier"
                && text(lhs, src) == "__tablename__"
                && let Some(rhs) = stmt.child_by_field_name("right")
                && rhs.kind() == "string"
            {
                return Some(strip_string_quotes(text(rhs, src)));
            }
        }
    }
    None
}

/// Detect a `<recv>.query(Model)` call and, when `Model` resolves to a known
/// SQLAlchemy model, emit an ACCESSES_DATA edge from `from` (the enclosing
/// accessor function/method) to the model's `data_entity:sql:<table>` node.
/// Deduped per (accessor, entity).
fn try_detect_data_access(call: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    if func.kind() != "attribute" {
        return;
    }
    let Some(attr) = func.child_by_field_name("attribute") else {
        return;
    };
    if text(attr, src) != "query" {
        return;
    }
    let Some(arglist) = call.child_by_field_name("arguments") else {
        return;
    };
    let args = positional_args(arglist);
    let Some(first) = args.first() else {
        return;
    };
    if first.kind() != "identifier" {
        return;
    }
    let model = text(*first, src);
    let Some(&entity_id) = acc.model_tables.get(model) else {
        return;
    };
    if !acc.accesses_data_seen.insert((from, entity_id)) {
        return;
    }
    acc.edges.push(Edge {
        from,
        to: entity_id,
        category: edge_category::ACCESSES_DATA,
        confidence: Confidence::Medium,
    });
}

// ============================================================================
// Test targets (substrate-gap py-tests)
// ============================================================================

/// pytest discovery convention: a test function is named `test_*`.
fn is_pytest_test(name: &str) -> bool {
    name.starts_with("test_")
}

/// Walk a pytest test function's body and emit a TESTS `UnresolvedRef` for each
/// bare-name call (`add(2, 3)`). The graph crate binds these by the module's
/// import bindings / symbols, so only calls that reach a real project function
/// (e.g. `from calc import add`) produce a resolved fn-level TESTS edge; bare
/// builtins simply stay unresolved. Deduped by callee name within the test.
fn collect_test_targets(body: TsNode, src: &[u8], from: NodeId, module_id: NodeId, acc: &mut Acc) {
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        // Don't descend into nested defs — their calls aren't this test's.
        if matches!(node.kind(), "function_definition" | "class_definition") {
            continue;
        }
        if node.kind() == "call"
            && let Some(func) = node.child_by_field_name("function")
            && func.kind() == "identifier"
        {
            let name = text(func, src);
            if seen.insert(name.to_string()) {
                acc.refs.push(UnresolvedRef {
                    from,
                    from_module: module_id,
                    qualifier: CallQualifier::Bare(name.to_string()),
                    category: edge_category::TESTS,
                });
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

// ============================================================================
// Client HTTP-call extraction (Pattern A) — requests / httpx
// ============================================================================
//
// A client call like `requests.get('/users')` / `httpx.post(url)` /
// `self.session.get(f'/users/{uid}')` is emitted as a shared ENDPOINT node (via
// `push_client_endpoint`) so the cross-graph HttpStackResolver can pair it with
// a server ROUTE (HTTP_CALLS). Server-side route decorators are handled
// separately by `check_route_decorator`; those receivers (`app`, `router`,
// `blueprint`) are not HTTP clients, so there is no phantom-ROUTE overlap.

const HTTP_VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

/// True if a call receiver names an HTTP client — `requests` / `httpx` /
/// `http` / `session` / `client` / `api`, or any `*_client` / `*session`
/// (e.g. `self.http_client`, `api_session`). Leading underscores are ignored.
/// The `/`-path guard downstream (a non-path first arg → `url_to_path` = None)
/// keeps loose matches from producing spurious endpoints.
fn is_http_client_receiver(name: &str) -> bool {
    let n = name.trim_start_matches('_').to_ascii_lowercase();
    matches!(
        n.as_str(),
        "requests" | "httpx" | "http" | "session" | "client" | "api"
    ) || n.ends_with("client")
        || n.ends_with("session")
}

/// The simple (trailing) name of a call receiver: `requests` → `requests`;
/// `self.session` (an `attribute`) → `session`. Chained/other shapes → None.
fn http_receiver_simple<'a>(object: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    match object.kind() {
        "identifier" => Some(text(object, src)),
        "attribute" => object.child_by_field_name("attribute").map(|a| text(a, src)),
        _ => None,
    }
}

/// Positional argument nodes of a call's `argument_list`, in order, skipping
/// `keyword_argument`s (`json=body`, `timeout=5`, …).
fn positional_args<'a>(arglist: TsNode<'a>) -> Vec<TsNode<'a>> {
    let mut out = Vec::new();
    let mut cursor = arglist.walk();
    for child in arglist.named_children(&mut cursor) {
        if child.kind() != "keyword_argument" {
            out.push(child);
        }
    }
    out
}

/// Reconstruct a Python string-literal path. Plain text comes from
/// `string_content`; every `interpolation` (`{uid}` in an f-string) becomes
/// `${…}` so it normalises to a wildcard downstream (matching Dart/TS).
/// Returns `(path, had_interpolation)`; a non-`string` node yields an empty path.
fn py_string_path(node: TsNode, src: &[u8]) -> (String, bool) {
    if node.kind() != "string" {
        return (String::new(), false);
    }
    let mut out = String::new();
    let mut interpolated = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "string_content" => out.push_str(text(child, src)),
            "interpolation" => {
                out.push_str("${…}");
                interpolated = true;
            }
            _ => {}
        }
    }
    (out, interpolated)
}

/// Pattern A: detect a client HTTP call and emit a shared ENDPOINT node + CALLS
/// edge from `from`. Covers `requests.get(url)` / `httpx.post(url)` /
/// `self.session.get(url)` (verb from the attribute) and
/// `requests.request("GET", url)` (verb from the first string arg).
fn try_detect_py_endpoint(
    call: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    if func.kind() != "attribute" {
        return;
    }
    let Some(object) = func.child_by_field_name("object") else {
        return;
    };
    let Some(attr) = func.child_by_field_name("attribute") else {
        return;
    };
    let Some(recv) = http_receiver_simple(object, src) else {
        return;
    };
    if !is_http_client_receiver(recv) {
        return;
    }
    let Some(arglist) = call.child_by_field_name("arguments") else {
        return;
    };
    let method_name = text(attr, src).to_ascii_lowercase();
    let args = positional_args(arglist);

    // Resolve (verb, url-arg-node).
    let (verb, url_node) = if HTTP_VERBS.contains(&method_name.as_str()) {
        let Some(url) = args.first().copied() else {
            return;
        };
        (method_name.to_ascii_uppercase(), url)
    } else if method_name == "request" {
        // requests.request("GET", url) — verb is the first string arg.
        let Some(verb_node) = args.first().copied() else {
            return;
        };
        let (verb_raw, _) = py_string_path(verb_node, src);
        let verb = verb_raw.trim().to_ascii_uppercase();
        if !HTTP_VERBS.contains(&verb.to_ascii_lowercase().as_str()) {
            return;
        }
        let Some(url) = args.get(1).copied() else {
            return;
        };
        (verb, url)
    } else {
        return;
    };

    // Path must be a string literal; reconstruct interpolations, then split a
    // full URL into its path (the ENDPOINT's identity) and its authority (the
    // `host` on ENDPOINT_HIT, A11.5). Bail on a bare variable / non-path arg.
    let (raw_path, interpolated) = py_string_path(url_node, src);
    if raw_path.is_empty() {
        return;
    }
    let (host, path) = endpoint::client_url_split(&raw_path);
    let Some(path) = path else {
        return;
    };
    let confidence = if interpolated {
        Confidence::Medium
    } else {
        Confidence::Strong
    };
    let pos = call.start_position();
    let ep = ClientEndpoint {
        method: verb,
        path,
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence,
    };
    let extras = HitExtras {
        host: host.as_deref(),
        ..HitExtras::default()
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

/// Callables whose call in a parameter default, an `Annotated[...]` marker or
/// a decorator's `dependencies=[...]` is a framework-wired dependency: `Depends` / `Security` are FastAPI,
/// `Provide` is dependency-injector's call form. Matched on the trailing
/// dotted segment, so `fastapi.Depends(...)` counts too.
const DEPENDS_MARKERS: &[&str] = &["Depends", "Security", "Provide"];

/// A7.4 — FastAPI dependency injection. Blind to the call walker because the
/// call lives in a parameter DEFAULT, inside an `Annotated[...]` subscript, or
/// in a decorator argument, and `collect_calls_in` only walks the body:
///
/// ```text
/// def read(db: Session = Depends(get_db)):            # typed_default_parameter.value
/// def read(db = Depends(get_db)):                      # default_parameter.value
/// def read(db: Annotated[Session, Depends(get_db)]):   # type -> generic_type / subscript
/// @router.get("/x", dependencies=[Depends(auth)])      # decorator keyword argument
/// ```
///
/// The INJECTS target is the *provider* (`get_db`), not the annotation type:
/// that is what FastAPI actually wires and what `trace` needs to walk. The
/// annotation type keeps its USES ref from `collect_type_refs`. A bare
/// `Depends()` (FastAPI infers the dependency from the annotation) targets the
/// annotated class instead. A route decorator's `dependencies=[...]` list
/// runs each provider for the route without passing its value; the handler
/// still INJECTS it. One ref per provider name per def.
fn collect_depends_refs(
    def: TsNode,
    decorators: &[TsNode],
    src: &[u8],
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let mut seen: std::collections::HashSet<String> = Default::default();
    if let Some(params) = def.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for p in params.named_children(&mut cursor) {
            let ty = p.child_by_field_name("type");
            if matches!(p.kind(), "default_parameter" | "typed_default_parameter")
                && let Some(v) = p.child_by_field_name("value")
            {
                push_depends_target(v, ty, src, from, module_id, &mut seen, acc);
            }
            // `Annotated[T, Depends(f)]` — the marker lives inside the type.
            if let Some(ty) = ty {
                scan_annotated_depends(ty, src, from, module_id, &mut seen, acc);
            }
        }
    }
    // `@router.get("/x", dependencies=[Depends(f), ...])`.
    for deco in decorators {
        let Some(args) = deco
            .named_child(0)
            .filter(|c| c.kind() == "call")
            .and_then(|c| c.child_by_field_name("arguments"))
        else {
            continue;
        };
        let mut cursor = args.walk();
        for kw in args.named_children(&mut cursor) {
            if kw.kind() != "keyword_argument"
                || kw.child_by_field_name("name").map(|n| text(n, src)) != Some("dependencies")
            {
                continue;
            }
            let Some(list) = kw
                .child_by_field_name("value")
                .filter(|v| matches!(v.kind(), "list" | "tuple"))
            else {
                continue;
            };
            let mut c = list.walk();
            for item in list.named_children(&mut c) {
                push_depends_target(item, None, src, from, module_id, &mut seen, acc);
            }
        }
    }
}

/// If `expr` is a `Depends(...)`-family call, push one INJECTS ref to its
/// provider. The provider is the first positional argument or the
/// `dependency=` keyword, read as an identifier or the trailing name of an
/// attribute (`deps.get_db` → `get_db`, the `emit_type_idents` convention).
/// Any other provider expression (a lambda, `Provide[Container.x]`) is not a
/// named symbol and emits nothing. With no provider argument at all,
/// `fallback_type` (the annotated type) names the dependency.
fn push_depends_target(
    expr: TsNode,
    fallback_type: Option<TsNode>,
    src: &[u8],
    from: NodeId,
    module_id: NodeId,
    seen: &mut std::collections::HashSet<String>,
    acc: &mut Acc,
) {
    if expr.kind() != "call" {
        return;
    }
    let Some(func) = expr.child_by_field_name("function") else {
        return;
    };
    let callee = text(func, src);
    let tail = callee.rsplit('.').next().unwrap_or(callee).trim();
    if !DEPENDS_MARKERS.contains(&tail) {
        return;
    }
    let provider = expr
        .child_by_field_name("arguments")
        .and_then(|args| depends_provider_arg(args, src));
    let name = match provider {
        Some(arg) => symbol_tail_name(arg, src),
        None => fallback_type.and_then(|t| first_type_name(t, src)),
    };
    let Some(name) = name else {
        return;
    };
    if is_type_noise(&name) || !seen.insert(name.clone()) {
        return;
    }
    acc.refs.push(UnresolvedRef {
        from,
        from_module: module_id,
        qualifier: CallQualifier::Bare(name),
        category: edge_category::INJECTS,
    });
    di_stats::record(DiShape::PyFastapiDepends);
}

/// The provider argument of a `Depends(...)` call: the first positional
/// argument, else the value of a `dependency=` keyword. `None` when the call
/// names no provider (`Depends()`, `Depends(use_cache=False)`).
fn depends_provider_arg<'t>(args: TsNode<'t>, src: &[u8]) -> Option<TsNode<'t>> {
    let mut cursor = args.walk();
    let mut keyword = None;
    for a in args.named_children(&mut cursor) {
        match a.kind() {
            "comment" => {}
            "keyword_argument" => {
                let is_dependency = a
                    .child_by_field_name("name")
                    .is_some_and(|n| text(n, src) == "dependency");
                if is_dependency && keyword.is_none() {
                    keyword = a.child_by_field_name("value");
                }
            }
            _ => return Some(a),
        }
    }
    keyword
}

/// `get_db` → `get_db`; `deps.get_db` → `get_db`. Anything else is not a
/// named symbol.
fn symbol_tail_name(n: TsNode, src: &[u8]) -> Option<String> {
    match n.kind() {
        "identifier" => Some(text(n, src).to_string()),
        "attribute" => n
            .child_by_field_name("attribute")
            .map(|a| text(a, src).to_string()),
        _ => None,
    }
}

/// First non-noise type name in `ty`, in source order: `CommonQueryParams`
/// for `Optional[CommonQueryParams]`. `Annotated` itself is skipped (it is a
/// wrapper, not the dependency).
fn first_type_name(ty: TsNode, src: &[u8]) -> Option<String> {
    let mut stack = vec![ty];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "identifier" | "attribute" => {
                if let Some(s) = symbol_tail_name(n, src)
                    && s != "Annotated"
                    && !is_type_noise(&s)
                {
                    return Some(s);
                }
            }
            // A call inside a type is a marker (`Depends()`), never the type.
            "call" => {}
            _ => {
                let mut c = n.walk();
                let children: Vec<TsNode> = n.named_children(&mut c).collect();
                stack.extend(children.into_iter().rev());
            }
        }
    }
    None
}

/// `Annotated[T, ...]` (as a `generic_type` or an expression `subscript`) →
/// its first argument `T`; `None` for anything else.
fn annotated_first_arg<'t>(n: TsNode<'t>, src: &[u8]) -> Option<TsNode<'t>> {
    let (head, first) = match n.kind() {
        "generic_type" => {
            let head = n.named_child(0)?;
            let mut c = n.walk();
            let params = n
                .named_children(&mut c)
                .find(|ch| ch.kind() == "type_parameter")?;
            (head, params.named_child(0)?)
        }
        "subscript" => (
            n.child_by_field_name("value")?,
            n.child_by_field_name("subscript")?,
        ),
        _ => return None,
    };
    let head_text = text(head, src);
    let tail = head_text.rsplit('.').next().unwrap_or(head_text).trim();
    (tail == "Annotated").then_some(first)
}

/// Walk a parameter's type annotation for `Depends(...)` markers. A `call` is
/// handed to [`push_depends_target`] and not descended into. Inside
/// `Annotated[T, ...]`, `T` is the fallback for a bare `Depends()`. Handles
/// nesting such as `Optional[Annotated[T, Depends(f)]]`.
fn scan_annotated_depends(
    ty: TsNode,
    src: &[u8],
    from: NodeId,
    module_id: NodeId,
    seen: &mut std::collections::HashSet<String>,
    acc: &mut Acc,
) {
    let mut stack: Vec<(TsNode, Option<TsNode>)> = vec![(ty, None)];
    while let Some((n, fallback)) = stack.pop() {
        if n.kind() == "call" {
            push_depends_target(n, fallback, src, from, module_id, seen, acc);
            continue;
        }
        let inner = annotated_first_arg(n, src).or(fallback);
        let mut c = n.walk();
        for child in n.named_children(&mut c) {
            stack.push((child, inner));
        }
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

/// Pre-pass: harvest module-level router/blueprint path prefixes, so a route
/// decorator can compose the prefix its *receiver* carries onto the fragment
/// the decorator itself names.
///
///   `router = APIRouter(prefix="/api/v1/users")` + `@router.get("/{id}")`
///       → `GET /api/v1/users/{id}` (not `GET /{id}`)
///   `bp = Blueprint("orders", __name__, url_prefix="/api/v1/orders")`
///       + `@bp.route("/<int:id>")` → `GET /api/v1/orders/<int:id>`
///
/// Cross-FILE `include_router` is deliberately OUT of scope, not missed: the
/// parser sees one file at a time, so a router defined in `routers/users.py`
/// and mounted in `main.py` still loses the mount prefix. Composing across
/// modules needs a graph-crate pass over the resolved import edges.
fn scan_router_prefixes(root: TsNode, src: &[u8], acc: &mut Acc) {
    // Top-level statements, unwrapping the `expression_statement` shell that
    // tree-sitter-python puts around a bare assignment or call.
    let mut stmts: Vec<TsNode> = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "expression_statement" {
            let mut inner = child.walk();
            stmts.extend(child.named_children(&mut inner));
        } else {
            stmts.push(child);
        }
    }

    // Pass 1 — the constructor assignments.
    for stmt in &stmts {
        if stmt.kind() != "assignment" {
            continue;
        }
        let (Some(lhs), Some(rhs)) = (
            stmt.child_by_field_name("left"),
            stmt.child_by_field_name("right"),
        ) else {
            continue;
        };
        if lhs.kind() != "identifier" || rhs.kind() != "call" {
            continue;
        }
        let Some(func) = rhs.child_by_field_name("function") else {
            continue;
        };
        // `APIRouter(...)` or `fastapi.APIRouter(...)` — match the tail.
        let kw = match text(func, src).rsplit('.').next().unwrap_or("") {
            "APIRouter" => "prefix",
            "Blueprint" => "url_prefix",
            _ => continue,
        };
        let Some(prefix) = keyword_string_arg(rhs, kw, src) else {
            continue;
        };
        if prefix.is_empty() {
            continue;
        }
        acc.router_prefixes.insert(text(lhs, src).to_string(), prefix);
    }

    // Pass 2 — same-file FastAPI mount: `app.include_router(router, prefix="/v2")`
    // folds the mount prefix onto that router's own prefix. A second pass so
    // the mount may sit above or below the router assignment.
    for stmt in &stmts {
        if stmt.kind() != "call" {
            continue;
        }
        let Some(func) = stmt.child_by_field_name("function") else {
            continue;
        };
        if text(func, src).rsplit('.').next() != Some("include_router") {
            continue;
        }
        let Some(args) = stmt.child_by_field_name("arguments") else {
            continue;
        };
        let mut cursor = args.walk();
        let Some(first) = args.named_children(&mut cursor).find(|a| a.kind() == "identifier")
        else {
            continue;
        };
        let Some(mount) = keyword_string_arg(*stmt, "prefix", src) else {
            continue;
        };
        if mount.is_empty() {
            continue;
        }
        let target = text(first, src).to_string();
        let own = acc.router_prefixes.get(&target).cloned().unwrap_or_default();
        let composed = if own.is_empty() {
            mount
        } else {
            endpoint::join_path(&mount, &own)
        };
        acc.router_prefixes.insert(target, composed);
    }
}

/// Read a `name="literal"` keyword argument off a `call`'s argument list.
/// Only a plain string literal is accepted — a variable or a computed prefix is
/// not something a single-file parser can resolve, and a wrong prefix would be
/// worse than none.
fn keyword_string_arg(call: TsNode, name: &str, src: &[u8]) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if arg.kind() != "keyword_argument" || child_text(arg, "name", src) != Some(name) {
            continue;
        }
        let value = arg.child_by_field_name("value")?;
        if value.kind() != "string" {
            return None;
        }
        return Some(strip_string_quotes(text(value, src)));
    }
    None
}

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
    // substrate-gap py-router-prefix — the receiver (`router` in `@router.get`)
    // may be an APIRouter/Blueprint carrying a prefix; compose it on. With no
    // prefix `join_path("", p)` is a pure pass-through and `abs_path` only
    // normalises the leading slash, so `@app.get("/users")` stays byte-identical.
    let receiver = head.rsplit_once('.').map(|(r, _)| r.trim()).unwrap_or("");
    let prefix = acc.router_prefixes.get(receiver).map(String::as_str).unwrap_or("");
    let full = endpoint::abs_path(&endpoint::join_path(prefix, &path));
    if !prefix.is_empty() {
        acc.routes_composed += 1;
    }
    for m in methods {
        emit_route(m, &full, handler_id, repo, acc);
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

    // substrate-gap py-router-prefix — the receiver's prefix composes onto the
    // decorator's own fragment.
    #[test]
    fn fastapi_router_prefix_composes() {
        let src = "from fastapi import APIRouter\nrouter = APIRouter(prefix='/api/v1/users')\n\n@router.get('/{id}')\nasync def get_user(id: int):\n    return {}\n";
        let parse = parse_file(src, "routers.py", "routers", repo()).unwrap();
        let rid = route_id("GET", "/api/v1/users/{id}");
        let handler =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "routers::get_user");
        assert!(
            parse.nodes.iter().any(|n| n.id == rid),
            "APIRouter(prefix=…) not composed; routes: {:?}",
            route_names(&parse)
        );
        assert!(has_edge(&parse, rid, handler, edge_category::HANDLED_BY));
    }

    #[test]
    fn flask_blueprint_url_prefix_composes() {
        let src = "from flask import Blueprint\nbp = Blueprint('orders', __name__, url_prefix='/api/v1/orders')\n\n@bp.route('/<int:id>', methods=['GET'])\ndef get_order(id):\n    return {}\n";
        let parse = parse_file(src, "routers.py", "routers", repo()).unwrap();
        let rid = route_id("GET", "/api/v1/orders/<int:id>");
        let handler =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "routers::get_order");
        assert!(
            parse.nodes.iter().any(|n| n.id == rid),
            "Blueprint(url_prefix=…) not composed; routes: {:?}",
            route_names(&parse)
        );
        assert!(has_edge(&parse, rid, handler, edge_category::HANDLED_BY));
    }

    #[test]
    fn include_router_prefix_folds() {
        let src = "from fastapi import APIRouter, FastAPI\napp = FastAPI()\nrouter = APIRouter(prefix='/users')\napp.include_router(router, prefix='/v2')\n\n@router.get('/{id}')\ndef get_user(id: int):\n    return {}\n";
        let parse = parse_file(src, "main.py", "main", repo()).unwrap();
        assert!(
            parse.nodes.iter().any(|n| n.id == route_id("GET", "/v2/users/{id}")),
            "include_router prefix not folded; routes: {:?}",
            route_names(&parse)
        );
    }

    /// Locks the no-prefix path: a bare `app` receiver is byte-identical to
    /// before composition existed, and emits exactly one ROUTE.
    #[test]
    fn app_decorator_without_prefix_unchanged() {
        let src = "from flask import Flask\napp = Flask(__name__)\n\n@app.get('/users')\ndef list_users():\n    return []\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        assert_eq!(route_names(&parse), vec!["GET /users".to_string()]);
    }

    /// Every ROUTE node's recorded name, sorted — for readable assert output.
    fn route_names(parse: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = parse
            .nodes
            .iter()
            .filter(|n| parse.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::ROUTE))
            .filter_map(|n| parse.nav.name_by_id.get(&n.id).cloned())
            .collect();
        out.sort();
        out
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

    // ----- Pattern A: client HTTP-call ENDPOINT extraction -----

    fn endpoint_id(method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            &format!("endpoint:{method}:{path}"),
        )
    }

    #[test]
    fn requests_client_call_emits_endpoint_not_route() {
        // `requests.get(f"http://api/users/{uid}")` / `requests.post(...)` in a
        // function body → ENDPOINT nodes (not phantom ROUTEs), each with a CALLS
        // edge from the enclosing function. Absolute URL → path; f-string
        // interpolation → `${…}`.
        let src = "import requests\n\n\ndef fetch_user(uid):\n    r = requests.get(f\"http://api/users/{uid}\")\n    return r.json()\n\n\ndef make_user(body):\n    requests.post(\"http://api/users\", json=body)\n";
        let parse = parse_file(src, "client.py", "client", repo()).unwrap();

        let fetch_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "client::fetch_user");
        let ep_get = endpoint_id("GET", "/users/${…}");
        let ep_post = endpoint_id("POST", "/users");

        assert!(
            parse.nodes.iter().any(|n| n.id == ep_get),
            "expected GET /users/${{…}} ENDPOINT node, nodes: {:?}",
            parse
                .nodes
                .iter()
                .filter(|n| parse.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::ENDPOINT))
                .map(|n| parse.nav.qname_by_id.get(&n.id))
                .collect::<Vec<_>>()
        );
        assert!(
            parse.nodes.iter().any(|n| n.id == ep_post),
            "expected POST /users ENDPOINT node"
        );
        // CALLS edge from the enclosing function into the endpoint.
        assert!(
            has_edge(&parse, fetch_id, ep_get, edge_category::CALLS),
            "expected CALLS edge fetch_user → GET endpoint"
        );
        // No phantom ROUTE for the client calls.
        let has_route = parse
            .nodes
            .iter()
            .any(|n| parse.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::ROUTE));
        assert!(!has_route, "client requests.* must not emit server ROUTEs");
    }

    /// A11.5 — an absolute URL's authority lands on the ENDPOINT_HIT cell as
    /// `host`; an f-string authority (`{base}`) names no service, so no `host`.
    #[test]
    fn requests_client_endpoint_carries_the_url_authority_as_host() {
        let src = "import requests\n\n\ndef make_user(body):\n    requests.post(\"http://api/users\", json=body)\n\n\ndef list_orders(base):\n    requests.get(f\"http://{base}/orders\")\n";
        let parse = parse_file(src, "client.py", "client", repo()).unwrap();
        let hit = |id: NodeId| -> String {
            let node = parse
                .nodes
                .iter()
                .find(|n| n.id == id)
                .expect("ENDPOINT node");
            match &node.cells[0].payload {
                CellPayload::Json(j) if node.cells[0].kind == cell_type::ENDPOINT_HIT => j.clone(),
                other => panic!("not an ENDPOINT_HIT json cell: {other:?}"),
            }
        };
        let users = hit(endpoint_id("POST", "/users"));
        assert!(
            users.ends_with(r#","confidence":"strong","host":"api"}"#),
            "{users}"
        );
        let orders = hit(endpoint_id("GET", "/orders"));
        assert!(!orders.contains("host"), "{orders}");
    }

    #[test]
    fn requests_request_verb_from_first_arg_and_session_receiver() {
        // `requests.request("GET", "/things")` → verb from first string arg.
        // `self.session.post("/things")` → receiver's trailing name is a client.
        let src = "import requests\n\n\nclass Api:\n    def run(self):\n        requests.request(\"GET\", \"/things\")\n        self.session.post(\"/things\")\n";
        let parse = parse_file(src, "api.py", "api", repo()).unwrap();
        let run_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "api::Api::run");
        let ep_get = endpoint_id("GET", "/things");
        let ep_post = endpoint_id("POST", "/things");
        assert!(parse.nodes.iter().any(|n| n.id == ep_get), "missing GET /things");
        assert!(parse.nodes.iter().any(|n| n.id == ep_post), "missing POST /things");
        assert!(has_edge(&parse, run_id, ep_get, edge_category::CALLS));
        assert!(has_edge(&parse, run_id, ep_post, edge_category::CALLS));
    }

    #[test]
    fn non_http_receiver_does_not_emit_endpoint() {
        // A `.get('/x')` on a non-client receiver (dict-like) must NOT become an
        // endpoint. `cfg` is not an HTTP client name.
        let src = "def f(cfg):\n    return cfg.get(\"/x\")\n";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let has_endpoint = parse
            .nodes
            .iter()
            .any(|n| parse.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::ENDPOINT));
        assert!(!has_endpoint, "cfg.get should not emit an ENDPOINT");
    }

    #[test]
    fn session_query_emits_accesses_data_from_enclosing_function() {
        // substrate-gap py-accesses-data — `session.query(User)` inside
        // `find_users` must anchor ACCESSES_DATA on the *function*, not the
        // module. `User.__tablename__ = "users"` gives the data-entity node.
        let src = "from sqlalchemy.orm import Session\n\n\nclass User(Base):\n    __tablename__ = \"users\"\n    id = Column(Integer)\n\n\ndef find_users(session):\n    return session.query(User).filter(User.name == \"x\").all()\n";
        let parse = parse_file(src, "store.py", "store", repo()).unwrap();

        let find_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "store::find_users");
        let entity_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            "data_entity:sql:users",
        );

        assert!(
            has_edge(&parse, find_id, entity_id, edge_category::ACCESSES_DATA),
            "expected find_users → data_entity:sql:users ACCESSES_DATA edge, edges: {:?}",
            parse
                .edges
                .iter()
                .filter(|e| e.category == edge_category::ACCESSES_DATA)
                .map(|e| (e.from, e.to))
                .collect::<Vec<_>>()
        );

        // The edge must NOT be anchored on the module.
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "store");
        assert!(
            !has_edge(&parse, module_id, entity_id, edge_category::ACCESSES_DATA),
            "parser must not emit the module-anchored ACCESSES_DATA edge"
        );
    }

    #[test]
    fn unknown_model_does_not_emit_accesses_data() {
        // No `__tablename__` for `Thing` → no data-entity resolution → no edge.
        let src = "def find(session):\n    return session.query(Thing).all()\n";
        let parse = parse_file(src, "store.py", "store", repo()).unwrap();
        let has_ad = parse
            .edges
            .iter()
            .any(|e| e.category == edge_category::ACCESSES_DATA);
        assert!(!has_ad, "query on an unknown model must not emit ACCESSES_DATA");
    }

    #[test]
    fn pytest_test_emits_fn_level_tests_ref() {
        // substrate-gap py-tests — `def test_add` calling bare `add` emits a
        // fn-level TESTS ref (resolved cross-file by the graph crate).
        let src = "from calc import add\n\n\ndef test_add():\n    assert add(2, 3) == 5\n";
        let parse = parse_file(src, "test_calc.py", "test_calc", repo()).unwrap();

        let test_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "test_calc::test_add");

        let has_tests_ref = parse.refs.iter().any(|r| {
            r.from == test_id
                && r.category == edge_category::TESTS
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "add")
        });
        assert!(
            has_tests_ref,
            "expected TESTS ref test_add → add, refs: {:?}",
            parse
                .refs
                .iter()
                .map(|r| (r.category, &r.qualifier))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_test_function_emits_no_tests_ref() {
        // A plain (non-`test_`) function calling `add` must NOT emit a TESTS ref.
        let src = "from calc import add\n\n\ndef run():\n    return add(1, 2)\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        let has_tests_ref = parse
            .refs
            .iter()
            .any(|r| r.category == edge_category::TESTS);
        assert!(!has_tests_ref, "non-test fn must not emit a TESTS ref");
    }

    /// Bare-name INJECTS targets pushed from `from`, in emission order.
    fn injects_from(parse: &FileParse, from: NodeId) -> Vec<String> {
        parse
            .refs
            .iter()
            .filter(|r| r.from == from && r.category == edge_category::INJECTS)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    fn fn_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname)
    }

    #[test]
    fn fastapi_depends_default_emits_injects_ref() {
        // Untyped and typed defaults, bare and module-qualified `Depends`.
        let src = "from fastapi import Depends\nimport fastapi\nfrom deps import get_db, get_user\n\n\n@app.get(\"/u\")\ndef read(db = Depends(get_db), user: User = fastapi.Depends(deps.get_user)):\n    return db\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        assert_eq!(
            injects_from(&parse, fn_id("app::read")),
            vec!["get_db".to_string(), "get_user".to_string()],
        );
    }

    #[test]
    fn fastapi_annotated_depends_emits_injects_ref() {
        // `Annotated[T, Depends(f)]`, also nested under Optional; `Security`
        // is the same shape. The provider is the target, never the type.
        let src = "from typing import Annotated, Optional\nfrom fastapi import Depends, Security\n\n\ndef admin(ok: Annotated[bool, Depends(verify_token)], u: Optional[Annotated[User, Security(current_user, scopes=[\"a\"])]] = None):\n    return ok\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        let mut got = injects_from(&parse, fn_id("app::admin"));
        got.sort();
        assert_eq!(got, vec!["current_user".to_string(), "verify_token".to_string()]);
    }

    #[test]
    fn non_depends_default_emits_no_injects() {
        // A call default that is not a Depends marker, a non-call default, and
        // a Depends whose provider is not a named symbol all emit nothing.
        let src = "def f(x = compute(), y = 3, z = Depends(lambda: 1), w: int = other(get_db)):\n    return x\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        assert!(
            parse.refs.iter().all(|r| r.category != edge_category::INJECTS),
            "expected no INJECTS refs, got {:?}",
            injects_from(&parse, fn_id("app::f"))
        );
    }

    #[test]
    fn fastapi_bare_depends_falls_back_to_annotation_type() {
        // `Depends()` with no provider: FastAPI instantiates the annotated
        // class. `dependency=` keyword names the provider explicitly.
        let src = "def list_items(q: Annotated[Pager, Depends(use_cache=False)], commons: CommonQueryParams = Depends(), r = Depends(dependency=get_repo)):\n    return commons\n";
        let parse = parse_file(src, "app.py", "app", repo()).unwrap();
        assert_eq!(
            injects_from(&parse, fn_id("app::list_items")),
            vec![
                "Pager".to_string(),
                "CommonQueryParams".to_string(),
                "get_repo".to_string()
            ],
        );
    }

    #[test]
    fn fastapi_decorator_dependencies_emit_injects_ref() {
        // Route-level `dependencies=[...]`: every provider in the list, deduped
        // against the parameter spellings; other keywords never fire.
        let src = "@router.get(\n    \"/\",\n    dependencies=[Depends(get_current_active_superuser), Security(audit)],\n    response_model=Depends(not_a_dep),\n)\ndef read_users(db = Depends(audit)):\n    return db\n";
        let parse = parse_file(src, "users.py", "users", repo()).unwrap();
        assert_eq!(
            injects_from(&parse, fn_id("users::read_users")),
            vec!["audit".to_string(), "get_current_active_superuser".to_string()],
        );
    }

    #[test]
    fn fastapi_depends_on_method_emits_injects_ref() {
        // Class-based dependency / view: the method visitor wires it too.
        let src = "class ItemService:\n    def __init__(self, db = Depends(get_db)):\n        self.db = db\n";
        let parse = parse_file(src, "svc.py", "svc", repo()).unwrap();
        let init_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "svc::ItemService::__init__",
        );
        assert_eq!(injects_from(&parse, init_id), vec!["get_db".to_string()]);
    }
}
