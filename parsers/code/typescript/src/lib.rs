//! repo-graph-parser-typescript — tree-sitter TypeScript → `repo_graph_core` types.
//!
//! Single-file scan: emit Module/Class/Interface/Function/Method nodes with
//! Code/Position cells, intra-file `defines` and `calls` edges. Cross-file
//! refs (imports, calls that bind to another module) are recorded as
//! `ImportStmt` / `CallSite` for the graph crate's cross-file resolver.
//!
//! `export …` wrappers are unwrapped transparently — `export class Foo {}`
//! produces the same node shape as `class Foo {}`.
//!
//! `const foo = () => {...}` and `const foo = function(){...}` are treated as
//! top-level Function nodes identical to `function foo() {}`.
//!
//! All code-domain primitives live in `repo-graph-code-domain` and are
//! re-exported from this crate for convenience.

use std::collections::HashMap;

use repo_graph_code_domain::di_stats::{self, DiShape};
use repo_graph_code_domain::endpoint;
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};

/// Parse one TypeScript source file.
///
/// `module_qname` is the module path in `::` form (e.g. `src::users::service`).
/// `file_rel_path` is the repo-relative file path stored in position cells.
pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();

    let mut acc = Acc {
        file_rel: file_rel_path.to_string(),
        repo: Some(repo),
        ..Acc::default()
    };
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
        visit_top(child, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    }

    resolve_intra_file(acc)
}

// ============================================================================
// Internal accumulator
// ============================================================================

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    /// Non-call cross-file references resolved into edges by the graph crate.
    /// Angular constructor-DI emits `INJECTS` refs here (class → service type).
    refs: Vec<UnresolvedRef>,
    unresolved: Vec<UnresolvedCall>,
    endpoints: Vec<EndpointCandidate>,
    /// A7.1: Angular 14+ `private x = inject(Foo)` class-field DI. Emitted as
    /// INJECTS refs in `resolve_intra_file` only when the callee is bound by a
    /// named `inject` import: a late import gate, like `EndpointCandidate::
    /// requires_import_alias`.
    inject_fn_candidates: Vec<InjectFnCandidate>,
    module_functions: HashMap<String, NodeId>,
    class_methods: HashMap<(NodeId, String), NodeId>,
    nav: CodeNav,
    /// Stashed at parse_file entry so endpoint emission can stamp position
    /// cells without threading `file_rel` through every call_collection helper.
    file_rel: String,
    repo: Option<RepoId>,
}

struct UnresolvedCall {
    from: NodeId,
    enclosing_class: Option<NodeId>,
    qualifier: CallQualifier,
}

/// A class field initialised by a bare call whose argument names a type:
/// `private api = inject(ApiService)`. Whether the call is Angular's `inject`
/// is only known once the file's imports are, so the INJECTS ref is minted in
/// `resolve_intra_file`.
struct InjectFnCandidate {
    class_id: NodeId,
    module_id: NodeId,
    /// The local name the call goes through (`inject`, or `i` after
    /// `import { inject as i }`).
    callee: String,
    type_name: String,
}

/// An HTTP-call shape detected during the call walk. Resolved into an Endpoint
/// node + CALLS edge in `resolve_intra_file` once the import-alias set is known.
struct EndpointCandidate {
    from: NodeId,
    method: String,
    path: String,
    confidence: Confidence,
    file_rel: String,
    line: usize,
    col: usize,
    /// Some(name) means this candidate only emits if `name` is a module-level
    /// import alias (shape 2: `axios.get(url)`). None = always emit (shape 1
    /// `this.x.method()` and shape 3 `fetch()`).
    requires_import_alias: Option<String>,
    /// A3.3: the call-site literal `path` was normalised from, set only when
    /// `normalise_client_path` or LB.5's leading-`/` canonicalisation actually
    /// changed it. Serialised as `"raw"` on ENDPOINT_HIT.
    raw_path: Option<String>,
    /// A11.2: the template literal with every `${expr}` substitution kept
    /// VERBATIM (`${environment.apiUrl}/users`), set only when the argument was
    /// a template with at least one substitution. Serialised as `"template"`
    /// on ENDPOINT_HIT. Not the same value as `raw_path`: that one is the
    /// pre-normalisation path with `${…}` placeholders and exists only when
    /// host/query stripping changed it; this one is the source the engine's
    /// endpoint-fold pass resolves through the repo ConstTable.
    template: Option<String>,
}

/// What `classify_path_arg` read off an HTTP call's first argument.
struct PathArg {
    /// Request path as written, `${…}` for every substitution.
    path: String,
    /// Substitution-preserving template source; see `EndpointCandidate::template`.
    template: Option<String>,
    confidence: Confidence,
}

// ============================================================================
// Top-level dispatch
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn visit_top(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    match n.kind() {
        "import_statement" => collect_import(n, src, module_qname, acc),
        "export_statement" => {
            if let Some(decl) = n.child_by_field_name("declaration") {
                // G19: surface module-level exported data constants
                // (`export const NAME = ...`) as STATE_VAR nodes. Arrow/function
                // consts are still hoisted to Function nodes by visit_lexical.
                if decl.kind() == "lexical_declaration" {
                    visit_exported_const(decl, src, file_rel, module_qname, module_id, repo, acc);
                }
                visit_top(decl, src, file_rel, module_qname, module_id, repo, acc);
            }
        }
        "class_declaration" => {
            visit_class(n, src, file_rel, module_qname, module_id, repo, acc);
        }
        "interface_declaration" => {
            visit_interface(n, src, file_rel, module_qname, module_id, repo, acc);
        }
        "function_declaration" => {
            visit_function_decl(n, src, file_rel, module_qname, module_id, repo, acc);
        }
        "lexical_declaration" | "variable_declaration" => {
            visit_lexical(n, src, file_rel, module_qname, module_id, repo, acc);
        }
        "expression_statement" => {
            // Top-level calls — record with module as source.
            collect_calls_in(n, src, module_id, None, acc);
        }
        _ => {}
    }
}

// ============================================================================
// Visitors
// ============================================================================

#[allow(clippy::too_many_arguments)]
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
    acc.nav
        .record(class_id, name, &class_qname, node_kind::CLASS, Some(module_id));

    // Class heritage: `class X extends Y implements I, J`.
    // `extends_clause` → INHERITS_FROM, each type in `implements_clause` →
    // IMPLEMENTS (class → interface). The superclass / interfaces are cross-file
    // references resolved later, so we record them as Inherits/Implements refs.
    collect_class_heritage(n, src, module_qname, class_id, repo, acc);

    let Some(body) = n.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "method_definition" => {
                visit_method(member, src, file_rel, &class_qname, class_id, repo, acc);
            }
            // The one class-field visitor: every per-field extraction goes here.
            "public_field_definition" => visit_field(member, src, module_id, class_id, acc),
            _ => {}
        }
    }

    // Constructor dependency injection (Angular Pattern E, NestJS). A DI-
    // decorated class declares its dependencies as typed constructor
    // parameters. Each class/interface-typed param becomes an INJECTS ref
    // (class → dependency type); the graph crate binds the bare type name to
    // the target node and forms the edge.
    collect_constructor_injects(n, body, src, module_id, class_id, acc);
}

/// Class decorators that mark an Angular class as an injection consumer.
/// `Injectable` also covers NestJS providers, guards, interceptors and pipes.
const DI_DECORATORS: &[&str] = &["Component", "Injectable", "Directive", "Pipe"];

/// NestJS class decorators that make a class an injection consumer without
/// `@Injectable`: controller, GraphQL resolver, WebSocket gateway.
const NEST_DI_DECORATORS: &[&str] = &["Controller", "Resolver", "WebSocketGateway"];

/// Angular / NestJS constructor-parameter decorators. Any one of them on a
/// constructor parameter proves the constructor is an injection site even when
/// the class carries no DI decorator.
const DI_PARAM_DECORATORS: &[&str] = &["Inject", "Optional", "Self", "SkipSelf", "Host"];

/// Emit `INJECTS` refs for each class/interface-typed constructor parameter of
/// a DI-decorated class (Angular or NestJS), or of any class whose constructor
/// has a DI parameter decorator (`@Inject(TOKEN)`, `@Optional()`, …). Gated so
/// plain data classes don't mint injection edges. Primitive-typed params
/// (`predefined_type`: string/number/boolean/…) are skipped.
fn collect_constructor_injects(
    class_node: TsNode,
    body: TsNode,
    src: &[u8],
    module_id: NodeId,
    class_id: NodeId,
    acc: &mut Acc,
) {
    // Find the `constructor` method.
    let mut cursor = body.walk();
    let Some(ctor) = body.named_children(&mut cursor).find(|m| {
        m.kind() == "method_definition"
            && child_text(*m, "name", src) == Some("constructor")
    }) else {
        return;
    };
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return;
    };
    let shape = match class_di_shape(class_node, src) {
        Some(shape) => shape,
        None if has_di_param_decorator(params, src) => DiShape::TsCtor,
        None => return,
    };
    let mut pc = params.walk();
    for param in params.named_children(&mut pc) {
        // Constructor params carrying an accessibility/readonly modifier are
        // `required_parameter`; plain ones may be `required_parameter` or
        // `optional_parameter`. Both expose a `type` field.
        if !matches!(param.kind(), "required_parameter" | "optional_parameter") {
            continue;
        }
        let Some(ty_ann) = param.child_by_field_name("type") else {
            continue;
        };
        // type_annotation wraps the actual type node; skip primitives.
        let mut tc = ty_ann.walk();
        let Some(ty) = ty_ann.named_children(&mut tc).next() else {
            continue;
        };
        if ty.kind() == "predefined_type" {
            continue;
        }
        let Some(type_name) = heritage_type_name(ty, src) else {
            continue;
        };
        acc.refs.push(UnresolvedRef {
            from: class_id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(type_name.to_string()),
            category: edge_category::INJECTS,
        });
        di_stats::record(shape);
    }
}

/// The constructor-DI shape a class's decorators select: `TsNestCtor` when a
/// NestJS controller / resolver / gateway decorator is present, `TsCtor` for
/// an Angular DI decorator (or `@Injectable`), `None` for an undecorated
/// class. Decorators attach either directly to the `class_declaration`
/// (`@Injectable() class Foo {}`) or to the parent `export_statement`
/// (`@Component({...}) export class Foo {}`).
fn class_di_shape(class_node: TsNode, src: &[u8]) -> Option<DiShape> {
    let mut names: Vec<&str> = Vec::new();
    let mut collect = |node: TsNode| {
        let mut c = node.walk();
        names.extend(
            node.named_children(&mut c)
                .filter(|ch| ch.kind() == "decorator")
                .filter_map(|dec| decorator_name(dec, src)),
        );
    };
    collect(class_node);
    if let Some(parent) = class_node.parent() {
        collect(parent);
    }
    if names.iter().any(|n| NEST_DI_DECORATORS.contains(n)) {
        Some(DiShape::TsNestCtor)
    } else if names.iter().any(|n| DI_DECORATORS.contains(n)) {
        Some(DiShape::TsCtor)
    } else {
        None
    }
}

/// True if any constructor parameter carries a DI parameter decorator
/// (`constructor(@Inject(API_URL) private url: string)`).
fn has_di_param_decorator(params: TsNode, src: &[u8]) -> bool {
    let mut pc = params.walk();
    params.named_children(&mut pc).any(|param| {
        let mut dc = param.walk();
        param
            .children_by_field_name("decorator", &mut dc)
            .filter_map(|dec| decorator_name(dec, src))
            .any(|n| DI_PARAM_DECORATORS.contains(&n))
    })
}

/// Visit one class field (`public_field_definition`). A7.1: a field
/// initialised by a bare call with a type-naming argument,
/// `private api = inject(ApiService)` (also `inject<T>(TOKEN)` and
/// `inject(ns.Foo)`), becomes an [`InjectFnCandidate`]. It is not gated on a
/// class decorator: `inject()` is only legal in an injection context, and the
/// import gate in `resolve_intra_file` proves the callee is `inject`.
fn visit_field(field: TsNode, src: &[u8], module_id: NodeId, class_id: NodeId, acc: &mut Acc) {
    let Some(value) = field.child_by_field_name("value") else {
        return;
    };
    if value.kind() != "call_expression" {
        return;
    }
    let Some(callee) = value.child_by_field_name("function") else {
        return;
    };
    if callee.kind() != "identifier" {
        return;
    }
    let Some(args) = value.child_by_field_name("arguments") else {
        return;
    };
    let mut ac = args.walk();
    let Some(arg) = args.named_children(&mut ac).find(|a| a.kind() != "comment") else {
        return;
    };
    // A class or an InjectionToken constant: `Foo` or `ns.Foo`. A string,
    // call or arrow argument names no type.
    if !matches!(arg.kind(), "identifier" | "member_expression") {
        return;
    }
    let Some(type_name) = heritage_type_name(arg, src) else {
        return;
    };
    if type_name == "undefined" {
        return;
    }
    acc.inject_fn_candidates.push(InjectFnCandidate {
        class_id,
        module_id,
        callee: text(callee, src).to_string(),
        type_name: type_name.to_string(),
    });
}

/// The leading identifier of a decorator: `@Component({...})` → "Component",
/// `@Injectable` → "Injectable".
fn decorator_name<'a>(dec: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let mut c = dec.walk();
    let inner = dec.named_children(&mut c).next()?;
    let ident = match inner.kind() {
        // `@Component({...})` — call_expression, name under `function`.
        "call_expression" => inner.child_by_field_name("function")?,
        // `@Injectable` — bare identifier.
        _ => inner,
    };
    // `@ns.Component(...)` — take the trailing simple name.
    let raw = text(ident, src).trim();
    Some(raw.rsplit('.').next().unwrap_or(raw))
}

/// Parse `class X extends Y implements I, J` heritage.
///
/// `extends_clause` → INHERITS_FROM (class → superclass), each type in
/// `implements_clause` → IMPLEMENTS (class → interface). The referenced types
/// are typically cross-file; we emit best-effort same-module target NodeIds
/// (qname = `<module_qname>::<TypeName>`) for the graph crate's cross-file
/// resolver to reconcile.
fn collect_class_heritage(
    class_node: TsNode,
    src: &[u8],
    module_qname: &str,
    class_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = class_node.walk();
    let Some(heritage) = class_node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "class_heritage")
    else {
        return;
    };

    let mut hc = heritage.walk();
    for clause in heritage.named_children(&mut hc) {
        match clause.kind() {
            "extends_clause" => {
                // The superclass expression(s) live under field `value`; sibling
                // `type_arguments` nodes are skipped by selecting the field.
                let mut ec = clause.walk();
                for ty in clause.children_by_field_name("value", &mut ec) {
                    if let Some(base) = heritage_type_name(ty, src) {
                        let to_qname = format!("{module_qname}::{base}");
                        let to_id =
                            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, &to_qname);
                        acc.edges.push(Edge {
                            from: class_id,
                            to: to_id,
                            category: edge_category::INHERITS_FROM,
                            confidence: Confidence::Weak,
                        });
                    }
                }
            }
            "implements_clause" => {
                let mut ic = clause.walk();
                for ty in clause.named_children(&mut ic) {
                    if let Some(iface) = heritage_type_name(ty, src) {
                        let to_qname = format!("{module_qname}::{iface}");
                        let to_id =
                            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INTERFACE, &to_qname);
                        acc.edges.push(Edge {
                            from: class_id,
                            to: to_id,
                            category: edge_category::IMPLEMENTS,
                            confidence: Confidence::Weak,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Extract the leading identifier of a heritage type node, stripping generic
/// arguments and module qualifiers (`ns.IFoo<T>` → `IFoo`).
fn heritage_type_name<'a>(ty: TsNode, src: &'a [u8]) -> Option<&'a str> {
    if ty.kind() == "comment" {
        return None;
    }
    let base = match ty.kind() {
        // `generic_type` wraps the name node under field `name`.
        "generic_type" => ty.child_by_field_name("name").unwrap_or(ty),
        _ => ty,
    };
    let raw = text(base, src).trim();
    if raw.is_empty() {
        return None;
    }
    // `ns.IFoo` / `IFoo<T>` → take the trailing simple name.
    let after_dot = raw.rsplit('.').next().unwrap_or(raw);
    let simple = after_dot.split('<').next().unwrap_or(after_dot).trim();
    if simple.is_empty() {
        None
    } else {
        Some(simple)
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_interface(
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
    let iface_qname = format!("{module_qname}::{name}");
    let iface_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INTERFACE, &iface_qname);
    acc.nodes.push(Node {
        id: iface_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&n, src, file_rel),
    });
    acc.edges.push(Edge {
        from: module_id,
        to: iface_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.nav.record(
        iface_id,
        name,
        &iface_qname,
        node_kind::INTERFACE,
        Some(module_id),
    );
}

#[allow(clippy::too_many_arguments)]
fn visit_method(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    class_qname: &str,
    class_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
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
    acc.nav.record(
        method_id,
        name,
        &method_qname,
        node_kind::METHOD,
        Some(class_id),
    );

    if let Some(body) = n.child_by_field_name("body") {
        collect_calls_in(body, src, method_id, Some(class_id), acc);
    }
}

#[allow(clippy::too_many_arguments)]
fn visit_function_decl(
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
    emit_function(n, name, src, file_rel, module_qname, module_id, repo, acc);
}

#[allow(clippy::too_many_arguments)]
fn visit_lexical(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // `const foo = () => {...}` or `const foo = function(){...}` are hoisted
    // to Function nodes. Other const/let bindings are data, not behaviour.
    let mut cursor = n.walk();
    for declarator in n.named_children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let Some(value) = declarator.child_by_field_name("value") else {
            continue;
        };
        if !matches!(value.kind(), "arrow_function" | "function_expression") {
            continue;
        }
        let Some(name_n) = declarator.child_by_field_name("name") else {
            continue;
        };
        if name_n.kind() != "identifier" {
            continue;
        }
        let name = text(name_n, src);
        emit_function_value(
            value, name, src, file_rel, module_qname, module_id, repo, acc,
        );
    }
}

/// G19: surface module-level exported data constants
/// (`export const NAME = ...`, `export const NAME: T = ...`) as STATE_VAR nodes.
///
/// Only `const` lexical declarations qualify. Arrow/function-valued consts are
/// left to `visit_lexical` (they become Function nodes). A noise gate drops
/// undocumented literal-primitive consts (number / short string / bool) so that
/// only documented or structurally-interesting (object / call / array)
/// constants reach the graph.
#[allow(clippy::too_many_arguments)]
fn visit_exported_const(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // Only `const` declarations (skip `let` / `var`).
    let is_const = {
        let mut c = n.walk();
        n.children(&mut c).any(|ch| ch.kind() == "const")
    };
    if !is_const {
        return;
    }

    // Resolve the doc once from the lexical_declaration (leading_doc hops up to
    // the export_statement wrapper to find the JSDoc above it).
    let doc = repo_graph_doc::leading_doc(&n, src);

    let mut cursor = n.walk();
    for declarator in n.named_children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_n) = declarator.child_by_field_name("name") else {
            continue;
        };
        if name_n.kind() != "identifier" {
            continue;
        }
        let value = declarator.child_by_field_name("value");
        // Arrow/function consts are hoisted to Function nodes by visit_lexical.
        if let Some(v) = value {
            if matches!(v.kind(), "arrow_function" | "function_expression") {
                continue;
            }
        }
        // Noise gate: drop undocumented literal-primitive consts.
        if doc.is_none() && value.map(|v| is_literal_primitive(v, src)).unwrap_or(true) {
            continue;
        }

        let name = text(name_n, src);
        let const_qname = format!("{module_qname}::{name}");
        let const_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &const_qname);

        // Cells: CODE/POSITION from the declarator, plus the resolved doc.
        let mut cells = vec![
            Cell {
                kind: cell_type::CODE,
                payload: CellPayload::Text(slice(&declarator, src).to_string()),
            },
            Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(position_json(&declarator, file_rel)),
            },
        ];
        if let Some(ref d) = doc {
            cells.push(Cell {
                kind: cell_type::DOC,
                payload: CellPayload::Text(d.clone()),
            });
        }

        acc.nodes.push(Node {
            id: const_id,
            repo,
            confidence: Confidence::Strong,
            cells,
        });
        acc.edges.push(Edge {
            from: module_id,
            to: const_id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
        });
        acc.nav.record(
            const_id,
            name,
            &const_qname,
            node_kind::STATE_VAR,
            Some(module_id),
        );
    }
}

/// Whether an initializer is a literal primitive that should be gated out when
/// undocumented: a number, a boolean, or a short string (< 32 chars). Objects,
/// arrays, calls, template strings, and longer strings are kept.
fn is_literal_primitive(value: TsNode, src: &[u8]) -> bool {
    match value.kind() {
        "number" | "true" | "false" => true,
        "string" => {
            // Measure the literal contents, excluding surrounding quotes.
            strip_string_quotes(text(value, src)).chars().count() < 32
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_function(
    n: TsNode,
    name: &str,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let func_qname = format!("{module_qname}::{name}");
    let func_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &func_qname);
    acc.nodes.push(Node {
        id: func_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&n, src, file_rel),
    });
    acc.edges.push(Edge {
        from: module_id,
        to: func_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.module_functions.insert(name.to_string(), func_id);
    acc.nav.record(
        func_id,
        name,
        &func_qname,
        node_kind::FUNCTION,
        Some(module_id),
    );

    if let Some(body) = n.child_by_field_name("body") {
        collect_calls_in(body, src, func_id, None, acc);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_function_value(
    value: TsNode,
    name: &str,
    src: &[u8],
    file_rel: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let func_qname = format!("{module_qname}::{name}");
    let func_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &func_qname);
    acc.nodes.push(Node {
        id: func_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&value, src, file_rel),
    });
    acc.edges.push(Edge {
        from: module_id,
        to: func_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
    });
    acc.module_functions.insert(name.to_string(), func_id);
    acc.nav.record(
        func_id,
        name,
        &func_qname,
        node_kind::FUNCTION,
        Some(module_id),
    );

    if let Some(body) = value.child_by_field_name("body") {
        collect_calls_in(body, src, func_id, None, acc);
    }
}

// ============================================================================
// Imports
// ============================================================================

fn collect_import(n: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let Some(source_node) = n.child_by_field_name("source") else {
        return;
    };
    let source = strip_string_quotes(text(source_node, src));

    let mut cursor = n.walk();
    let clause = n
        .named_children(&mut cursor)
        .find(|c| c.kind() == "import_clause");

    let Some(clause) = clause else {
        // Side-effect import: `import "polyfill";`
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target: ImportTarget::Module {
                path: source,
                alias: None,
            },
        });
        return;
    };

    let mut cursor2 = clause.walk();
    for part in clause.named_children(&mut cursor2) {
        match part.kind() {
            "identifier" => {
                // `import Foo from "src"` — default import.
                acc.imports.push(ImportStmt {
                    from_module: from_module.to_string(),
                    target: ImportTarget::Symbol {
                        module: source.clone(),
                        name: "default".to_string(),
                        alias: Some(text(part, src).to_string()),
                        level: 0,
                    },
                });
            }
            "namespace_import" => {
                // `import * as Foo from "src"`
                let mut ns_cursor = part.walk();
                if let Some(id) = part.named_children(&mut ns_cursor).next()
                    && id.kind() == "identifier"
                {
                    acc.imports.push(ImportStmt {
                        from_module: from_module.to_string(),
                        target: ImportTarget::Module {
                            path: source.clone(),
                            alias: Some(text(id, src).to_string()),
                        },
                    });
                }
            }
            "named_imports" => {
                // `import { a, b as c } from "src"`
                let mut ni_cursor = part.walk();
                for spec in part.named_children(&mut ni_cursor) {
                    if spec.kind() != "import_specifier" {
                        continue;
                    }
                    let Some(name_node) = spec.child_by_field_name("name") else {
                        continue;
                    };
                    let name = text(name_node, src).to_string();
                    let alias = spec
                        .child_by_field_name("alias")
                        .map(|a| text(a, src).to_string());
                    acc.imports.push(ImportStmt {
                        from_module: from_module.to_string(),
                        target: ImportTarget::Symbol {
                            module: source.clone(),
                            name,
                            alias,
                            level: 0,
                        },
                    });
                }
            }
            _ => {}
        }
    }
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
        // Named-declaration/class bodies own their own from-node (emitted as
        // separate Function/Method/Class nodes) — walked separately, so skip.
        //
        // Anonymous callback bodies — `arrow_function` and `function_expression`
        // — are NOT hoisted to their own nodes when nested (e.g. the
        // `useEffect(() => { fetch(...) })` / `useCallback` / `.then(() => …)`
        // callback, or `arr.map(function(){…})`). Their calls belong to the
        // enclosing `from` node, so we descend into them here rather than skip.
        // (Top-level arrow/function-expression consts are handled at the
        // declaration seam via `emit_function_value`, which walks the callback
        // body directly and never routes the callback node through here — so
        // descending here does not double-emit.)
        if matches!(
            kind,
            "function_declaration"
                | "method_definition"
                | "class_declaration"
                | "class_expression"
        ) {
            continue;
        }
        if kind == "call_expression" {
            if let Some(q) = extract_call_qualifier(node, src) {
                acc.unresolved.push(UnresolvedCall {
                    from,
                    enclosing_class,
                    qualifier: q,
                });
            }
            try_detect_endpoint(node, src, from, acc);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

// ============================================================================
// Endpoint extraction (v0.4.4)
// ============================================================================
//
// Three call shapes get classified as HTTP endpoint hits:
//   1. `this.<x>.<method>(<first>, …)`  — Angular HttpClient via DI and any
//      service-with-HTTP-client-field pattern. Always emit (no import check).
//   2. `<x>.<method>(<first>, …)` where `<x>` is a module-level import alias
//      — direct axios/got/ky calls.
//   3. `fetch(<first>, <opts>?)` — built-in. Method defaults to GET unless
//      `opts` carries `method: '...'`.
//
// Path is classified into a Confidence:
//   - String literal                                        → Strong
//   - Template with only static parts                       → Strong
//   - Template with interpolations                          → Medium (path
//     keeps literal prefix + `${…}` placeholders)
//   - Call expression (URL-builder wrapper) — pluck inner   → Weak
//     literal as hint
//   - Anything else (identifier, conditional, …)            → Weak,
//     path = `<unresolved>`
//
// Method/path normalisation (e.g. `:id` ↔ `{id}`) is HttpStackResolver's job;
// the parser stores the raw text as written.

const HTTP_METHOD_PROPS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

fn try_detect_endpoint(call: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let func = match call.child_by_field_name("function") {
        Some(f) => f,
        None => return,
    };

    // Shape 3 — `fetch(...)`.
    if func.kind() == "identifier" && text(func, src) == "fetch" {
        let args = match call.child_by_field_name("arguments") {
            Some(a) => a,
            None => return,
        };
        let first = match args.named_child(0) {
            Some(n) => n,
            None => return,
        };
        let mut arg = classify_path_arg(first, src);
        let method = fetch_method_from_opts(args.named_child(1), src).unwrap_or_else(|| {
            // Method override is opaque (variable, spread, conditional) — drop
            // confidence one tier.
            if args.named_child(1).is_some() {
                arg.confidence = downgrade(arg.confidence);
            }
            "GET".to_string()
        });
        push_endpoint(call, from, method, arg, None, acc);
        return;
    }

    // Shapes 1 & 2 — member call `<obj>.<method>(...)`.
    if func.kind() != "member_expression" {
        return;
    }
    let prop = match func.child_by_field_name("property") {
        Some(p) => p,
        None => return,
    };
    let method_lower = text(prop, src);
    if !HTTP_METHOD_PROPS.contains(&method_lower) {
        return;
    }
    let object = match func.child_by_field_name("object") {
        Some(o) => o,
        None => return,
    };

    // Shape 1: this.<x>.<method>(...) — object is itself a member_expression
    // whose object is `this`.
    let requires_alias = match object.kind() {
        "member_expression" => {
            let inner_obj = match object.child_by_field_name("object") {
                Some(o) => o,
                None => return,
            };
            if inner_obj.kind() != "this" {
                return;
            }
            None
        }
        // Shape 2: <alias>.<method>(...) — object is a plain identifier that
        // must be a module-level import alias. Validation happens in
        // resolve_intra_file once all imports are known.
        "identifier" => Some(text(object, src).to_string()),
        _ => return,
    };

    let args = match call.child_by_field_name("arguments") {
        Some(a) => a,
        None => return,
    };
    let first = match args.named_child(0) {
        Some(n) => n,
        None => return,
    };
    let arg = classify_path_arg(first, src);
    push_endpoint(
        call,
        from,
        method_lower.to_uppercase(),
        arg,
        requires_alias,
        acc,
    );
}

fn push_endpoint(
    call: TsNode,
    from: NodeId,
    method: String,
    arg: PathArg,
    requires_import_alias: Option<String>,
    acc: &mut Acc,
) {
    let start = call.start_position();
    let PathArg {
        path,
        template,
        confidence,
    } = arg;
    // A3.3: the single funnel for every client-call shape, so host + query
    // stripping happens once. A normaliser, never a filter: interpolated bases
    // (`${…}/users`) come back as-is.
    let (norm, changed) = endpoint::normalise_client_path(&path);
    // LB.5: a relative hint (`auth/login`) gains its one canonical leading
    // `/`, so `this.http.delete('protected/x')` and `…('/protected/x')` are
    // one ENDPOINT. The literal it was rewritten from rides along as `raw`.
    let canonical = endpoint::canonical_http_path(&norm).into_owned();
    let raw_path = (changed || canonical != norm).then_some(path);
    let norm = canonical;
    acc.endpoints.push(EndpointCandidate {
        from,
        method,
        path: norm,
        confidence,
        file_rel: acc.file_rel.clone(),
        line: start.row + 1,
        col: start.column + 1,
        requires_import_alias,
        raw_path,
        template,
    });
}

fn classify_path_arg(arg: TsNode, src: &[u8]) -> PathArg {
    let plain = |path: String, confidence| PathArg {
        path,
        template: None,
        confidence,
    };
    match arg.kind() {
        "string" => {
            let raw = text(arg, src);
            plain(strip_string_quotes(raw), Confidence::Strong)
        }
        "template_string" => classify_template(arg, src),
        "call_expression" => {
            // URL-builder wrapper like `this.api.buildUrl('auth/login')` —
            // pluck the innermost string literal as a hint, weak confidence.
            plain(
                find_first_string_literal(arg, src)
                    .map(|s| strip_string_quotes(&s))
                    .unwrap_or_else(|| "<unresolved>".to_string()),
                Confidence::Weak,
            )
        }
        _ => plain("<unresolved>".to_string(), Confidence::Weak),
    }
}

/// A template literal as two parallel strings built from the same children:
/// `path` writes `${…}` for each substitution (the identity the node is minted
/// from, unchanged since v0.4.4), `template` writes the substitution's source
/// text (`${environment.apiUrl}`). Replacing every `${expr}` in `template`
/// with `${…}` therefore gives back `path` exactly, which is what lets the
/// engine's endpoint fold leave an unresolvable endpoint bit-identical.
fn classify_template(template: TsNode, src: &[u8]) -> PathArg {
    let mut out = String::new();
    let mut source = String::new();
    let mut has_subst = false;
    let mut cursor = template.walk();
    for child in template.named_children(&mut cursor) {
        match child.kind() {
            "template_substitution" => {
                has_subst = true;
                out.push_str("${…}");
                source.push_str(text(child, src));
            }
            "string_fragment" => {
                out.push_str(text(child, src));
                source.push_str(text(child, src));
            }
            _ => {}
        }
    }
    PathArg {
        path: out,
        template: has_subst.then_some(source),
        confidence: if has_subst {
            Confidence::Medium
        } else {
            Confidence::Strong
        },
    }
}

fn find_first_string_literal(n: TsNode, src: &[u8]) -> Option<String> {
    let mut stack = vec![n];
    while let Some(node) = stack.pop() {
        if node.kind() == "string" {
            return Some(text(node, src).to_string());
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

/// `fetch(url, { method: 'POST', … })` — pluck a string-literal `method:` value
/// from the second-arg object literal. Returns None if the second arg isn't a
/// plain object or the method value isn't a string literal.
fn fetch_method_from_opts(opts: Option<TsNode>, src: &[u8]) -> Option<String> {
    let opts = opts?;
    if opts.kind() != "object" {
        return None;
    }
    let mut cursor = opts.walk();
    for prop in opts.named_children(&mut cursor) {
        if prop.kind() != "pair" {
            continue;
        }
        let key = prop.child_by_field_name("key")?;
        let key_text = match key.kind() {
            "property_identifier" => text(key, src),
            "string" => {
                let raw = text(key, src);
                if raw.len() >= 2 {
                    &raw[1..raw.len() - 1]
                } else {
                    raw
                }
            }
            _ => continue,
        };
        if key_text != "method" {
            continue;
        }
        let value = prop.child_by_field_name("value")?;
        if value.kind() != "string" {
            return None;
        }
        return Some(strip_string_quotes(text(value, src)).to_uppercase());
    }
    None
}

fn downgrade(c: Confidence) -> Confidence {
    match c {
        Confidence::Strong => Confidence::Medium,
        Confidence::Medium => Confidence::Weak,
        Confidence::Weak => Confidence::Weak,
    }
}

/// The TS-local ENDPOINT_HIT writer. Field order and the trailing optional
/// `raw` mirror `repo_graph_code_domain::endpoint`'s writer, so a TS and a Dart
/// endpoint carry the same payload shape; `raw` is skipped when `None`, which
/// keeps every un-normalised payload byte-identical.
///
/// `template` (A11.2) follows `raw` and is skipped the same way, so only an
/// endpoint whose argument was a template with a substitution gains a field.
/// The two are different values — see `EndpointCandidate`.
fn endpoint_hit_cell(cand: &EndpointCandidate) -> Cell {
    #[derive(serde::Serialize)]
    struct Payload<'a> {
        method: &'a str,
        path: &'a str,
        file: &'a str,
        line: usize,
        col: usize,
        confidence: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        template: Option<&'a str>,
    }
    let conf_str = match cand.confidence {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    };
    let json = serde_json::to_string(&Payload {
        method: &cand.method,
        path: &cand.path,
        file: &cand.file_rel,
        line: cand.line,
        col: cand.col,
        confidence: conf_str,
        raw: cand.raw_path.as_deref(),
        template: cand.template.as_deref(),
    })
    .unwrap_or_else(|_| String::from("{}"));
    Cell {
        kind: cell_type::ENDPOINT_HIT,
        payload: CellPayload::Json(json),
    }
}

fn extract_call_qualifier(call: TsNode, src: &[u8]) -> Option<CallQualifier> {
    let func = call.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => Some(CallQualifier::Bare(text(func, src).to_string())),
        "member_expression" => {
            let object = func.child_by_field_name("object")?;
            let prop = func.child_by_field_name("property")?;
            let name = text(prop, src).to_string();
            match object.kind() {
                "this" => Some(CallQualifier::SelfMethod(name)),
                "identifier" => Some(CallQualifier::Attribute {
                    base: text(object, src).to_string(),
                    name,
                }),
                _ => Some(CallQualifier::ComplexReceiver {
                    receiver: text(object, src).to_string(),
                    name,
                }),
            }
        }
        _ => None,
    }
}

// ============================================================================
// Intra-file resolution
// ============================================================================

fn resolve_intra_file(mut acc: Acc) -> Result<FileParse, ParseError> {
    let mut out = FileParse {
        nodes: std::mem::take(&mut acc.nodes),
        edges: std::mem::take(&mut acc.edges),
        imports: std::mem::take(&mut acc.imports),
        calls: Vec::new(),
        refs: std::mem::take(&mut acc.refs),
        nav: std::mem::take(&mut acc.nav),
        properties: Default::default(),
    };
    for uc in acc.unresolved {
        let resolved: Option<NodeId> = match &uc.qualifier {
            CallQualifier::Bare(name) => acc.module_functions.get(name).copied(),
            CallQualifier::SelfMethod(name) => uc
                .enclosing_class
                .and_then(|cid| acc.class_methods.get(&(cid, name.clone())).copied()),
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

    // Endpoint emission. Build the import-alias set from `out.imports` so
    // shape-2 candidates (`axios.get(url)`) can be filtered to only those
    // whose base is a real module-level binding.
    let alias_set = build_alias_set(&out.imports);

    // A7.1: `x = inject(T)` class fields. The callee must be the local binding
    // of a named `inject` import (`import { inject }`, or `{ inject as i }`),
    // so a project-local function that happens to be called `inject` emits
    // nothing. Fails closed on a re-export under another name.
    let inject_bindings = inject_import_bindings(&out.imports);
    for cand in acc.inject_fn_candidates {
        if !inject_bindings.contains(cand.callee.as_str()) {
            continue;
        }
        out.refs.push(UnresolvedRef {
            from: cand.class_id,
            from_module: cand.module_id,
            qualifier: CallQualifier::Bare(cand.type_name),
            category: edge_category::INJECTS,
        });
        di_stats::record(DiShape::TsInjectFn);
    }

    let mut endpoint_nav_seen: std::collections::HashSet<NodeId> =
        std::collections::HashSet::new();
    let repo = acc.repo.expect("Acc.repo set in parse_file");
    for cand in acc.endpoints {
        if let Some(req) = cand.requires_import_alias.as_ref()
            && !alias_set.contains(req.as_str())
        {
            continue;
        }
        // `cand.path` is already canonical (`push_endpoint`); the shared
        // builder keeps ONE definition of the qname shape.
        let qname = endpoint::endpoint_qname(&cand.method, &cand.path);
        let endpoint_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENDPOINT, &qname);
        let cell = endpoint_hit_cell(&cand);
        out.nodes.push(Node {
            id: endpoint_id,
            repo,
            confidence: cand.confidence,
            cells: vec![cell],
        });
        if endpoint_nav_seen.insert(endpoint_id) {
            let display = format!("{} {}", cand.method, cand.path);
            out.nav
                .record(endpoint_id, &display, &qname, node_kind::ENDPOINT, None);
        }
        out.edges.push(Edge {
            from: cand.from,
            to: endpoint_id,
            category: edge_category::CALLS,
            confidence: cand.confidence,
        });
    }

    Ok(out)
}

/// Local names bound to an imported `inject` function: `inject` for
/// `import { inject } from …`, `i` for `import { inject as i } from …`.
fn inject_import_bindings(imports: &[ImportStmt]) -> std::collections::HashSet<&str> {
    imports
        .iter()
        .filter_map(|imp| match &imp.target {
            ImportTarget::Symbol { name, alias, .. } if name == "inject" => {
                Some(alias.as_deref().unwrap_or(name.as_str()))
            }
            _ => None,
        })
        .collect()
}

fn build_alias_set(imports: &[ImportStmt]) -> std::collections::HashSet<&str> {
    let mut set = std::collections::HashSet::new();
    for imp in imports {
        match &imp.target {
            ImportTarget::Module {
                alias: Some(a), ..
            } => {
                set.insert(a.as_str());
            }
            ImportTarget::Symbol {
                name,
                alias: Some(a),
                ..
            } => {
                set.insert(a.as_str());
                // For default imports (alias is the binding, name="default"),
                // also keep `name` if useful — skipped to avoid false positives.
                let _ = name;
            }
            ImportTarget::Symbol {
                name, alias: None, ..
            } => {
                set.insert(name.as_str());
            }
            _ => {}
        }
    }
    set
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
    if let Some(doc) = repo_graph_doc::leading_doc(n, src) {
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

fn strip_string_quotes(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"'))
            || (t.starts_with('\'') && t.ends_with('\''))
            || (t.starts_with('`') && t.ends_with('`')))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::EdgeCategoryId;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts_smoke")
    }

    #[test]
    fn angular_constructor_di_emits_injects_refs() {
        // @Component class with constructor DI: AppComponent INJECTS ApiService
        // and HttpClient; the primitive `number` param is skipped.
        let src = "\
import { Component } from \"@angular/core\";
import { ApiService } from \"./api.service\";

@Component({ selector: \"app-root\", template: \"<div></div>\" })
export class AppComponent {
  constructor(private api: ApiService, public http: HttpClient, private tries: number) {}
}
";
        let parse = parse_file(src, "src/app.component.ts", "src::app_component", repo()).unwrap();

        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::app_component");
        let class_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::CLASS,
            "src::app_component::AppComponent",
        );

        let inject_targets: Vec<&str> = parse
            .refs
            .iter()
            .filter(|r| {
                r.category == edge_category::INJECTS
                    && r.from == class_id
                    && r.from_module == module_id
            })
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            inject_targets.contains(&"ApiService"),
            "expected INJECTS ref Bare(\"ApiService\") from AppComponent, got refs: {:?}",
            parse.refs
        );
        assert!(
            inject_targets.contains(&"HttpClient"),
            "expected INJECTS ref Bare(\"HttpClient\"), got: {inject_targets:?}"
        );
        assert!(
            !inject_targets.contains(&"number"),
            "primitive-typed ctor param must not emit INJECTS, got: {inject_targets:?}"
        );
    }

    #[test]
    fn plain_class_ctor_does_not_emit_injects() {
        // No DI decorator → no INJECTS refs, even with typed ctor params.
        let src = "\
export class PlainData {
  constructor(private svc: SomeService) {}
}
";
        let parse = parse_file(src, "src/plain.ts", "src::plain", repo()).unwrap();
        assert!(
            !parse
                .refs
                .iter()
                .any(|r| r.category == edge_category::INJECTS),
            "undecorated class must not mint INJECTS refs, got: {:?}",
            parse.refs
        );
    }

    /// Bare-name INJECTS targets emitted from `class_qname`.
    fn inject_targets(parse: &FileParse, module_qname: &str, class_qname: &str) -> Vec<String> {
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module_qname);
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, class_qname);
        parse
            .refs
            .iter()
            .filter(|r| {
                r.category == edge_category::INJECTS
                    && r.from == class_id
                    && r.from_module == module_id
            })
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn angular_inject_fn_field_emits_injects_ref() {
        let body = "\
import { ApiService } from \"./api.service\";
import * as models from \"./models\";

@Component({ selector: \"app-dash\", template: \"<div></div>\" })
export class DashboardComponent {
  private api = inject(ApiService);
  private store = inject<Store>(models.StoreToken);
  private tries = 3;
  private label = String(\"x\");
  private cfg = inject(\"CONFIG\");
  load() { return this.api.list(); }
}
";
        let with_import = format!("import {{ Component, inject }} from \"@angular/core\";\n{body}");
        let parse = parse_file(&with_import, "src/dash.component.ts", "src::dash", repo()).unwrap();
        let targets = inject_targets(&parse, "src::dash", "src::dash::DashboardComponent");
        assert_eq!(
            targets,
            vec!["ApiService".to_string(), "StoreToken".to_string()],
            "inject(T) fields -> INJECTS Bare(T); literal / non-inject initialisers emit nothing"
        );

        // Aliased import still binds.
        let aliased = format!(
            "import {{ Component, inject as di }} from \"@angular/core\";\n{}",
            body.replace("inject(", "di(").replace("inject<", "di<")
        );
        let parse = parse_file(&aliased, "src/dash.component.ts", "src::dash", repo()).unwrap();
        assert_eq!(
            inject_targets(&parse, "src::dash", "src::dash::DashboardComponent"),
            vec!["ApiService".to_string(), "StoreToken".to_string()],
        );

        // Without the `inject` import the same call is a local function: no ref.
        let without = format!("import {{ Component }} from \"@angular/core\";\n{body}");
        let parse = parse_file(&without, "src/dash.component.ts", "src::dash", repo()).unwrap();
        assert!(
            !parse.refs.iter().any(|r| r.category == edge_category::INJECTS),
            "un-imported inject() must not mint INJECTS refs, got: {:?}",
            parse.refs
        );
    }

    #[test]
    fn nest_controller_ctor_emits_injects_ref() {
        let src = "\
import { Controller, Get } from \"@nestjs/common\";
import { UsersService } from \"./users.service\";

@Controller(\"users\")
export class UsersController {
  constructor(private readonly users: UsersService, private readonly pageSize: number) {}
  @Get()
  findAll() { return this.users.findAll(); }
}

@Resolver(() => User)
export class UsersResolver {
  constructor(private readonly users: UsersService) {}
}

@WebSocketGateway()
export class EventsGateway {
  constructor(private readonly users: UsersService) {}
}
";
        let parse = parse_file(src, "src/users.controller.ts", "src::users", repo()).unwrap();
        for class in ["UsersController", "UsersResolver", "EventsGateway"] {
            assert_eq!(
                inject_targets(&parse, "src::users", &format!("src::users::{class}")),
                vec!["UsersService".to_string()],
                "{class}: Nest ctor DI -> INJECTS Bare(\"UsersService\"); `number` skipped"
            );
        }
    }

    #[test]
    fn di_param_decorator_gates_undecorated_ctor() {
        // No class decorator, but an `@Inject` param decorator marks the ctor
        // as an injection site. `string` stays skipped as a primitive.
        let src = "\
export class ReportJob {
  constructor(@Inject(API_URL) private url: string, @Optional() private log: Logger) {}
}
";
        let parse = parse_file(src, "src/report.ts", "src::report", repo()).unwrap();
        assert_eq!(
            inject_targets(&parse, "src::report", "src::report::ReportJob"),
            vec!["Logger".to_string()],
        );
    }

    fn has_edge(parse: &FileParse, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> bool {
        parse
            .edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == cat)
    }

    #[test]
    fn parses_module_with_function_decl_and_arrow_const() {
        let src = "\
function greet(name: string): string {
    return hello(name);
}

const hello = (n: string) => `hi ${n}`;
";
        let parse = parse_file(src, "src/greet.ts", "src::greet", repo()).unwrap();

        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::greet");
        let greet_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "src::greet::greet",
        );
        let hello_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "src::greet::hello",
        );

        assert!(parse.nodes.iter().any(|n| n.id == module_id));
        assert!(parse.nodes.iter().any(|n| n.id == greet_id));
        assert!(
            parse.nodes.iter().any(|n| n.id == hello_id),
            "arrow const should be hoisted to a Function node"
        );

        assert!(has_edge(&parse, module_id, greet_id, edge_category::DEFINES));
        assert!(has_edge(&parse, module_id, hello_id, edge_category::DEFINES));

        // Intra-file bare call: greet → hello
        assert!(
            has_edge(&parse, greet_id, hello_id, edge_category::CALLS),
            "expected bare call to resolve intra-file, calls: {:?}",
            parse.calls
        );
    }

    #[test]
    fn parses_class_methods_and_this_call() {
        let src = "\
export class User {
    login(password: string): boolean {
        return hashPassword(password).length > 0;
    }

    save(): void {
        this.login(\"x\");
    }
}
";
        let parse = parse_file(src, "src/user.ts", "src::user", repo()).unwrap();

        let mod_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::user");
        let class_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "src::user::User");
        let login_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "src::user::User::login",
        );
        let save_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "src::user::User::save",
        );

        assert!(parse.nodes.iter().any(|n| n.id == class_id));
        assert!(parse.nodes.iter().any(|n| n.id == login_id));
        assert!(parse.nodes.iter().any(|n| n.id == save_id));

        assert!(has_edge(&parse, mod_id, class_id, edge_category::DEFINES));
        assert!(has_edge(&parse, class_id, login_id, edge_category::DEFINES));
        assert!(has_edge(&parse, class_id, save_id, edge_category::DEFINES));

        // this.login() inside save resolves to User::login.
        assert!(
            has_edge(&parse, save_id, login_id, edge_category::CALLS),
            "this.login should resolve to User::login"
        );

        // hashPassword() inside login — cross-file, stays unresolved.
        assert!(
            parse.calls.iter().any(|c| c.from == login_id
                && matches!(&c.qualifier, CallQualifier::Bare(n) if n == "hashPassword")),
            "hashPassword should be unresolved, got: {:?}",
            parse.calls
        );
    }

    #[test]
    fn collects_all_import_shapes() {
        let src = "\
import \"./polyfill\";
import Default from \"./default-src\";
import * as ns from \"./ns-src\";
import { a, b as c } from \"./named-src\";
import { UserService } from \"@angular/core\";
";
        let parse = parse_file(src, "src/index.ts", "src::index", repo()).unwrap();

        // Side-effect import — module path, no alias.
        assert!(
            parse.imports.iter().any(|i| matches!(
                &i.target,
                ImportTarget::Module { path, alias: None } if path == "./polyfill"
            )),
            "side-effect import missing"
        );

        // Default import — symbol named "default" with alias.
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Symbol { module, name, alias: Some(a), level: 0 }
                if module == "./default-src" && name == "default" && a == "Default"
        )));

        // Namespace import — module with alias.
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Module { path, alias: Some(a) }
                if path == "./ns-src" && a == "ns"
        )));

        // Named, plain `a`.
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Symbol { module, name, alias: None, level: 0 }
                if module == "./named-src" && name == "a"
        )));

        // Named with alias, `b as c`.
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Symbol { module, name, alias: Some(a), level: 0 }
                if module == "./named-src" && name == "b" && a == "c"
        )));

        // Bare-module import (no leading dot).
        assert!(parse.imports.iter().any(|i| matches!(
            &i.target,
            ImportTarget::Symbol { module, name, .. }
                if module == "@angular/core" && name == "UserService"
        )));
    }

    #[test]
    fn interface_and_attribute_call_unresolved() {
        let src = "\
interface Greeter {
    hello(name: string): string;
}

export function doGreet(g: Greeter) {
    return g.hello(\"x\");
}
";
        let parse = parse_file(src, "src/g.ts", "src::g", repo()).unwrap();

        let iface_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, "src::g::Greeter");
        let fn_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "src::g::doGreet",
        );

        assert!(
            parse.nodes.iter().any(|n| n.id == iface_id),
            "interface node missing"
        );
        assert!(parse.nodes.iter().any(|n| n.id == fn_id));

        // g.hello(...) is an Attribute-qualified call — unresolved at parse time.
        assert!(
            parse.calls.iter().any(|c| c.from == fn_id
                && matches!(
                    &c.qualifier,
                    CallQualifier::Attribute { base, name } if base == "g" && name == "hello"
                )),
            "attribute call missing, got: {:?}",
            parse.calls
        );
    }

    #[test]
    fn syntax_error_produces_partial_graph() {
        // tree-sitter's error recovery still yields the valid top-level function.
        let src = "function ok(): void {}\n\nthis is !!! not typescript\n";
        let parse = parse_file(src, "broken.ts", "broken", repo()).unwrap();
        let ok_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "broken::ok");
        assert!(parse.nodes.iter().any(|n| n.id == ok_id));
    }

    // ========================================================================
    // Endpoint extraction (v0.4.4)
    // ========================================================================

    fn endpoint_id(repo: RepoId, method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::ENDPOINT,
            &format!("endpoint:{method}:{path}"),
        )
    }

    fn endpoint_payloads(parse: &FileParse, ep: NodeId) -> Vec<serde_json::Value> {
        parse
            .nodes
            .iter()
            .filter(|n| n.id == ep)
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => serde_json::from_str(s).ok(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn this_http_post_string_literal_emits_strong_endpoint() {
        let src = "\
export class AuthService {
    constructor(private readonly http: any) {}
    login(payload: any): void {
        this.http.post('/api/auth/login', payload);
    }
}
";
        let parse = parse_file(src, "src/auth.service.ts", "src::auth::service", repo()).unwrap();

        let ep = endpoint_id(repo(), "POST", "/api/auth/login");
        let login_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "src::auth::service::AuthService::login",
        );

        assert!(
            parse.nodes.iter().any(|n| n.id == ep),
            "endpoint node missing"
        );
        assert!(
            parse
                .nodes
                .iter()
                .find(|n| n.id == ep)
                .map(|n| n.confidence == Confidence::Strong)
                .unwrap_or(false),
            "string literal arg should emit Strong endpoint"
        );
        assert!(
            has_edge(&parse, login_id, ep, edge_category::CALLS),
            "expected CALLS edge from login method to endpoint"
        );

        let payloads = endpoint_payloads(&parse, ep);
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["method"], "POST");
        assert_eq!(payloads[0]["path"], "/api/auth/login");
        assert_eq!(payloads[0]["confidence"], "strong");
    }

    #[test]
    fn fetch_defaults_to_get_unless_method_overridden() {
        let src = "\
function loadUsers(): void {
    fetch('/api/users');
    fetch('/api/users', { method: 'POST' });
}
";
        let parse = parse_file(src, "src/loader.ts", "src::loader", repo()).unwrap();

        let get_ep = endpoint_id(repo(), "GET", "/api/users");
        let post_ep = endpoint_id(repo(), "POST", "/api/users");

        assert!(parse.nodes.iter().any(|n| n.id == get_ep), "GET missing");
        assert!(
            parse.nodes.iter().any(|n| n.id == post_ep),
            "POST override missing"
        );
    }

    #[test]
    fn axios_get_only_emits_when_axios_imported() {
        // With import — emits.
        let with_import = "\
import axios from 'axios';
export function loadHealth(): void {
    axios.get('/health');
}
";
        let parse = parse_file(with_import, "src/h.ts", "src::h", repo()).unwrap();
        assert!(
            parse
                .nodes
                .iter()
                .any(|n| n.id == endpoint_id(repo(), "GET", "/health")),
            "axios.get with import should emit endpoint"
        );

        // Without import — `axios` could be a local variable; skip.
        let without_import = "\
export function loadHealth(axios: any): void {
    axios.get('/health');
}
";
        let parse2 = parse_file(without_import, "src/h2.ts", "src::h2", repo()).unwrap();
        assert!(
            !parse2
                .nodes
                .iter()
                .any(|n| n.id == endpoint_id(repo(), "GET", "/health")),
            "axios.get without import should be skipped (could be a local)"
        );
    }

    #[test]
    fn template_with_interpolation_emits_medium_with_placeholder_path() {
        let src = "\
export class UserService {
    constructor(private readonly http: any) {}
    show(id: string): void {
        this.http.get(`/api/users/${id}`);
    }
}
";
        let parse = parse_file(src, "src/user.service.ts", "src::user::service", repo()).unwrap();

        let ep = endpoint_id(repo(), "GET", "/api/users/${…}");
        assert!(
            parse.nodes.iter().any(|n| n.id == ep),
            "templated endpoint with placeholder missing"
        );
        let node = parse.nodes.iter().find(|n| n.id == ep).unwrap();
        assert_eq!(node.confidence, Confidence::Medium);
    }

    #[test]
    fn url_builder_wrapper_pluck_inner_literal_weak() {
        let src = "\
export class AuthService {
    constructor(private readonly http: any, private readonly api: any) {}
    login(payload: any): void {
        this.http.post(this.api.buildApiUrl('auth/login'), payload);
    }
}
";
        let parse = parse_file(src, "src/auth.ts", "src::auth", repo()).unwrap();

        // Inner literal 'auth/login' becomes the path hint, Weak confidence,
        // with its one canonical leading `/` (LB.5).
        let ep = endpoint_id(repo(), "POST", "/auth/login");
        assert!(
            parse.nodes.iter().any(|n| n.id == ep),
            "URL-builder wrapped endpoint missing"
        );
        let node = parse.nodes.iter().find(|n| n.id == ep).unwrap();
        assert_eq!(node.confidence, Confidence::Weak);
    }

    /// A3.3 — an absolute URL with a query string is keyed on its request path,
    /// and the literal it came from rides on ENDPOINT_HIT as `raw`.
    #[test]
    fn absolute_url_endpoint_strips_host_and_query() {
        let src = "\
export const listUsers = () => fetch('https://api.example.com/users?active=1');
";
        let parse = parse_file(src, "src/api.ts", "src::api", repo()).unwrap();

        let ep = endpoint_id(repo(), "GET", "/users");
        assert!(
            parse.nodes.iter().any(|n| n.id == ep),
            "expected endpoint:GET:/users, got {:?}",
            parse.nav.qname_by_id.values().collect::<Vec<_>>()
        );
        let stale = endpoint_id(repo(), "GET", "https://api.example.com/users?active=1");
        assert!(
            !parse.nodes.iter().any(|n| n.id == stale),
            "host + query must not survive into the endpoint qname"
        );
        let payloads = endpoint_payloads(&parse, ep);
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["path"], "/users");
        assert_eq!(payloads[0]["raw"], "https://api.example.com/users?active=1");
        // Normalising the path is not a confidence change.
        assert_eq!(payloads[0]["confidence"], "strong");

        // Same through the member-call shape and a template literal.
        let src2 = "\
export class UserService {
    constructor(private readonly http: any) {}
    show(id: string): void {
        this.http.get(`https://api.example.com/users/${id}?expand=${id}`);
    }
}
";
        let parse2 = parse_file(src2, "src/user.service.ts", "src::user::service", repo()).unwrap();
        let ep2 = endpoint_id(repo(), "GET", "/users/${…}");
        let payloads2 = endpoint_payloads(&parse2, ep2);
        assert_eq!(payloads2.len(), 1, "templated absolute URL must normalise too");
        assert_eq!(
            payloads2[0]["raw"],
            "https://api.example.com/users/${…}?expand=${…}"
        );
    }

    /// A3.3 regression guard — the normaliser must never touch a path it
    /// cannot improve: a plain path and an interpolated base keep their qname
    /// AND a payload with no `raw` key. LB.5: a relative URL-builder hint is
    /// the one rewrite — it gains a leading `/` and records the literal as
    /// `raw`.
    #[test]
    fn relative_builder_hint_is_untouched() {
        let src = "\
export class AuthService {
    constructor(private readonly http: any, private readonly api: any) {}
    login(payload: any): void {
        this.http.post(this.api.buildApiUrl('auth/login'), payload);
        this.http.get('/api/users');
        this.http.get(`${this.base}/users`);
    }
}
";
        let parse = parse_file(src, "src/auth.ts", "src::auth", repo()).unwrap();
        let hint = endpoint_payloads(&parse, endpoint_id(repo(), "POST", "/auth/login"));
        assert_eq!(hint.len(), 1, "relative hint must be keyed on /auth/login");
        assert_eq!(hint[0]["path"], "/auth/login");
        assert_eq!(hint[0]["raw"], "auth/login");
        assert!(
            !parse.nodes.iter().any(|n| n.id == endpoint_id(repo(), "POST", "auth/login")),
            "the unslashed id must be gone"
        );
        for (method, path) in [("GET", "/api/users"), ("GET", "${…}/users")] {
            let ep = endpoint_id(repo(), method, path);
            let payloads = endpoint_payloads(&parse, ep);
            assert_eq!(payloads.len(), 1, "missing endpoint:{method}:{path}");
            assert!(
                payloads[0].get("raw").is_none(),
                "unchanged path {path} must not carry raw: {}",
                payloads[0]
            );
        }
    }

    /// LB.5 — a relative and a slashed call to one path are ONE ENDPOINT id
    /// (one nav entry, one ENDPOINT_HIT per call site), and the relative call's
    /// hit records the literal it was rewritten from.
    #[test]
    fn relative_and_slashed_calls_share_one_endpoint() {
        let src = "\
export class SettingsService {
    constructor(private readonly http: any) {}
    remove(): void {
        this.http.delete('protected/settings/account');
    }
    removeAgain(): void {
        this.http.delete('/protected/settings/account');
    }
}
";
        let parse = parse_file(src, "src/settings.ts", "src::settings", repo()).unwrap();
        let ep = endpoint_id(repo(), "DELETE", "/protected/settings/account");
        let payloads = endpoint_payloads(&parse, ep);
        assert_eq!(payloads.len(), 2, "both call sites hit the canonical node");
        assert!(
            payloads
                .iter()
                .all(|p| p["path"] == "/protected/settings/account")
        );
        assert_eq!(
            payloads
                .iter()
                .filter(|p| p["raw"] == "protected/settings/account")
                .count(),
            1
        );
        let endpoints: Vec<&str> = parse
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ENDPOINT)
            .filter_map(|(id, _)| parse.nav.qname_by_id.get(id).map(String::as_str))
            .collect();
        assert_eq!(
            endpoints,
            vec!["endpoint:DELETE:/protected/settings/account"]
        );
        assert_eq!(
            parse.nav.name_by_id.get(&ep).map(String::as_str),
            Some("DELETE /protected/settings/account")
        );
    }

    /// A11.2 — a template argument keeps its substitution SOURCE on the cell as
    /// `template` for the engine's endpoint fold, while the node identity keeps
    /// the `${…}` placeholder. A plain literal gets no `template` key.
    #[test]
    fn template_endpoint_carries_substitution_source() {
        let src = "\
import { environment } from './environment';
export class UserService {
    constructor(private readonly http: any) {}
    list(): void {
        this.http.get(`${environment.apiUrl}/users`);
        this.http.get(`/api/users/${id}?q=${ environment.flag }`);
        this.http.get('/api/health');
        fetch(`http://svc:8080/x/${ROOT}`);
    }
}
";
        let parse = parse_file(src, "src/user.service.ts", "src::user::service", repo()).unwrap();

        let base = endpoint_id(repo(), "GET", "${…}/users");
        let payloads = endpoint_payloads(&parse, base);
        assert_eq!(payloads.len(), 1, "identity is still the placeholder path");
        assert_eq!(payloads[0]["path"], "${…}/users");
        assert_eq!(payloads[0]["template"], "${environment.apiUrl}/users");
        assert!(payloads[0].get("raw").is_none(), "{}", payloads[0]);

        let tail = endpoint_id(repo(), "GET", "/api/users/${…}");
        let payloads = endpoint_payloads(&parse, tail);
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["template"], "/api/users/${id}?q=${ environment.flag }");
        assert_eq!(payloads[0]["raw"], "/api/users/${…}?q=${…}");

        let plain = endpoint_id(repo(), "GET", "/api/health");
        let payloads = endpoint_payloads(&parse, plain);
        assert_eq!(payloads.len(), 1);
        assert!(payloads[0].get("template").is_none(), "{}", payloads[0]);

        let abs = endpoint_id(repo(), "GET", "/x/${…}");
        let payloads = endpoint_payloads(&parse, abs);
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["template"], "http://svc:8080/x/${ROOT}");
        assert_eq!(payloads[0]["raw"], "http://svc:8080/x/${…}");
    }

    #[test]
    fn multiple_callsites_same_method_path_collapse_with_stacked_cells() {
        let src = "\
export class HealthService {
    constructor(private readonly http: any) {}
    pollA(): void { this.http.get('/api/health'); }
    pollB(): void { this.http.get('/api/health'); }
}
";
        let parse =
            parse_file(src, "src/health.service.ts", "src::health::service", repo()).unwrap();

        let ep = endpoint_id(repo(), "GET", "/api/health");
        // Parser emits two Node entries — graph-build merges them into one with
        // two stacked cells.
        let occurrences = parse.nodes.iter().filter(|n| n.id == ep).count();
        assert_eq!(occurrences, 2, "expected 2 Node emissions for same endpoint");
        let payloads = endpoint_payloads(&parse, ep);
        assert_eq!(payloads.len(), 2, "expected 2 ENDPOINT_HIT cells");
    }

    #[test]
    fn fetch_nested_in_react_callbacks_emits_endpoints() {
        // React puts fetch() inside a useEffect(() => {…}) callback and inside
        // an async arrow. Both are anonymous callbacks nested in the component
        // function body — the call walk must descend into them so the fetch is
        // detected and attributed to the enclosing component function.
        let src = "\
import { useEffect, useState } from \"react\";

export function UserList() {
    const [users, setUsers] = useState([]);

    useEffect(() => {
        fetch(\"/users\")
            .then((r) => r.json())
            .then(setUsers);
    }, []);

    const addUser = async (body) => {
        await fetch(\"/users\", { method: \"POST\", body: JSON.stringify(body) });
    };

    return null;
}
";
        let parse = parse_file(src, "client/UserList.tsx", "client::UserList", repo()).unwrap();

        let get_ep = endpoint_id(repo(), "GET", "/users");
        let post_ep = endpoint_id(repo(), "POST", "/users");
        let comp_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "client::UserList::UserList",
        );

        // GET from the useEffect callback.
        assert!(
            parse.nodes.iter().any(|n| n.id == get_ep),
            "fetch inside useEffect callback should emit a GET endpoint, nodes: {:?}",
            parse.nodes.iter().map(|n| n.id).collect::<Vec<_>>()
        );
        // POST from the addUser async arrow.
        assert!(
            parse.nodes.iter().any(|n| n.id == post_ep),
            "fetch inside the async arrow should emit a POST endpoint"
        );

        // Both CALLS edges are attributed to the enclosing component function.
        assert!(
            has_edge(&parse, comp_id, get_ep, edge_category::CALLS),
            "expected CALLS edge UserList -> GET /users"
        );
        assert!(
            has_edge(&parse, comp_id, post_ep, edge_category::CALLS),
            "expected CALLS edge UserList -> POST /users"
        );

        // No double-emit: exactly one Node emission per (method,path).
        assert_eq!(
            parse.nodes.iter().filter(|n| n.id == get_ep).count(),
            1,
            "GET /users must be emitted exactly once (no double-emit)"
        );
        assert_eq!(
            parse.nodes.iter().filter(|n| n.id == post_ep).count(),
            1,
            "POST /users must be emitted exactly once (no double-emit)"
        );
    }

    #[test]
    fn g195_exported_const_state_var_and_implements_edge() {
        // G19: documented exported const → STATE_VAR node with a DOC cell.
        // G12.5: `implements I` → IMPLEMENTS edge; `extends Y` → INHERITS_FROM.
        let src = "\
/** Fee. */
export const FEE_BPS = 250;

export const TAG = 7;

interface IFoo {}

class X extends Base implements IFoo {}
";
        let parse = parse_file(src, "src/fees.ts", "src::fees", repo()).unwrap();

        let mod_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::fees");

        // FEE_BPS is documented → emitted as STATE_VAR with a DOC cell.
        let fee_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "src::fees::FEE_BPS");
        let fee = parse
            .nodes
            .iter()
            .find(|n| n.id == fee_id)
            .expect("FEE_BPS should be a STATE_VAR node");
        assert!(
            has_edge(&parse, mod_id, fee_id, edge_category::DEFINES),
            "module should DEFINE the const"
        );
        assert!(
            fee.cells.iter().any(|c| c.kind == cell_type::DOC
                && matches!(&c.payload, CellPayload::Text(t) if t.contains("Fee"))),
            "FEE_BPS should carry its JSDoc, cells: {:?}",
            fee.cells
        );

        // TAG is an undocumented literal-primitive number → gated out.
        let tag_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "src::fees::TAG");
        assert!(
            !parse.nodes.iter().any(|n| n.id == tag_id),
            "undocumented numeric const should be suppressed by the noise gate"
        );

        // G12.5: class X implements IFoo → IMPLEMENTS; extends Base → INHERITS_FROM.
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "src::fees::X");
        let iface_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, "src::fees::IFoo");
        let base_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "src::fees::Base");
        assert!(
            has_edge(&parse, class_id, iface_id, edge_category::IMPLEMENTS),
            "expected X --IMPLEMENTS--> IFoo, edges: {:?}",
            parse.edges
        );
        assert!(
            has_edge(&parse, class_id, base_id, edge_category::INHERITS_FROM),
            "expected X --INHERITS_FROM--> Base"
        );
    }
}
