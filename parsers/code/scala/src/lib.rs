use std::sync::OnceLock;

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_code_domain::di_stats::{self, DiShape};
use glia_code_domain::endpoint::{ClientEndpoint, push_client_endpoint, url_to_path};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let acc = parse_acc(source, file_rel_path, module_qname, repo)?;
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

/// [`parse_file`]'s walk, its per-file counters kept (the tests read them).
fn parse_acc(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<Acc, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_scala::LANGUAGE.into();
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

    // LB.7a: top-level types are members of the PACKAGE (the directory), so
    // their qnames hang off the directory scope, not the file module. The
    // MODULE node above keeps `module_qname`, and so do imports, the package
    // clause and top-level `def` / `val` (Scala 3 makes those file members).
    let scope = type_scope(module_qname);
    let top_level_types = visit_top(
        root,
        src,
        file_rel_path,
        module_qname,
        scope,
        module_id,
        module_id,
        repo,
        &mut acc,
    );
    if top_level_types > 0 && qname_debug() {
        eprintln!(
            "[qname] scala: {top_level_types} top-level types scoped to {scope} (file stem dropped) file={file_rel_path}"
        );
    }
    let routes = scan_scala_routes(source, root, module_id, repo, &mut acc);
    if routes.routes > 0 {
        eprintln!("{}", routes.marker(file_rel_path));
    }
    if acc.endpoint_hits > 0 {
        eprintln!(
            "[scala-http-client] {} endpoints in {}",
            acc.endpoint_hits, file_rel_path
        );
    }
    if acc.tests.suites > 0 {
        eprintln!("{}", acc.tests.marker(file_rel_path));
    }
    Ok(acc)
}

/// LB.7a: a Scala top-level type belongs to its package (its directory, the
/// LB.2 rule), not its file, so drop the file-stem segment the engine's
/// `path_to_qname` puts last: `src::main::scala::shop::Widget` ->
/// `src::main::scala::shop`, and a file at the repo root (`Widget`) -> `""`.
/// The directory, not the declared `package`, is the scope on purpose: two
/// services of one monorepo that both declare `package shop` must keep
/// distinct NodeIds. Scala forbids two same-named top-level types in one
/// package, so the change never merges distinct declarations.
///
/// The public class `Widget` of `Widget.scala` therefore shares its qname with
/// the file MODULE (different kind, different NodeId); `MergedGraph::pick_primary`
/// ranks the declaration over the container, so qname lookups land on the type.
fn type_scope(module_qname: &str) -> &str {
    module_qname.rsplit_once("::").map_or("", |(dir, _stem)| dir)
}

/// `scope::name`, or the bare `name` for the empty (repo-root) scope.
fn scoped(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{scope}::{name}")
    }
}

/// `GLIA_QNAME_DEBUG=1` turns on the per-file `[qname] scala:` marker, read
/// once. Off by default: it would print for every Scala file of a build.
///   `GLIA_QNAME_DEBUG=1 glia analyze <repo> 2>&1 | grep '\[qname\] scala:'`
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
    /// Dedups the ENDPOINT node per `(method, path)` within a file.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Client HTTP call sites emitted in this file (drives the fired_on marker).
    endpoint_hits: usize,
    /// Dedups INJECTS refs per `(consumer, injected type)` within a file.
    inject_seen: std::collections::HashSet<(NodeId, String)>,
    /// Test suites / cases / containers read in this file (DG.11 marker).
    tests: TestTally,
}

/// Walk the file's top-level declarations. `parent_qname` is the file MODULE's
/// qname (imports, the package clause and top-level `def` / `val` hang off it);
/// `scope` is the package scope ([`type_scope`]) the top-level types hang off.
/// Returns how many top-level type nodes were emitted (the `[qname] scala:`
/// marker counts them).
#[allow(clippy::too_many_arguments)]
fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    scope: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> usize {
    let mut top_level_types = 0usize;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_declaration" => collect_import(child, src, parent_qname, acc),
            "package_clause" => collect_package(child, src, parent_qname, acc),
            "object_definition" | "class_definition" => {
                top_level_types += usize::from(visit_type_def(
                    child, src, file_rel, scope, parent_id, module_id, repo, node_kind::CLASS, acc,
                ));
            }
            "trait_definition" => {
                top_level_types += usize::from(visit_type_def(
                    child, src, file_rel, scope, parent_id, module_id, repo, node_kind::INTERFACE, acc,
                ));
            }
            "function_definition" | "val_definition" | "var_definition" => {
                visit_function(
                    child,
                    src,
                    file_rel,
                    parent_qname,
                    parent_id,
                    module_id,
                    repo,
                    acc,
                );
            }
            _ => {}
        }
    }
    top_level_types
}

/// Emit one class / object / trait and everything under it. `scope` is the
/// qname the type hangs off: the package scope ([`type_scope`]) for a top-level
/// type, the outer type's qname for a nested one. A companion `object Widget`
/// and `class Widget` share one qname and one kind (CLASS), so `merge_parses`
/// folds them into one node. Returns whether a type node was emitted.
#[allow(clippy::too_many_arguments)]
fn visit_type_def(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    scope: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    kind: glia_core::NodeKindId,
    acc: &mut Acc,
) -> bool {
    let Some(name_node) = node.child_by_field_name("name") else {
        return false;
    };
    let name = text_of(name_node, src);
    let qname = scoped(scope, name);
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
        cells: Vec::new(),
    });
    acc.nav.record(id, name, &qname, kind, Some(parent_id));

    emit_heritage_refs(&node, src, id, module_id, acc);
    emit_class_injects(node, src, name, id, module_id, acc);

    if let Some(body) = node.child_by_field_name("body") {
        visit_body_members(body, src, file_rel, &qname, id, module_id, repo, acc);
        if is_test_suite(node, src) {
            acc.tests.suites += 1;
            let mut suite = Suite::new(src, file_rel, &qname, id, repo);
            visit_test_registrations(body, "", &mut suite, acc);
        }
    }
    true
}

/// Scala class/trait/object heritage. The `extend`/`extends_clause` node holds
/// one or more `type` fields: `class Dog extends Animal with Runnable` →
/// `Animal` (first, the primary supertype) becomes INHERITS_FROM, each mixin
/// trait after `with` becomes IMPLEMENTS. We emit an `UnresolvedRef` with a
/// `Bare(TypeName)` qualifier; `resolve_refs` binds it to the uniquely-named
/// class/interface node across the repo and forms the heritage edge.
fn emit_heritage_refs(node: &TsNode, src: &[u8], from_id: NodeId, module_id: NodeId, acc: &mut Acc) {
    let Some(ext) = node.child_by_field_name("extend") else {
        return;
    };
    let mut cursor = ext.walk();
    let mut first = true;
    for ty in ext.children_by_field_name("type", &mut cursor) {
        // Strip generic args (`Ordered[Dog]` → `Ordered`) and take the trailing
        // simple name (`pkg.Animal` → `Animal`).
        let raw = text_of(ty, src);
        let base = raw.split('[').next().unwrap_or(raw).trim();
        let simple = base.rsplit(['.', ':']).next().unwrap_or(base).trim();
        if simple.is_empty() {
            continue;
        }
        let category = if first {
            edge_category::INHERITS_FROM
        } else {
            edge_category::IMPLEMENTS
        };
        first = false;
        acc.refs.push(UnresolvedRef {
            from: from_id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(simple.to_string()),
            category,
            line: line_at(ty),
        });
    }
}

fn visit_body_members(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "function_definition" | "val_definition" | "var_definition" => {
                emit_macwire_injects(child, src, parent_id, module_id, acc);
                visit_method(
                    child,
                    src,
                    file_rel,
                    parent_qname,
                    parent_id,
                    module_id,
                    repo,
                    acc,
                );
            }
            "object_definition" | "class_definition" => {
                visit_type_def(child, src, file_rel, parent_qname, parent_id, module_id, repo, node_kind::CLASS, acc);
            }
            "trait_definition" => {
                visit_type_def(child, src, file_rel, parent_qname, parent_id, module_id, repo, node_kind::INTERFACE, acc);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_function(
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::FUNCTION, Some(parent_id));
    emit_context_param_injects(node, src, id, module_id, acc);

    if let Some(body) = node.child_by_field_name("body") {
        // Top-level `def` (parent is a MODULE): a bare `foo()` binds against the
        // module's top-level symbols, so keep it `Bare`.
        collect_calls_in(body, src, id, false, acc);
        let hits = collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
        acc.endpoint_hits += hits;
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_method(
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
        cells: Vec::new(),
    });
    acc.nav
        .record(id, name, &qname, node_kind::METHOD, Some(parent_id));
    emit_context_param_injects(node, src, id, module_id, acc);

    if let Some(body) = node.child_by_field_name("body") {
        // Method inside a type body (parent is CLASS/INTERFACE): an unqualified
        // `foo()` is an implicit `this.foo()` — a sibling method of the enclosing
        // type. Classify as `SelfMethod` so `resolve_calls` binds it against
        // `class_methods[<enclosing type>]` (a bare `module_symbols` lookup would
        // miss, since sibling methods live under the type, not the module).
        collect_calls_in(body, src, id, true, acc);
        let hits = collect_client_endpoints_in(body, src, id, repo, file_rel, acc);
        acc.endpoint_hits += hits;
    }
}

fn collect_import(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let path = text.trim_start_matches("import ").trim();
    acc.imports.push(ImportStmt {
        from_module: from_module.to_string(),
        target: ImportTarget::Module {
            path: path.to_string(),
            alias: None,
        },
        line: line_at(node),
    });
}

fn collect_package(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let text = text_of(node, src).trim().to_string();
    let pkg = text.trim_start_matches("package ").trim();
    acc.imports.push(ImportStmt {
        from_module: from_module.to_string(),
        target: ImportTarget::Module {
            path: pkg.to_string(),
            alias: None,
        },
        line: line_at(node),
    });
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, in_type: bool, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some(func) = n.child_by_field_name("function")
        {
            let qualifier = classify_call(func, src, in_type);
            acc.calls.push(CallSite { from, qualifier, line: line_at(n) });
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "function_definition" | "class_definition" | "object_definition" | "lambda_expression"
            ) {
                stack.push(child);
            }
        }
    }
}

fn classify_call(func_node: TsNode, src: &[u8], in_type: bool) -> CallQualifier {
    match func_node.kind() {
        "identifier" => {
            let name = text_of(func_node, src).to_string();
            // Inside a type body an unqualified call is an implicit self-call;
            // emit `SelfMethod` so it binds against the enclosing type's methods.
            // At module scope it stays `Bare` (binds against module symbols).
            if in_type {
                CallQualifier::SelfMethod(name)
            } else {
                CallQualifier::Bare(name)
            }
        }
        "field_expression" => {
            let obj = func_node
                .child_by_field_name("value")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            let field = func_node
                .child_by_field_name("field")
                .map(|n| text_of(n, src))
                .unwrap_or("");
            if obj == "this" {
                CallQualifier::SelfMethod(field.to_string())
            } else if func_node
                .child_by_field_name("value")
                .is_some_and(|v| v.kind() == "identifier")
            {
                CallQualifier::Attribute {
                    base: obj.to_string(),
                    name: field.to_string(),
                }
            } else {
                CallQualifier::ComplexReceiver {
                    receiver: obj.to_string(),
                    name: field.to_string(),
                }
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

// ---------------------------------------------------------------------------
// Dependency injection (A7.7) -> INJECTS refs
// ---------------------------------------------------------------------------
//
// A DI-managed service, a case class and a value object all share the
// `class Foo(bar: Baz)` shape, so class parameters are read only behind a gate.
// Three shapes are read, all off the AST:
//   * class parameters of a DI-named, non-case class, or of an `@Inject` class;
//   * any `implicit` / `using` parameter list, on a class or a def: that is the
//     language's own context-passing mechanism, so it needs no gate;
//   * Macwire `val x = wire[Foo]` in a type body.
// Each ref is `Bare(TypeName)`; `resolve_refs` binds a uniquely-named type
// across the repo and leaves library types unresolved.

/// Name suffixes that mark a DI-managed class. The grammar exposes no DI
/// marker, so this is a name heuristic like C#'s and PHP's gates. It under-fires
/// on ZIO / cats-effect code, which names almost nothing `…Service`.
const SCALA_DI_SUFFIXES: &str =
    "Service Controller Repository Handler Manager Module Component Dao Client";

/// Never a DI target: value types, wrappers and collections, and the implicit
/// evidence that fills most `implicit` lists without being a service.
const SCALA_NON_INJECTABLE: &str = "Int Long Short Byte Double Float Boolean Char String Unit \
    Any AnyRef AnyVal Nothing BigInt BigDecimal Option Seq List Map Set Vector Array Either Try \
    Future ExecutionContext ClassTag TypeTag Ordering";

/// INJECTS refs from a class / trait declaration's parameters.
fn emit_class_injects(
    node: TsNode,
    src: &[u8],
    name: &str,
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let skip = type_param_names(node.child_by_field_name("type_parameters"), src);
    let mut cursor = node.walk();
    let children: Vec<TsNode> = node.children(&mut cursor).collect();
    let is_case = children.iter().any(|c| c.kind() == "case");
    let mut injected = false;
    for ann in children.iter().filter(|c| c.kind() == "annotation") {
        let ann_name = ann
            .child_by_field_name("name")
            .map_or("", |n| text_of(n, src));
        if ann_name.rsplit('.').next() != Some("Inject") {
            continue;
        }
        injected = true;
        // Play/Guice `class C @Inject() (a: A)`: tree-sitter-scala 0.25.1 has no
        // constructor-annotation rule, so the parameter list parses as a second
        // `arguments` of the annotation (`a: A` = `ascription_expression`) and
        // `class_parameters` is absent.
        let mut ac = ann.walk();
        for args in ann.children_by_field_name("arguments", &mut ac) {
            let mut pc = args.walk();
            for p in args.named_children(&mut pc) {
                let mut tc = p.walk();
                if p.kind() == "ascription_expression"
                    && let Some(ty) = p.named_children(&mut tc).last()
                {
                    push_inject(ty, src, &skip, from, module_id, acc);
                }
            }
        }
    }
    let suffixed = SCALA_DI_SUFFIXES
        .split_whitespace()
        .any(|s| name.ends_with(s));
    let gated = injected || (!is_case && suffixed);
    let mut lc = node.walk();
    for list in node.children_by_field_name("class_parameters", &mut lc) {
        if gated || is_context_param_list(list) {
            emit_param_list(list, src, &skip, from, module_id, acc);
        }
    }
}

/// INJECTS refs from a def's `implicit` / `using` parameter lists. `parameters`
/// is a multiple field (one node per list) that also holds the `[T]` list.
fn emit_context_param_injects(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    let lists: Vec<TsNode> = node
        .children_by_field_name("parameters", &mut cursor)
        .collect();
    let skip: Vec<&str> = lists
        .iter()
        .filter(|l| l.kind() == "type_parameters")
        .flat_map(|l| type_param_names(Some(*l), src))
        .collect();
    for list in lists {
        if list.kind() == "parameters" && is_context_param_list(list) {
            emit_param_list(list, src, &skip, from, module_id, acc);
        }
    }
}

/// `(implicit …)` / `(using …)`. The keyword is an anonymous token child of the
/// list node in tree-sitter-scala 0.25.1, not a field.
fn is_context_param_list(list: TsNode) -> bool {
    let mut cursor = list.walk();
    list.children(&mut cursor)
        .any(|c| matches!(c.kind(), "implicit" | "using"))
}

/// One INJECTS ref per parameter of a `class_parameters` / `parameters` list.
fn emit_param_list(
    list: TsNode,
    src: &[u8],
    skip: &[&str],
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let mut cursor = list.walk();
    for p in list.named_children(&mut cursor) {
        if matches!(p.kind(), "class_parameter" | "parameter")
            && let Some(ty) = p.child_by_field_name("type")
        {
            push_inject(ty, src, skip, from, module_id, acc);
        }
    }
}

/// Macwire `val x = wire[Foo]`: the value is a `generic_function` whose
/// `function` is the identifier `wire`. The ref comes from the enclosing type.
fn emit_macwire_injects(def: TsNode, src: &[u8], from: NodeId, module_id: NodeId, acc: &mut Acc) {
    let Some(value) = def.child_by_field_name("value") else {
        return;
    };
    let callee = value
        .child_by_field_name("function")
        .map(|f| text_of(f, src));
    if value.kind() != "generic_function" || callee != Some("wire") {
        return;
    }
    if let Some(args) = value.child_by_field_name("type_arguments") {
        let mut cursor = args.walk();
        if let Some(ty) = args.named_children(&mut cursor).next() {
            push_inject(ty, src, &[], from, module_id, acc);
        }
    }
}

/// Names declared by a `[A, F[_]]` list, so `(implicit ev: A)` is not read as a
/// dependency on a type called `A`.
fn type_param_names<'a>(tp: Option<TsNode<'a>>, src: &'a [u8]) -> Vec<&'a str> {
    let Some(tp) = tp else {
        return Vec::new();
    };
    let mut cursor = tp.walk();
    tp.children_by_field_name("name", &mut cursor)
        .map(|n| text_of(n, src))
        .collect()
}

fn push_inject(
    ty: TsNode,
    src: &[u8],
    skip: &[&str],
    from: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let Some(name) = scala_injectable_type_name(ty, src) else {
        return;
    };
    if skip.contains(&name.as_str()) || !acc.inject_seen.insert((from, name.clone())) {
        return;
    }
    acc.refs.push(UnresolvedRef {
        from,
        from_module: module_id,
        qualifier: CallQualifier::Bare(name),
        category: edge_category::INJECTS,
        line: line_at(ty),
    });
    di_stats::record(DiShape::ScalaCtor);
}

/// The bare type a parameter depends on: `Foo`, `Foo[F]` → `Foo`, `pkg.Foo` →
/// `Foo`. Function, tuple, infix, compound and wildcard types are ambiguous and
/// yield `None`, as do the [`SCALA_NON_INJECTABLE`] names.
fn scala_injectable_type_name(ty: TsNode, src: &[u8]) -> Option<String> {
    let name = match ty.kind() {
        "type_identifier" => text_of(ty, src),
        "stable_type_identifier" => text_of(ty, src).rsplit('.').next().unwrap_or(""),
        "generic_type" => {
            return ty
                .child_by_field_name("type")
                .and_then(|head| scala_injectable_type_name(head, src));
        }
        _ => return None,
    };
    let name = name.trim();
    let denied = SCALA_NON_INJECTABLE.split_whitespace().any(|t| t == name);
    (!name.is_empty() && !denied).then(|| name.to_string())
}

// ---------------------------------------------------------------------------
// Client HTTP calls (Play WS / sttp / Akka HTTP) -> shared ENDPOINT nodes
// ---------------------------------------------------------------------------

const SCALA_HTTP_VERBS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Upper-case `s` if it names an HTTP verb.
fn http_verb(s: &str) -> Option<String> {
    let up = s.trim().to_ascii_uppercase();
    SCALA_HTTP_VERBS.contains(&up.as_str()).then_some(up)
}

/// Outbound HTTP call sites in a def/method body become shared ENDPOINT nodes
/// (+ a CALLS edge from the enclosing symbol) so `HttpStackResolver` can pair
/// them with a server ROUTE. Three Scala client shapes are recognised:
///   Play WS  `ws.url("…").get()`        — URL on `.url(…)`, verb further up the chain
///   sttp     `basicRequest.get(uri"…")` — verb on the call, path in the `uri` interpolator
///   Akka     `HttpRequest(uri = "…", method = HttpMethods.POST)` / `HttpRequest(GET, "…")`
/// Returns the number of call sites emitted.
fn collect_client_endpoints_in(
    body: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> usize {
    let mut hits = 0;
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "call_expression"
            && let Some((method, raw, interpolated)) = client_call_candidate(n, src)
            && let Some(path) = url_to_path(&raw)
        {
            let pos = n.start_position();
            // A literal path with an explicit verb is Strong; an interpolated
            // path or a verb we had to infer is Medium (same rule as swift).
            let confidence = if interpolated {
                Confidence::Medium
            } else {
                Confidence::Strong
            };
            let ep = ClientEndpoint {
                method,
                path,
                file: file_rel.to_string(),
                line: pos.row + 1,
                col: pos.column + 1,
                confidence,
            };
            push_client_endpoint(
                repo,
                &ep,
                from,
                &mut acc.nodes,
                &mut acc.edges,
                &mut acc.nav,
                &mut acc.endpoint_seen,
            );
            hits += 1;
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            // A nested definition is visited (and attributed) on its own.
            if !matches!(
                child.kind(),
                "function_definition" | "class_definition" | "object_definition"
            ) {
                stack.push(child);
            }
        }
    }
    hits
}

/// `(method, raw_url, interpolated_or_inferred)` for a `call_expression` that is
/// a client HTTP call, else None.
fn client_call_candidate(n: TsNode, src: &[u8]) -> Option<(String, String, bool)> {
    let func = n.child_by_field_name("function")?;
    let args = n.child_by_field_name("arguments")?;
    match func.kind() {
        // Akka HTTP: `HttpRequest(…)` (also the inner call of `singleRequest(…)`).
        "identifier" if text_of(func, src) == "HttpRequest" => akka_http_request(args, src),
        "field_expression" => {
            let field = func
                .child_by_field_name("field")
                .map(|f| text_of(f, src))
                .unwrap_or("");
            if field == "url" {
                // Play WS: the URL is here, the verb is up the fluent chain.
                let (raw, interp) = first_string_arg(args, src)?;
                let (method, inferred) = match play_ws_verb(n, src) {
                    Some(v) => (v, false),
                    None => ("GET".to_string(), true),
                };
                Some((method, raw, interp || inferred))
            } else if let Some(verb) = http_verb(field) {
                // sttp: only a `uri"…"` interpolator argument counts, so a plain
                // `config.get("user." + id)` is not mistaken for an HTTP call.
                let arg = args.named_child(0)?;
                let is_uri = arg.kind() == "interpolated_string_expression"
                    && arg
                        .child_by_field_name("interpolator")
                        .map(|i| text_of(i, src))
                        == Some("uri");
                if !is_uri {
                    return None;
                }
                let (raw, interp) = scala_string_path(arg, src)?;
                Some((verb, raw, interp))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Walk UP a Play WS fluent chain from the `.url(…)` call to the verb call
/// (`.get()`, `.post(body)`, possibly past `.addHttpHeaders(…)` etc).
fn play_ws_verb(url_call: TsNode, src: &[u8]) -> Option<String> {
    let mut cur = url_call;
    for _ in 0..8 {
        let parent = cur.parent()?;
        if parent.kind() != "field_expression"
            || !parent
                .child_by_field_name("value")
                .is_some_and(|v| v == cur)
        {
            return None;
        }
        let field = parent
            .child_by_field_name("field")
            .map(|f| text_of(f, src))
            .unwrap_or("");
        if let Some(verb) = http_verb(field) {
            return Some(verb);
        }
        let grand = parent.parent()?;
        if grand.kind() != "call_expression" {
            return None;
        }
        cur = grand;
    }
    None
}

/// Akka `HttpRequest(…)` arguments: named `uri =` / `method =`, or the
/// positional `HttpRequest(GET, "…")` form. Verb defaults to GET.
fn akka_http_request(args: TsNode, src: &[u8]) -> Option<(String, String, bool)> {
    let mut method: Option<String> = None;
    let mut url: Option<(String, bool)> = None;
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        let (key, value) = if arg.kind() == "assignment_expression" {
            let key = arg
                .child_by_field_name("left")
                .map(|l| text_of(l, src))
                .unwrap_or("");
            match arg.child_by_field_name("right") {
                Some(right) => (key, right),
                None => continue,
            }
        } else {
            ("", arg)
        };
        if (key == "method" || key.is_empty())
            && method.is_none()
            && let Some(verb) = verb_of_expr(value, src)
        {
            method = Some(verb);
            continue;
        }
        if (key == "uri" || key.is_empty())
            && url.is_none()
            && let Some(s) = scala_string_path(value, src)
        {
            url = Some(s);
        }
    }
    let (raw, interp) = url?;
    let inferred = method.is_none();
    Some((method.unwrap_or_else(|| "GET".to_string()), raw, interp || inferred))
}

/// `GET` (identifier) or `HttpMethods.POST` (field_expression) → the verb.
fn verb_of_expr(n: TsNode, src: &[u8]) -> Option<String> {
    let text = match n.kind() {
        "identifier" => text_of(n, src),
        "field_expression" => n
            .child_by_field_name("field")
            .map(|f| text_of(f, src))
            .unwrap_or(""),
        _ => return None,
    };
    http_verb(text)
}

/// First string-ish argument of an `arguments` node.
fn first_string_arg(args: TsNode, src: &[u8]) -> Option<(String, bool)> {
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .find_map(|a| scala_string_path(a, src))
}

/// Reconstruct a scala `string` or `interpolated_string_expression`, replacing
/// every `$id` / `${expr}` with `${…}` so it normalises like a TS template path
/// (`normalise_http_path` collapses any segment containing `${` to `{}`).
/// Returns `(text_without_quotes, had_interpolation)`.
fn scala_string_path(n: TsNode, src: &[u8]) -> Option<(String, bool)> {
    match n.kind() {
        "string" => Some((text_of(n, src).trim_matches('"').to_string(), false)),
        "interpolated_string_expression" => {
            let mut cursor = n.walk();
            let lit = n
                .named_children(&mut cursor)
                .find(|c| c.kind() == "interpolated_string")?;
            let text = text_of(lit, src);
            let base = lit.start_byte();
            let mut out = String::new();
            let mut at = 0usize;
            let mut interpolated = false;
            let mut inner = lit.walk();
            for child in lit.named_children(&mut inner) {
                if child.kind() != "interpolation" {
                    continue;
                }
                let (start, end) = (
                    child.start_byte().saturating_sub(base),
                    child.end_byte().saturating_sub(base),
                );
                if start < at {
                    continue;
                }
                out.push_str(text.get(at..start).unwrap_or(""));
                out.push_str("${…}");
                at = end;
                interpolated = true;
            }
            out.push_str(text.get(at..).unwrap_or(""));
            Some((out.trim_matches('"').to_string(), interpolated))
        }
        _ => None,
    }
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

// ============================================================================
// Test-case registrations (DG.11): ScalaTest / MUnit cases -> FUNCTION nodes
// ============================================================================
//
// A ScalaTest or MUnit case is a registration in the suite's class body, not
// a def, so the body walk above attributes its calls to nothing:
//
//   FlatSpec     "X" should "y" in { .. }     it should "z" in { .. }
//                behavior of "X"              "X" should "y" taggedAs(T) in { .. }
//   FunSuite     test("y") { .. }             (MUnit's FunSuite the same)
//   WordSpec     "X" when { "y" should { "z" in { .. } } }
//   FreeSpec     "X" - { "y" in { .. } }
//   FunSpec      describe("X") { it("y") { .. } }
//   PropSpec     property("y") { .. }
//   FeatureSpec  Feature("X") { Scenario("y") { .. } }
//
// Each case becomes a FUNCTION `<suite qname>::<description>` under the
// suite, holding the calls of its body, so CL.5b's function-level TESTS pass
// (engine/src/passes.rs `tests_fn_edges`) has a root to start from. Only a
// suite's body is read (its extends clause names a [`SCALA_TEST_BASES`] type
// by its last segment), so a `"a" in b` in production code is never a case.
// Known limit: a suite extending an in-house base (`class S extends BaseSpec`)
// is not recognised unless the base is in the list.
//
// fired_on, once per file with a suite:
//   `[scala-tests] suites=<s> cases=<c> containers=<k> path=<file>`
//   `glia analyze <repo> 2>&1 | grep '\[scala-tests\]'`

/// The suite types whose body holds case registrations, matched by the last
/// segment of an extends / with type (`org.scalatest.flatspec.AnyFlatSpec`,
/// `munit.FunSuite`).
const SCALA_TEST_BASES: &[&str] = &[
    // ScalaTest 3.1+ styles, their async twins and the `*Like` traits a
    // TestKit suite mixes in.
    "AnyFlatSpec",
    "AnyFunSuite",
    "AnyWordSpec",
    "AnyFreeSpec",
    "AnyFunSpec",
    "AnyPropSpec",
    "AnyFeatureSpec",
    "AsyncFlatSpec",
    "AsyncFunSuite",
    "AsyncWordSpec",
    "AsyncFreeSpec",
    "AsyncFunSpec",
    "AsyncPropSpec",
    "AsyncFeatureSpec",
    "AnyFlatSpecLike",
    "AnyFunSuiteLike",
    "AnyWordSpecLike",
    "AnyFreeSpecLike",
    "AnyFunSpecLike",
    "AnyPropSpecLike",
    "AnyFeatureSpecLike",
    // Pre-3.1 ScalaTest; `FunSuite` is also MUnit's.
    "FlatSpec",
    "FunSuite",
    "WordSpec",
    "FreeSpec",
    "FunSpec",
    "PropSpec",
    "FeatureSpec",
    // MUnit.
    "ScalaCheckSuite",
    "CatsEffectSuite",
];

/// The verbs a description chain joins (`"X" should "y"`) and a WordSpec
/// container opens (`"X" when { .. }`).
const CASE_VERBS: &[&str] = &["should", "must", "can", "when", "which", "that"];

/// Calls that register one case: `test("y") { .. }`, `it("y") { .. }`.
const CASE_CALLS: &[&str] = &[
    "test", "it", "they", "ignore", "property", "scenario", "Scenario",
];

/// Calls whose block holds cases: `describe("X") { .. }`, `Feature("X") { .. }`.
const CONTAINER_CALLS: &[&str] = &["describe", "Feature", "feature"];

/// The longest case description kept, in chars.
const CASE_NAME_MAX: usize = 120;

/// Per-file counts behind the `[scala-tests]` marker.
#[derive(Debug, Default, PartialEq, Eq)]
struct TestTally {
    /// Classes / objects / traits whose extends clause names a suite base.
    suites: usize,
    /// FUNCTION nodes minted, one per case registration.
    cases: usize,
    /// WordSpec / FreeSpec / FunSpec / FeatureSpec blocks that prefix cases.
    containers: usize,
}

impl TestTally {
    fn marker(&self, file_rel: &str) -> String {
        format!(
            "[scala-tests] suites={} cases={} containers={} path={file_rel}",
            self.suites, self.cases, self.containers
        )
    }
}

/// Does the type's extends clause name a [`SCALA_TEST_BASES`] type?
fn is_test_suite(type_node: TsNode, src: &[u8]) -> bool {
    let Some(ext) = type_node.child_by_field_name("extend") else {
        return false;
    };
    let mut cursor = ext.walk();
    ext.children_by_field_name("type", &mut cursor).any(|ty| {
        let raw = text_of(ty, src);
        let base = raw.split('[').next().unwrap_or(raw).trim();
        let simple = base.rsplit('.').next().unwrap_or(base).trim();
        SCALA_TEST_BASES.contains(&simple)
    })
}

/// The suite whose body is being read, and the case names it already holds.
struct Suite<'a> {
    src: &'a [u8],
    file_rel: &'a str,
    qname: &'a str,
    id: NodeId,
    repo: RepoId,
    names: std::collections::HashSet<String>,
}

impl<'a> Suite<'a> {
    fn new(src: &'a [u8], file_rel: &'a str, qname: &'a str, id: NodeId, repo: RepoId) -> Self {
        Suite {
            src,
            file_rel,
            qname,
            id,
            repo,
            names: std::collections::HashSet::new(),
        }
    }

    /// A free name for `base` in this suite: `base`, then `base #2`, `#3`, ..
    /// (ScalaTest rejects a duplicate name, but a nested prefix can collide).
    fn claim(&mut self, base: String) -> String {
        let mut name = base.clone();
        let mut k = 1usize;
        while !self.names.insert(name.clone()) {
            k += 1;
            name = format!("{base} #{k}");
        }
        name
    }
}

/// Read every statement of `parent` (a suite body or a container's block) as
/// a possible registration, in tree order. `prefix` is the enclosing
/// containers' text. The FlatSpec subject (`behavior of "X"`, or the string a
/// `"X" should "y" in` chain starts with) names a later `it` / `they`.
fn visit_test_registrations(parent: TsNode, prefix: &str, suite: &mut Suite, acc: &mut Acc) {
    let mut subject: Option<String> = None;
    let mut cursor = parent.walk();
    for stmt in parent.named_children(&mut cursor) {
        visit_test_registration(stmt, prefix, &mut subject, suite, acc);
    }
}

fn visit_test_registration(
    stmt: TsNode,
    prefix: &str,
    subject: &mut Option<String>,
    suite: &mut Suite,
    acc: &mut Acc,
) {
    let src = suite.src;
    match stmt.kind() {
        "infix_expression" => {
            let (Some(left), Some(op), Some(right)) = (
                stmt.child_by_field_name("left"),
                stmt.child_by_field_name("operator"),
                stmt.child_by_field_name("right"),
            ) else {
                return;
            };
            let op = text_of(op, src);
            if matches!(op, "in" | "ignore") && right.kind() == "block" {
                // A case: `<description chain> in { .. }`.
                if let Some(text) = case_chain_text(left, src, subject.as_deref()) {
                    if let Some(head) = chain_subject(left, src) {
                        *subject = Some(head);
                    }
                    emit_test_case(stmt, right, &joined(prefix, &text), suite, acc);
                }
            } else if op == "of" && left.kind() == "identifier" && text_of(left, src) == "behavior"
            {
                // FlatSpec `behavior of "X"`: the subject of later `it`s.
                if let Some(s) = string_content(right, src) {
                    *subject = Some(s);
                }
            } else if (op == "-" || CASE_VERBS.contains(&op)) && right.kind() == "block" {
                // A container: `"X" should { .. }`, FreeSpec `"X" - { .. }`.
                let Some(text) = string_content(left, src) else {
                    return;
                };
                let text = if op == "-" {
                    text
                } else {
                    format!("{text} {op}")
                };
                acc.tests.containers += 1;
                visit_test_registrations(right, &joined(prefix, &text), suite, acc);
            }
        }
        "call_expression" => {
            // `name("desc", ..) { body }`: an outer call whose function is the
            // inner call naming the registration.
            let (Some(inner), Some(body)) = (
                stmt.child_by_field_name("function"),
                stmt.child_by_field_name("arguments"),
            ) else {
                return;
            };
            if inner.kind() != "call_expression" {
                return;
            }
            let (Some(callee), Some(args)) = (
                inner.child_by_field_name("function"),
                inner.child_by_field_name("arguments"),
            ) else {
                return;
            };
            if callee.kind() != "identifier" {
                return;
            }
            let callee = text_of(callee, src);
            let mut ac = args.walk();
            let Some(text) = args
                .named_children(&mut ac)
                .next()
                .and_then(|a| case_arg_name(a, src))
            else {
                return;
            };
            if CASE_CALLS.contains(&callee) && matches!(body.kind(), "block" | "arguments") {
                emit_test_case(stmt, body, &joined(prefix, &text), suite, acc);
            } else if CONTAINER_CALLS.contains(&callee) && body.kind() == "block" {
                acc.tests.containers += 1;
                visit_test_registrations(body, &joined(prefix, &text), suite, acc);
            }
        }
        _ => {}
    }
}

/// The text of a case's description chain: string contents and verbs joined
/// by spaces (`"X" should "y"` -> `X should y`); `it` / `they` reads as the
/// FlatSpec subject when there is one; a `taggedAs(..)` tail is dropped. Any
/// other shape is not a description: None.
fn case_chain_text(n: TsNode, src: &[u8], subject: Option<&str>) -> Option<String> {
    match n.kind() {
        "string" | "interpolated_string_expression" => string_content(n, src),
        "identifier" => {
            let word = text_of(n, src);
            matches!(word, "it" | "they").then(|| subject.unwrap_or(word).to_string())
        }
        "infix_expression" => {
            let op = text_of(n.child_by_field_name("operator")?, src);
            let left = case_chain_text(n.child_by_field_name("left")?, src, subject)?;
            if op == "taggedAs" {
                return Some(left);
            }
            if !CASE_VERBS.contains(&op) {
                return None;
            }
            let right = string_content(n.child_by_field_name("right")?, src)?;
            Some(format!("{left} {op} {right}"))
        }
        _ => None,
    }
}

/// The string a verb chain starts with (`"X" should "y"` -> `X`): the
/// FlatSpec subject a later `it should ..` takes. A bare `"y"` (WordSpec's
/// leaf) and an `it` chain set none.
fn chain_subject(n: TsNode, src: &[u8]) -> Option<String> {
    let mut head = n;
    let mut verb = false;
    while head.kind() == "infix_expression" {
        verb |= head
            .child_by_field_name("operator")
            .is_some_and(|op| CASE_VERBS.contains(&text_of(op, src)));
        head = head.child_by_field_name("left")?;
    }
    verb.then(|| string_content(head, src)).flatten()
}

/// The description a registration call's first argument names: a string, or
/// MUnit's `"y".ignore` / `"y".tag(Slow)` around one.
fn case_arg_name(n: TsNode, src: &[u8]) -> Option<String> {
    let mut cur = n;
    for _ in 0..8 {
        cur = match cur.kind() {
            "string" | "interpolated_string_expression" => return string_content(cur, src),
            "field_expression" => cur.child_by_field_name("value")?,
            "call_expression" => cur.child_by_field_name("function")?,
            _ => return None,
        };
    }
    None
}

/// The text inside a string literal (`"y"`, `"""y"""`, `s"y $x"`).
fn string_content(n: TsNode, src: &[u8]) -> Option<String> {
    let lit = match n.kind() {
        "string" => n,
        "interpolated_string_expression" => {
            let mut cursor = n.walk();
            n.named_children(&mut cursor)
                .find(|c| c.kind() == "interpolated_string")?
        }
        _ => return None,
    };
    let text = text_of(lit, src);
    let inner = text
        .strip_prefix("\"\"\"")
        .and_then(|t| t.strip_suffix("\"\"\""))
        .or_else(|| text.strip_prefix('"').and_then(|t| t.strip_suffix('"')))?;
    Some(inner.to_string())
}

/// `prefix text`, or `text` alone under no container.
fn joined(prefix: &str, text: &str) -> String {
    if prefix.is_empty() {
        text.to_string()
    } else {
        format!("{prefix} {text}")
    }
}

/// A description as a qname segment: whitespace collapsed, `::` read as `:`,
/// at most [`CASE_NAME_MAX`] chars. Empty is no name.
fn case_name(raw: &str) -> Option<String> {
    let mut name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    while name.contains("::") {
        name = name.replace("::", ":");
    }
    let name: String = name.chars().take(CASE_NAME_MAX).collect();
    let name = name.trim_end();
    (!name.is_empty()).then(|| name.to_string())
}

/// Mint the FUNCTION `<suite>::<description>` for one registration and
/// collect the calls of its body. A fixture case `{ fx => .. }` holds its
/// calls in the lambda, which [`collect_calls_in`] does not enter.
fn emit_test_case(stmt: TsNode, body: TsNode, description: &str, suite: &mut Suite, acc: &mut Acc) {
    let Some(base) = case_name(description) else {
        return;
    };
    let name = suite.claim(base);
    let qname = format!("{}::{name}", suite.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, suite.repo, node_kind::FUNCTION, &qname);
    acc.nodes.push(Node {
        id,
        repo: suite.repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&stmt, suite.src, suite.file_rel),
    });
    acc.edges.push(Edge {
        from: suite.id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, &name, &qname, node_kind::FUNCTION, Some(suite.id));
    acc.tests.cases += 1;

    let mut cursor = body.walk();
    let stmts: Vec<TsNode> = body
        .named_children(&mut cursor)
        .filter(|c| !matches!(c.kind(), "comment" | "block_comment"))
        .collect();
    match stmts.as_slice() {
        [lambda] if lambda.kind() == "lambda_expression" => {
            let params = lambda.child_by_field_name("parameters");
            let mut lc = lambda.walk();
            for part in lambda.named_children(&mut lc) {
                if Some(part) != params {
                    // In a suite body: a bare `helper()` is the suite's own.
                    collect_calls_in(part, suite.src, id, true, acc);
                }
            }
        }
        _ => collect_calls_in(body, suite.src, id, true, acc),
    }
}

// ============================================================================
// Scala route extraction (v0.4.11a R-scala; DG.10)
// ============================================================================
//
// Coverage (text scan for the sites, tree-sitter for the http4s handler):
//
//   Akka / Pekko HTTP:  path("users") { ... }                   -> ANY /users
//                       path("ws" / "chat") { ... }             -> ANY /ws/chat
//                       pathPrefix("api") { path("u" / IntNumber) { .. } }
//                                                               -> ANY /api/u/:id
//   http4s:             case GET -> Root / "users" => list()    -> GET /users
//                       case POST -> Root / "u" / IntVar(id)    -> POST /u/:id
//
// An http4s ROUTE also gets a HANDLED_BY ref naming what its case body calls
// (`=> getUser(id)`); an Akka route's handler is an inline directive block, so
// it gets none.
//
// Play Framework's primary routing is a `conf/routes` file (not Scala source)
// parsed by sbt compiler — out of scope for this parser. Play annotation
// forms are rare in practice.
//
// fired_on, once per file with a route:
//   `[scala-routes] routes=<n> handled=<h> akka_composed=<c> path=<file>`
//   `glia analyze <repo> 2>&1 | grep '\[scala-routes\]'`

/// Per-file route counts behind the `[scala-routes]` marker.
#[derive(Debug, Default, PartialEq, Eq)]
struct RouteTally {
    /// Distinct ROUTE nodes the file minted.
    routes: usize,
    /// HANDLED_BY refs emitted from an http4s case body.
    handled: usize,
    /// Distinct Akka ROUTEs whose path took more than one matcher segment or
    /// an enclosing `pathPrefix` block.
    akka_composed: usize,
}

impl RouteTally {
    fn marker(&self, file_rel: &str) -> String {
        format!(
            "[scala-routes] routes={} handled={} akka_composed={} path={file_rel}",
            self.routes, self.handled, self.akka_composed
        )
    }
}

fn scan_scala_routes(
    source: &str,
    root: TsNode,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) -> RouteTally {
    let mut seen = std::collections::HashSet::new();
    let mut tally = RouteTally::default();

    // Akka / Pekko HTTP `path(<matcher>) {` under its enclosing `pathPrefix`
    // blocks — emit ANY <prefixes>/<segments>. A matcher (or an enclosing
    // prefix) this reader cannot spell mints nothing: no path beats a wrong one.
    if source.contains("path(") {
        let text = ScalaText::new(source);
        let prefixes = text.prefix_blocks();
        for site in text.directive_sites("path(") {
            let Some(mut segments) = akka_prefixes(&prefixes, site) else {
                continue;
            };
            let composed = !segments.is_empty();
            let Some(own) = text
                .call_args(site + "path".len())
                .and_then(|(open, close)| akka_matcher_path(&source[open + 1..close]))
            else {
                continue;
            };
            let composed = composed || own.len() > 1;
            segments.extend(own);
            let before = seen.len();
            emit_scala_route("ANY", &format!("/{}", segments.join("/")), repo, acc, &mut seen);
            if composed && seen.len() > before {
                tally.akka_composed += 1;
            }
        }
    }

    // http4s pattern `case METHOD -> Root / "seg" ...` — split by method
    // and accumulate segments; the case body names the handler.
    let src = source.as_bytes();
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        let needle = format!("{method} -> Root");
        let mut idx = 0;
        while let Some(pos) = source[idx..].find(&needle) {
            let site = idx + pos;
            let start = site + needle.len();
            let after = &source[start..];
            let path = collect_http4s_path(after);
            // Bare `METHOD -> Root` with no segments = root path.
            let path = if path.is_empty() { "/".to_string() } else { path };
            let route_id = emit_scala_route(method, &path, repo, acc, &mut seen);
            if let Some((qualifier, line)) = http4s_handler(root, src, site, method.len()) {
                let duplicate = acc.refs.iter().any(|r| {
                    r.from == route_id
                        && r.category == edge_category::HANDLED_BY
                        && r.qualifier == qualifier
                });
                if !duplicate {
                    acc.refs.push(UnresolvedRef {
                        from: route_id,
                        from_module: module_id,
                        qualifier,
                        category: edge_category::HANDLED_BY,
                        line,
                    });
                    tally.handled += 1;
                }
            }
            idx = start;
        }
    }
    tally.routes = seen.len();
    tally
}

fn collect_http4s_path(after: &str) -> String {
    // Consume a sequence of `/ "segment"` or `/ IntVar(id)` tokens.
    let mut out = String::new();
    let mut rest = after;
    loop {
        let trimmed = rest.trim_start();
        let Some(slash_off) = trimmed.strip_prefix('/') else { break };
        let next = slash_off.trim_start();
        if let Some(lit_rest) = next.strip_prefix('"')
            && let Some(end) = lit_rest.find('"')
        {
            out.push('/');
            out.push_str(&lit_rest[..end]);
            rest = &lit_rest[end + 1..];
            continue;
        }
        // Segment variable: IntVar / LongVar / UUIDVar → treat as :id
        let next_bytes = next.as_bytes();
        if next_bytes.first().is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') {
            let end = next
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(next.len());
            let ident = &next[..end];
            // Common http4s Var extractors — inject :id placeholder.
            if matches!(
                ident,
                "IntVar" | "LongVar" | "UUIDVar" | "IntPathVar" | "LongPathVar"
            ) {
                out.push_str("/:id");
                // Skip the `(id)` call tail if present.
                let tail = &next[end..];
                let tail = tail.trim_start();
                rest = if let Some(after_paren) = tail.strip_prefix('(')
                    && let Some(cp) = after_paren.find(')')
                {
                    &after_paren[cp + 1..]
                } else {
                    tail
                };
                continue;
            }
            // Unknown identifier — treat as dynamic segment.
            out.push_str("/:");
            out.push_str(ident);
            rest = &next[end..];
            continue;
        }
        break;
    }
    out
}

/// The handler an http4s `case METHOD -> Root / .. => body` names, for the
/// route site whose METHOD token starts at byte `at` (`len` bytes): the
/// qualifier of the HANDLED_BY ref and the case's 0-based row. The site must
/// sit in the pattern of its `case_clause` (a commented-out site inside another
/// case's body names nothing). The body's value — its last statement — must be
/// a call:
///   `getUser(id)`                     -> the enclosing type's own method, or Bare
///   `svc.find(id).flatMap(Ok(_))`     -> Attribute { svc, find } (the chain's head)
///   `typed[IO](x)`                    -> as `typed(x)`
///   `Ok(..)`, `Created(..)`, a block, a `for`, a match -> None
/// A bare `f` (or `this.f`) is `Attribute { base: T, name: f }` when T, the
/// nearest enclosing object / class / trait, declares `def f` in this file, so
/// resolve_refs binds T's own method (CA.5a, `module_type_method`) and a
/// same-named function elsewhere cannot take it; otherwise it stays `Bare(f)`.
fn http4s_handler(root: TsNode, src: &[u8], at: usize, len: usize) -> Option<(CallQualifier, u32)> {
    let mut n = root.descendant_for_byte_range(at, at + len)?;
    let case = loop {
        if n.kind() == "case_clause" {
            break n;
        }
        n = n.parent()?;
    };
    let pattern = case.child_by_field_name("pattern")?;
    if at < pattern.start_byte() || at >= pattern.end_byte() {
        return None;
    }
    let mut cursor = case.walk();
    let body = case.children_by_field_name("body", &mut cursor).last()?;
    let qualifier = match handler_callee(body, src)? {
        (Some(base), name) if base != "this" => CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        },
        (_, name) => match enclosing_type_declaring(case, src, name) {
            Some(ty) => CallQualifier::Attribute {
                base: ty.to_string(),
                name: name.to_string(),
            },
            None => CallQualifier::Bare(name.to_string()),
        },
    };
    Some((qualifier, line_at(case)))
}

/// `(receiver, callee)` of the call a case body evaluates to: `f(..)` ->
/// `(None, f)`, `a.f(..)` -> `(Some(a), f)`, and a receiver chain
/// `a.f(..).g(..)` reads its head call `a.f`. A callee starting with an upper
/// case letter (an http4s status constructor `Ok` / `Created` / `NotFound`, a
/// companion `apply`) or any receiver that is not an identifier is None.
fn handler_callee<'a>(expr: TsNode<'a>, src: &'a [u8]) -> Option<(Option<&'a str>, &'a str)> {
    let mut cur = expr;
    loop {
        if cur.kind() != "call_expression" {
            return None;
        }
        let mut func = cur.child_by_field_name("function")?;
        if func.kind() == "generic_function" {
            func = func.child_by_field_name("function")?;
        }
        let (base, name) = match func.kind() {
            "identifier" => (None, text_of(func, src)),
            "field_expression" => {
                let value = func.child_by_field_name("value")?;
                match value.kind() {
                    "call_expression" => {
                        cur = value;
                        continue;
                    }
                    "identifier" => (
                        Some(text_of(value, src)),
                        text_of(func.child_by_field_name("field")?, src),
                    ),
                    _ => return None,
                }
            }
            _ => return None,
        };
        let lower = name
            .chars()
            .next()
            .is_some_and(|c| c.is_lowercase() || c == '_');
        return lower.then_some((base, name));
    }
}

/// The name of the nearest object / class / trait enclosing `node`, iff its
/// own body declares `def <method>`.
fn enclosing_type_declaring<'a>(node: TsNode<'a>, src: &'a [u8], method: &str) -> Option<&'a str> {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if matches!(n.kind(), "object_definition" | "class_definition" | "trait_definition") {
            let body = n.child_by_field_name("body")?;
            let mut cursor = body.walk();
            let declares = body.named_children(&mut cursor).any(|m| {
                m.kind() == "function_definition"
                    && m.child_by_field_name("name").map(|id| text_of(id, src)) == Some(method)
            });
            return declares
                .then(|| n.child_by_field_name("name").map(|id| text_of(id, src)))
                .flatten();
        }
        cur = n.parent();
    }
    None
}

// ---------------------------------------------------------------------------
// Akka / Pekko HTTP path directives
// ---------------------------------------------------------------------------

/// Byte classes of a Scala source: what is code, what sits in a string /
/// char literal, and what is a comment. The Akka scan finds directives, parens
/// and braces in code bytes only, so a `{` in a string or a commented-out
/// `path("old")` never counts.
const CODE: u8 = 0;
const LITERAL: u8 = 1;
const COMMENT: u8 = 2;

struct ScalaText<'a> {
    src: &'a [u8],
    class: Vec<u8>,
}

impl<'a> ScalaText<'a> {
    fn new(source: &'a str) -> Self {
        let src = source.as_bytes();
        let mut class = vec![CODE; src.len()];
        let mut i = 0;
        while i < src.len() {
            let rest = &src[i..];
            let (end, kind) = if rest.starts_with(b"//") {
                // Line comment: to the end of the line.
                let len = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
                (i + len, COMMENT)
            } else if rest.starts_with(b"/*") {
                // Block comment: Scala's nest; an unclosed one runs to the end.
                let mut depth = 0usize;
                let mut j = i;
                while j < src.len() {
                    if src[j..].starts_with(b"/*") {
                        depth += 1;
                        j += 2;
                    } else if src[j..].starts_with(b"*/") {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                (j, COMMENT)
            } else if rest.starts_with(b"\"\"\"") {
                // Triple-quoted string: no escapes, ends at the next `"""`.
                let end = rest[3..]
                    .windows(3)
                    .position(|w| w == b"\"\"\"")
                    .map_or(src.len(), |p| i + 3 + p + 3);
                (end, LITERAL)
            } else if rest[0] == b'"' {
                // Single-line string (plain or interpolated): an unescaped
                // `"` or the end of the line closes it.
                let mut j = i + 1;
                while j < src.len() && src[j] != b'"' && src[j] != b'\n' {
                    j += if src[j] == b'\\' { 2 } else { 1 };
                }
                (j + 1, LITERAL)
            } else if let Some(len) = (rest[0] == b'\'').then(|| char_literal_len(rest)).flatten() {
                // A char literal `'a'` / `'\n'`; a `'sym` or a lone quote is code.
                (i + len, LITERAL)
            } else {
                i += 1;
                continue;
            };
            let end = end.clamp(i + 1, src.len());
            class[i..end].fill(kind);
            i = end;
        }
        ScalaText { src, class }
    }

    fn is_code(&self, at: usize) -> bool {
        self.class.get(at) == Some(&CODE)
    }

    /// Code offsets of `needle` (`path(`, `pathPrefix(`) that start at an
    /// identifier boundary, so `subpath(` and a user's `filepath(` are not
    /// directives.
    fn directive_sites(&self, needle: &str) -> Vec<usize> {
        let n = needle.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(p) = self.src.get(i..).and_then(|s| s.windows(n.len()).position(|w| w == n)) {
            let at = i + p;
            let boundary = at == 0 || !is_ident_byte(self.src[at - 1]);
            if boundary && self.is_code(at) {
                out.push(at);
            }
            i = at + 1;
        }
        out
    }

    /// The `(open, close)` paren offsets of the call whose `(` is at `open`.
    fn call_args(&self, open: usize) -> Option<(usize, usize)> {
        (self.src.get(open) == Some(&b'(')).then_some(())?;
        self.matching(open, b'(', b')').map(|close| (open, close))
    }

    /// The offset of the code `close` byte balancing the code `open` byte at `at`.
    fn matching(&self, at: usize, open: u8, close: u8) -> Option<usize> {
        let mut depth = 0usize;
        for j in at..self.src.len() {
            if !self.is_code(j) {
                continue;
            }
            if self.src[j] == open {
                depth += 1;
            } else if self.src[j] == close {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(j);
                }
            }
        }
        None
    }

    /// The first offset at or after `at` that is neither whitespace nor comment.
    fn skip_blank(&self, mut at: usize) -> usize {
        while at < self.src.len()
            && (self.class[at] == COMMENT || self.src[at].is_ascii_whitespace())
        {
            at += 1;
        }
        at
    }

    /// Every `pathPrefix(<matcher>) { .. }` block of the file: the block's
    /// brace offsets and the matcher's segments (None when unreadable). The
    /// directive may be combined before its block, `(pathPrefix("api") & get) {`.
    fn prefix_blocks(&self) -> Vec<PrefixBlock> {
        let mut out = Vec::new();
        for site in self.directive_sites("pathPrefix(") {
            let Some((open, close)) = self.call_args(site + "pathPrefix".len()) else {
                continue;
            };
            let Some(brace) = self.block_after(close + 1) else {
                continue;
            };
            let Some(end) = self.matching(brace, b'{', b'}') else {
                continue;
            };
            let args = std::str::from_utf8(&self.src[open + 1..close]).ok();
            out.push(PrefixBlock {
                open: brace,
                close: end,
                segments: args.and_then(akka_matcher_path),
            });
        }
        out
    }

    /// The `{` opening the block a directive applies to, from just after its
    /// argument list: directly, or past `& <directive>[(..)]` conjunctions and
    /// the `)` closing the group they sit in. A `)` with no conjunction before
    /// it closes some other call's argument list: no block.
    fn block_after(&self, from: usize) -> Option<usize> {
        let mut at = self.skip_blank(from);
        let mut conjoined = false;
        loop {
            match self.src.get(at)? {
                b'{' => return Some(at),
                b')' if conjoined => at = self.skip_blank(at + 1),
                b'&' if self.src.get(at + 1) != Some(&b'&') => {
                    conjoined = true;
                    at = self.skip_blank(at + 1);
                    let start = at;
                    while at < self.src.len() && (is_ident_byte(self.src[at]) || self.src[at] == b'.') {
                        at += 1;
                    }
                    if at == start {
                        return None;
                    }
                    if self.src.get(at) == Some(&b'(') {
                        at = self.matching(at, b'(', b')')? + 1;
                    }
                    at = self.skip_blank(at);
                }
                _ => return None,
            }
        }
    }
}

/// One `pathPrefix(..) { .. }` block: `open` / `close` are its braces.
struct PrefixBlock {
    open: usize,
    close: usize,
    segments: Option<Vec<String>>,
}

/// The prefix segments of the `pathPrefix` blocks enclosing `site`, outer
/// first, or None when one of them has an unreadable matcher (the route's path
/// is then unknown).
fn akka_prefixes(blocks: &[PrefixBlock], site: usize) -> Option<Vec<String>> {
    let mut enclosing: Vec<&PrefixBlock> = blocks
        .iter()
        .filter(|b| b.open < site && site < b.close)
        .collect();
    enclosing.sort_by_key(|b| b.open);
    let mut out = Vec::new();
    for block in enclosing {
        out.extend(block.segments.clone()?);
    }
    Some(out)
}

/// The segments of one Akka path matcher — a `path(..)` / `pathPrefix(..)`
/// argument list — split at its top-level `/`:
///   a string literal                     -> its text (an inner `/` kept)
///   `Segment`                            -> `:segment`
///   `IntNumber` `LongNumber` `HexIntNumber` `HexLongNumber` `DoubleNumber`
///   `JavaUUID`                           -> `:id` (http4s `IntVar`'s spelling)
///   `Remaining` `RemainingPath`          -> `*` (kept literal by the HTTP resolver)
/// Any other token (a `val` matcher, `separateOnSlashes(..)`, `Slash`, an
/// interpolated string) makes the matcher unreadable: None. DG.5's Akka
/// WebSocket reader in the extractors crate spells paths with this same table.
fn akka_matcher_path(args: &str) -> Option<Vec<String>> {
    let text = ScalaText::new(args);
    let src = args.as_bytes();
    let mut parts: Vec<(usize, usize)> = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (j, &b) in src.iter().enumerate() {
        if !text.is_code(j) {
            continue;
        }
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'/' if depth == 0 => {
                parts.push((start, j));
                start = j + 1;
            }
            _ => {}
        }
    }
    parts.push((start, src.len()));
    let mut out = Vec::new();
    for (s, e) in parts {
        // Comments read as blanks; what is left must be one token. A byte maps
        // to one char, so a non-ASCII token reads garbled, and is rejected:
        // is_pathlike and the matcher names are ASCII.
        let token: String = (s..e)
            .map(|j| if text.class[j] == COMMENT { ' ' } else { char::from(src[j]) })
            .collect();
        let token = token.trim();
        let segment = if let Some(lit) = token
            .strip_prefix('"')
            .and_then(|t| t.strip_suffix('"'))
            .filter(|_| !token.starts_with("\"\"\""))
        {
            if lit.is_empty() || lit.len() > 256 || lit.contains(['"', '\\']) || !is_pathlike(lit) {
                return None;
            }
            // `path("/users/")` reads as `users`; a bare `"/"` adds nothing.
            let lit = lit.trim_matches('/');
            if lit.is_empty() {
                continue;
            }
            lit.to_string()
        } else {
            match token {
                "Segment" => ":segment".to_string(),
                "IntNumber" | "LongNumber" | "HexIntNumber" | "HexLongNumber" | "DoubleNumber"
                | "JavaUUID" => ":id".to_string(),
                "Remaining" | "RemainingPath" => "*".to_string(),
                _ => return None,
            }
        };
        out.push(segment);
    }
    Some(out)
}

/// The byte length of the char literal `rest` starts with (`'a'`, `'\n'`,
/// `'A'`, a multi-byte `'é'`), or None for a symbol / lone quote.
fn char_literal_len(rest: &[u8]) -> Option<usize> {
    if rest.get(1) == Some(&b'\\') {
        return rest
            .iter()
            .take(10)
            .skip(3)
            .position(|&b| b == b'\'')
            .map(|p| p + 4);
    }
    let lead = *rest.get(1)?;
    let width = match lead {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    (lead != b'\'' && lead != b'\n' && rest.get(1 + width) == Some(&b'\'')).then_some(width + 2)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_pathlike(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | ':' | '*'))
}

/// Mint the ROUTE `<method> <path>` once per file and return its id — also
/// when `seen` already holds it, so a second site still attaches its handler.
fn emit_scala_route(
    method: &str,
    path: &str,
    repo: RepoId,
    acc: &mut Acc,
    seen: &mut std::collections::HashSet<(String, String)>,
) -> NodeId {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    if !seen.insert((method.to_string(), path)) {
        return route_id;
    }
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
    route_id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    /// Sorted qnames of every nav-recorded node of `kind`.
    fn qnames_of(fp: &FileParse, kind: glia_core::NodeKindId) -> Vec<&str> {
        let mut out: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == kind)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).map(|s| s.as_str()))
            .collect();
        out.sort_unstable();
        out
    }

    /// LB.7a — top-level classes / objects / traits hang off the package
    /// (directory) scope, never the file module: no doubled `Widget::Widget`
    /// segment. The companion object folds into the class node (same qname,
    /// same kind); a Scala 3 top-level `def` keeps the file scope. Source is
    /// the `scala-package-qnames` fixture's.
    #[test]
    fn top_level_types_are_package_scoped() {
        let source = include_str!(
            "../../../../bench/substrate-gap/fixtures/scala-package-qnames/src/main/scala/shop/Widget.scala"
        );
        let fp = parse_file(
            source,
            "src/main/scala/shop/Widget.scala",
            "src::main::scala::shop::Widget",
            repo(),
        )
        .unwrap();

        assert_eq!(
            qnames_of(&fp, node_kind::CLASS),
            vec!["src::main::scala::shop::Widget"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::INTERFACE),
            vec!["src::main::scala::shop::Gadget"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            // `Gadget::go` is an abstract `function_declaration`, which the
            // body walk does not emit (unchanged by LB.7a).
            vec![
                "src::main::scala::shop::Widget::helper",
                "src::main::scala::shop::Widget::make",
                "src::main::scala::shop::Widget::run",
            ]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::FUNCTION),
            vec!["src::main::scala::shop::Widget::topLevel"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::MODULE),
            vec!["src::main::scala::shop::Widget"]
        );

        let module_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::MODULE,
            "src::main::scala::shop::Widget",
        );
        let class_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::CLASS,
            "src::main::scala::shop::Widget",
        );
        assert_ne!(module_id, class_id, "type and file MODULE share a qname, not an id");
        assert_eq!(fp.nav.parent_of.get(&class_id), Some(&module_id));
        for member in ["run", "make"] {
            let id = NodeId::from_parts(
                GRAPH_TYPE,
                repo(),
                node_kind::METHOD,
                &format!("src::main::scala::shop::Widget::{member}"),
            );
            assert_eq!(
                fp.nav.parent_of.get(&id),
                Some(&class_id),
                "{member} must hang off the (companion-folded) CLASS node"
            );
        }
        let top_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "src::main::scala::shop::Widget::topLevel",
        );
        assert_eq!(fp.nav.parent_of.get(&top_id), Some(&module_id));
        assert!(
            fp.edges.iter().any(|e| e.from == module_id
                && e.to == class_id
                && e.category == edge_category::DEFINES),
            "DEFINES stays MODULE -> type"
        );
    }

    /// LB.7a — a nested type hangs off its outer type, not the package scope.
    #[test]
    fn nested_types_keep_their_outer_type_scope() {
        let source = r#"
object Outer {
  class Inner {
    def go(): Unit = {}
  }
  trait Port
}
"#;
        let fp = parse_file(source, "app/Outer.scala", "app::Outer", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::CLASS),
            vec!["app::Outer", "app::Outer::Inner"]
        );
        assert_eq!(qnames_of(&fp, node_kind::INTERFACE), vec!["app::Outer::Port"]);
        assert_eq!(qnames_of(&fp, node_kind::METHOD), vec!["app::Outer::Inner::go"]);
    }

    /// LB.7a — a file at the repo root has an empty package scope: its type's
    /// qname is the bare type name.
    #[test]
    fn root_level_file_types_have_bare_qnames() {
        let source = "class App {\n  def run(): Unit = {}\n}\n";
        let fp = parse_file(source, "App.scala", "App", repo()).unwrap();
        assert_eq!(qnames_of(&fp, node_kind::CLASS), vec!["App"]);
        assert_eq!(qnames_of(&fp, node_kind::METHOD), vec!["App::run"]);
        assert_eq!(qnames_of(&fp, node_kind::MODULE), vec!["App"]);
        assert_eq!(type_scope("App"), "");
        assert_eq!(type_scope("src::main::scala::shop::Widget"), "src::main::scala::shop");
        assert_eq!(scoped("", "App"), "App");
        assert_eq!(scoped("shop", "Widget"), "shop::Widget");
    }

    #[test]
    fn object_and_trait() {
        let source = r#"
trait UserService {
  def getUser(id: Int): User
}

object UserServiceImpl {
  def getUser(id: Int): User = {
    db.findById(id)
  }
}
"#;
        let fp = parse_file(source, "src/UserService.scala", "src::UserService", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::INTERFACE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
        // LB.7a: package-scoped, not `src::UserService::UserService`.
        assert_eq!(qnames_of(&fp, node_kind::INTERFACE), vec!["src::UserService"]);
        assert_eq!(qnames_of(&fp, node_kind::CLASS), vec!["src::UserServiceImpl"]);
    }

    #[test]
    fn class_with_methods() {
        let source = r#"
class Config {
  def load(): Map[String, String] = {
    readFile("config.yml")
  }
  def save(): Unit = {}
}
"#;
        let fp = parse_file(source, "src/Config.scala", "src::Config", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 2);
        // LB.7a: package-scoped, not `src::Config::Config::load`.
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec!["src::Config::load", "src::Config::save"]
        );
    }

    #[test]
    fn intra_object_call_is_self_method() {
        // `compute` calls sibling `helper` bare inside `object Calc`. The parser
        // must emit `SelfMethod("helper")` (not `Bare`) so `resolve_calls` binds
        // it against the enclosing type's methods (`class_methods[Calc]`).
        let source = r#"
object Calc {
  def helper(x: Int): Int = x + 1

  def compute(n: Int): Int = {
    helper(n) * 2
  }
}
"#;
        let fp = parse_file(source, "app.scala", "app::Calc", repo()).unwrap();
        assert!(
            fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::SelfMethod("helper".to_string())),
            "expected SelfMethod(\"helper\") CallSite from compute(): {:?}",
            fp.calls
        );
        assert!(
            !fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::Bare("helper".to_string())),
            "intra-object bare call must be SelfMethod, not Bare: {:?}",
            fp.calls
        );
    }

    #[test]
    fn top_level_call_is_bare() {
        // A bare call from a top-level `def` (module scope) stays `Bare` so it
        // binds against module-level symbols.
        let source = r#"
def helper(x: Int): Int = x + 1

def compute(n: Int): Int = {
  helper(n) * 2
}
"#;
        let fp = parse_file(source, "app.scala", "app", repo()).unwrap();
        assert!(
            fp.calls
                .iter()
                .any(|c| c.qualifier == CallQualifier::Bare("helper".to_string())),
            "expected Bare(\"helper\") CallSite from top-level compute(): {:?}",
            fp.calls
        );
    }

    #[test]
    fn class_extends_emits_inherits_from_ref() {
        // `class Dog extends Animal` must emit an UnresolvedRef with a
        // Bare("Animal") qualifier under INHERITS_FROM, from the CLASS node.
        // A `with` mixin trait becomes an IMPLEMENTS ref. resolve_refs binds
        // the bare type name to the trait/class node across the repo.
        let source = r#"
trait Animal {
  def speak(): String
}

trait Runnable {
  def run(): Unit
}

class Dog extends Animal with Runnable {
  def speak(): String = "woof"
  def run(): Unit = {}
}
"#;
        let fp = parse_file(source, "src/Animals.scala", "src::Animals", repo()).unwrap();

        let dog_id = fp
            .nav
            .name_by_id
            .iter()
            .find(|(_, n)| **n == "Dog")
            .map(|(id, _)| *id)
            .expect("Dog CLASS node recorded");

        // INHERITS_FROM ref: Dog → Animal (primary supertype).
        assert!(
            fp.refs.iter().any(|r| r.from == dog_id
                && r.category == edge_category::INHERITS_FROM
                && r.qualifier == CallQualifier::Bare("Animal".to_string())),
            "expected INHERITS_FROM Bare(\"Animal\") ref from Dog: {:?}",
            fp.refs
        );

        // IMPLEMENTS ref: Dog → Runnable (mixin trait after `with`).
        assert!(
            fp.refs.iter().any(|r| r.from == dog_id
                && r.category == edge_category::IMPLEMENTS
                && r.qualifier == CallQualifier::Bare("Runnable".to_string())),
            "expected IMPLEMENTS Bare(\"Runnable\") ref from Dog: {:?}",
            fp.refs
        );

        // from_module must be the module node (whose binding table resolves).
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::Animals");
        assert!(
            fp.refs
                .iter()
                .all(|r| r.category != edge_category::INHERITS_FROM || r.from_module == module_id),
            "heritage refs must carry the module node as from_module: {:?}",
            fp.refs
        );
    }

    #[test]
    fn imports() {
        let source = r#"
import scala.collection.mutable
import akka.actor.ActorSystem
"#;
        let fp = parse_file(source, "src/App.scala", "src::App", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
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
    fn akka_http_path_routes_emit() {
        let source = r#"
val route = path("users") {
  get {
    complete("ok")
  }
} ~ path("admin") {
  post {
    complete("ok")
  }
}
"#;
        let fp = parse_file(source, "src/Routes.scala", "src::Routes", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("ANY", "/admin")));
    }

    fn endpoint_names(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ENDPOINT)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn play_ws_url_get_emits_endpoint() {
        // The URL sits on `.url(…)` but the verb is further up the fluent
        // chain — getting that wrong emits every Play WS call as GET.
        let source = r#"
class ApiClient(ws: WSClient) {
  def fetchUser(id: String) = ws.url(s"http://users-svc/api/users/$id").get()
  def createUser(body: String) = ws.url("/api/users").post(body)
  def headers(body: String) = ws.url("/api/orders").addHttpHeaders("a" -> "b").put(body)
}
"#;
        let fp = parse_file(source, "client/ApiClient.scala", "client::ApiClient", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /api/users/${…}".to_string(),
                "POST /api/users".to_string(),
                "PUT /api/orders".to_string(),
            ],
            "nav: {:?}",
            fp.nav.name_by_id
        );

        // Each ENDPOINT carries a CALLS edge from the enclosing METHOD.
        let fetch = fp
            .nav
            .name_by_id
            .iter()
            .find(|(id, n)| *n == "fetchUser" && fp.nav.kind_by_id[id] == node_kind::METHOD)
            .map(|(id, _)| *id)
            .expect("fetchUser METHOD recorded");
        let ep = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            "endpoint:GET:/api/users/${…}",
        );
        assert!(
            fp.edges
                .iter()
                .any(|e| e.from == fetch && e.to == ep && e.category == edge_category::CALLS),
            "expected CALLS fetchUser -> ENDPOINT: {:?}",
            fp.edges
        );
    }

    #[test]
    fn sttp_uri_interpolation_becomes_wildcard() {
        // sttp puts the path in a `uri"…"` interpolator on the verb call; any
        // `${…}` segment must survive so it normalises against `/items/{id}`.
        let source = r#"
object Client {
  def list() = basicRequest.get(uri"http://orders-svc/api/orders")
  def one(id: String) = basicRequest.post(uri"/api/items/$id").body("x")
}
"#;
        let fp = parse_file(source, "client/Client.scala", "client::Client", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /api/orders".to_string(),
                "POST /api/items/${…}".to_string(),
            ],
            "nav: {:?}",
            fp.nav.name_by_id
        );
    }

    #[test]
    fn akka_http_request_named_and_positional() {
        // `HttpRequest(uri = …)` defaults to GET; `method = HttpMethods.POST`
        // and the positional `HttpRequest(GET, "…")` form set it explicitly.
        let source = r#"
object Client {
  def a() = Http().singleRequest(HttpRequest(uri = "http://svc/api/orders"))
  def b() = Http().singleRequest(HttpRequest(method = HttpMethods.POST, uri = "/api/orders"))
  def c() = Http().singleRequest(HttpRequest(GET, "/api/things"))
}
"#;
        let fp = parse_file(source, "client/Akka.scala", "client::Akka", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "GET /api/orders".to_string(),
                "GET /api/things".to_string(),
                "POST /api/orders".to_string(),
            ],
            "nav: {:?}",
            fp.nav.name_by_id
        );
    }

    #[test]
    fn scala_client_calls_emit_no_route() {
        // A client call site must never be mistaken for a server ROUTE — the
        // `scan_scala_routes` text scan is server-side only.
        let source = r#"
class ApiClient(ws: WSClient) {
  def fetchUser(id: String) = ws.url("/api/users").get()
}
"#;
        let fp = parse_file(source, "client/ApiClient.scala", "client::ApiClient", repo()).unwrap();
        assert_eq!(
            fp.nav
                .kind_by_id
                .values()
                .filter(|k| **k == node_kind::ROUTE)
                .count(),
            0,
            "client file emitted a ROUTE: {:?}",
            fp.nav.name_by_id
        );
        assert_eq!(endpoint_names(&fp), vec!["GET /api/users".to_string()]);
    }

    #[test]
    fn scala_non_url_string_is_dropped() {
        // `config.get("user." + id)` has a verb-shaped method name and a string
        // argument but is not an HTTP call; `ws.url("relative")` has no path.
        let source = r#"
class ApiClient(ws: WSClient, config: Config) {
  def cached(id: String) = config.get("user." + id)
  def bare() = ws.url("users-svc").get()
  def named() = client.post("/api/x")
}
"#;
        let fp = parse_file(source, "client/ApiClient.scala", "client::ApiClient", repo()).unwrap();
        assert!(
            endpoint_names(&fp).is_empty(),
            "non-HTTP calls produced ENDPOINTs: {:?}",
            endpoint_names(&fp)
        );
    }

    #[test]
    fn http4s_get_post_routes_emit() {
        let source = r#"
val service = HttpRoutes.of[IO] {
  case GET -> Root / "users" => Ok("list")
  case POST -> Root / "users" => Ok("created")
  case GET -> Root / "users" / IntVar(id) => Ok(s"user $id")
}
"#;
        let fp = parse_file(source, "src/Api.scala", "src::Api", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users/:id")));
    }

    /// Sorted bare type names of the INJECTS refs from the node named `from`.
    fn injects_from(fp: &FileParse, from: &str) -> Vec<String> {
        let ids: Vec<NodeId> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(_, n)| *n == from)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(
            ids.len(),
            1,
            "expected one node named {from}: {:?}",
            fp.nav.name_by_id
        );
        let mut v: Vec<String> = fp
            .refs
            .iter()
            .filter(|r| r.from == ids[0] && r.category == edge_category::INJECTS)
            .map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => n.clone(),
                other => panic!("INJECTS ref must be Bare: {other:?}"),
            })
            .collect();
        v.sort();
        v
    }

    fn injects_count(fp: &FileParse) -> usize {
        fp.refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .count()
    }

    #[test]
    fn scala_service_class_params_emit_injects_refs() {
        // A DI-named class injects every class-typed parameter; value types and
        // implicit evidence (`ExecutionContext`) are skipped, `pkg.Config` and
        // `Cache[F]` reduce to their simple head. Play's `@Inject()` form parses
        // its parameters into the annotation and must still be read.
        let source = r#"
class UserService[F[_]](repo: UserRepo, name: String, cfg: pkg.Config, cache: Cache[F])(implicit ec: ExecutionContext, ev: F[Int])

@Singleton
class HomeController @Inject() (cc: ControllerComponents, svc: UserService) extends AbstractController(cc)
"#;
        let fp = parse_file(source, "app/Svc.scala", "app::Svc", repo()).unwrap();
        assert_eq!(
            injects_from(&fp, "UserService"),
            vec!["Cache", "Config", "UserRepo"]
        );
        assert_eq!(
            injects_from(&fp, "HomeController"),
            vec!["ControllerComponents", "UserService"]
        );
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "app::Svc");
        assert!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::INJECTS)
                .all(|r| r.from_module == module_id),
            "INJECTS refs must carry the module node as from_module: {:?}",
            fp.refs
        );
    }

    #[test]
    fn scala_case_class_emits_no_injects() {
        // No DI suffix and no implicit list: a value object. A case class with a
        // DI-shaped name is still a value object, and an ungated plain class
        // with a class-typed parameter stays silent too.
        let source = r#"
class Currency
case class Money(amount: Int, currency: Currency)
case class OrderService(currency: Currency)
class Wallet(currency: Currency)
"#;
        let fp = parse_file(source, "app/Money.scala", "app::Money", repo()).unwrap();
        assert_eq!(
            injects_count(&fp),
            0,
            "value objects emitted INJECTS: {:?}",
            fp.refs
        );
    }

    #[test]
    fn scala_implicit_and_using_lists_emit_injects_refs() {
        // The context list is found among several `parameters` lists, the `[T]`
        // list's names are not dependencies, and an ungated class contributes
        // only its `using` list.
        let source = r#"
object Handlers {
  def render(id: Int)(implicit svc: UserService): String = svc.get(id)
  def show[T](x: T)(using repo: UserRepo, ev: T, ord: Ordering[T]): String = ""
  def plain(svc: UserService): String = ""
}
class Plain(x: Foo)(using db: Database)
def top(id: Int)(implicit clock: Clock): Int = id
"#;
        let fp = parse_file(source, "app/Handlers.scala", "app::Handlers", repo()).unwrap();
        assert_eq!(injects_from(&fp, "render"), vec!["UserService"]);
        assert_eq!(injects_from(&fp, "show"), vec!["UserRepo"]);
        assert!(injects_from(&fp, "plain").is_empty());
        assert_eq!(injects_from(&fp, "Plain"), vec!["Database"]);
        assert_eq!(injects_from(&fp, "top"), vec!["Clock"]);
    }

    #[test]
    fn scala_macwire_wire_emits_injects_ref() {
        // `wire[T]` injects into the enclosing object, once per type.
        let source = r#"
import com.softwaremill.macwire._

object AppWiring {
  lazy val users = wire[UserService]
  lazy val again: UserService = wire[UserService]
  val repo: UserRepo = wire[pkg.UserRepo]
  val n = List[Int](1)
}
"#;
        let fp = parse_file(source, "app/Wiring.scala", "app::Wiring", repo()).unwrap();
        assert_eq!(
            injects_from(&fp, "AppWiring"),
            vec!["UserRepo", "UserService"]
        );
        assert_eq!(injects_count(&fp), 2);
    }

    // ---- DG.10: http4s handlers and composed Akka paths ---------------------

    /// Sorted names of every ROUTE the file minted.
    fn route_names(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    /// The HANDLED_BY refs from the ROUTE `<method> <path>`, in emit order.
    fn handled_by(fp: &FileParse, method: &str, path: &str) -> Vec<(CallQualifier, u32)> {
        let id = route_id(method, path);
        fp.refs
            .iter()
            .filter(|r| r.from == id && r.category == edge_category::HANDLED_BY)
            .map(|r| (r.qualifier.clone(), r.line))
            .collect()
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute { base: base.to_string(), name: name.to_string() }
    }

    #[test]
    fn http4s_case_bodies_name_their_handlers() {
        // The matrix scala/http_server file: a bare `getUser(id)` names the
        // enclosing object's own method, as a type-qualified ref.
        let source = include_str!(
            "../../../../bench/substrate-gap/matrix/scala/http_server/server/UserRoutes.scala"
        );
        let fp = parse_file(source, "UserRoutes.scala", "UserRoutes", repo()).unwrap();
        assert_eq!(handled_by(&fp, "GET", "/users/:id"), vec![(attr("UserRoutes", "getUser"), 10)]);
        assert_eq!(handled_by(&fp, "POST", "/users"), vec![(attr("UserRoutes", "createUser"), 11)]);
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "UserRoutes");
        assert!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::HANDLED_BY)
                .all(|r| r.from_module == module_id),
            "HANDLED_BY refs carry the file MODULE: {:?}",
            fp.refs
        );

        let source = r#"
object Api {
  val routes = HttpRoutes.of[IO] {
    case GET -> Root / "ok" => Ok("x")
    case GET -> Root / "find" / id => svc.find(id).flatMap(Ok(_))
    case GET -> Root / "bare" => helper()
    case GET -> Root / "gen" => typed[IO](1)
    case GET -> Root / "self" => this.typed(2)
    case req @ POST -> Root / "multi" =>
      // case GET -> Root / "ghost" => ghost()
      log(req)
      Api.create(req)
    case GET -> Root / "for" => for { x <- y } yield x
    case GET -> Root / "status" => NotFound()
  }
  def typed[F[_]](x: Int) = ???
}
"#;
        let fp = parse_file(source, "src/Api.scala", "src::Api", repo()).unwrap();
        assert!(handled_by(&fp, "GET", "/ok").is_empty(), "`Ok(..)` is a status, not a handler");
        assert_eq!(handled_by(&fp, "GET", "/find/:id"), vec![(attr("svc", "find"), 4)]);
        // `Api` declares no `helper`: the ref stays Bare for the module lookup.
        assert_eq!(
            handled_by(&fp, "GET", "/bare"),
            vec![(CallQualifier::Bare("helper".to_string()), 5)]
        );
        assert_eq!(handled_by(&fp, "GET", "/gen"), vec![(attr("Api", "typed"), 6)]);
        assert_eq!(handled_by(&fp, "GET", "/self"), vec![(attr("Api", "typed"), 7)]);
        // A multi-statement body evaluates to its last statement.
        assert_eq!(handled_by(&fp, "POST", "/multi"), vec![(attr("Api", "create"), 8)]);
        // The commented-out site still mints its ROUTE (the text scan, as at
        // HEAD) but sits in another case's body, so it names no handler.
        assert!(route_names(&fp).contains(&"GET /ghost".to_string()));
        assert!(handled_by(&fp, "GET", "/ghost").is_empty());
        assert!(handled_by(&fp, "GET", "/for").is_empty());
        assert!(handled_by(&fp, "GET", "/status").is_empty());
        assert_eq!(
            fp.refs.iter().filter(|r| r.category == edge_category::HANDLED_BY).count(),
            5,
            "refs: {:?}",
            fp.refs
        );
    }

    #[test]
    fn akka_path_matchers_compose() {
        // The matrix scala/ws file: every segment, not the first literal.
        let source = include_str!(
            "../../../../bench/substrate-gap/matrix/scala/ws/server/ChatRoutes.scala"
        );
        let fp = parse_file(source, "ChatRoutes.scala", "ChatRoutes", repo()).unwrap();
        assert_eq!(route_names(&fp), vec!["ANY /ws/chat".to_string()]);

        let source = r#"
object Routes {
  val api = pathPrefix("api") {
    path("users" / IntNumber) { id =>
      get { complete(id.toString) }
    } ~
    pathPrefix("v1" / "admin") {
      path(Segment / "items") { name => complete(name) }
    }
  }
  val files = pathPrefix("files") {
    path(Remaining) { rest => complete(rest) }
  }
  val grouped = (pathPrefix("v2") & get) {
    path("ping") { complete("pong") }
  }
  val users = path(UsersPath) { complete("x") }
  val split = path(separateOnSlashes("a/b")) { complete("x") }
  val hidden = pathPrefix(ApiPrefix) {
    path("orders") { complete("x") }
  }
  val tricky = path("orders" /* the list */ / LongNumber) {
    val brace = "}"
    complete(brace)
  }
  // path("commented") { complete("x") }
  val text = "path(\"instring\")"
}
"#;
        let fp = parse_file(source, "src/Routes.scala", "src::Routes", repo()).unwrap();
        assert_eq!(
            route_names(&fp),
            vec![
                "ANY /api/users/:id".to_string(),
                "ANY /api/v1/admin/:segment/items".to_string(),
                "ANY /files/*".to_string(),
                "ANY /orders/:id".to_string(),
                "ANY /v2/ping".to_string(),
            ],
            "a val matcher, separateOnSlashes and an unreadable prefix mint nothing"
        );
        assert!(
            fp.refs.iter().all(|r| r.category != edge_category::HANDLED_BY),
            "an Akka route names no handler: {:?}",
            fp.refs
        );

        let tree = {
            let mut p = Parser::new();
            let lang: tree_sitter::Language = tree_sitter_scala::LANGUAGE.into();
            p.set_language(&lang).unwrap();
            p.parse(source, None).unwrap()
        };
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::Routes");
        let tally = scan_scala_routes(source, tree.root_node(), module_id, repo(), &mut Acc::default());
        assert_eq!(tally, RouteTally { routes: 5, handled: 0, akka_composed: 5 });
    }

    #[test]
    fn akka_matcher_table() {
        let read = |s: &str| akka_matcher_path(s).map(|v| v.join("/"));
        assert_eq!(read(r#""ws" / "chat""#).as_deref(), Some("ws/chat"));
        assert_eq!(read(r#""api/v1" / Segment"#).as_deref(), Some("api/v1/:segment"));
        for m in ["IntNumber", "LongNumber", "HexIntNumber", "HexLongNumber", "DoubleNumber", "JavaUUID"] {
            assert_eq!(read(&format!(r#""u" / {m}"#)).as_deref(), Some("u/:id"), "{m}");
        }
        assert_eq!(read("Remaining").as_deref(), Some("*"));
        assert_eq!(read(r#""f" / RemainingPath"#).as_deref(), Some("f/*"));
        assert_eq!(read(r#""/users/""#).as_deref(), Some("users"));
        for unreadable in [
            "UsersPath",
            "Slash",
            r#"separateOnSlashes("a/b")"#,
            r#"s"users/$id""#,
            r#""a" + "b""#,
            r#""""triple""""#,
            r#""has space""#,
            r#""""#,
            "",
            r#""u" / IntNumber.?"#,
        ] {
            assert_eq!(read(unreadable), None, "{unreadable}");
        }
    }

    #[test]
    fn subpath_is_not_a_directive() {
        let source = r#"
object Files {
  def filepath(name: String) = name
  val a = subpath("users") { complete("x") }
  val b = filepath("admin")
  val c = rawPathPrefix("raw") { path("x") { complete("y") } }
  val d = pathPrefix("only") { complete("prefix alone is no route") }
}
"#;
        let fp = parse_file(source, "src/Files.scala", "src::Files", repo()).unwrap();
        // `rawPathPrefix(` is not a `pathPrefix(` block, so `path("x")` is bare.
        assert_eq!(route_names(&fp), vec!["ANY /x".to_string()]);
    }

    #[test]
    fn route_dedup_still_returns_the_id() {
        let mut acc = Acc::default();
        let mut seen = std::collections::HashSet::new();
        let first = emit_scala_route("GET", "/users", repo(), &mut acc, &mut seen);
        let again = emit_scala_route("GET", "users", repo(), &mut acc, &mut seen);
        assert_eq!(first, route_id("GET", "/users"));
        assert_eq!(again, first);
        assert_eq!(acc.nodes.len(), 1);

        // Two HttpRoutes serving one route: one ROUTE, a ref per handler, and
        // a repeated (route, handler) pair once.
        let source = r#"
object Users {
  def listA() = ???
  def listB() = ???
  val a = HttpRoutes.of[IO] { case GET -> Root / "users" => listA() }
  val b = HttpRoutes.of[IO] { case GET -> Root / "users" => listB() }
  val c = HttpRoutes.of[IO] { case GET -> Root / "users" => listB() }
}
"#;
        let fp = parse_file(source, "src/Users.scala", "src::Users", repo()).unwrap();
        assert_eq!(route_names(&fp), vec!["GET /users".to_string()]);
        assert_eq!(
            handled_by(&fp, "GET", "/users"),
            vec![(attr("Users", "listA"), 4), (attr("Users", "listB"), 5)]
        );
    }

    #[test]
    fn truncated_route_sources_never_panic() {
        // Every prefix of a DSL-heavy file is a malformed file: unclosed
        // strings, comments, char literals, parens and blocks. None may panic.
        let source = "object R {\n  val r = pathPrefix(\"api\" /* c /* n */ */) {\n    path(\"a\" / 'x' / '\\n' / \"\"\"t\"\"\" / IntNumber) { _ => complete(\"}\") } ~\n    (pathPrefix(\"g\") & get) { path(\"é\") { complete(s\"${x}\") } }\n  }\n  val h = HttpRoutes.of[IO] { case GET -> Root / \"u\" / id => getUser(id) }\n  def getUser(id: String) = ???\n}\n";
        for end in (0..=source.len()).filter(|&e| source.is_char_boundary(e)) {
            let cut = &source[..end];
            let fp = parse_file(cut, "src/R.scala", "src::R", repo());
            assert!(fp.is_ok(), "parse failed at {end}");
        }
    }

    #[test]
    fn scala_routes_marker_counts() {
        let source = include_str!(
            "../../../../bench/substrate-gap/matrix/scala/http_server/server/UserRoutes.scala"
        );
        let mut p = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_scala::LANGUAGE.into();
        p.set_language(&lang).unwrap();
        let tree = p.parse(source, None).unwrap();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "UserRoutes");
        let tally = scan_scala_routes(source, tree.root_node(), module_id, repo(), &mut Acc::default());
        assert_eq!(
            tally.marker("UserRoutes.scala"),
            "[scala-routes] routes=2 handled=2 akka_composed=0 path=UserRoutes.scala"
        );
    }

    // ---- DG.11: ScalaTest / MUnit case registrations -------------------------

    /// The FUNCTION nodes' names, in the order they were minted (tree order).
    fn case_names(acc: &Acc) -> Vec<&str> {
        acc.nodes
            .iter()
            .filter(|n| acc.nav.kind_by_id.get(&n.id) == Some(&node_kind::FUNCTION))
            .filter_map(|n| acc.nav.name_by_id.get(&n.id).map(String::as_str))
            .collect()
    }

    /// The call qualifiers collected from the node whose qname is `qname`.
    fn calls_of(acc: &Acc, qname: &str) -> Vec<CallQualifier> {
        let ids: Vec<NodeId> = acc
            .nav
            .qname_by_id
            .iter()
            .filter(|(_, q)| q.as_str() == qname)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(ids.len(), 1, "one node {qname}: {:?}", acc.nav.qname_by_id);
        acc.calls
            .iter()
            .filter(|c| c.from == ids[0])
            .map(|c| c.qualifier.clone())
            .collect()
    }

    fn self_call(name: &str) -> CallQualifier {
        CallQualifier::SelfMethod(name.to_string())
    }

    fn receiver_call(receiver: &str, name: &str) -> CallQualifier {
        CallQualifier::ComplexReceiver {
            receiver: receiver.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn flat_spec_cases_are_functions() {
        // The matrix scala/tests spec: `"OrderService" should "sum" in { .. }`.
        let source = include_str!(
            "../../../../bench/substrate-gap/matrix/scala/tests/src/test/scala/shop/OrderServiceSpec.scala"
        );
        let file = "src/test/scala/shop/OrderServiceSpec.scala";
        let acc = parse_acc(
            source,
            file,
            "src::test::scala::shop::OrderServiceSpec",
            repo(),
        )
        .unwrap();
        let case = "src::test::scala::shop::OrderServiceSpec::OrderService should sum";
        let fp = parse_file(
            source,
            file,
            "src::test::scala::shop::OrderServiceSpec",
            repo(),
        )
        .unwrap();
        assert_eq!(qnames_of(&fp, node_kind::FUNCTION), vec![case]);
        assert_eq!(
            acc.tests,
            TestTally {
                suites: 1,
                cases: 1,
                containers: 0
            }
        );
        assert_eq!(
            acc.tests.marker(file),
            "[scala-tests] suites=1 cases=1 containers=0 path=src/test/scala/shop/OrderServiceSpec.scala"
        );

        let class_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::CLASS,
            "src::test::scala::shop::OrderServiceSpec",
        );
        let case_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, case);
        assert_eq!(acc.nav.parent_of.get(&case_id), Some(&class_id));
        assert_eq!(
            acc.nav.name_by_id.get(&case_id).map(String::as_str),
            Some("OrderService should sum")
        );
        assert!(
            acc.edges.iter().any(|e| e.from == class_id
                && e.to == case_id
                && e.category == edge_category::DEFINES),
            "the suite DEFINES its case"
        );
        let node = acc
            .nodes
            .iter()
            .find(|n| n.id == case_id)
            .expect("case node");
        assert_eq!(node.confidence, Confidence::Strong);
        let code = node
            .cells
            .iter()
            .find(|c| c.kind == cell_type::CODE)
            .expect("CODE cell");
        assert!(
            matches!(&code.payload, CellPayload::Text(t) if t.starts_with("\"OrderService\" should \"sum\" in {")),
            "CODE is the registration: {:?}",
            code.payload
        );
        assert!(node.cells.iter().any(|c| c.kind == cell_type::POSITION));
        let calls = calls_of(&acc, case);
        assert!(
            calls.contains(&receiver_call("new OrderService()", "total")),
            "the case holds the call into the service: {calls:?}"
        );
        assert!(calls.contains(&self_call("assert")), "{calls:?}");
        let line = acc
            .calls
            .iter()
            .find(|c| {
                c.from == case_id && c.qualifier == receiver_call("new OrderService()", "total")
            })
            .map(|c| c.line);
        assert_eq!(line, Some(6), "the call's own 0-based row");

        // FlatSpec subjects: `behavior of`, then the string a verb chain starts
        // with, name a later `it` / `they`; tags drop; `ignore` is a case; a
        // `pending` body (no block) is not.
        let source = r#"
class StackSpec extends org.scalatest.flatspec.AnyFlatSpec with Matchers {
  it should "stand alone" in { a() }
  behavior of "A Stack"
  it should "pop" in { s.pop() }
  "A Queue" should "enqueue" taggedAs(Slow) in { q.add() }
  it must "dequeue" in { q.poll() }
  they can "skip" ignore { q.skip() }
  it should "wait" in pending
  def helper(): Int = 1
}
"#;
        let acc = parse_acc(
            source,
            "src/test/StackSpec.scala",
            "src::test::StackSpec",
            repo(),
        )
        .unwrap();
        assert_eq!(
            case_names(&acc),
            vec![
                "it should stand alone",
                "A Stack should pop",
                "A Queue should enqueue",
                "A Queue must dequeue",
                "A Queue can skip",
            ]
        );
        assert_eq!(
            calls_of(&acc, "src::test::StackSpec::A Stack should pop"),
            vec![receiver_call_attr("s", "pop")]
        );
        assert_eq!(
            acc.tests,
            TestTally {
                suites: 1,
                cases: 5,
                containers: 0
            }
        );
    }

    fn receiver_call_attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn fun_suite_and_munit_tests() {
        let source = r#"
import org.scalatest.funsuite.AnyFunSuite

class CalcSuite extends AnyFunSuite {
  test("adds") { add(1, 2) }
  test("""adds
      twice""", Slow) {
    add(2, 2)
  }
  test("by value")(check(3))
  ignore("later") { calc.later() }
  def add(a: Int, b: Int) = a + b
}

class CalcMunit extends munit.FunSuite {
  test("m".ignore) { Calc.run() }
  test("tagged".tag(Slow)) { Calc.go() }
  test("fx") { fx =>
    fx.check()
  }
  test(s"interp $n") { Calc.n() }
}

object AsyncSuite extends AsyncFunSuite {
  test("async") { Future(Calc.run()) }
}
"#;
        let acc = parse_acc(source, "src/test/Calc.scala", "src::test::Calc", repo()).unwrap();
        assert_eq!(
            case_names(&acc),
            vec![
                "adds",
                "adds twice",
                "by value",
                "later",
                "m",
                "tagged",
                "fx",
                "interp $n",
                "async"
            ]
        );
        assert_eq!(
            acc.tests,
            TestTally {
                suites: 3,
                cases: 9,
                containers: 0
            }
        );
        // A bare call in a case is the suite's own method, as in a def.
        assert_eq!(
            calls_of(&acc, "src::test::CalcSuite::adds"),
            vec![self_call("add")]
        );
        assert_eq!(
            calls_of(&acc, "src::test::CalcSuite::adds twice"),
            vec![self_call("add")]
        );
        assert_eq!(
            calls_of(&acc, "src::test::CalcSuite::by value"),
            vec![self_call("check")]
        );
        assert_eq!(
            calls_of(&acc, "src::test::CalcMunit::m"),
            vec![receiver_call_attr("Calc", "run")]
        );
        // The fixture lambda's body holds the case's calls.
        assert_eq!(
            calls_of(&acc, "src::test::CalcMunit::fx"),
            vec![receiver_call_attr("fx", "check")]
        );
        let mut async_calls = calls_of(&acc, "src::test::AsyncSuite::async");
        async_calls.sort_by_key(|q| format!("{q:?}"));
        assert_eq!(
            async_calls,
            vec![receiver_call_attr("Calc", "run"), self_call("Future")]
        );
        // The helper def is still a METHOD of the suite, not a case.
        assert!(
            qnames_of_acc(&acc, node_kind::METHOD)
                .contains(&"src::test::CalcSuite::add".to_string())
        );
    }

    fn qnames_of_acc(acc: &Acc, kind: glia_core::NodeKindId) -> Vec<String> {
        let mut v: Vec<String> = acc
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == kind)
            .filter_map(|(id, _)| acc.nav.qname_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn word_and_free_spec_containers_prefix() {
        let source = r#"
class StackSpec extends AnyWordSpec {
  "A Stack" when {
    "empty" should {
      "pop" in { s.pop() }
      "peek" ignore { s.peek() }
    }
    "full" which {
      "push" in { s.push(1) }
    }
  }
}

class TreeSpec extends AnyFreeSpec {
  "A Tree" - {
    "grows" in { t.grow() }
    "when cut" - {
      "falls" in { t.fall() }
    }
  }
}
"#;
        let acc = parse_acc(source, "src/test/Spec.scala", "src::test::Spec", repo()).unwrap();
        assert_eq!(
            case_names(&acc),
            vec![
                "A Stack when empty should pop",
                "A Stack when empty should peek",
                "A Stack when full which push",
                "A Tree grows",
                "A Tree when cut falls",
            ]
        );
        assert_eq!(
            acc.tests,
            TestTally {
                suites: 2,
                cases: 5,
                containers: 5
            }
        );
        assert_eq!(
            calls_of(&acc, "src::test::TreeSpec::A Tree when cut falls"),
            vec![receiver_call_attr("t", "fall")]
        );
        // Every case hangs off its suite, whatever its container depth.
        let suite =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "src::test::StackSpec");
        let pop = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "src::test::StackSpec::A Stack when empty should pop",
        );
        assert_eq!(acc.nav.parent_of.get(&pop), Some(&suite));
    }

    #[test]
    fn fun_spec_describe_it() {
        let source = r#"
class CalcSpec extends AnyFunSpec {
  describe("Calc") {
    it("adds") { calc.add(1) }
    describe("when negative") {
      they("subtract") { calc.sub(1) }
    }
  }
}

class ShopFeature extends AnyFeatureSpec with GivenWhenThen {
  Feature("Checkout") {
    Scenario("pays") { shop.pay() }
  }
}

class PropSuite extends PropSpec {
  property("commutes") { calc.add(2) }
}
"#;
        let acc = parse_acc(source, "src/test/Calc.scala", "src::test::Calc", repo()).unwrap();
        assert_eq!(
            case_names(&acc),
            vec![
                "Calc adds",
                "Calc when negative subtract",
                "Checkout pays",
                "commutes"
            ]
        );
        assert_eq!(
            acc.tests,
            TestTally {
                suites: 3,
                cases: 4,
                containers: 3
            }
        );
        assert_eq!(
            calls_of(&acc, "src::test::CalcSpec::Calc when negative subtract"),
            vec![receiver_call_attr("calc", "sub")]
        );
        assert_eq!(
            calls_of(&acc, "src::test::ShopFeature::Checkout pays"),
            vec![receiver_call_attr("shop", "pay")]
        );
    }

    #[test]
    fn non_suite_class_infix_in_is_not_a_case() {
        // Registration shapes outside a suite body are production code: a
        // router's `"a" in { .. }`, a `test(..) { .. }` helper. An in-house
        // base (`BaseSpec`) is not recognised (the known limit).
        let source = r#"
class Router {
  "a" should "b" in { route() }
  test("x") { run() }
  describe("y") { it("z") { go() } }
  def handle(): Unit = { "c" in { d() } }
}

object Plain extends Base with Serializable {
  "a" in { b() }
}

class MySpec extends BaseSpec {
  "A" should "b" in { c() }
}
"#;
        let acc = parse_acc(source, "src/main/Router.scala", "src::main::Router", repo()).unwrap();
        assert!(case_names(&acc).is_empty(), "{:?}", case_names(&acc));
        assert_eq!(acc.tests, TestTally::default());
        assert!(!is_test_suite_src("class S extends Spec"));
        assert!(is_test_suite_src(
            "class S extends org.scalatest.wordspec.AnyWordSpec"
        ));
        assert!(is_test_suite_src(
            "class S extends TestKit(ActorSystem(\"x\")) with AnyWordSpecLike with Matchers"
        ));
        assert!(is_test_suite_src("class S extends CatsEffectSuite"));
        assert!(is_test_suite_src("trait S extends AnyFlatSpec"));
    }

    /// Parse `source`, a single type declaration, and gate it.
    fn is_test_suite_src(source: &str) -> bool {
        let mut p = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_scala::LANGUAGE.into();
        p.set_language(&lang).unwrap();
        let tree = p.parse(source, None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let ty = root
            .named_children(&mut cursor)
            .next()
            .expect("one declaration");
        is_test_suite(ty, source.as_bytes())
    }

    #[test]
    fn duplicate_descriptions_are_numbered() {
        let long = "x".repeat(150);
        let source = format!(
            r#"
class DupSpec extends AnyFreeSpec {{
  "A" - {{ "b" in {{ x() }} }}
  "A" - {{ "b" in {{ y() }} }}
  "A b" in {{ z() }}
  "Long::name   with" + "x" in {{ q() }}
  "Long::name   with  space" in {{ w() }}
  "{long}" in {{ v() }}
  "" in {{ e() }}
}}
"#
        );
        let acc = parse_acc(
            &source,
            "src/test/DupSpec.scala",
            "src::test::DupSpec",
            repo(),
        )
        .unwrap();
        let truncated = "x".repeat(CASE_NAME_MAX);
        assert_eq!(
            case_names(&acc),
            vec![
                "A b",
                "A b #2",
                "A b #3",
                "Long:name with space",
                truncated.as_str()
            ]
        );
        assert_eq!(
            calls_of(&acc, "src::test::DupSpec::A b #2"),
            vec![self_call("y")],
            "the second registration keeps its own calls"
        );
        // A name holds no `::`, so the case's qname has one more segment.
        assert_eq!(case_name("a :: b ::: c"), Some("a : b : c".to_string()));
        assert_eq!(case_name("  \n "), None);
    }
}
