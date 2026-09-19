//! glia-parser-typescript — tree-sitter TypeScript → `glia_core` types.
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
//! All code-domain primitives live in `glia-code-domain` and are
//! re-exported from this crate for convenience.

use std::collections::HashMap;

use glia_code_domain::data_entity;
use glia_code_domain::di_stats::{self, DiShape};
use glia_code_domain::endpoint;
use glia_code_domain::evidence;
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
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
    acc.module_id = Some(module_id);
    acc.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&root, src, file_rel_path),
    });
    let module_simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    acc.nav
        .record(module_id, module_simple, module_qname, node_kind::MODULE, None);

    // A13.15: ES imports are hoisted, so a `typeorm` import below the first
    // `@Entity` class still gates it; read them all before any visit.
    (acc.typeorm.import, acc.typeorm.access) = typeorm_imports(root, src);

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        visit_top(child, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    }

    let orm = &acc.typeorm;
    if orm.declared > 0 || orm.repo_calls > 0 {
        eprintln!(
            "[orm-typeorm] entities={} table_cells={} repo_calls={} file={file_rel_path}",
            orm.declared, orm.table_cells, orm.repo_calls
        );
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
    /// LA.30c: this file's enums by bare name. A second declaration of the
    /// same name (TS declaration merging) reuses the entry.
    enums: HashMap<String, NodeId>,
    /// LA.30c: `(enum name, member name)` -> the member's ATTRIBUTE node.
    enum_members: HashMap<(String, String), NodeId>,
    /// LA.30c: `X.Y` reads that are not a call's callee, as
    /// `(from, X, Y, row)`, deduped per `(from, X, Y)` in walk order (the row
    /// is the first read's, LC.3b). Resolved into USES edges / refs in
    /// `resolve_intra_file` once every enum is known.
    member_refs: Vec<(NodeId, String, String, u32)>,
    member_ref_seen: std::collections::HashSet<(NodeId, String, String)>,
    /// The file's MODULE node: `from_module` of the member USES refs.
    module_id: Option<NodeId>,
    nav: CodeNav,
    /// Stashed at parse_file entry so endpoint emission can stamp position
    /// cells without threading `file_rel` through every call_collection helper.
    file_rel: String,
    repo: Option<RepoId>,
    /// A13.15: this file's TypeORM evidence and the `[orm-typeorm]` counters.
    typeorm: TypeOrmFile,
}

/// A13.15: what one file shows of TypeORM. The two import flags gate the
/// detectors, because `@Entity` is also a Mikro-ORM / Nest decorator and
/// `getRepository` / `manager.find` are generic names elsewhere.
#[derive(Default)]
struct TypeOrmFile {
    /// The file imports `typeorm` (or a `typeorm/…` subpath): gates `@Entity`.
    import: bool,
    /// The file imports `typeorm` or `@nestjs/typeorm`: gates the query sites.
    access: bool,
    /// Entity ids this file has pushed a node for, so each is pushed once.
    entities: std::collections::HashSet<NodeId>,
    /// `(from, entity)` ACCESSES_DATA edges already emitted.
    access_seen: std::collections::HashSet<(NodeId, NodeId)>,
    /// `@Entity` classes declared here (`entities=`).
    declared: usize,
    /// Declarations whose decorator names the table (`table_cells=`).
    table_cells: usize,
    /// Repository / manager / `@InjectRepository` sites (`repo_calls=`).
    repo_calls: usize,
}

struct UnresolvedCall {
    from: NodeId,
    enclosing_class: Option<NodeId>,
    qualifier: CallQualifier,
    /// 0-based row of the call expression (LC.3b).
    line: u32,
}

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
}

/// The engine's routing tag for a file this parser reads, for the
/// `parser:<tag>` emitter of the CALLS edges it resolves itself (LC.3b), so
/// they name the same parser as the file's other edges, which the engine
/// stamps with the tag. The angular and vue crates parse through
/// [`parse_file`]; the split mirrors the engine's `detect_language`.
fn lang_tag(file_rel: &str) -> &'static str {
    if file_rel.ends_with(".vue") {
        "vue"
    } else if file_rel.contains(".component.ts") {
        "angular"
    } else {
        "typescript"
    }
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
    /// A6.2b: `(field name, declared type)` for an unannotated field, recorded
    /// on `CodeNav::field_types` once the callee passes the `inject` import gate.
    field_type: Option<(String, String)>,
    /// 0-based row of the `inject(...)` call (LC.3b).
    line: u32,
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
        // `enum` and `const enum` share this node; `export enum` arrives
        // through the export_statement recursion above. `declare enum` is an
        // `ambient_declaration` and deliberately falls through to `_`.
        "enum_declaration" => {
            visit_enum(n, src, file_rel, module_qname, module_id, repo, acc);
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
        cells: Vec::new(),
    });
    acc.nav
        .record(class_id, name, &class_qname, node_kind::CLASS, Some(module_id));

    // Class heritage: `class X extends Y implements I, J`.
    // `extends_clause` → INHERITS_FROM, each type in `implements_clause` →
    // IMPLEMENTS (class → interface). The superclass / interfaces are usually
    // cross-file, so they are recorded as refs and bound by the graph crate.
    collect_class_heritage(n, src, class_id, module_id, acc);
    // A13.15: `@Entity(…) class User` -> the model-keyed entity it defines.
    emit_typeorm_entity(n, name, class_id, src, repo, acc);

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
    // A6.2b: every constructor parameter's declared type, decorated class or
    // not, so `this.api.fetchUser()` binds on `api`'s type.
    collect_ctor_param_field_types(body, src, class_id, acc);
}

/// The class body's `constructor` method, if it declares one.
fn find_constructor<'t>(body: TsNode<'t>, src: &[u8]) -> Option<TsNode<'t>> {
    let mut cursor = body.walk();
    body.named_children(&mut cursor).find(|m| {
        m.kind() == "method_definition" && child_text(*m, "name", src) == Some("constructor")
    })
}

/// A6.2b: record each constructor parameter's declared class type as a field
/// type of `class_id` on `acc.nav.field_types`, so A6.2a's receiver pass in
/// `resolve_calls` binds `this.api.fetchUser()` (a `ComplexReceiver` with
/// receiver `this.api`) to `fetchUser` on `api`'s type. Parameter properties
/// (`private api: ApiService`) are fields by declaration; a plain
/// `constructor(api: ApiService)` that assigns `this.api = api` has the same
/// shape, so every param counts, decorated class or not. Destructuring and rest
/// patterns name no single field; primitive and non-class types have no method
/// table to bind against (`annotated_class_type`).
fn collect_ctor_param_field_types(body: TsNode, src: &[u8], class_id: NodeId, acc: &mut Acc) {
    let Some(params) =
        find_constructor(body, src).and_then(|c| c.child_by_field_name("parameters"))
    else {
        return;
    };
    let mut pc = params.walk();
    for param in params.named_children(&mut pc) {
        if !matches!(param.kind(), "required_parameter" | "optional_parameter") {
            continue;
        }
        let Some(pattern) = param.child_by_field_name("pattern") else {
            continue;
        };
        if pattern.kind() != "identifier" {
            continue;
        }
        if let Some(type_name) = param
            .child_by_field_name("type")
            .and_then(|t| annotated_class_type(t, src))
        {
            acc.nav
                .record_field_type(class_id, text(pattern, src), type_name);
        }
    }
}

/// A6.2b: the class-shaped type a `type_annotation` declares, as a simple name
/// (`ns.Foo` / `Foo<T>` -> `Foo`, via `heritage_type_name`). A nullable union
/// (`Foo | null`, `Foo | undefined`) is its one non-nullish member. Primitives
/// (`predefined_type`), arrays, tuples, function / object / literal types and
/// multi-type unions own no method table, so they return `None`.
fn annotated_class_type<'a>(ty_ann: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let mut tc = ty_ann.walk();
    let ty = ty_ann.named_children(&mut tc).next()?;
    class_type_name(ty, src)
}

fn class_type_name<'a>(ty: TsNode, src: &'a [u8]) -> Option<&'a str> {
    match ty.kind() {
        "type_identifier" | "nested_type_identifier" | "generic_type" => {
            heritage_type_name(ty, src)
        }
        "union_type" => {
            let mut uc = ty.walk();
            let mut members = ty.named_children(&mut uc).filter(|m| {
                !(m.kind() == "literal_type"
                    && matches!(text(*m, src).trim(), "null" | "undefined"))
            });
            let only = members.next()?;
            if members.next().is_some() {
                return None;
            }
            class_type_name(only, src)
        }
        _ => None,
    }
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
    let Some(ctor) = find_constructor(body, src) else {
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
            line: line_at(param),
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
    let names: Vec<&str> = class_decorators(class_node)
        .into_iter()
        .filter_map(|dec| decorator_name(dec, src))
        .collect();
    if names.iter().any(|n| NEST_DI_DECORATORS.contains(n)) {
        Some(DiShape::TsNestCtor)
    } else if names.iter().any(|n| DI_DECORATORS.contains(n)) {
        Some(DiShape::TsCtor)
    } else {
        None
    }
}

/// A class's decorators, which attach either directly to the
/// `class_declaration` (`@Injectable() class Foo {}`) or to the parent
/// `export_statement` (`@Component({...}) export class Foo {}`).
fn class_decorators<'t>(class_node: TsNode<'t>) -> Vec<TsNode<'t>> {
    let mut decorators = Vec::new();
    let mut collect = |node: TsNode<'t>| {
        let mut c = node.walk();
        decorators.extend(
            node.named_children(&mut c)
                .filter(|ch| ch.kind() == "decorator"),
        );
    };
    collect(class_node);
    if let Some(parent) = class_node.parent() {
        collect(parent);
    }
    decorators
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

/// Visit one class field (`public_field_definition`).
///
/// A6.2b: a field with a class-shaped type annotation (`private api:
/// ApiService;`, `api?: ApiService | null`, `#api: ApiService`) records that
/// type as a field type of `class_id`, like a constructor parameter property.
///
/// A7.1: a field initialised by a bare call with a type-naming argument,
/// `private api = inject(ApiService)` (also `inject<T>(TOKEN)` and
/// `inject(ns.Foo)`), becomes an [`InjectFnCandidate`]. It is not gated on a
/// class decorator: `inject()` is only legal in an injection context, and the
/// import gate in `resolve_intra_file` proves the callee is `inject`. An
/// unannotated one carries its field type (the `<T>` argument, else the
/// argument) to that gate, which records it (A6.2b).
fn visit_field(field: TsNode, src: &[u8], module_id: NodeId, class_id: NodeId, acc: &mut Acc) {
    // A13.15: property injection, `@InjectRepository(User) repo: Repository<User>`.
    // No method encloses a field, so the access is the class's.
    collect_inject_repository(field, src, class_id, acc);
    let field_name = field.child_by_field_name("name").map(|n| text(n, src));
    let annotated = field
        .child_by_field_name("type")
        .and_then(|t| annotated_class_type(t, src));
    if let (Some(name), Some(type_name)) = (field_name, annotated) {
        acc.nav.record_field_type(class_id, name, type_name);
    }
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
    // `inject<ApiService>(API_TOKEN)` returns the `<T>` (none when `T` is not
    // class-shaped: `inject<string>(API_URL)`); `inject(ApiService)` returns
    // the argument's class. An annotation, already recorded, wins.
    let field_type = match (field_name, annotated) {
        (Some(name), None) => match value.child_by_field_name("type_arguments") {
            Some(ta) => {
                let mut tc = ta.walk();
                let first = ta.named_children(&mut tc).next();
                first.and_then(|t| class_type_name(t, src))
            }
            None => Some(type_name),
        }
        .map(|t| (name.to_string(), t.to_string())),
        _ => None,
    };
    acc.inject_fn_candidates.push(InjectFnCandidate {
        class_id,
        module_id,
        callee: text(callee, src).to_string(),
        type_name: type_name.to_string(),
        field_type,
        line: line_at(value),
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
/// `implements_clause` → IMPLEMENTS (class → interface). The parser only
/// EXTRACTS: each supertype becomes an `UnresolvedRef` with a
/// `Bare(<simple name>)` qualifier, and the graph crate's `resolve_refs` binds
/// it — through the module's import bindings (`import { Base } from "./base"`),
/// then the module's own symbols (a same-file base), then a unique repo-wide
/// type name. An unbindable base (`extends Component` from a package) stays in
/// `unresolved_refs` instead of becoming an edge into a fabricated NodeId.
/// Same shape as parser-java's `emit_heritage_ref`.
fn collect_class_heritage(
    class_node: TsNode,
    src: &[u8],
    class_id: NodeId,
    module_id: NodeId,
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
        let mut tc = clause.walk();
        let (category, types): (_, Vec<TsNode>) = match clause.kind() {
            // The superclass expression(s) live under field `value`; sibling
            // `type_arguments` nodes are skipped by selecting the field.
            "extends_clause" => (
                edge_category::INHERITS_FROM,
                clause.children_by_field_name("value", &mut tc).collect(),
            ),
            "implements_clause" => {
                (edge_category::IMPLEMENTS, clause.named_children(&mut tc).collect())
            }
            _ => continue,
        };
        for ty in types {
            if let Some(base) = heritage_type_name(ty, src) {
                acc.refs.push(UnresolvedRef {
                    from: class_id,
                    from_module: module_id,
                    qualifier: CallQualifier::Bare(base.to_string()),
                    category,
                    line: line_at(ty),
                });
            }
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
        cells: Vec::new(),
    });
    acc.nav.record(
        iface_id,
        name,
        &iface_qname,
        node_kind::INTERFACE,
        Some(module_id),
    );
    collect_interface_heritage(n, src, iface_id, module_id, acc);
}

/// LD.7a: `interface A extends B, ns.C<T>` → one INHERITS_FROM ref per
/// super-interface (interface → super-interface, the direction of class →
/// superclass). The `extends_type_clause` child (no field name) lists each
/// supertype under field `type`; each becomes the A6.3 heritage shape, an
/// `UnresolvedRef` with a `Bare(<simple name>)` qualifier that `resolve_refs`
/// binds through imports, same-file symbols, then a unique repo-wide type. A
/// merged interface declaration repeating the same `extends` adds no second
/// ref, so declaration merging never doubles the edge.
fn collect_interface_heritage(
    iface: TsNode,
    src: &[u8],
    iface_id: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let mut cursor = iface.walk();
    for clause in iface.named_children(&mut cursor) {
        if clause.kind() != "extends_type_clause" {
            continue;
        }
        let mut tc = clause.walk();
        for ty in clause.children_by_field_name("type", &mut tc) {
            let Some(base) = heritage_type_name(ty, src) else {
                continue;
            };
            let seen = acc.refs.iter().any(|r| {
                r.from == iface_id
                    && r.category == edge_category::INHERITS_FROM
                    && matches!(&r.qualifier, CallQualifier::Bare(b) if b == base)
            });
            if seen {
                continue;
            }
            acc.refs.push(UnresolvedRef {
                from: iface_id,
                from_module: module_id,
                qualifier: CallQualifier::Bare(base.to_string()),
                category: edge_category::INHERITS_FROM,
                line: line_at(ty),
            });
        }
    }
}

/// LA.30c: `enum E { … }` / `const enum E { … }` → an ENUM node (DEFINES from
/// the module) with one ATTRIBUTE per member (HAS_ATTRIBUTE). Kind choice
/// follows LA.3 / Python: no new id.
///
/// Members are the `enum_body`'s `_property_name` children and its
/// `enum_assignment`s' `name`: a `property_identifier` by its text, a `string`
/// unquoted (`'my-key' = 1` → `my-key`). `number` and `computed_property_name`
/// names are skipped. A second declaration of the same enum in this file (TS
/// declaration merging) reuses the ENUM id and skips members already recorded,
/// so `nav.children_of` never lists one member twice.
#[allow(clippy::too_many_arguments)]
fn visit_enum(
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
    let enum_qname = format!("{module_qname}::{name}");
    let enum_id = match acc.enums.get(name) {
        Some(id) => *id,
        None => {
            let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENUM, &enum_qname);
            acc.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: build_cells(&n, src, file_rel),
            });
            acc.edges.push(Edge {
                from: module_id,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            acc.nav
                .record(id, name, &enum_qname, node_kind::ENUM, Some(module_id));
            acc.enums.insert(name.to_string(), id);
            id
        }
    };

    let Some(body) = n.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for m in body.named_children(&mut cursor) {
        let name_node = match m.kind() {
            "enum_assignment" => match m.child_by_field_name("name") {
                Some(nn) => nn,
                None => continue,
            },
            _ => m,
        };
        let member = match name_node.kind() {
            "property_identifier" => text(name_node, src).to_string(),
            "string" => strip_string_quotes(text(name_node, src)),
            // number / computed_property_name / comment: no member name.
            _ => continue,
        };
        if member.is_empty() {
            continue;
        }
        let key = (name.to_string(), member);
        if acc.enum_members.contains_key(&key) {
            continue;
        }
        let member_qname = format!("{enum_qname}::{}", key.1);
        let member_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ATTRIBUTE, &member_qname);
        acc.nodes.push(Node {
            id: member_id,
            repo,
            confidence: Confidence::Strong,
            cells: build_cells(&m, src, file_rel),
        });
        acc.edges.push(Edge {
            from: enum_id,
            to: member_id,
            category: edge_category::HAS_ATTRIBUTE,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav.record(
            member_id,
            &key.1,
            &member_qname,
            node_kind::ATTRIBUTE,
            Some(enum_id),
        );
        acc.enum_members.insert(key, member_id);
    }
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
        cells: Vec::new(),
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

    // A13.15: `constructor(@InjectRepository(User) private repo: …)`.
    if let Some(params) = n.child_by_field_name("parameters") {
        let mut pc = params.walk();
        for param in params.named_children(&mut pc) {
            collect_inject_repository(param, src, method_id, acc);
        }
    }

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
    let doc = glia_doc::leading_doc(&n, src);

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
            cells: Vec::new(),
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
        cells: Vec::new(),
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
        cells: Vec::new(),
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
            line: line_at(n),
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
                    line: line_at(n),
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
                        line: line_at(n),
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
                        line: line_at(n),
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
                    line: line_at(node),
                });
            }
            try_detect_endpoint(node, src, from, acc);
            try_detect_typeorm_access(node, src, from, acc);
        }
        if kind == "member_expression" {
            record_member_ref(node, src, from, acc);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// LA.30c: record `X.Y` — object an `identifier`, property a
/// `property_identifier` — as a candidate enum-member read, unless it is the
/// callee of its parent call (`Api.load()` is a CallSite, never a member use).
/// Which reads become USES is decided in `resolve_intra_file`, once the file's
/// enums and imports are all known.
fn record_member_ref(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    if let Some(parent) = node.parent()
        && parent.kind() == "call_expression"
        && parent.child_by_field_name("function") == Some(node)
    {
        return;
    }
    let (Some(object), Some(property)) = (
        node.child_by_field_name("object"),
        node.child_by_field_name("property"),
    ) else {
        return;
    };
    if object.kind() != "identifier" || property.kind() != "property_identifier" {
        return;
    }
    let (base, name) = (text(object, src).to_string(), text(property, src).to_string());
    if acc.member_ref_seen.insert((from, base.clone(), name.clone())) {
        acc.member_refs.push((from, base, name, line_at(node)));
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
/// `raw` mirror `glia_code_domain::endpoint`'s writer, so a TS and a Dart
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
// TypeORM (A13.15)
// ============================================================================
//
// A TypeORM service issues no SQL strings, so TypeScript's only DATA_ENTITY
// paths (the language-blind mongoose / collection scans) see none of its data
// access. Per A13.1's ORM identity rule an entity is keyed on its MODEL name,
// `data_entity:sql:User`: the one token every query site in any file names
// (`getRepository(User)`, `dataSource.getRepository(User)`,
// `@InjectRepository(User)`, `manager.find(User, …)`). The `@Entity(…)` class
// DEFINES that entity, and a table its decorator names rides a table cell at
// the declaration only. The cell stacks onto the query sites' node at graph
// build and DbResolver joins through it. `@Entity()` names no table: TypeORM's
// default naming strategy (snake_case of the class name) is what DbResolver's
// own fold derives from the model name. TypeORM on the MongoDB driver
// (`@ObjectIdColumn`, `MongoRepository<T>`, `getMongoRepository`, a
// `mongo…Manager`) maps collections, not tables, and is left out rather than
// minted under the `sql` flavor.

/// Callees whose argument #0 is the entity a repository is fetched for, bare
/// (`getRepository(User)`, the pre-0.3 global) or on any receiver
/// (`dataSource.getRepository(User)`, `manager.getTreeRepository(User)`).
/// `getMongoRepository` is left out: its entity is a Mongo collection.
const TYPEORM_REPOSITORY_FNS: &[&str] = &["getRepository", "getTreeRepository"];

/// `EntityManager` methods whose argument #0 is the entity class and that read
/// or write the database (`manager.find(User, { … })`). `create` and `merge`
/// only build instances in memory and are left out.
const TYPEORM_MANAGER_METHODS: &[&str] = &[
    "find",
    "findBy",
    "findOne",
    "findOneBy",
    "findOneOrFail",
    "findOneByOrFail",
    "findAndCount",
    "findAndCountBy",
    "count",
    "countBy",
    "exists",
    "existsBy",
    "sum",
    "average",
    "minimum",
    "maximum",
    "save",
    "insert",
    "update",
    "upsert",
    "delete",
    "softDelete",
    "restore",
    "remove",
    "increment",
    "decrement",
    "preload",
    "clear",
    "createQueryBuilder",
];

/// Whether the file imports TypeORM, as `(entity gate, query-site gate)`. The
/// entity gate takes `typeorm` or a `typeorm/…` subpath; the query-site gate
/// also takes `@nestjs/typeorm`, where `@InjectRepository` comes from.
fn typeorm_imports(root: TsNode, src: &[u8]) -> (bool, bool) {
    let mut typeorm = false;
    let mut nest = false;
    let mut cursor = root.walk();
    for stmt in root.named_children(&mut cursor) {
        if stmt.kind() != "import_statement" {
            continue;
        }
        let Some(source) = stmt.child_by_field_name("source") else {
            continue;
        };
        let source = strip_string_quotes(text(source, src));
        if source == "typeorm" || source.starts_with("typeorm/") {
            typeorm = true;
        } else if source == "@nestjs/typeorm" {
            nest = true;
        }
    }
    (typeorm, typeorm || nest)
}

/// The model-keyed `data_entity:sql:<Model>` qname and id: the one
/// construction site shared by the declaration and every query site.
fn typeorm_entity(model: &str, repo: RepoId) -> (String, NodeId) {
    let qname = format!("data_entity:sql:{model}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
    (qname, id)
}

/// `@Entity(…) class User` in a file importing `typeorm`: push the model-keyed
/// entity owned by the class, a DEFINES edge from the class, and a
/// `data_entity::table_cell` when the decorator names the table. A query site
/// earlier in the file may have pushed the node already; the declaration then
/// carries the cell onto that same node.
fn emit_typeorm_entity(
    class_node: TsNode,
    class_name: &str,
    class_id: NodeId,
    src: &[u8],
    repo: RepoId,
    acc: &mut Acc,
) {
    if !acc.typeorm.import {
        return;
    }
    let Some(decorator) = class_decorators(class_node)
        .into_iter()
        .find(|dec| decorator_name(*dec, src) == Some("Entity"))
    else {
        return;
    };
    if is_typeorm_mongo_entity(class_node, src) {
        return;
    }
    let table_cell = entity_decorator_table(decorator, src)
        .map(|table| data_entity::table_cell(&table, data_entity::orm::TYPEORM));
    acc.typeorm.declared += 1;
    if table_cell.is_some() {
        acc.typeorm.table_cells += 1;
    }
    let (qname, entity_id) = typeorm_entity(class_name, repo);
    if acc.typeorm.entities.insert(entity_id) {
        acc.nodes.push(Node {
            id: entity_id,
            repo,
            confidence: Confidence::Strong,
            cells: table_cell.into_iter().collect(),
        });
    } else if let Some(existing) = acc.nodes.iter_mut().find(|n| n.id == entity_id) {
        existing.cells.extend(table_cell);
    }
    acc.nav.record(
        entity_id,
        class_name,
        &qname,
        node_kind::DATA_ENTITY,
        Some(class_id),
    );
    acc.edges.push(Edge {
        from: class_id,
        to: entity_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
}

/// True for a TypeORM entity on the MongoDB driver: a field carries
/// `@ObjectIdColumn`, a decorator only that driver accepts. Its entity is a
/// Mongo collection, so minting `data_entity:sql:` for it would join it to SQL
/// tables of the same name.
fn is_typeorm_mongo_entity(class_node: TsNode, src: &[u8]) -> bool {
    let Some(body) = class_node.child_by_field_name("body") else {
        return false;
    };
    let mut bc = body.walk();
    body.named_children(&mut bc).any(|member| {
        let mut mc = member.walk();
        member.named_children(&mut mc).any(|dec| {
            dec.kind() == "decorator" && decorator_name(dec, src) == Some("ObjectIdColumn")
        })
    })
}

/// The table an `@Entity(…)` decorator names: `@Entity("app_users")`,
/// `@Entity("app_users", { schema })` or `@Entity({ name: "app_users" })`.
/// `@Entity()`, a non-literal argument, or an options object without a literal
/// `name` names none.
fn entity_decorator_table(decorator: TsNode, src: &[u8]) -> Option<String> {
    let mut dc = decorator.walk();
    let call = decorator.named_children(&mut dc).next()?;
    if call.kind() != "call_expression" {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let mut ac = args.walk();
    let first = args
        .named_children(&mut ac)
        .find(|a| a.kind() != "comment")?;
    let table = match first.kind() {
        "object" => object_string_prop(first, "name", src)?,
        _ => literal_string(first, src)?,
    };
    let table = table.trim();
    (!table.is_empty()).then(|| table.to_string())
}

/// A string literal's contents: `"x"`, `'x'`, or a template with no `${…}`.
fn literal_string(node: TsNode, src: &[u8]) -> Option<String> {
    match node.kind() {
        "string" => Some(strip_string_quotes(text(node, src))),
        "template_string" => {
            let mut c = node.walk();
            let substituted = node
                .named_children(&mut c)
                .any(|ch| ch.kind() == "template_substitution");
            (!substituted).then(|| strip_string_quotes(text(node, src)))
        }
        _ => None,
    }
}

/// The literal string value of property `key` in an object literal:
/// `{ name: "x" }` or `{ "name": 'x' }`.
fn object_string_prop(obj: TsNode, key: &str, src: &[u8]) -> Option<String> {
    let mut c = obj.walk();
    let pair = obj.named_children(&mut c).find(|p| {
        p.kind() == "pair"
            && p.child_by_field_name("key")
                .is_some_and(|k| match k.kind() {
                    "property_identifier" => text(k, src) == key,
                    "string" => strip_string_quotes(text(k, src)) == key,
                    _ => false,
                })
    })?;
    literal_string(pair.child_by_field_name("value")?, src)
}

/// A TypeORM query site in a file importing TypeORM: `getRepository(User)`,
/// `<any>.getRepository(User)` / `.getTreeRepository(User)`, or an
/// `EntityManager` method on a receiver named `…manager` with the entity class
/// at argument #0. Emits ACCESSES_DATA from `from`, the enclosing fn / method.
fn try_detect_typeorm_access(call: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    if !acc.typeorm.access {
        return;
    }
    let (Some(func), Some(args)) = (
        call.child_by_field_name("function"),
        call.child_by_field_name("arguments"),
    ) else {
        return;
    };
    let names_entity = match func.kind() {
        "identifier" => TYPEORM_REPOSITORY_FNS.contains(&text(func, src)),
        "member_expression" => {
            let (Some(object), Some(prop)) = (
                func.child_by_field_name("object"),
                func.child_by_field_name("property"),
            ) else {
                return;
            };
            let method = text(prop, src);
            TYPEORM_REPOSITORY_FNS.contains(&method)
                || (TYPEORM_MANAGER_METHODS.contains(&method) && is_entity_manager(object, src))
        }
        _ => false,
    };
    if !names_entity {
        return;
    }
    if let Some(model) = typeorm_model_arg(args, src) {
        emit_typeorm_access(model, from, acc);
    }
}

/// A receiver naming a SQL `EntityManager`: its trailing name ends in
/// `manager` (`manager`, `entityManager`, `this.manager`,
/// `queryRunner.manager`, `transactionalEntityManager`) and does not name
/// Mongo (`mongoManager` is a `MongoEntityManager`, whose entities are
/// collections).
fn is_entity_manager(receiver: TsNode, src: &[u8]) -> bool {
    let name = match receiver.kind() {
        "identifier" => Some(text(receiver, src)),
        "member_expression" => receiver
            .child_by_field_name("property")
            .map(|p| text(p, src)),
        _ => None,
    };
    name.map(str::to_ascii_lowercase)
        .is_some_and(|n| n.ends_with("manager") && !n.contains("mongo"))
}

/// The entity class a call's argument #0 names: `User`, or `models.User` ->
/// `User`, Upper-initial. A string entity name, a variable or an instance
/// names none.
fn typeorm_model_arg<'a>(args: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let mut c = args.walk();
    let first = args
        .named_children(&mut c)
        .find(|a| a.kind() != "comment")?;
    let name = match first.kind() {
        "identifier" => text(first, src),
        "member_expression" => text(first.child_by_field_name("property")?, src),
        _ => return None,
    };
    name.starts_with(|ch: char| ch.is_ascii_uppercase())
        .then_some(name)
}

/// `@InjectRepository(User)` on `owner` (a constructor parameter or a class
/// field) in a file importing TypeORM or `@nestjs/typeorm`: ACCESSES_DATA from
/// `from` (the constructor, or the class for a field) to the entity. A
/// `MongoRepository<T>`-typed owner injects a Mongo collection and is skipped.
fn collect_inject_repository(owner: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    if !acc.typeorm.access {
        return;
    }
    let declared = owner
        .child_by_field_name("type")
        .and_then(|t| annotated_class_type(t, src));
    if declared == Some("MongoRepository") {
        return;
    }
    let mut c = owner.walk();
    let decorators: Vec<TsNode> = owner
        .named_children(&mut c)
        .filter(|d| d.kind() == "decorator" && decorator_name(*d, src) == Some("InjectRepository"))
        .collect();
    for dec in decorators {
        let mut dc = dec.walk();
        let args = dec
            .named_children(&mut dc)
            .next()
            .filter(|call| call.kind() == "call_expression")
            .and_then(|call| call.child_by_field_name("arguments"));
        if let Some(model) = args.and_then(|a| typeorm_model_arg(a, src)) {
            emit_typeorm_access(model, from, acc);
        }
    }
}

/// Emit (once per `from` × entity) ACCESSES_DATA to the model-keyed entity,
/// pushing its node when this file has not. The query-site node records no nav
/// parent, so the `@Entity` class stays the entity's only owner whatever the
/// file order.
fn emit_typeorm_access(model: &str, from: NodeId, acc: &mut Acc) {
    let Some(repo) = acc.repo else {
        return;
    };
    acc.typeorm.repo_calls += 1;
    let (qname, entity_id) = typeorm_entity(model, repo);
    if acc.typeorm.entities.insert(entity_id) {
        acc.nodes.push(Node {
            id: entity_id,
            repo,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(entity_id, model, &qname, node_kind::DATA_ENTITY, None);
    }
    if acc.typeorm.access_seen.insert((from, entity_id)) {
        acc.edges.push(Edge {
            from,
            to: entity_id,
            category: edge_category::ACCESSES_DATA,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
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
            Some(to) => out
                .edges
                .push(evidence::intra_file_call(lang_tag(&acc.file_rel), uc.from, to, uc.line)),
            None => out.calls.push(CallSite {
                from: uc.from,
                qualifier: uc.qualifier,
                line: uc.line,
            }),
        }
    }

    // Endpoint emission. Build the import-alias set from `out.imports` so
    // shape-2 candidates (`axios.get(url)`) can be filtered to only those
    // whose base is a real module-level binding.
    let alias_set = build_alias_set(&out.imports);

    // LA.30c: `X.Y` member reads, in walk order. A member of a same-file enum
    // is a USES edge now. An import-bound base with an Upper-initial base AND
    // member becomes a USES `Attribute` ref for the graph crate's
    // `resolve_refs`: an ENUM base binds its ATTRIBUTE member (LA.30a); a
    // namespace-import MODULE base or a CLASS base binds through the generic
    // attribute lookup like any other ref (`Models.User` -> the class,
    // `User.Build` read as a value -> the static method). Everything else
    // (`environment.apiUrl`, `Math.PI`, a same-file enum's non-member) is
    // dropped, so it never reaches the persisted unresolved refs.
    let mut member_uses = 0usize;
    let mut member_ref_count = 0usize;
    for (from, base, name, line) in std::mem::take(&mut acc.member_refs) {
        if let Some(&member) = acc.enum_members.get(&(base.clone(), name.clone())) {
            out.edges.push(Edge {
                from,
                to: member,
                category: edge_category::USES,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            member_uses += 1;
            continue;
        }
        if acc.enums.contains_key(&base) {
            continue;
        }
        let Some(module_id) = acc.module_id else {
            continue;
        };
        let upper = |s: &str| s.starts_with(|c: char| c.is_ascii_uppercase());
        if alias_set.contains(base.as_str()) && upper(&base) && upper(&name) {
            out.refs.push(UnresolvedRef {
                from,
                from_module: module_id,
                qualifier: CallQualifier::Attribute { base, name },
                category: edge_category::USES,
                line,
            });
            member_ref_count += 1;
        }
    }
    if !acc.enums.is_empty() || member_uses > 0 || member_ref_count > 0 {
        eprintln!(
            "[ts-enums] enums={} members={} member_uses={} member_refs={} file={}",
            acc.enums.len(),
            acc.enum_members.len(),
            member_uses,
            member_ref_count,
            acc.file_rel
        );
    }

    // A7.1: `x = inject(T)` class fields. The callee must be the local binding
    // of a named `inject` import (`import { inject }`, or `{ inject as i }`),
    // so a project-local function that happens to be called `inject` emits
    // nothing. Fails closed on a re-export under another name.
    let inject_bindings = inject_import_bindings(&out.imports);
    for cand in acc.inject_fn_candidates {
        if !inject_bindings.contains(cand.callee.as_str()) {
            continue;
        }
        if let Some((field, type_name)) = &cand.field_type {
            out.nav.record_field_type(cand.class_id, field, type_name);
        }
        out.refs.push(UnresolvedRef {
            from: cand.class_id,
            from_module: cand.module_id,
            qualifier: CallQualifier::Bare(cand.type_name),
            category: edge_category::INJECTS,
            line: cand.line,
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
            cells: Vec::new(),
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
    if let Some(doc) = glia_doc::leading_doc(n, src) {
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
    use glia_core::EdgeCategoryId;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts_smoke")
    }

    /// LC.3b: a CALLS edge this parser resolves in-file carries the call's own
    /// row (not the caller's declaration row), rule `intra_file`, and the
    /// emitter of the file's routing tag; the cross-file call site keeps its
    /// row for the graph crate.
    #[test]
    fn intra_file_calls_carry_the_call_row_and_the_file_tag() {
        use glia_code_domain::evidence::{Basis, Evidence};
        let src = "import { far } from \"./far\";\n\nfunction near() {\n  return 1;\n}\n\n\
                   export function run() {\n  const x = 2;\n  far();\n  return near() + x;\n}\n";
        for (path, tag) in [
            ("src/run.ts", "parser:typescript"),
            ("src/run.component.ts", "parser:angular"),
        ] {
            let parse = parse_file(src, path, "src::run", repo()).unwrap();
            let run = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "src::run::run");
            let near = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "src::run::near");
            let edge = parse
                .edges
                .iter()
                .find(|e| e.from == run && e.to == near && e.category == edge_category::CALLS)
                .expect("run -> near resolves in-file");
            let ev = Evidence::of(edge).expect("intra-file CALLS carries evidence");
            assert_eq!(ev.emitter, tag, "{path}");
            assert_eq!(ev.rule.as_deref(), Some("intra_file"));
            assert_eq!((ev.line, ev.basis), (Some(9), Basis::Site), "`return near() + x;` is row 9");
            let far = parse
                .calls
                .iter()
                .find(|c| matches!(&c.qualifier, CallQualifier::Bare(n) if n == "far"))
                .expect("far() stays a cross-file CallSite");
            assert_eq!(far.line, 8);
            assert_eq!(parse.imports.first().map(|i| i.line), Some(0));
        }
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
        // G12.5 / A6.3: `implements I` → an IMPLEMENTS ref; `extends Y` → an
        // INHERITS_FROM ref. The graph crate binds both (resolve_refs).
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

        // G12.5 / A6.3: class X implements IFoo → an IMPLEMENTS ref; extends
        // Base → an INHERITS_FROM ref. Both carry the bare supertype name and
        // the file module; no heritage EDGE is minted by the parser (a
        // name-derived target id dangles whenever the base is imported).
        let class_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "src::fees::X");
        let has_ref = |name: &str, cat: EdgeCategoryId| {
            parse.refs.iter().any(|r| {
                r.from == class_id
                    && r.from_module == mod_id
                    && r.qualifier == CallQualifier::Bare(name.to_string())
                    && r.category == cat
            })
        };
        assert!(
            has_ref("IFoo", edge_category::IMPLEMENTS),
            "expected X --IMPLEMENTS--> Bare(IFoo) ref, refs: {:?}",
            parse.refs
        );
        assert!(
            has_ref("Base", edge_category::INHERITS_FROM),
            "expected X --INHERITS_FROM--> Bare(Base) ref, refs: {:?}",
            parse.refs
        );
        assert!(
            !parse.edges.iter().any(|e| e.from == class_id
                && (e.category == edge_category::IMPLEMENTS
                    || e.category == edge_category::INHERITS_FROM)),
            "the parser must not mint heritage edges, edges: {:?}",
            parse.edges
        );
    }

    #[test]
    fn interface_extends_emits_inherits_from_ref() {
        // LD.7a: `interface Catalog extends Readable, ns.Paged<T>` → one
        // INHERITS_FROM ref per super-interface, from the interface, in the A6.3
        // shape (Bare(<simple name>), the file module). A merged declaration
        // repeating the same `extends` adds no second ref; the parser mints no
        // heritage edge.
        let src = "\
export interface Readable { read(id: string): string; }
export interface Catalog extends Readable, ns.Paged<string> { search(q: string): string[]; }
export interface Catalog extends Readable { count(): number; }
";
        let parse = parse_file(src, "catalog.ts", "catalog", repo()).unwrap();
        let mod_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "catalog");
        let iface_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, "catalog::Catalog");
        let inherits: Vec<&UnresolvedRef> = parse
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INHERITS_FROM)
            .collect();
        assert_eq!(inherits.len(), 2, "Readable once, Paged once: {inherits:?}");
        for name in ["Readable", "Paged"] {
            assert!(
                inherits.iter().any(|r| r.from == iface_id
                    && r.from_module == mod_id
                    && r.qualifier == CallQualifier::Bare(name.to_string())),
                "expected Catalog --INHERITS_FROM--> Bare({name}) ref: {inherits:?}"
            );
        }
        assert!(
            !parse.edges.iter().any(|e| e.category == edge_category::INHERITS_FROM),
            "the parser must not mint heritage edges, edges: {:?}",
            parse.edges
        );
    }

    // ---- LA.30c: enums -------------------------------------------------------

    fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn has_node(parse: &FileParse, id: NodeId) -> bool {
        parse.nodes.iter().any(|n| n.id == id)
    }

    /// Every `(base, name)` of a USES `Attribute` ref emitted from `from`.
    fn uses_refs(parse: &FileParse, from: NodeId) -> Vec<(String, String)> {
        parse
            .refs
            .iter()
            .filter(|r| r.category == edge_category::USES && r.from == from)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Attribute { base, name } => Some((base.clone(), name.clone())),
                _ => None,
            })
            .collect()
    }

    fn uses_edges(parse: &FileParse) -> Vec<(NodeId, NodeId)> {
        parse
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES)
            .map(|e| (e.from, e.to))
            .collect()
    }

    #[test]
    fn enum_declaration_emits_enum_and_member_attributes() {
        let src = "\
export enum Color {
  Red = 'red',
  /** The calm one. */
  Green = 'green',
  Blue = 'blue',
}
";
        let parse = parse_file(src, "src/color.ts", "src::color", repo()).unwrap();
        let module = id(node_kind::MODULE, "src::color");
        let color = id(node_kind::ENUM, "src::color::Color");
        assert!(has_node(&parse, color), "ENUM Color missing: {:?}", parse.nodes);
        assert!(has_edge(&parse, module, color, edge_category::DEFINES));
        let members: Vec<NodeId> = ["Red", "Green", "Blue"]
            .iter()
            .map(|m| id(node_kind::ATTRIBUTE, &format!("src::color::Color::{m}")))
            .collect();
        for (m, name) in members.iter().zip(["Red", "Green", "Blue"]) {
            assert!(has_node(&parse, *m), "ATTRIBUTE {name} missing");
            assert!(has_edge(&parse, color, *m, edge_category::HAS_ATTRIBUTE), "{name}");
            assert_eq!(parse.nav.parent_of.get(m), Some(&color), "{name} nav parent");
            assert_eq!(parse.nav.name_by_id.get(m).map(String::as_str), Some(name));
        }
        let attr_count = parse
            .nodes
            .iter()
            .filter(|n| parse.nav.kind_by_id.get(&n.id) == Some(&node_kind::ATTRIBUTE))
            .count();
        assert_eq!(attr_count, 3, "one ATTRIBUTE per member, the enum name is not one");

        // POSITION of a member is its own line (0-based); DOC from its JSDoc.
        let green = parse.nodes.iter().find(|n| n.id == members[1]).unwrap();
        let pos = green
            .cells
            .iter()
            .find_map(|c| match (&c.payload, c.kind == cell_type::POSITION) {
                (CellPayload::Json(j), true) => Some(j.clone()),
                _ => None,
            })
            .expect("member POSITION cell");
        assert!(pos.contains("\"start_line\":3,"), "Green sits on row 3: {pos}");
        assert!(
            green.cells.iter().any(|c| c.kind == cell_type::DOC
                && matches!(&c.payload, CellPayload::Text(t) if t.contains("calm"))),
            "member JSDoc: {:?}",
            green.cells
        );
        assert!(
            green.cells.iter().any(|c| c.kind == cell_type::CODE
                && matches!(&c.payload, CellPayload::Text(t) if t == "Green = 'green'")),
            "member CODE carries its initialiser: {:?}",
            green.cells
        );
    }

    #[test]
    fn const_and_non_exported_enums_are_emitted() {
        let src = "\
export const enum Dir {
  Up,
  Down,
}

enum Local {
  A = 1,
  B,
}
";
        let parse = parse_file(src, "src/dir.ts", "src::dir", repo()).unwrap();
        for (e, members) in [("Dir", ["Up", "Down"]), ("Local", ["A", "B"])] {
            let enum_id = id(node_kind::ENUM, &format!("src::dir::{e}"));
            assert!(has_node(&parse, enum_id), "ENUM {e} missing");
            for m in members {
                let m_id = id(node_kind::ATTRIBUTE, &format!("src::dir::{e}::{m}"));
                assert!(has_edge(&parse, enum_id, m_id, edge_category::HAS_ATTRIBUTE), "{e}::{m}");
            }
        }
    }

    #[test]
    fn quoted_member_name_is_unquoted() {
        let src = "enum E { 'my-key' = 1, \"other\" = 2, 3 = 4 }\n";
        let parse = parse_file(src, "src/e.ts", "src::e", repo()).unwrap();
        let e = id(node_kind::ENUM, "src::e::E");
        for m in ["my-key", "other"] {
            let m_id = id(node_kind::ATTRIBUTE, &format!("src::e::E::{m}"));
            assert!(has_edge(&parse, e, m_id, edge_category::HAS_ATTRIBUTE), "{m}");
        }
        assert_eq!(
            parse.nav.children_of.get(&e).map(Vec::len),
            Some(2),
            "a numeric member name is skipped"
        );
    }

    #[test]
    fn same_file_member_reference_is_a_uses_edge() {
        let src = "\
export enum Color { Red = 'red', Green = 'green' }

export function warm(c: Color): boolean {
  return c === Color.Red || c === Color.Red;
}

export function other(): string {
  return Color.Missing;
}
";
        let parse = parse_file(src, "src/color.ts", "src::color", repo()).unwrap();
        let warm = id(node_kind::FUNCTION, "src::color::warm");
        let red = id(node_kind::ATTRIBUTE, "src::color::Color::Red");
        assert_eq!(uses_edges(&parse), vec![(warm, red)], "one deduped USES, the named member only");
        assert!(
            parse.refs.iter().all(|r| r.category != edge_category::USES),
            "a same-file enum never emits a USES ref (Color.Missing is dropped): {:?}",
            parse.refs
        );
    }

    #[test]
    fn imported_enum_member_reference_is_a_uses_ref() {
        let src = "\
import { Color } from './color';

export function pick(): Color {
  return Color.Green;
}
";
        let parse = parse_file(src, "src/paint.ts", "src::paint", repo()).unwrap();
        let pick = id(node_kind::FUNCTION, "src::paint::pick");
        assert_eq!(uses_refs(&parse, pick), vec![("Color".to_string(), "Green".to_string())]);
        let r = parse.refs.iter().find(|r| r.category == edge_category::USES).unwrap();
        assert_eq!(r.from_module, id(node_kind::MODULE, "src::paint"));
        assert!(uses_edges(&parse).is_empty(), "the imported member binds in the graph crate");
    }

    #[test]
    fn lowercase_base_emits_nothing() {
        let src = "\
import { environment } from './env';
import { Color } from './color';

export function url(): string {
  const p = Math.PI;
  const c = Color.green;
  return environment.apiUrl;
}
";
        let parse = parse_file(src, "src/url.ts", "src::url", repo()).unwrap();
        assert!(
            parse.refs.iter().all(|r| r.category != edge_category::USES),
            "lower-case base or member, or an un-imported base, emits no USES ref: {:?}",
            parse.refs
        );
        assert!(uses_edges(&parse).is_empty());
    }

    #[test]
    fn call_callee_member_expression_is_not_a_member_ref() {
        let src = "\
import { Api } from './api';

export function load() {
  return Api.Load();
}
";
        let parse = parse_file(src, "src/load.ts", "src::load", repo()).unwrap();
        let load = id(node_kind::FUNCTION, "src::load::load");
        assert!(uses_refs(&parse, load).is_empty(), "a callee is a CallSite, not a USES ref");
        assert!(
            parse.calls.iter().any(|c| c.from == load
                && c.qualifier
                    == CallQualifier::Attribute { base: "Api".into(), name: "Load".into() }),
            "Api.Load() stays a CallSite: {:?}",
            parse.calls
        );
    }

    #[test]
    fn declare_enum_is_skipped() {
        let src = "\
declare enum Ambient { A, B }
export declare const enum AmbientExported { C }
";
        let parse = parse_file(src, "src/types.d.ts", "src::types", repo()).unwrap();
        assert!(
            !parse
                .nodes
                .iter()
                .any(|n| matches!(parse.nav.kind_by_id.get(&n.id), Some(k) if *k == node_kind::ENUM || *k == node_kind::ATTRIBUTE)),
            "an ambient enum describes an external shape and mints nothing: {:?}",
            parse.nav.qname_by_id
        );
    }

    #[test]
    fn merged_enum_declarations_do_not_duplicate_members() {
        let src = "\
enum Merged { A = 1, B = 2 }
enum Merged { B = 2, C = 3 }
";
        let parse = parse_file(src, "src/m.ts", "src::m", repo()).unwrap();
        let merged = id(node_kind::ENUM, "src::m::Merged");
        assert_eq!(parse.nodes.iter().filter(|n| n.id == merged).count(), 1, "one ENUM node");
        let module = id(node_kind::MODULE, "src::m");
        assert_eq!(
            parse.nav.children_of.get(&module).map(|c| c.iter().filter(|x| **x == merged).count()),
            Some(1),
            "the module lists the enum once"
        );
        let children = parse.nav.children_of.get(&merged).cloned().unwrap_or_default();
        let expect: Vec<NodeId> = ["A", "B", "C"]
            .iter()
            .map(|m| id(node_kind::ATTRIBUTE, &format!("src::m::Merged::{m}")))
            .collect();
        assert_eq!(children, expect, "A, B, C once each, in declaration order");
        assert_eq!(
            parse.edges.iter().filter(|e| e.category == edge_category::HAS_ATTRIBUTE).count(),
            3
        );
    }

    // ---- A6.2b: declared field types -----------------------------------------

    /// `(field, type)` pairs recorded for `class_qname`, sorted.
    fn field_types(parse: &FileParse, class_qname: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = parse
            .nav
            .field_types
            .get(&id(node_kind::CLASS, class_qname))
            .map(|m| m.iter().map(|(f, t)| (f.clone(), t.clone())).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(f, t)| (f.to_string(), t.to_string())).collect()
    }

    #[test]
    fn ctor_param_property_records_field_type() {
        let src = "export class C { constructor(private api: ApiService) {} }\n";
        let fp = parse_file(src, "src/c.ts", "src::c", repo()).unwrap();
        let class_id = id(node_kind::CLASS, "src::c::C");
        assert_eq!(fp.nav.field_types[&class_id]["api"], "ApiService");
    }

    #[test]
    fn typed_class_field_records_field_type() {
        let src = "export class C {\n  private api: ApiService;\n}\n";
        let fp = parse_file(src, "src/c.ts", "src::c", repo()).unwrap();
        let class_id = id(node_kind::CLASS, "src::c::C");
        assert_eq!(fp.nav.field_types[&class_id]["api"], "ApiService");
    }

    #[test]
    fn primitive_ctor_param_records_no_field_type() {
        let src = "export class C { constructor(private count: number) {} }\n";
        let fp = parse_file(src, "src/c.ts", "src::c", repo()).unwrap();
        assert!(
            fp.nav.field_types.is_empty(),
            "predefined_type is skipped, got: {:?}",
            fp.nav.field_types
        );
    }

    /// Every param counts (decorated or not, with or without an accessibility
    /// modifier); only a single identifier with a class-shaped type records,
    /// and each class keeps its own fields.
    #[test]
    fn field_types_cover_param_and_field_shapes() {
        let src = "\
export class C {
  constructor(
    api: ApiService,
    readonly b?: ns.Bar,
    public override c: Repo<User>,
    @Inject(TOKEN) private g: Gateway,
    private f: Foo | null,
    { d }: Opts,
    private n: string,
    e = 1,
    private h: A | B,
    ...rest: X[]
  ) {}
  private x: XService;
  y!: YService;
  z?: ZService | undefined;
  #p: PrivService;
  static s: StatService;
  cb: (u: User) => void;
  list: Item[];
  obj: { a: number };
  plain = 3;
}
export class D {
  constructor(private api: OtherApi) {}
}
";
        let fp = parse_file(src, "src/c.ts", "src::c", repo()).unwrap();
        assert_eq!(
            field_types(&fp, "src::c::C"),
            pairs(&[
                ("#p", "PrivService"),
                ("api", "ApiService"),
                ("b", "Bar"),
                ("c", "Repo"),
                ("f", "Foo"),
                ("g", "Gateway"),
                ("s", "StatService"),
                ("x", "XService"),
                ("y", "YService"),
                ("z", "ZService"),
            ])
        );
        assert_eq!(field_types(&fp, "src::c::D"), pairs(&[("api", "OtherApi")]));
    }

    /// `x = inject(T)` records `x: T` only once the callee passes A7.1's
    /// `inject` import gate; `inject<T>(TOKEN)` records the `<T>`, a non-class
    /// `<T>` records nothing, and an annotation wins over the initialiser.
    #[test]
    fn inject_fn_field_records_field_type_behind_import_gate() {
        let body = "\
export class Dash {
  private api = inject(ApiService);
  private store = inject<Store>(STORE_TOKEN);
  private url = inject<string>(API_URL);
  private typed: Typed = inject(TYPED_TOKEN);
  private nested = inject(models.Repo);
  private cfg = inject(\"CONFIG\");
}
";
        let with_import = format!("import {{ inject }} from \"@angular/core\";\n{body}");
        let fp = parse_file(&with_import, "src/dash.ts", "src::dash", repo()).unwrap();
        assert_eq!(
            field_types(&fp, "src::dash::Dash"),
            pairs(&[
                ("api", "ApiService"),
                ("nested", "Repo"),
                ("store", "Store"),
                ("typed", "Typed"),
            ])
        );

        let without = format!("import {{ Component }} from \"@angular/core\";\n{body}");
        let fp = parse_file(&without, "src/dash.ts", "src::dash", repo()).unwrap();
        assert_eq!(
            field_types(&fp, "src::dash::Dash"),
            pairs(&[("typed", "Typed")]),
            "an un-imported inject() is a local function: only the annotation records"
        );
    }

    // ---- A13.15: TypeORM ----

    fn typeorm_entity_id(model: &str) -> NodeId {
        let qname = format!("data_entity:sql:{model}");
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::DATA_ENTITY, &qname)
    }

    fn typeorm_entity_nodes<'p>(parse: &'p FileParse, model: &str) -> Vec<&'p Node> {
        let id = typeorm_entity_id(model);
        parse.nodes.iter().filter(|n| n.id == id).collect()
    }

    fn data_entity_count(parse: &FileParse) -> usize {
        parse
            .nav
            .kind_by_id
            .values()
            .filter(|k| **k == node_kind::DATA_ENTITY)
            .count()
    }

    fn accesses(parse: &FileParse, from: NodeId, model: &str) -> usize {
        let to = typeorm_entity_id(model);
        parse
            .edges
            .iter()
            .filter(|e| e.from == from && e.to == to && e.category == edge_category::ACCESSES_DATA)
            .count()
    }

    #[test]
    fn typeorm_entity_decorator_arg_is_table() {
        let src = "\
import { Entity, PrimaryGeneratedColumn, Column } from \"typeorm\";

@Entity(\"app_users\")
export class User {
  @PrimaryGeneratedColumn() id!: number;
  @Column() email!: string;
}

@Entity({ name: 'shop_orders', schema: 'shop' })
export class Order {}

@Entity(`audit_log`, { schema: \"ops\" })
class Audit {}
";
        let fp = parse_file(src, "entity/User.ts", "entity::User", repo()).unwrap();
        for (model, table) in [
            ("User", "app_users"),
            ("Order", "shop_orders"),
            ("Audit", "audit_log"),
        ] {
            let nodes = typeorm_entity_nodes(&fp, model);
            assert_eq!(nodes.len(), 1, "{model}: one model-keyed entity node");
            assert_eq!(
                data_entity::table_of(&nodes[0].cells).as_deref(),
                Some(table),
                "{model}: the decorator's table rides a table cell"
            );
            let class_id = id(node_kind::CLASS, &format!("entity::User::{model}"));
            let entity = typeorm_entity_id(model);
            assert!(has_edge(&fp, class_id, entity, edge_category::DEFINES));
            assert_eq!(fp.nav.parent_of.get(&entity), Some(&class_id));
            assert_eq!(
                fp.nav.name_by_id.get(&entity).map(String::as_str),
                Some(model)
            );
        }
        let CellPayload::Json(payload) = &typeorm_entity_nodes(&fp, "User")[0].cells[0].payload
        else {
            panic!("table cell is JSON");
        };
        assert!(payload.contains("\"orm\":\"typeorm\""), "{payload}");
        assert_eq!(data_entity_count(&fp), 3);
    }

    #[test]
    fn typeorm_entity_without_arg_uses_class_name() {
        let src = "\
import { Entity, Column } from \"typeorm\";

@Entity()
export class UserProfile {
  @Column() bio!: string;
}

@Entity({ schema: \"ops\" })
export class Setting {}

@Entity(TABLE_NAME)
export class Dynamic {}
";
        let fp = parse_file(src, "entity/profile.ts", "entity::profile", repo()).unwrap();
        for model in ["UserProfile", "Setting", "Dynamic"] {
            let nodes = typeorm_entity_nodes(&fp, model);
            assert_eq!(nodes.len(), 1, "{model}: keyed on the class name");
            assert_eq!(
                data_entity::table_of(&nodes[0].cells),
                None,
                "{model}: no literal table, so no table cell"
            );
            let class_id = id(node_kind::CLASS, &format!("entity::profile::{model}"));
            assert!(has_edge(
                &fp,
                class_id,
                typeorm_entity_id(model),
                edge_category::DEFINES
            ));
        }
    }

    #[test]
    fn entity_decorator_without_typeorm_import_is_ignored() {
        let src = "\
import { Entity, PrimaryKey } from \"@mikro-orm/core\";
import { getRepository } from \"./db\";

@Entity({ tableName: \"users\" })
export class User {
  @PrimaryKey() id!: number;
}

export function listUsers(manager: Manager) {
  manager.find(User, {});
  return getRepository(User).find();
}
";
        let fp = parse_file(src, "src/user.ts", "src::user", repo()).unwrap();
        assert_eq!(data_entity_count(&fp), 0, "no typeorm import: no entity");
        assert!(typeorm_entity_nodes(&fp, "User").is_empty());
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.category == edge_category::ACCESSES_DATA),
            "no typeorm import: query-shaped calls emit nothing"
        );
    }

    #[test]
    fn typeorm_repository_sites_emit_accesses_data() {
        let src = "\
import { DataSource, EntityManager, Repository } from \"typeorm\";
import { InjectRepository } from \"@nestjs/typeorm\";
import { User } from \"../entity/User\";
import * as models from \"../entity\";

export async function listUsers() {
  return getRepository(User).find();
}

export class UserService {
  constructor(
    @InjectRepository(User) private repo: Repository<User>,
    private dataSource: DataSource,
    private entityManager: EntityManager,
  ) {}
  async orders() { return this.dataSource.getRepository(models.Order).find(); }
  async invoices(qr: QueryRunner) { return qr.manager.count(Invoice, {}); }
  async twice() { await getRepository(User).find(); return getRepository(User).count(); }
  async cached() { return this.cache.find(User); }
  async instance(user: User) { return this.entityManager.save(user); }
  async build() { return this.entityManager.create(User, {}); }
}
";
        let fp = parse_file(
            src,
            "service/UserService.ts",
            "service::UserService",
            repo(),
        )
        .unwrap();
        let svc = "service::UserService::UserService";
        let method = |m: &str| id(node_kind::METHOD, &format!("{svc}::{m}"));
        let list = id(node_kind::FUNCTION, "service::UserService::listUsers");
        assert_eq!(accesses(&fp, list, "User"), 1, "getRepository(User)");
        assert_eq!(
            accesses(&fp, method("constructor"), "User"),
            1,
            "@InjectRepository(User)"
        );
        assert_eq!(
            accesses(&fp, method("orders"), "Order"),
            1,
            "dataSource.getRepository(models.Order)"
        );
        assert_eq!(
            accesses(&fp, method("invoices"), "Invoice"),
            1,
            "qr.manager.count(Invoice)"
        );
        assert_eq!(
            accesses(&fp, method("twice"), "User"),
            1,
            "one edge per fn x entity"
        );
        for m in ["cached", "instance", "build"] {
            assert!(
                !fp.edges
                    .iter()
                    .any(|e| e.from == method(m) && e.category == edge_category::ACCESSES_DATA),
                "{m}: not a repository or manager query naming an entity class"
            );
        }
        for model in ["User", "Order", "Invoice"] {
            let nodes = typeorm_entity_nodes(&fp, model);
            assert_eq!(nodes.len(), 1, "{model}: pushed once");
            assert!(
                nodes[0].cells.is_empty(),
                "{model}: a query site carries no table"
            );
            assert_eq!(fp.nav.parent_of.get(&typeorm_entity_id(model)), None);
        }
        assert_eq!(data_entity_count(&fp), 3);
    }

    #[test]
    fn nestjs_typeorm_import_gates_inject_repository_but_not_entity() {
        let src = "\
import { InjectRepository } from \"@nestjs/typeorm\";
import { Entity } from \"./decorators\";

@Entity(\"widgets\")
export class Widget {}

export class WidgetService {
  @InjectRepository(Widget) private readonly repo: WidgetRepo;
}
";
        let fp = parse_file(src, "src/widget.ts", "src::widget", repo()).unwrap();
        let service = id(node_kind::CLASS, "src::widget::WidgetService");
        assert_eq!(
            accesses(&fp, service, "Widget"),
            1,
            "field @InjectRepository -> class"
        );
        let class_id = id(node_kind::CLASS, "src::widget::Widget");
        assert!(
            !has_edge(
                &fp,
                class_id,
                typeorm_entity_id("Widget"),
                edge_category::DEFINES
            ),
            "@Entity needs a typeorm import, not @nestjs/typeorm"
        );
        let nodes = typeorm_entity_nodes(&fp, "Widget");
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].cells.is_empty());
    }

    #[test]
    fn typeorm_query_before_declaration_shares_one_node() {
        let src = "\
import { Entity, getRepository } from \"typeorm\";

export function first() {
  return getRepository(User).findOneBy({ id: 1 });
}

@Entity(\"app_users\")
export class User {
  static all() { return getRepository(User).find(); }
}
";
        let fp = parse_file(src, "src/user.ts", "src::user", repo()).unwrap();
        let nodes = typeorm_entity_nodes(&fp, "User");
        assert_eq!(nodes.len(), 1, "query site and declaration: one node");
        assert_eq!(
            data_entity::table_of(&nodes[0].cells).as_deref(),
            Some("app_users")
        );
        let class_id = id(node_kind::CLASS, "src::user::User");
        let entity = typeorm_entity_id("User");
        assert_eq!(fp.nav.parent_of.get(&entity), Some(&class_id));
        assert_eq!(
            accesses(&fp, id(node_kind::FUNCTION, "src::user::first"), "User"),
            1
        );
        assert_eq!(
            accesses(&fp, id(node_kind::METHOD, "src::user::User::all"), "User"),
            1
        );
    }

    #[test]
    fn typeorm_mongo_entities_are_not_sql() {
        let entity = "\
import { Column, Entity, ObjectId, ObjectIdColumn } from 'typeorm';

@Entity()
export class Photo {
  @ObjectIdColumn()
  id: ObjectId;

  @Column()
  name: string;
}
";
        let fp = parse_file(entity, "src/photo.entity.ts", "src::photo_entity", repo()).unwrap();
        assert_eq!(
            data_entity_count(&fp),
            0,
            "an @ObjectIdColumn entity is a Mongo collection"
        );

        let service = "\
import { InjectRepository } from '@nestjs/typeorm';
import { MongoRepository, MongoEntityManager } from 'typeorm';

export class PhotoService {
  constructor(
    @InjectRepository(Photo) private readonly photos: MongoRepository<Photo>,
    private mongoManager: MongoEntityManager,
  ) {}
  async all() { return this.mongoManager.find(Photo, {}); }
  async viaMongo() { return getMongoRepository(Photo).find(); }
}
";
        let fp = parse_file(
            service,
            "src/photo.service.ts",
            "src::photo_service",
            repo(),
        )
        .unwrap();
        assert_eq!(
            data_entity_count(&fp),
            0,
            "Mongo repositories and managers mint no sql entity"
        );
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.category == edge_category::ACCESSES_DATA)
        );
    }
}
