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

use std::collections::{HashMap, HashSet};

use glia_code_domain::NavFact;
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
    parse_file_stats(source, file_rel_path, module_qname, repo).map(|(parse, ..)| parse)
}

/// [`parse_file`], plus the file's CG.1 function-field counters (the
/// `[ts-fields]` marker's numbers), its CH.1 abstract-class counters (the
/// `[ts-abstract]` marker's numbers), its CH.2 call-initialised field
/// counters (the `[ts-state]` marker's numbers) and its CH.3a URL-argument
/// counters (the `[ts-endpoint-args]` marker's numbers), which the tests read
/// back.
fn parse_file_stats(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<
    (
        FileParse,
        FnFieldStats,
        AbstractStats,
        StateFieldStats,
        EndpointArgStats,
    ),
    ParseError,
> {
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
    // CG.1: function-valued class fields minted as METHODs, per file.
    let ff = acc.fn_fields;
    if ff.fields() + ff.shadowed > 0 {
        eprintln!(
            "[ts-fields] function fields={} (arrow={} function={}) calls={} shadowed={} \
             file={file_rel_path}",
            ff.fields(),
            ff.arrow,
            ff.function,
            ff.calls,
            ff.shadowed
        );
    }
    // CH.2: call-initialised class fields minted as STATE_VARs, per file.
    let sf = acc.state_fields;
    if sf.fields > 0 {
        eprintln!(
            "[ts-state] call fields={} (signal={} other={}) calls={} file={file_rel_path}",
            sf.fields,
            sf.signal,
            sf.fields.saturating_sub(sf.signal),
            sf.calls
        );
    }
    // CH.1: `abstract class` declarations and their bodiless members, per file.
    let ab = acc.abstract_stats;
    if ab.classes + ab.methods > 0 {
        eprintln!(
            "[ts-abstract] classes={} abstract_methods={} file={file_rel_path}",
            ab.classes, ab.methods
        );
    }
    // CH.3a: HTTP call-site arguments read through a builder, a URL method, a
    // `const` local or a readonly field / getter, per file.
    let ea = endpoint_arg_stats(&acc);
    if ea.read() > 0 {
        eprintln!(
            "[ts-endpoint-args] wrapper={} local={} field={} method={} unresolved={} \
             file={file_rel_path}",
            ea.wrapper, ea.local, ea.field, ea.method, ea.unresolved
        );
    }

    resolve_intra_file(acc).map(|parse| (parse, ff, ab, sf, ea))
}

/// CH.3a: count the file's HTTP call sites by the arm that read their URL
/// argument. Only candidates `resolve_intra_file` turns into ENDPOINTs count:
/// a shape-2 call (`x.get(url)`) whose `x` is not an import alias does not.
fn endpoint_arg_stats(acc: &Acc) -> EndpointArgStats {
    let aliases = build_alias_set(&acc.imports);
    let mut stats = EndpointArgStats::default();
    let emitted = acc.endpoints.iter().filter(|c| {
        c.requires_import_alias
            .as_deref()
            .is_none_or(|a| aliases.contains(a))
    });
    for cand in emitted {
        if cand.path == UNRESOLVED_PATH {
            stats.unresolved += 1;
            continue;
        }
        match cand.read {
            ArgRead::Direct => {}
            ArgRead::Wrapper => stats.wrapper += 1,
            ArgRead::Method => stats.method += 1,
            ArgRead::Local => stats.local += 1,
            ArgRead::Field => stats.field += 1,
        }
    }
    stats
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
    /// CG.1: the `[ts-fields]` counters.
    fn_fields: FnFieldStats,
    /// CH.1: the `[ts-abstract]` counters.
    abstract_stats: AbstractStats,
    /// CH.2: the `[ts-state]` counters.
    state_fields: StateFieldStats,
    /// CH.3b: reads of an API-prefix-named member ([`API_PREFIX_NAMES`]:
    /// `this.apiPrefix`, `cfg.basePath`, `const { apiPrefix } = cfg`), as
    /// `(from, name as written, start byte)`. The call walk pops siblings
    /// right to left; `url_prefix_facts` puts each scope's reads back in
    /// source order.
    prefix_reads: Vec<(NodeId, String, usize)>,
    /// CH.3b: each fn / METHOD this file defines (a method, a function field,
    /// a function declaration or a function-valued `const`) -> its parameter
    /// count. The callables `url_prefix_facts` closes over.
    callable_params: HashMap<NodeId, usize>,
}

/// CH.2: what one file's call-initialised class fields gave the graph (the
/// `[ts-state]` marker).
#[derive(Default, Clone, Copy)]
struct StateFieldStats {
    /// `x = f(…)` fields minted as STATE_VARs.
    fields: usize,
    /// Of those, fields whose factory is an Angular signal primitive
    /// ([`SIGNAL_FACTORIES`]); the rest print as `other=`.
    signal: usize,
    /// Call sites (resolved or not) and client HTTP calls found in their
    /// initializers, the factory call itself included.
    calls: usize,
}

/// CH.1: what one file's `abstract class` declarations gave the graph (the
/// `[ts-abstract]` marker).
#[derive(Default, Clone, Copy)]
struct AbstractStats {
    /// `abstract class` declarations minted as CLASSes.
    classes: usize,
    /// `abstract m(…): T;` members minted as METHODs.
    methods: usize,
}

/// CG.1: what one file's function-valued class fields gave the graph (the
/// `[ts-fields]` marker).
#[derive(Default, Clone, Copy)]
struct FnFieldStats {
    /// `x = (…) => …` fields minted as METHODs.
    arrow: usize,
    /// `x = function (…) {…}` and `x = function* (…) {…}` fields minted as METHODs.
    function: usize,
    /// Call sites (resolved or not) and client HTTP calls found in their bodies.
    calls: usize,
    /// Function fields whose name a method (or an earlier field) of the same
    /// class already holds: the method keeps its node, the field mints none.
    shadowed: usize,
}

impl FnFieldStats {
    /// Function fields minted as METHODs.
    fn fields(self) -> usize {
        self.arrow + self.function
    }
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
    /// CH.3a: the leaf name of the single-argument URL builder the path was
    /// read through (`buildApiUrl` for `this.urls.buildApiUrl('x/y')`).
    /// Serialised as `"wrapper"` on ENDPOINT_HIT, after `template`.
    wrapper: Option<String>,
    /// CH.3b: the field `f` the builder was reached through when its callee
    /// is `this.<f>.<m>` (`urls` for `this.urls.buildApiUrl('x/y')`), set
    /// only beside `wrapper`. `resolve_intra_file` looks `f` up in `class`'s
    /// field types and writes the type as `"wrapper_of"` on ENDPOINT_HIT.
    wrapper_recv: Option<String>,
    /// CH.3b: the class whose method / field walk found the call (the
    /// `enclosing_class` of `collect_calls_in`); None outside a class.
    class: Option<NodeId>,
    /// CH.3a: which arm of `classify_at` read the argument (the
    /// `[ts-endpoint-args]` counters).
    read: ArgRead,
}

/// The path text of an HTTP call whose URL argument could not be read.
const UNRESOLVED_PATH: &str = "<unresolved>";

/// CH.3a: the outermost arm of `classify_at` that read an HTTP call's URL
/// argument. Each call site counts once in `[ts-endpoint-args]`, under this
/// arm, or under `unresolved=` when its path stayed `<unresolved>`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum ArgRead {
    /// The argument itself (a literal, a template, today's fallbacks).
    #[default]
    Direct,
    /// The one literal argument of a URL-builder call (arm a).
    Wrapper,
    /// The one `return` of a zero-argument same-class method (arm b).
    Method,
    /// The value of a `const` declared in an enclosing scope (arm d).
    Local,
    /// A `readonly` field initializer or a one-`return` getter (arm e).
    Field,
}

/// What `classify_path_arg` read off an HTTP call's first argument.
struct PathArg {
    /// Request path as written, `${…}` for every substitution.
    path: String,
    /// Substitution-preserving template source; see `EndpointCandidate::template`.
    template: Option<String>,
    confidence: Confidence,
    /// CH.3a: see `EndpointCandidate::wrapper`.
    wrapper: Option<String>,
    /// CH.3b: see `EndpointCandidate::wrapper_recv`.
    wrapper_recv: Option<String>,
    /// CH.3a: see `EndpointCandidate::read`.
    read: ArgRead,
}

impl PathArg {
    /// A path read straight off a literal (or today's fallbacks): no template,
    /// no wrapper.
    fn plain(path: String, confidence: Confidence) -> Self {
        PathArg {
            path,
            template: None,
            confidence,
            wrapper: None,
            wrapper_recv: None,
            read: ArgRead::Direct,
        }
    }

    /// The argument could not be read.
    fn unresolved() -> Self {
        Self::plain(UNRESOLVED_PATH.to_string(), Confidence::Weak)
    }

    /// The path minus every `${…}` substitution holds a `/`: what an
    /// indirect read must show before it is taken as a URL (CH.3a rule f).
    fn static_text_has_slash(&self) -> bool {
        self.path.replace("${…}", "").contains('/')
    }
}

/// CH.3a: what one file's HTTP call-site arguments gave the graph (the
/// `[ts-endpoint-args]` marker). Only call sites that become ENDPOINTs count
/// (a shape-2 call whose base is not an import alias does not).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct EndpointArgStats {
    /// Read through a single-argument URL builder's literal (arm a).
    wrapper: usize,
    /// Read through a `const` local (arm d).
    local: usize,
    /// Read through a `readonly` field or a getter (arm e).
    field: usize,
    /// Read through a zero-argument same-class URL method (arm b).
    method: usize,
    /// Still `<unresolved>`.
    unresolved: usize,
}

impl EndpointArgStats {
    /// Call sites one of CH.3a's arms read.
    fn read(self) -> usize {
        self.wrapper + self.local + self.field + self.method
    }
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
        // CH.1: `abstract class X` is its own node kind with the same fields
        // (name / body / decorator / type_parameters + a class_heritage
        // child), so it takes the class visitor whole. `declare abstract class`
        // is an `ambient_declaration` and falls through to `_`, as `declare
        // class` does.
        "class_declaration" | "abstract_class_declaration" => {
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
    if n.kind() == "abstract_class_declaration" {
        acc.abstract_stats.classes += 1;
    }

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
    // CG.1: every method name of the body, declared before or after a field,
    // so a function-valued field named like a method mints no second node. An
    // abstract member (CH.1) is a method by name too.
    let method_names: HashSet<&str> = {
        let mut mc = body.walk();
        body.named_children(&mut mc)
            .filter(|m| matches!(m.kind(), "method_definition" | "abstract_method_signature"))
            .filter_map(|m| child_text(m, "name", src))
            .collect()
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "method_definition" => {
                visit_method(member, src, file_rel, &class_qname, class_id, repo, acc);
            }
            // CH.1: `abstract m(…): T;` — a METHOD with no body. An overload
            // `method_signature` stays ignored: the implementation after it
            // carries the node.
            "abstract_method_signature" => {
                visit_abstract_method(member, src, file_rel, &class_qname, class_id, repo, acc);
            }
            // `visit_field` reads a field's type and DI shape; a function-valued
            // field is also a METHOD (CG.1), a call-initialised one a
            // STATE_VAR (CH.2).
            "public_field_definition" => {
                visit_field(member, src, module_id, class_id, acc);
                visit_function_field(
                    member,
                    src,
                    file_rel,
                    &class_qname,
                    class_id,
                    repo,
                    &method_names,
                    acc,
                );
                visit_state_field(
                    member,
                    src,
                    file_rel,
                    &class_qname,
                    class_id,
                    repo,
                    &method_names,
                    acc,
                );
            }
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

/// CG.1: a class field whose value is a function — `private onResize = (e) =>
/// {…}`, `save = function () {…}`, `items = function* () {…}`, `static create =
/// () => …`, `#tick = () => …` — is a METHOD `<class>::<field>`. It is called
/// as `this.onResize()`, passed as a handler and holds behaviour, so it gets
/// [`visit_method`]'s treatment: DEFINES from the class, the `class_methods`
/// entry `this.m()` binds through (before `resolve_intra_file`, so a call from
/// a method declared above the field binds too), the nav record, and a call
/// walk of its body (an expression body or a block alike), whose calls, client
/// HTTP calls and TypeORM sites are the field METHOD's. Parameters are not
/// walked, as for a method.
///
/// Its CODE / POSITION / DOC are the field node's own: tree-sitter-typescript
/// keeps a field's decorators INSIDE `public_field_definition`, so CB.4's
/// [`first_decorator`] widening (for a `method_definition`, whose decorators
/// are `class_body` siblings) does not apply. A computed, string or number
/// name mints nothing here, nor does any other value (`rafId = 0`, `users$ =
/// this.api.list()`, `x = computed(() => …)`; the call-initialised two are
/// [`visit_state_field`]'s STATE_VARs, CH.2). A name a method of the class
/// holds (`method_names`, collected before the member walk) or an earlier
/// field took keeps that node and counts as `shadowed`, so no NodeId is
/// pushed twice.
#[allow(clippy::too_many_arguments)]
fn visit_function_field(
    field: TsNode,
    src: &[u8],
    file_rel: &str,
    class_qname: &str,
    class_id: NodeId,
    repo: RepoId,
    method_names: &HashSet<&str>,
    acc: &mut Acc,
) {
    let Some(value) = field.child_by_field_name("value") else {
        return;
    };
    let arrow = match value.kind() {
        "arrow_function" => true,
        "function_expression" | "generator_function" => false,
        _ => return,
    };
    let Some(name_node) = field.child_by_field_name("name") else {
        return;
    };
    if !matches!(
        name_node.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return;
    }
    // `#tick` keeps its `#`, as `visit_method` names `#m() {}`.
    let name = text(name_node, src);
    if method_names.contains(name) || acc.class_methods.contains_key(&(class_id, name.to_string()))
    {
        acc.fn_fields.shadowed += 1;
        return;
    }
    let method_qname = format!("{class_qname}::{name}");
    let method_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &method_qname);
    acc.nodes.push(Node {
        id: method_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&field, src, file_rel),
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

    record_callable(method_id, value, acc);

    let before = acc.unresolved.len() + acc.endpoints.len();
    if let Some(body) = value.child_by_field_name("body") {
        collect_calls_in(body, src, method_id, Some(class_id), acc);
    }
    let after = acc.unresolved.len() + acc.endpoints.len();
    acc.fn_fields.calls += after.saturating_sub(before);
    if arrow {
        acc.fn_fields.arrow += 1;
    } else {
        acc.fn_fields.function += 1;
    }
}

/// CH.2: the Angular factories a signal-shaped field is initialised by, for the
/// `[ts-state]` marker's `signal=` count only: every call-initialised field
/// mints the same STATE_VAR whichever factory it names. `input.required` /
/// `model.required` / `viewChild.required` count by their object.
const SIGNAL_FACTORIES: &[&str] = &[
    "signal",
    "computed",
    "linkedSignal",
    "toSignal",
    "effect",
    "input",
    "model",
    "output",
    "outputFromObservable",
    "viewChild",
    "viewChildren",
    "contentChild",
    "contentChildren",
    "resource",
    "rxResource",
    "httpResource",
];

/// CH.2: a class field initialised by a call — `readonly page = signal(1)`,
/// `total = computed(() => this.rows().length)`, `rows = input.required<T>()`,
/// `countries = toSignal(this.api.list())`, `logger = effect(() => …)`,
/// `users$ = this.subject.asObservable()`, any call but `x = inject(T)` — is
/// state its class owns: a STATE_VAR `<class>::<field>` (the kind Java / C#
/// class constants and Dart top-level vars take), DEFINED by the class. Its
/// initializer runs code, so the WHOLE initializer is walked for calls from
/// the STATE_VAR: the factory call itself (an ordinary library CallSite), the
/// calls in an arrow / function argument (a computed's or effect's body), a
/// client HTTP call's ENDPOINT, TypeORM sites and enum member reads. It takes
/// the `class_methods` entry `this.<field>()` binds through, so a signal read
/// (`this.page()`, from a method or another field's initializer, declared
/// above or below it) is an intra-file CALLS into the STATE_VAR; the graph
/// crate's `enclosing_class_or_struct` walks STATE_VAR -> CLASS, so A6.2a's
/// receiver typing binds `this.api.list()` in an initializer on `api`'s type.
///
/// Only the kind test peels `(…)`, `as T`, `satisfies T` and `!` off the
/// value. `x = inject(T)` — the callee `inject`, or a local binding of an
/// `inject` import collected before the class (`import { inject as di }`,
/// A7.1's rule) — is DI, not state: [`visit_field`] keeps its INJECTS
/// candidate and field type, and no node is minted. A `new X()`, literal,
/// object, arrow or function value mints nothing here (the last two are
/// CG.1's METHODs). A computed, string or number name mints nothing; `#x`
/// keeps its `#`. A name a method of the class holds (`method_names`, every
/// method of the body) or an earlier field took keeps that node, so no NodeId
/// is pushed twice. CODE / POSITION / DOC are the field node's own (its
/// decorators sit inside `public_field_definition`, CG.1's finding).
#[allow(clippy::too_many_arguments)]
fn visit_state_field(
    field: TsNode,
    src: &[u8],
    file_rel: &str,
    class_qname: &str,
    class_id: NodeId,
    repo: RepoId,
    method_names: &HashSet<&str>,
    acc: &mut Acc,
) {
    let Some(value) = field.child_by_field_name("value") else {
        return;
    };
    let mut inner = value;
    while matches!(
        inner.kind(),
        "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression"
    ) {
        let mut ic = inner.walk();
        let Some(next) = inner
            .named_children(&mut ic)
            .find(|c| c.kind() != "comment")
        else {
            return;
        };
        inner = next;
    }
    if inner.kind() != "call_expression" {
        return;
    }
    let callee = inner.child_by_field_name("function");
    if let Some(f) = callee
        && f.kind() == "identifier"
    {
        let name = text(f, src);
        if name == "inject" || inject_import_bindings(&acc.imports).contains(name) {
            return;
        }
    }
    let Some(name_node) = field.child_by_field_name("name") else {
        return;
    };
    if !matches!(
        name_node.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return;
    }
    let name = text(name_node, src);
    if method_names.contains(name)
        || acc
            .class_methods
            .contains_key(&(class_id, name.to_string()))
    {
        return;
    }
    let state_qname = format!("{class_qname}::{name}");
    let state_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &state_qname);
    acc.nodes.push(Node {
        id: state_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells(&field, src, file_rel),
    });
    acc.edges.push(Edge {
        from: class_id,
        to: state_id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.class_methods
        .insert((class_id, name.to_string()), state_id);
    acc.nav.record(
        state_id,
        name,
        &state_qname,
        node_kind::STATE_VAR,
        Some(class_id),
    );

    let before = acc.unresolved.len() + acc.endpoints.len();
    collect_calls_in(value, src, state_id, Some(class_id), acc);
    let after = acc.unresolved.len() + acc.endpoints.len();
    let stats = &mut acc.state_fields;
    stats.calls += after.saturating_sub(before);
    stats.fields += 1;
    if callee
        .and_then(|f| signal_factory_name(f, src))
        .is_some_and(|n| SIGNAL_FACTORIES.contains(&n))
    {
        stats.signal += 1;
    }
}

/// CH.2: the factory name a field initializer's callee names, for the
/// `signal=` count: `signal` for `signal(…)` / `signal<T>(…)`, `input` for
/// `input.required<T>()`. Any other member call (`this.subject.asObservable()`,
/// `Array.from(…)`) names none.
fn signal_factory_name<'a>(callee: TsNode, src: &'a [u8]) -> Option<&'a str> {
    match callee.kind() {
        "identifier" => Some(text(callee, src)),
        "member_expression" => {
            let object = callee.child_by_field_name("object")?;
            let property = callee.child_by_field_name("property")?;
            (object.kind() == "identifier" && text(property, src) == "required")
                .then(|| text(object, src))
        }
        _ => None,
    }
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
        // CB.4: a decorated method's span opens at its first decorator, so a
        // decorator-line marker (`@OnEvent(...)`) anchors to the method.
        cells: build_cells_from(&n, first_decorator(n), src, file_rel),
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

    record_callable(method_id, n, acc);

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

/// CH.1: an abstract class member, `abstract fetchOne(id: string): T;` or
/// `protected abstract label(): string;`, is a METHOD `<class>::<name>`: it is
/// called as `this.m()`, overridden and implemented, exactly CB.9's rule for a
/// Dart bodiless member. It gets [`visit_method`]'s treatment minus the body:
/// DEFINES from the class, the `class_methods` entry `this.m()` binds through
/// (so a concrete member declared above it binds too), and the nav record. Its
/// CODE / POSITION are the signature, opening at its first decorator (CB.4: a
/// member's decorators are `class_body` siblings). There is no body, so no
/// call walk; parameters are not walked either (`visit_method` walks only
/// `collect_inject_repository` over them, which a signature never carries).
///
/// A computed, string or number name mints nothing (as CG.1). A name the class
/// already holds (a repeated signature, which TS rejects) keeps one NodeId.
#[allow(clippy::too_many_arguments)]
fn visit_abstract_method(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    class_qname: &str,
    class_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = n.child_by_field_name("name") else {
        return;
    };
    if !matches!(
        name_node.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return;
    }
    let name = text(name_node, src);
    if acc.class_methods.contains_key(&(class_id, name.to_string())) {
        return;
    }
    let method_qname = format!("{class_qname}::{name}");
    let method_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &method_qname);
    acc.nodes.push(Node {
        id: method_id,
        repo,
        confidence: Confidence::Strong,
        cells: build_cells_from(&n, first_decorator(n), src, file_rel),
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
    // CH.1b: the graph crate pairs a subclass method of this name with it
    // (`emit_abstract_implements`). A repeated signature returned above, so
    // one fact per METHOD.
    acc.nav.record_fact(method_id, NavFact::AbstractMethod);
    acc.abstract_stats.methods += 1;
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
                payload: CellPayload::Json(glia_doc::position_json(&declarator, file_rel)),
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
    record_callable(func_id, n, acc);

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
    record_callable(func_id, value, acc);

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
                | "abstract_class_declaration"
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
            try_detect_endpoint(node, src, from, enclosing_class, acc);
            try_detect_typeorm_access(node, src, from, acc);
        }
        if kind == "member_expression" {
            record_member_ref(node, src, from, acc);
        }
        if matches!(
            kind,
            "member_expression" | "shorthand_property_identifier_pattern" | "pair_pattern"
        ) {
            record_prefix_read(node, src, from, acc);
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

/// CH.3b: member names that name a URL's API prefix, compared against
/// [`prefix_name_key`]'s form (`apiPrefix`, `API_PREFIX`, `api_base_path`,
/// `#basePath` all match). A bare `prefix` does not: it names log, cache and
/// i18n prefixes as often as URLs.
const API_PREFIX_NAMES: &[&str] = &[
    "apiprefix",
    "apibasepath",
    "apipath",
    "apiroot",
    "basepath",
    "pathprefix",
    "urlprefix",
    "routeprefix",
];

/// CH.3b: a member name ASCII-lowercased with `_` / `-` and a leading `#`
/// removed, the form [`API_PREFIX_NAMES`] holds and two spellings of one key
/// compare equal in.
fn prefix_name_key(name: &str) -> String {
    name.strip_prefix('#')
        .unwrap_or(name)
        .chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// CH.3b: record a read of an API-prefix-named member by `from`: a
/// `member_expression`'s property, whatever its object (`this.apiPrefix`,
/// `cfg.apiPrefix`, `environment.apiPrefix`, `this.#basePath`), or a
/// destructured one (`const { apiPrefix } = cfg`, `const { apiPrefix: p } =
/// cfg`). An assignment's target is a write, not a read. Which callable keys
/// on one is decided by `url_prefix_facts` once the file's intra-file calls
/// are resolved.
fn record_prefix_read(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let name = match node.kind() {
        "member_expression" => {
            if let Some(parent) = node.parent()
                && matches!(
                    parent.kind(),
                    "assignment_expression" | "augmented_assignment_expression"
                )
                && parent.child_by_field_name("left") == Some(node)
            {
                return;
            }
            match node.child_by_field_name("property") {
                Some(p)
                    if matches!(
                        p.kind(),
                        "property_identifier" | "private_property_identifier"
                    ) =>
                {
                    text(p, src)
                }
                _ => return,
            }
        }
        "shorthand_property_identifier_pattern" => text(node, src),
        "pair_pattern" => match node.child_by_field_name("key") {
            Some(k) if k.kind() == "property_identifier" => text(k, src),
            _ => return,
        },
        _ => return,
    };
    if !API_PREFIX_NAMES.contains(&prefix_name_key(name).as_str()) {
        return;
    }
    acc.prefix_reads
        .push((from, name.to_string(), node.start_byte()));
}

/// CH.3b: a callable's parameter count: the named, non-comment children of
/// its `parameters`, or 1 for an arrow's bare `parameter` (`x => …`).
fn param_count(func: TsNode) -> usize {
    if let Some(params) = func.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        params
            .named_children(&mut cursor)
            .filter(|c| c.kind() != "comment")
            .count()
    } else {
        usize::from(func.child_by_field_name("parameter").is_some())
    }
}

/// CH.3b: record `id`'s parameter count. A name declared twice (a getter and
/// its setter share one METHOD id) keeps the larger count.
fn record_callable(id: NodeId, func: TsNode, acc: &mut Acc) {
    let n = param_count(func);
    let slot = acc.callable_params.entry(id).or_insert(0);
    *slot = (*slot).max(n);
}

/// CH.3b: what `url_prefix_facts` found in one file.
#[derive(Default, Debug, PartialEq, Eq)]
struct UrlPrefixScan {
    /// `(callable, key as first spelled)`, by callable NodeId.
    facts: Vec<(NodeId, String)>,
    /// Callables (>= 1 parameter) whose closure reads two or more keys.
    ambiguous: usize,
}

/// CH.3b: the callables of one file that build a URL under an API prefix.
/// For each callable `X` with at least one parameter, in NodeId order: the
/// prefix-named members `X` reads (in source order), then those its
/// same-file callees read, then their callees' (two deep, over the
/// intra-file CALLS `edges` between `params`' callables, in edge order, each
/// callable once). Exactly one distinct key (by
/// [`prefix_name_key`]) is a fact, under the first spelling met; two or more
/// are ambiguous and record nothing (fails closed). quokka's
/// `buildApiUrl(path) { return buildApiUrlFrom(this.config, path); }` keys on
/// `apiPrefix` through `buildApiUrlFrom`'s `rawConfig.apiPrefix`, while
/// `buildApiRootUrl`, whose callees read no prefix, keys on nothing.
fn url_prefix_facts(
    params: &HashMap<NodeId, usize>,
    reads: &[(NodeId, String, usize)],
    edges: &[Edge],
) -> UrlPrefixScan {
    let mut callees: std::collections::BTreeMap<u64, Vec<NodeId>> = Default::default();
    for e in edges {
        if e.category == edge_category::CALLS
            && params.contains_key(&e.from)
            && params.contains_key(&e.to)
        {
            let list = callees.entry(e.from.0).or_default();
            if !list.contains(&e.to) {
                list.push(e.to);
            }
        }
    }
    let mut read_by: std::collections::BTreeMap<u64, Vec<(usize, &str)>> = Default::default();
    for (from, name, at) in reads {
        read_by.entry(from.0).or_default().push((*at, name));
    }
    for scope_reads in read_by.values_mut() {
        scope_reads.sort_unstable();
    }
    let mut ids: Vec<(NodeId, usize)> = params.iter().map(|(&id, &n)| (id, n)).collect();
    ids.sort_by_key(|(id, _)| id.0);

    let mut scan = UrlPrefixScan::default();
    for (x, n) in ids {
        if n == 0 {
            continue;
        }
        // (normalised key, first spelling), in closure order.
        let mut keys: Vec<(String, &str)> = Vec::new();
        let mut visited: HashSet<NodeId> = HashSet::from([x]);
        let mut level = vec![x];
        for depth in 0..=2 {
            for scope in &level {
                for &(_, name) in read_by.get(&scope.0).into_iter().flatten() {
                    let norm = prefix_name_key(name);
                    if !keys.iter().any(|(k, _)| *k == norm) {
                        keys.push((norm, name));
                    }
                }
            }
            if depth == 2 {
                break;
            }
            let mut next = Vec::new();
            for scope in &level {
                for &c in callees.get(&scope.0).into_iter().flatten() {
                    if visited.insert(c) {
                        next.push(c);
                    }
                }
            }
            level = next;
        }
        match keys.as_slice() {
            [] => {}
            [(_, key)] => scan.facts.push((x, (*key).to_string())),
            _ => scan.ambiguous += 1,
        }
    }
    scan
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
//   - Call expression whose one argument is a literal       → Weak,
//     (URL-builder wrapper, CH.3a) — that literal, the       `wrapper` on
//     callee's leaf name recorded                            ENDPOINT_HIT
//   - `this.m()`, a `const` local, `this.<readonly field>`  → the value's
//     (CH.3a) — the one declaration's value, depth one       own tier
//   - Other call expression — pluck inner literal as hint   → Weak
//   - Anything else (parameter, `let`, conditional, …)      → Weak,
//     path = `<unresolved>`
//
// Method/path normalisation (e.g. `:id` ↔ `{id}`) is HttpStackResolver's job;
// the parser stores the raw text as written.

const HTTP_METHOD_PROPS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

/// `enclosing_class` is the class whose method or field the call sits in
/// (`collect_calls_in`'s), stored on the candidate so a builder read names
/// its receiver's declared type (CH.3b `wrapper_of`).
fn try_detect_endpoint(
    call: TsNode,
    src: &[u8],
    from: NodeId,
    enclosing_class: Option<NodeId>,
    acc: &mut Acc,
) {
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
        push_endpoint(call, from, enclosing_class, method, arg, None, acc);
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
        enclosing_class,
        method_lower.to_uppercase(),
        arg,
        requires_alias,
        acc,
    );
}

fn push_endpoint(
    call: TsNode,
    from: NodeId,
    class: Option<NodeId>,
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
        wrapper,
        wrapper_recv,
        read,
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
        wrapper,
        wrapper_recv,
        class,
        read,
    });
}

fn classify_path_arg(arg: TsNode, src: &[u8]) -> PathArg {
    classify_at(arg, src, true)
}

/// CH.3a: read an HTTP call's URL argument from what the source states at the
/// call site and in the caller's own scope chain or class. Arms, in order:
///
/// - a string / template literal: itself (Strong / Medium);
/// - (a) a call whose one argument is a literal (`this.urls.buildApiUrl(…)`):
///   that literal, Weak, `wrapper` = the callee's leaf name; a template whose
///   static text holds no `/` (`this.i18n.t(`errors.${code}`)`) is no URL;
/// - (b) `this.<m>()` with no arguments: the one `return` of the same-class
///   method `m` ([`same_class_method_return`]);
/// - (c) any other call: its first string literal, Weak (the v0.4.4 hint);
/// - (d) an identifier: the value of a `const` an enclosing scope declares
///   before the use ([`local_const_value`]);
/// - (e) `this.<p>`: a `readonly` field initializer or a one-`return` getter
///   of the caller's own class ([`class_member_value`]);
/// - anything else: `<unresolved>`.
///
/// `indirect_ok` gates (b), (d) and (e): an indirect read classifies the
/// declaration it found with `indirect_ok = false`, so depth is one and no
/// read follows a second declaration. No parameter, assignment, `let` / `var`
/// or arbitrary call's return value is followed: no value data-flow.
fn classify_at(arg: TsNode, src: &[u8], indirect_ok: bool) -> PathArg {
    match arg.kind() {
        "string" => {
            let raw = text(arg, src);
            PathArg::plain(strip_string_quotes(raw), Confidence::Strong)
        }
        "template_string" => classify_template(arg, src),
        "call_expression" => {
            if let Some(read) = builder_literal(arg, src) {
                return read;
            }
            if indirect_ok
                && let Some(method) = this_zero_arg_call(arg, src)
                && let Some(expr) = same_class_method_return(arg, method, src)
            {
                return indirect(classify_at(expr, src, false), ArgRead::Method);
            }
            // URL-builder wrapper like `this.api.buildUrl('auth/login', x)` —
            // pluck the innermost string literal as a hint, weak confidence.
            find_first_string_literal(arg, src)
                .map(|s| PathArg::plain(strip_string_quotes(&s), Confidence::Weak))
                .unwrap_or_else(PathArg::unresolved)
        }
        "identifier" if indirect_ok => match local_const_value(arg, src) {
            Some(value) => indirect(classify_at(value, src, false), ArgRead::Local),
            None => PathArg::unresolved(),
        },
        "member_expression"
            if indirect_ok
                && arg
                    .child_by_field_name("object")
                    .is_some_and(|o| o.kind() == "this") =>
        {
            match class_member_value(arg, src) {
                Some(value) => indirect(classify_at(value, src, false), ArgRead::Field),
                None => PathArg::unresolved(),
            }
        }
        _ => PathArg::unresolved(),
    }
}

/// CH.3a rule f: what an indirect read (arms b, d, e) of `inner` gives. A
/// method's builder-call return keeps arm a's reading (the class wrote that
/// method to return a URL: `this.healthUrl()` returning
/// `this.urls.buildApiRootUrl('healthz')` reads `/healthz`); every other read
/// is a URL only when its static text holds a `/`, so a local such as
/// `const id = this.route.snapshot.paramMap.get('id')`, a bare
/// `const key = 'draft'` or a whole-substitution `` `${x}` `` stays
/// `<unresolved>`. Confidence is the value's own.
fn indirect(inner: PathArg, read: ArgRead) -> PathArg {
    let builder_method = read == ArgRead::Method && inner.wrapper.is_some();
    let taken = inner.path != UNRESOLVED_PATH && (builder_method || inner.static_text_has_slash());
    if taken {
        PathArg { read, ..inner }
    } else {
        PathArg {
            read,
            ..PathArg::unresolved()
        }
    }
}

/// CH.3a arm a: a call whose `arguments` are exactly one string or template
/// literal, read as that literal through a URL builder. Weak (the builder
/// may transform it), `wrapper` = the callee's leaf name. A template whose
/// static text holds no `/` is no URL: `<unresolved>`, no wrapper. None when
/// the call has another argument shape or a callee with no name, which leaves
/// it to the v0.4.4 first-literal hint.
fn builder_literal(call: TsNode, src: &[u8]) -> Option<PathArg> {
    let literal = single_literal_arg(call)?;
    let wrapper = callee_leaf_name(call, src)?;
    let mut read = if literal.kind() == "string" {
        PathArg::plain(strip_string_quotes(text(literal, src)), Confidence::Weak)
    } else {
        let read = classify_template(literal, src);
        if !read.static_text_has_slash() {
            return Some(PathArg::unresolved());
        }
        read
    };
    read.confidence = Confidence::Weak;
    read.wrapper = Some(wrapper.to_string());
    read.wrapper_recv = this_field_receiver(call, src).map(str::to_string);
    read.read = ArgRead::Wrapper;
    Some(read)
}

/// CH.3b: `f` for a call whose callee is `this.<f>.<m>` (`urls` for
/// `this.urls.buildApiUrl('x')`, `#urls` for `this.#urls.build('x')`), when
/// that `this` is the enclosing class's instance ([`this_class`]: a
/// `function` callback rebinds it). The class's field types then name the
/// builder's declared type.
fn this_field_receiver<'a>(call: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "member_expression" {
        return None;
    }
    let inner = callee.child_by_field_name("object")?;
    if inner.kind() != "member_expression" || inner.child_by_field_name("object")?.kind() != "this"
    {
        return None;
    }
    let field = inner.child_by_field_name("property")?;
    if !matches!(
        field.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return None;
    }
    this_class(inner)?;
    Some(text(field, src))
}

/// The one argument of `call` when it is a string or template literal and
/// nothing else (comments aside). A tagged template's `arguments` is the
/// template itself, not an `arguments` list: None.
fn single_literal_arg(call: TsNode) -> Option<TsNode> {
    let args = call
        .child_by_field_name("arguments")
        .filter(|a| a.kind() == "arguments")?;
    let mut cursor = args.walk();
    let mut named = args
        .named_children(&mut cursor)
        .filter(|c| c.kind() != "comment");
    let only = named.next()?;
    if named.next().is_some() {
        return None;
    }
    matches!(only.kind(), "string" | "template_string").then_some(only)
}

/// A call's callee leaf: an identifier's text or a member's `property`.
fn callee_leaf_name<'a>(call: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let callee = call.child_by_field_name("function")?;
    match callee.kind() {
        "identifier" => Some(text(callee, src)),
        "member_expression" => callee.child_by_field_name("property").map(|p| text(p, src)),
        _ => None,
    }
}

/// `m` for a `this.m()` call with no arguments.
fn this_zero_arg_call<'a>(call: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "member_expression"
        || callee.child_by_field_name("object")?.kind() != "this"
    {
        return None;
    }
    let args = call
        .child_by_field_name("arguments")
        .filter(|a| a.kind() == "arguments")?;
    if has_named_non_comment(args) {
        return None;
    }
    callee.child_by_field_name("property").map(|p| text(p, src))
}

/// True when `n` has a named child that is not a comment.
fn has_named_non_comment(n: TsNode) -> bool {
    let mut cursor = n.walk();
    n.named_children(&mut cursor).any(|c| c.kind() != "comment")
}

/// True when `n` has an anonymous child token of `kind` (`readonly`,
/// `static`, `get`, `set`): a member NAMED `get` is a named node, not this.
fn has_token(n: TsNode, kind: &str) -> bool {
    let mut cursor = n.walk();
    n.children(&mut cursor)
        .any(|c| !c.is_named() && c.kind() == kind)
}

/// The expression of a method's body when the body is exactly one `return`
/// of an expression (comments aside).
fn sole_return(method: TsNode) -> Option<TsNode> {
    let body = method.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let mut stmts = body
        .named_children(&mut cursor)
        .filter(|s| s.kind() != "comment");
    let only = stmts.next()?;
    if stmts.next().is_some() || only.kind() != "return_statement" {
        return None;
    }
    let mut inner = only.walk();
    only.named_children(&mut inner)
        .find(|e| e.kind() != "comment")
}

/// The class `this` names at `n`, as its `class_body` and whether `this` is
/// the class itself (a `static` member or a static block). An arrow keeps
/// `this`; a `function` (expression, declaration or generator) rebinds it and
/// an object-literal method is not the class's: None.
fn this_class(n: TsNode) -> Option<(TsNode, bool)> {
    let mut cur = n;
    while let Some(parent) = cur.parent() {
        match parent.kind() {
            "function_expression"
            | "function_declaration"
            | "generator_function"
            | "generator_function_declaration"
            | "class_body"
            | "program" => return None,
            "method_definition" | "public_field_definition" | "class_static_block" => {
                let body = parent.parent().filter(|b| b.kind() == "class_body")?;
                let is_static =
                    parent.kind() == "class_static_block" || has_token(parent, "static");
                return Some((body, is_static));
            }
            _ => {}
        }
        cur = parent;
    }
    None
}

/// CH.3a arm b: for `this.<m>()` with no arguments, the expression the
/// caller's own class's method `m` returns, when `m` is a plain method (no
/// `get` / `set`) of the same static-ness, takes no parameters and its body
/// is exactly one `return` (`private sessionUrl() { return
/// this.urls.buildApiUrl('protected/user/profile'); }`). Anything else: None.
fn same_class_method_return<'t>(call: TsNode<'t>, m: &str, src: &[u8]) -> Option<TsNode<'t>> {
    let (body, is_static) = this_class(call)?;
    let mut cursor = body.walk();
    let method = body.named_children(&mut cursor).find(|d| {
        d.kind() == "method_definition"
            && d.child_by_field_name("name")
                .is_some_and(|n| text(n, src) == m)
            && !has_token(*d, "get")
            && !has_token(*d, "set")
            && has_token(*d, "static") == is_static
    })?;
    if method
        .child_by_field_name("parameters")
        .is_some_and(has_named_non_comment)
    {
        return None;
    }
    sole_return(method)
}

/// CH.3a arm e: for `this.<p>`, the initializer of the caller's own class's
/// `readonly` field `p`, else the expression its getter `p` returns when the
/// getter's body is one `return`; static-ness must match the caller's `this`.
/// A non-readonly field (reassignable anywhere in the class), a constructor
/// parameter property, an inherited member, or a read inside a `function`
/// callback (another `this`): None.
fn class_member_value<'t>(member: TsNode<'t>, src: &[u8]) -> Option<TsNode<'t>> {
    let p = text(member.child_by_field_name("property")?, src);
    let (body, is_static) = this_class(member)?;
    let named = |d: &TsNode| {
        d.child_by_field_name("name")
            .is_some_and(|n| text(n, src) == p)
            && has_token(*d, "static") == is_static
    };
    let mut cursor = body.walk();
    let members: Vec<TsNode<'t>> = body.named_children(&mut cursor).collect();
    if let Some(field) = members
        .iter()
        .find(|d| d.kind() == "public_field_definition" && named(d))
    {
        return if has_token(*field, "readonly") {
            field.child_by_field_name("value")
        } else {
            None
        };
    }
    let getter = members
        .iter()
        .find(|d| d.kind() == "method_definition" && has_token(**d, "get") && named(d))?;
    sole_return(*getter)
}

/// How a scope binds a name before a use: not at all, as a `const` with a
/// readable value, or some other way (`let`, `var`, a destructuring, a
/// function or class, or only after the use).
enum ScopeBinding<'t> {
    Unbound,
    Const(TsNode<'t>),
    Shadowed,
}

/// CH.3a arm d: the value of the `const` that `ident` names, declared in an
/// enclosing scope (a block, a switch body or the module) and ending before
/// the use. The walk stops at the first scope that binds the name: a `let`,
/// `var`, destructuring, function or class binding, or one declared only
/// after the use (the TDZ), gives None, as does a parameter (defaulted ones
/// included), a `for` / `for … of` loop variable, a catch parameter, a
/// function expression's own name, or a `var` a crossed function hoists.
fn local_const_value<'t>(ident: TsNode<'t>, src: &[u8]) -> Option<TsNode<'t>> {
    let name = text(ident, src);
    let at = ident.start_byte();
    let mut cur = ident;
    while let Some(parent) = cur.parent() {
        if matches!(parent.kind(), "statement_block" | "program" | "switch_body") {
            match scope_binding(parent, name, at, src) {
                ScopeBinding::Unbound => {}
                ScopeBinding::Const(value) => return Some(value),
                ScopeBinding::Shadowed => return None,
            }
        } else if binds_outside_block(parent, name, src) {
            return None;
        }
        cur = parent;
    }
    None
}

/// True when `node`, an ancestor of a use, binds `name` outside its blocks: a
/// function-like's parameters, a function expression's own name, a `var` the
/// function hoists, a `for` / `for … of` loop variable or a catch parameter.
fn binds_outside_block(node: TsNode, name: &str, src: &[u8]) -> bool {
    let field_binds = |field: &str| {
        node.child_by_field_name(field)
            .is_some_and(|f| binds_name(f, name, src))
    };
    match node.kind() {
        "arrow_function"
        | "function_declaration"
        | "generator_function_declaration"
        | "method_definition" => params_bind(node, name, src) || hoists_var(node, name, src),
        "function_expression" | "generator_function" => {
            field_binds("name") || params_bind(node, name, src) || hoists_var(node, name, src)
        }
        "for_in_statement" => field_binds("left"),
        "for_statement" => node
            .child_by_field_name("initializer")
            .is_some_and(|i| declaration_binds(i, name, src)),
        "catch_clause" => field_binds("parameter"),
        _ => false,
    }
}

/// The statements a scope declares names with: a block's or the module's
/// children (an `export` unwrapped to its declaration), or every case body of
/// a `switch` (one scope in JS).
fn scope_statements(scope: TsNode) -> Vec<TsNode> {
    let mut cursor = scope.walk();
    let children: Vec<TsNode> = scope.named_children(&mut cursor).collect();
    if scope.kind() != "switch_body" {
        return children;
    }
    let mut out = Vec::new();
    for case in children {
        let value = case.child_by_field_name("value");
        let mut inner = case.walk();
        out.extend(
            case.named_children(&mut inner)
                .filter(|s| Some(*s) != value),
        );
    }
    out
}

/// How `scope` binds `name` for a use starting at byte `at`: the LAST binding
/// whose statement ends before the use decides; a binding that exists only
/// at or after the use still shadows every outer one (the TDZ).
fn scope_binding<'t>(scope: TsNode<'t>, name: &str, at: usize, src: &[u8]) -> ScopeBinding<'t> {
    let mut seen = false;
    let mut before: Option<ScopeBinding<'t>> = None;
    for stmt in scope_statements(scope) {
        let decl = if stmt.kind() == "export_statement" {
            match stmt.child_by_field_name("declaration") {
                Some(d) => d,
                None => continue,
            }
        } else {
            stmt
        };
        let Some(binding) = statement_binding(decl, name, src) else {
            continue;
        };
        seen = true;
        if stmt.end_byte() <= at {
            before = Some(binding);
        }
    }
    match before {
        Some(b) => b,
        None if seen => ScopeBinding::Shadowed,
        None => ScopeBinding::Unbound,
    }
}

/// How one statement binds `name`, if it does: `const name = value` is
/// `Const(value)`; any other declarator binding it, or a function / class
/// declaration of that name, is `Shadowed`.
fn statement_binding<'t>(decl: TsNode<'t>, name: &str, src: &[u8]) -> Option<ScopeBinding<'t>> {
    match decl.kind() {
        "lexical_declaration" | "variable_declaration" => {
            let is_const = decl.kind() == "lexical_declaration"
                && decl
                    .child_by_field_name("kind")
                    .is_some_and(|k| k.kind() == "const");
            let mut cursor = decl.walk();
            let declarator = decl.named_children(&mut cursor).find(|d| {
                d.kind() == "variable_declarator"
                    && d.child_by_field_name("name")
                        .is_some_and(|n| binds_name(n, name, src))
            })?;
            let target = declarator.child_by_field_name("name")?;
            match declarator.child_by_field_name("value") {
                Some(value) if is_const && target.kind() == "identifier" => {
                    Some(ScopeBinding::Const(value))
                }
                _ => Some(ScopeBinding::Shadowed),
            }
        }
        "function_declaration"
        | "generator_function_declaration"
        | "class_declaration"
        | "abstract_class_declaration" => decl
            .child_by_field_name("name")
            .is_some_and(|n| text(n, src) == name)
            .then_some(ScopeBinding::Shadowed),
        _ => None,
    }
}

/// True when a `for` initializer declares `name` (or, for an expression
/// initializer, mentions it: conservative).
fn declaration_binds(init: TsNode, name: &str, src: &[u8]) -> bool {
    match init.kind() {
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = init.walk();
            init.named_children(&mut cursor).any(|d| {
                d.kind() == "variable_declarator"
                    && d.child_by_field_name("name")
                        .is_some_and(|n| binds_name(n, name, src))
            })
        }
        _ => binds_name(init, name, src),
    }
}

/// True when a function-like's parameters bind `name`: an arrow's single
/// `parameter`, or any formal parameter's pattern (a defaulted
/// `path = '/def'` binds `path`).
fn params_bind(func: TsNode, name: &str, src: &[u8]) -> bool {
    if let Some(p) = func.child_by_field_name("parameter") {
        return binds_name(p, name, src);
    }
    let Some(params) = func.child_by_field_name("parameters") else {
        return false;
    };
    let mut cursor = params.walk();
    params.named_children(&mut cursor).any(|p| {
        let pattern = p
            .child_by_field_name("pattern")
            .or_else(|| p.child_by_field_name("name"))
            .unwrap_or(p);
        binds_name(pattern, name, src)
    })
}

/// True when a function's body declares `var name` in a nested block (a
/// `var` is function-scoped, so it shadows an outer `const` everywhere in the
/// function); nested functions and classes are their own scopes, not walked.
fn hoists_var(func: TsNode, name: &str, src: &[u8]) -> bool {
    let Some(body) = func.child_by_field_name("body") else {
        return false;
    };
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "variable_declaration" && declaration_binds(n, name, src) {
            return true;
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor).filter(|c| {
            !matches!(
                c.kind(),
                "arrow_function"
                    | "function_expression"
                    | "function_declaration"
                    | "generator_function"
                    | "generator_function_declaration"
                    | "class_declaration"
                    | "abstract_class_declaration"
                    | "class"
            )
        }));
    }
    false
}

/// True when the binding pattern binds `name`: it is that identifier, or an
/// `identifier` / shorthand property pattern inside it is. Identifiers in a
/// default value count too, which only ever makes a read more conservative.
fn binds_name(pattern: TsNode, name: &str, src: &[u8]) -> bool {
    let mut stack = vec![pattern];
    while let Some(n) = stack.pop() {
        if matches!(
            n.kind(),
            "identifier" | "shorthand_property_identifier_pattern"
        ) && text(n, src) == name
        {
            return true;
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    false
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
        wrapper: None,
        wrapper_recv: None,
        read: ArgRead::Direct,
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
///
/// `wrapper` (CH.3a) follows `template`, skipped the same way: only a path
/// read through a single-argument URL builder names it.
///
/// `wrapper_of` (CH.3b) follows `wrapper`, skipped the same way: the declared
/// type of the field a builder was reached through (`this.<f>.<m>(…)`), when
/// the enclosing class records one (a constructor parameter property, an
/// annotated field or an `inject(T)` field).
fn endpoint_hit_cell(cand: &EndpointCandidate, wrapper_of: Option<&str>) -> Cell {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        wrapper: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        wrapper_of: Option<&'a str>,
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
        wrapper: cand.wrapper.as_deref(),
        wrapper_of,
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
                // CH.1b: `super.m()` is the superclass chain's `m`, skipping
                // the caller's own class; the graph crate's
                // `resolve_inherited_calls` binds it along INHERITS_FROM.
                // `super(...)` has callee kind `super`, not a member: None.
                "super" => Some(CallQualifier::SuperMethod(name)),
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

    // CH.3b: `out.edges` now holds every intra-file CALLS, so each callable's
    // same-file call closure is known: the URL builders keyed on exactly one
    // API-prefix member get a build-time UrlPrefixKey fact.
    let prefix = url_prefix_facts(&acc.callable_params, &acc.prefix_reads, &out.edges);
    for (callable, key) in &prefix.facts {
        out.nav
            .record_fact(*callable, NavFact::UrlPrefixKey { key: key.clone() });
    }
    if !prefix.facts.is_empty() {
        let keys: std::collections::BTreeSet<&str> =
            prefix.facts.iter().map(|(_, k)| k.as_str()).collect();
        eprintln!(
            "[ts-url-prefix] builders={} ambiguous={} keys={} file={}",
            prefix.facts.len(),
            prefix.ambiguous,
            keys.into_iter().collect::<Vec<_>>().join(","),
            acc.file_rel
        );
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
        // CH.3b: the builder's receiver type, from the class's field types
        // (the A7.1 loop above has recorded its `inject(T)` fields).
        let wrapper_of = cand
            .class
            .zip(cand.wrapper_recv.as_deref())
            .and_then(|(c, f)| out.nav.field_types.get(&c)?.get(f).cloned());
        let cell = endpoint_hit_cell(&cand, wrapper_of.as_deref());
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
    build_cells_from(n, None, src, file_rel)
}

/// The CODE / POSITION / DOC cells of `n`, its CODE and POSITION starting at
/// `first` when given (CB.4: a decorated method's first decorator, which
/// tree-sitter-typescript puts BESIDE the `method_definition` in `class_body`,
/// not inside it). DOC is `n`'s leading doc either way:
/// [`glia_doc::leading_doc`] steps over the decorators to the JSDoc above them.
fn build_cells_from(n: &TsNode, first: Option<TsNode>, src: &[u8], file_rel: &str) -> Vec<Cell> {
    let start = first.as_ref().unwrap_or(n);
    let code = Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Text(
            src.get(start.start_byte()..n.end_byte())
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or("")
                .to_string(),
        ),
    };
    let pos = Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(glia_doc::position_json_span(start, n, file_rel)),
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

/// CB.4: the earliest of the decorators directly above a class-body
/// `method_definition` (its contiguous run of `decorator` previous named
/// siblings), or `None` for an undecorated method. A comment between two
/// decorators ends the run, leaving the upper decorators outside the span.
fn first_decorator(n: TsNode) -> Option<TsNode> {
    let mut first = None;
    let mut cur = n.prev_named_sibling();
    while let Some(sib) = cur.filter(|s| s.kind() == "decorator") {
        first = Some(sib);
        cur = sib.prev_named_sibling();
    }
    first
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

    // ========================================================================
    // CH.3a: URL builders and indirect URL arguments
    // ========================================================================

    /// CH.3a: the METHOD `<module>::<Class>::<name>` CALLS `endpoint:<M>:<path>`.
    fn method_calls(parse: &FileParse, module: &str, owner: &str, http: &str, path: &str) -> bool {
        let from = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            &format!("{module}::{owner}"),
        );
        has_edge(
            parse,
            from,
            endpoint_id(repo(), http, path),
            edge_category::CALLS,
        )
    }

    /// CH.3a (a): a template inside a single-argument URL builder is read with
    /// today's template rules (static parts, `${…}` per substitution), Weak,
    /// and the builder rides on ENDPOINT_HIT as `wrapper`.
    #[test]
    fn wrapper_template_reads_static_parts() {
        let src = "\
export class FriendService {
    constructor(private readonly http: any, private readonly urls: any) {}
    accept(id: string) {
        return this.http.post(this.urls.buildApiUrl(`protected/friends/accept/${encodeURIComponent(id)}`), {});
    }
}
";
        let parse = parse_file(src, "src/friend.ts", "src::friend", repo()).unwrap();
        let ep = endpoint_id(repo(), "POST", "/protected/friends/accept/${…}");
        let node = parse
            .nodes
            .iter()
            .find(|n| n.id == ep)
            .expect("builder template endpoint");
        assert_eq!(node.confidence, Confidence::Weak);
        assert!(method_calls(
            &parse,
            "src::friend",
            "FriendService::accept",
            "POST",
            "/protected/friends/accept/${…}"
        ));
        let p = endpoint_payloads(&parse, ep);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["wrapper"], "buildApiUrl");
        assert_eq!(
            p[0]["template"],
            "protected/friends/accept/${encodeURIComponent(id)}"
        );
        assert_eq!(p[0]["raw"], "protected/friends/accept/${…}");
        assert_eq!(p[0]["confidence"], "weak");
        assert!(
            !parse
                .nodes
                .iter()
                .any(|n| n.id == endpoint_id(repo(), "POST", "<unresolved>")),
            "the builder template must not fall into <unresolved>"
        );
    }

    /// CH.3a (b): a builder's plain string literal keeps today's reading and
    /// gains only the `wrapper` field, after `template` (absent here).
    #[test]
    fn wrapper_literal_gains_the_wrapper_field() {
        let src = "\
export class AuthService {
    constructor(private readonly http: any, private readonly api: any) {}
    login(payload: any): void {
        this.http.post(this.api.buildApiUrl('auth/login'), payload);
    }
}
";
        let parse = parse_file(src, "src/auth.ts", "src::auth", repo()).unwrap();
        let ep = endpoint_id(repo(), "POST", "/auth/login");
        let node = parse
            .nodes
            .iter()
            .find(|n| n.id == ep)
            .expect("builder literal endpoint");
        assert_eq!(node.confidence, Confidence::Weak);
        let p = endpoint_payloads(&parse, ep);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["wrapper"], "buildApiUrl");
        assert!(p[0].get("template").is_none());
        let raw = match &node
            .cells
            .iter()
            .find(|c| c.kind == cell_type::ENDPOINT_HIT)
            .unwrap()
            .payload
        {
            CellPayload::Json(s) => s.clone(),
            _ => String::new(),
        };
        assert!(
            raw.ends_with(r#""raw":"auth/login","wrapper":"buildApiUrl"}"#),
            "wrapper must follow raw / template: {raw}"
        );
        // A direct literal carries no wrapper: byte-identical to before CH.3a.
        let direct = "\
export class S {
    constructor(private readonly http: any) {}
    load(): void { this.http.get('/api/users'); }
}
";
        let parse = parse_file(direct, "src/s.ts", "src::s", repo()).unwrap();
        let ep = endpoint_id(repo(), "GET", "/api/users");
        let cell = parse
            .nodes
            .iter()
            .find(|n| n.id == ep)
            .and_then(|n| n.cells.iter().find(|c| c.kind == cell_type::ENDPOINT_HIT))
            .unwrap();
        assert_eq!(
            cell.payload,
            CellPayload::Json(
                r#"{"method":"GET","path":"/api/users","file":"src/s.ts","line":3,"col":20,"confidence":"strong"}"#
                    .into()
            )
        );
    }

    /// CH.3a (c): `this.sessionUrl()` with no arguments reads the one
    /// `return` of the same-class method; two statements, a parameter or a
    /// getter keep it `<unresolved>`.
    #[test]
    fn zero_arg_url_method_is_read() {
        let src = "\
export class AuthService {
    constructor(private readonly http: any, private readonly urls: any) {}
    checkSession() {
        return this.http.get(this.sessionUrl());
    }
    health() {
        return this.http.get(this.healthUrl());
    }
    private sessionUrl(): string {
        return this.urls.buildApiUrl('protected/user/profile');
    }
    private healthUrl(): string {
        return this.urls.buildApiRootUrl('healthz');
    }
}
";
        let parse = parse_file(src, "src/auth.ts", "src::auth", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::auth",
            "AuthService::checkSession",
            "GET",
            "/protected/user/profile"
        ));
        let p = endpoint_payloads(
            &parse,
            endpoint_id(repo(), "GET", "/protected/user/profile"),
        );
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["wrapper"], "buildApiUrl");
        // A method's builder-call return keeps arm a's reading (no `/` rule).
        assert!(method_calls(
            &parse,
            "src::auth",
            "AuthService::health",
            "GET",
            "/healthz"
        ));

        for (label, method) in [
            (
                "two statements",
                "private sessionUrl(): string {\n        const p = 'x';\n        return this.urls.buildApiUrl('protected/user/profile');\n    }",
            ),
            (
                "a parameter",
                "private sessionUrl(p?: string): string {\n        return this.urls.buildApiUrl('protected/user/profile');\n    }",
            ),
            (
                "a getter",
                "private get sessionUrl(): any {\n        return this.urls.buildApiUrl('protected/user/profile');\n    }",
            ),
        ] {
            let src = format!(
                "export class AuthService {{\n    constructor(private readonly http: any, private readonly urls: any) {{}}\n    checkSession() {{\n        return this.http.get(this.sessionUrl());\n    }}\n    {method}\n}}\n"
            );
            let parse = parse_file(&src, "src/auth.ts", "src::auth", repo()).unwrap();
            assert!(
                method_calls(
                    &parse,
                    "src::auth",
                    "AuthService::checkSession",
                    "GET",
                    "<unresolved>"
                ),
                "{label}: must stay <unresolved>"
            );
            assert!(
                !parse
                    .nodes
                    .iter()
                    .any(|n| n.id == endpoint_id(repo(), "GET", "/protected/user/profile")),
                "{label}: must not be read"
            );
        }
    }

    /// CH.3a (d): a `const` declared before the use in an enclosing scope is
    /// read; a parameter, a `let`, a later `const`, a shadowing parameter or
    /// loop variable, and a value with no `/` are not.
    #[test]
    fn local_const_and_shadowing() {
        let src = "\
export const API_URL = '/api/x';
export class GeoService {
    constructor(private readonly http: any) {}
    async reverse(lat: number) {
        const url = `https://nominatim.openstreetmap.org/reverse?lat=${lat}`;
        return fetch(url);
    }
    moduleConst() {
        return this.http.get(API_URL);
    }
}
";
        let parse = parse_file(src, "src/geo.ts", "src::geo", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::geo",
            "GeoService::reverse",
            "GET",
            "/reverse"
        ));
        let p = endpoint_payloads(&parse, endpoint_id(repo(), "GET", "/reverse"));
        assert_eq!(p.len(), 1);
        assert_eq!(
            p[0]["raw"],
            "https://nominatim.openstreetmap.org/reverse?lat=${…}"
        );
        assert_eq!(
            p[0]["template"],
            "https://nominatim.openstreetmap.org/reverse?lat=${lat}"
        );
        assert_eq!(p[0]["confidence"], "medium");
        assert!(
            p[0].get("wrapper").is_none(),
            "a local read names no builder"
        );
        // The hit is the call's, not the declaration's.
        assert_eq!(p[0]["line"], 6);
        assert!(method_calls(
            &parse,
            "src::geo",
            "GeoService::moduleConst",
            "GET",
            "/api/x"
        ));
        // A module const read inside a module function, and a const in a
        // `switch` case before the use (the switch body is one scope).
        let src = "\
export const API_URL = '/api/x';
export function loadX() {
    return fetch(API_URL);
}
export function pick(x: number) {
    switch (x) {
        case 1:
            const url = '/api/one';
            return fetch(url);
        default:
            return null;
    }
}
";
        let parse = parse_file(src, "src/fns.ts", "src::fns", repo()).unwrap();
        let fn_calls = |name: &str, path: &str| {
            let from = NodeId::from_parts(
                GRAPH_TYPE,
                repo(),
                node_kind::FUNCTION,
                &format!("src::fns::{name}"),
            );
            has_edge(
                &parse,
                from,
                endpoint_id(repo(), "GET", path),
                edge_category::CALLS,
            )
        };
        assert!(fn_calls("loadX", "/api/x"));
        assert!(fn_calls("pick", "/api/one"));

        for (label, owner, body) in [
            (
                "a parameter",
                "S::download",
                "download(url: string) { return this.http.get(url); }",
            ),
            (
                "a defaulted parameter",
                "S::download",
                "download(url = '/api/def') { return this.http.get(url); }",
            ),
            (
                "a let",
                "S::load",
                "load() { let u = '/a'; return this.http.get(u); }",
            ),
            (
                "a const after the use",
                "S::load",
                "load() { this.http.get(url); const url = '/a'; }",
            ),
            (
                "an arrow parameter",
                "S::load",
                "load(urls: string[]) { const url = '/a'; urls.forEach((url) => this.http.get(url)); }",
            ),
            (
                "a for-of variable",
                "S::load",
                "load(urls: string[]) { const url = '/a'; for (const url of urls) { this.http.get(url); } }",
            ),
            (
                "a catch parameter",
                "S::load",
                "load() { const url = '/a'; try { } catch (url) { this.http.get(url); } }",
            ),
            (
                "a hoisted var",
                "S::load",
                "load(x: boolean) { const url = '/a'; return (() => { if (x) { var url = '/b'; } return this.http.get(url); })(); }",
            ),
            (
                "a value with no slash",
                "S::load",
                "load() { const id = this.route.snapshot.paramMap.get('id'); return this.http.get(id); }",
            ),
            (
                "a bare value",
                "S::load",
                "load() { const key = 'draft'; return this.http.get(key); }",
            ),
            (
                "a whole-substitution template",
                "S::load",
                "load(x: string) { const u = `${x}`; return this.http.get(u); }",
            ),
            (
                "a destructured const",
                "S::load",
                "load(o: any) { const { url } = o; return this.http.get(url); }",
            ),
            (
                "a switch-case const after the use",
                "S::load",
                "load(x: number) { switch (x) { case 1: this.http.get(url); break; default: const url = '/a'; } }",
            ),
        ] {
            let src = format!(
                "const url = '/api/outer';\nexport class S {{\n    constructor(private readonly http: any, private readonly route: any) {{}}\n    {body}\n}}\n"
            );
            let parse = parse_file(&src, "src/s.ts", "src::s", repo()).unwrap();
            assert!(
                method_calls(&parse, "src::s", owner, "GET", "<unresolved>"),
                "{label}: must stay <unresolved>"
            );
            assert!(
                !parse
                    .nodes
                    .iter()
                    .any(|n| n.id == endpoint_id(repo(), "GET", "/api/outer")),
                "{label}: the outer const must not be read"
            );
        }
    }

    /// CH.3a (e): a `readonly` field initializer and a one-`return` getter of
    /// the caller's own class are read; a non-readonly field and a read inside
    /// a `function () {}` callback (another `this`) are not.
    #[test]
    fn readonly_field_and_getter() {
        let src = "\
export class NotificationsApi {
    private readonly notificationsUrl = '/api/notifications';
    private readonly base = `${environment.apiUrl}/notices`;
    private draftUrl = '/api/drafts';
    constructor(private readonly http: any) {}
    get usersUrl() { return '/api/users'; }
    notifications() { return this.http.get(this.notificationsUrl); }
    users() { return this.http.get(this.usersUrl); }
    notices() { return this.http.get(this.base, { params: {} }); }
    drafts() { return this.http.get(this.draftUrl); }
    callback(xs: any[]) { xs.forEach(function () { this.http.get(this.notificationsUrl); }); }
    arrow(xs: any[]) { xs.forEach(() => this.http.get(this.usersUrl)); }
}
";
        let parse = parse_file(src, "src/n.ts", "src::n", repo()).unwrap();
        let m = "src::n";
        assert!(method_calls(
            &parse,
            m,
            "NotificationsApi::notifications",
            "GET",
            "/api/notifications"
        ));
        let n = parse
            .nodes
            .iter()
            .find(|n| n.id == endpoint_id(repo(), "GET", "/api/notifications"))
            .unwrap();
        assert_eq!(
            n.confidence,
            Confidence::Strong,
            "a readonly string is as good as the literal"
        );
        assert!(method_calls(
            &parse,
            m,
            "NotificationsApi::users",
            "GET",
            "/api/users"
        ));
        assert!(
            method_calls(&parse, m, "NotificationsApi::arrow", "GET", "/api/users"),
            "an arrow keeps this"
        );
        // The field's template rides along for the engine's endpoint fold.
        assert!(method_calls(
            &parse,
            m,
            "NotificationsApi::notices",
            "GET",
            "${…}/notices"
        ));
        let p = endpoint_payloads(&parse, endpoint_id(repo(), "GET", "${…}/notices"));
        assert_eq!(p[0]["template"], "${environment.apiUrl}/notices");
        assert!(p[0].get("wrapper").is_none());
        assert!(method_calls(
            &parse,
            m,
            "NotificationsApi::drafts",
            "GET",
            "<unresolved>"
        ));
        assert!(method_calls(
            &parse,
            m,
            "NotificationsApi::callback",
            "GET",
            "<unresolved>"
        ));
        assert!(!method_calls(
            &parse,
            m,
            "NotificationsApi::callback",
            "GET",
            "/api/notifications"
        ));
        // A static member is not the instance's field.
        let st = "\
export class S {
    private static http: any;
    private readonly url = '/api/a';
    private static readonly staticUrl = '/api/b';
    static load() { return this.http.get(this.url); }
    static loadStatic() { return this.http.get(this.staticUrl); }
}
";
        let parse = parse_file(st, "src/st.ts", "src::st", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::st",
            "S::load",
            "GET",
            "<unresolved>"
        ));
        assert!(
            !parse
                .nodes
                .iter()
                .any(|n| n.id == endpoint_id(repo(), "GET", "/api/a"))
        );
        assert!(method_calls(
            &parse,
            "src::st",
            "S::loadStatic",
            "GET",
            "/api/b"
        ));
    }

    /// CH.3a (f): a single-argument non-URL call around a template (no `/`
    /// in its static text) is no URL: `<unresolved>`, no wrapper.
    #[test]
    fn non_url_wrapper_template_stays_unresolved() {
        let src = "\
export class S {
    constructor(private readonly http: any, private readonly i18n: any) {}
    load(code: string) { return this.http.get(this.i18n.t(`errors.${code}`)); }
}
";
        let parse = parse_file(src, "src/s.ts", "src::s", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::s",
            "S::load",
            "GET",
            "<unresolved>"
        ));
        let p = endpoint_payloads(&parse, endpoint_id(repo(), "GET", "<unresolved>"));
        assert_eq!(p.len(), 1);
        assert!(p[0].get("wrapper").is_none(), "{}", p[0]);
    }

    /// CH.3a: `[ts-endpoint-args]` counts each emitted call site once, under
    /// the outermost arm that read it, or `unresolved=`.
    #[test]
    fn endpoint_arg_stats_count_each_site_once() {
        let friend = "\
export class FriendService {
    constructor(private readonly http: any, private readonly urls: any) {}
    accept(publicId: string) {
        return this.http.post(this.urls.buildApiUrl(`protected/friends/accept/${encodeURIComponent(publicId)}`), {});
    }
    list() { return this.http.get(this.urls.buildApiUrl('protected/friends')); }
    checkSession() { return this.http.get(this.sessionUrl()); }
    download(url: string) { return this.http.get(url); }
    private sessionUrl(): string { return this.urls.buildApiUrl('protected/user/profile'); }
}
";
        let (.., stats) =
            parse_file_stats(friend, "src/friend.service.ts", "src::friend", repo()).unwrap();
        assert_eq!(
            stats,
            EndpointArgStats {
                wrapper: 2,
                local: 0,
                field: 0,
                method: 1,
                unresolved: 1
            }
        );
        let geo = "\
export class GeoService {
    private readonly notificationsUrl = '/api/notifications';
    private draftUrl = '/api/drafts';
    constructor(private readonly http: any) {}
    async reverse(lat: number, lon: number) {
        const url = `https://nominatim.openstreetmap.org/reverse?lat=${lat}&lon=${lon}`;
        return fetch(url);
    }
    notifications() { return this.http.get(this.notificationsUrl); }
    drafts() { return this.http.get(this.draftUrl); }
    mutable() { let u = '/api/a'; u = '/api/b'; return this.http.get(u); }
    plain() { return this.http.get('/api/plain'); }
}
";
        let (.., stats) = parse_file_stats(geo, "src/geo.service.ts", "src::geo", repo()).unwrap();
        assert_eq!(
            stats,
            EndpointArgStats {
                wrapper: 0,
                local: 1,
                field: 1,
                method: 0,
                unresolved: 2
            }
        );
        // A shape-2 call whose base is no import alias emits nothing: not counted.
        let gated = "export function f(axios: any) { const u = '/a/b'; axios.get(u); }\n";
        let (.., stats) = parse_file_stats(gated, "src/g.ts", "src::g", repo()).unwrap();
        assert_eq!(stats, EndpointArgStats::default());
    }

    /// CH.3b: the build-time facts `parse` records for the node `kind` /
    /// `qname` (empty when none).
    fn facts_of(parse: &FileParse, kind: glia_core::NodeKindId, qname: &str) -> Vec<NavFact> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        parse.nav.nav_facts.get(&id).cloned().unwrap_or_default()
    }

    fn prefix_key(key: &str) -> Vec<NavFact> {
        vec![NavFact::UrlPrefixKey { key: key.into() }]
    }

    /// CH.3b (a): quokka's api-url-builder.service.ts reduced. `buildApiUrl`
    /// reads no prefix itself but calls the same-file `buildApiUrlFrom`,
    /// which reads `config.apiPrefix`: both key on `apiPrefix`.
    /// `buildApiRootUrl`'s callees read none, and `this.config.apiBaseUrl` is
    /// no prefix name: no fact, so the fold leaves `healthz` alone.
    #[test]
    fn builder_through_same_file_function_gets_the_key() {
        let src = "\
import { Injectable } from '@angular/core';

export interface Cfg { apiBaseUrl: string; apiPrefix: string; }

@Injectable({ providedIn: 'root' })
export class ApiUrlBuilderService {
    private readonly config: Cfg = { apiBaseUrl: 'http://localhost:8080', apiPrefix: '/api' };

    buildApiUrl(path: string): string {
        return buildApiUrlFrom(this.config, path);
    }

    buildApiRootUrl(path: string): string {
        const rootPath = `/${stripLeadingSlash(path)}`;
        return new URL(rootPath, ensureTrailingSlash(this.config.apiBaseUrl)).toString();
    }
}

export function buildApiUrlFrom(config: Partial<Cfg>, path: string): string {
    const prefix = normalizeApiPrefix(config.apiPrefix || '/api');
    return `${prefix}/${stripLeadingSlash(path)}`;
}

function normalizeApiPrefix(raw: string): string {
    return raw.trim();
}

function stripLeadingSlash(value: string): string {
    return value.replace(/^\\/+/, '');
}

function ensureTrailingSlash(value: string): string {
    return value.endsWith('/') ? value : `${value}/`;
}
";
        let parse = parse_file(src, "src/api-url-builder.service.ts", "src::urls", repo()).unwrap();
        let m = |name: &str| facts_of(&parse, node_kind::METHOD, &format!("src::urls::ApiUrlBuilderService::{name}"));
        let f = |name: &str| facts_of(&parse, node_kind::FUNCTION, &format!("src::urls::{name}"));
        assert_eq!(m("buildApiUrl"), prefix_key("apiPrefix"), "through buildApiUrlFrom");
        assert_eq!(f("buildApiUrlFrom"), prefix_key("apiPrefix"), "its own read");
        assert_eq!(m("buildApiRootUrl"), [], "reads apiBaseUrl only");
        for helper in ["normalizeApiPrefix", "stripLeadingSlash", "ensureTrailingSlash"] {
            assert_eq!(f(helper), [], "{helper} reads no prefix member");
        }
        // The facts are build-time only: no node, edge or cell changes.
        assert!(
            parse.nodes.iter().all(|n| n.cells.iter().all(|c| match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => !s.contains("UrlPrefixKey"),
                _ => true,
            }))
        );
    }

    /// CH.3b (b): a direct read keys the callable, whatever the object or
    /// spelling (`this.apiPrefix`, `env.API_PREFIX`, a destructured
    /// `{ apiPrefix }` or `{ basePath: base }`, `this.#basePath`, an arrow
    /// field's bare parameter counts as one); a zero-parameter callable
    /// builds no URL from a path and gets none; a read three calls deep is
    /// past the closure.
    #[test]
    fn direct_member_read() {
        let src = "\
declare const env: { API_PREFIX: string };

export class Urls {
    readonly apiPrefix = '/api';
    #basePath = '/v2';

    buildWsUrl(p: string): string {
        return `${this.apiPrefix}/${p}`;
    }

    logUrls(): void {
        console.info(this.apiPrefix);
    }

    fromEnv(p: string): string {
        return env.API_PREFIX + p;
    }

    fromCfg(cfg: { apiPrefix: string }, p: string): string {
        const { apiPrefix } = cfg;
        return apiPrefix + p;
    }

    fromPair(cfg: { basePath: string }, p: string): string {
        const { basePath: base } = cfg;
        return base + p;
    }

    onPath = p => this.#basePath + p;
}

export function deep(p: string): string { return d1(p); }
function d1(p: string): string { return d2(p); }
function d2(p: string): string { return d3(p); }
function d3(p: string): string { return cfg.apiPrefix + p; }
declare const cfg: { apiPrefix: string };
";
        let parse = parse_file(src, "src/urls.ts", "src::urls", repo()).unwrap();
        let m = |name: &str| facts_of(&parse, node_kind::METHOD, &format!("src::urls::Urls::{name}"));
        let f = |name: &str| facts_of(&parse, node_kind::FUNCTION, &format!("src::urls::{name}"));
        assert_eq!(m("buildWsUrl"), prefix_key("apiPrefix"));
        assert_eq!(m("logUrls"), [], "no parameter: not a URL builder");
        assert_eq!(m("fromEnv"), prefix_key("API_PREFIX"), "spelling as written");
        assert_eq!(m("fromCfg"), prefix_key("apiPrefix"), "a destructured read");
        assert_eq!(m("fromPair"), prefix_key("basePath"), "a renamed destructured read");
        assert_eq!(m("onPath"), prefix_key("#basePath"), "an arrow field, one bare parameter");
        assert_eq!(f("d3"), prefix_key("apiPrefix"));
        assert_eq!(f("d2"), prefix_key("apiPrefix"), "one call deep");
        assert_eq!(f("d1"), prefix_key("apiPrefix"), "two calls deep");
        assert_eq!(f("deep"), [], "three calls deep is past the closure");
    }

    /// CH.3b (c): two distinct keys in one closure fail closed; two
    /// spellings of one key are one key, under the first spelling met.
    #[test]
    fn two_keys_is_ambiguous() {
        let src = "\
export class Urls {
    readonly apiPrefix = '/api';
    readonly basePath = '/base';
    readonly API_PREFIX = '/api';

    build(p: string): string {
        return `${this.apiPrefix}${this.basePath}/${p}`;
    }

    viaHelper(p: string): string {
        return this.apiPrefix + helper(p);
    }

    sameKeyTwice(p: string): string {
        return this.apiPrefix + this.API_PREFIX + p;
    }
}

function helper(p: string): string {
    return cfg.basePath + p;
}
declare const cfg: { basePath: string };
";
        let parse = parse_file(src, "src/urls.ts", "src::urls", repo()).unwrap();
        let m = |name: &str| facts_of(&parse, node_kind::METHOD, &format!("src::urls::Urls::{name}"));
        assert_eq!(m("build"), [], "apiPrefix + basePath: ambiguous");
        assert_eq!(m("viaHelper"), [], "a callee's second key is ambiguous too");
        assert_eq!(m("sameKeyTwice"), prefix_key("apiPrefix"), "one key, first spelling");
        assert_eq!(
            facts_of(&parse, node_kind::FUNCTION, "src::urls::helper"),
            prefix_key("basePath")
        );
        let scan = url_prefix_facts(&HashMap::new(), &[], &[]);
        assert_eq!(scan, UrlPrefixScan::default());
    }

    /// CH.3b (d): a builder-read ENDPOINT_HIT names the builder's receiver
    /// type after `wrapper`: a constructor parameter property, an `inject(T)`
    /// field, or through a zero-argument URL method's `return`. An untyped
    /// receiver (`any`) or a `function` callback's `this` names none.
    #[test]
    fn wrapper_of_names_the_receiver_type() {
        let payload = |parse: &FileParse, http: &str, path: &str| -> String {
            parse
                .nodes
                .iter()
                .filter(|n| n.id == endpoint_id(repo(), http, path))
                .flat_map(|n| n.cells.iter())
                .find_map(|c| match (&c.payload, c.kind == cell_type::ENDPOINT_HIT) {
                    (CellPayload::Json(s), true) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no ENDPOINT_HIT on {http} {path}"))
        };
        let ctor = "\
import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { ApiUrlBuilderService } from './api-url-builder.service';

@Injectable({ providedIn: 'root' })
export class FriendService {
    constructor(private readonly http: HttpClient, private readonly apiUrlBuilder: ApiUrlBuilderService) {}

    list() {
        return this.http.get(this.apiUrlBuilder.buildApiUrl('protected/friends'));
    }

    checkSession() {
        return this.http.get(this.sessionUrl());
    }

    later() {
        setTimeout(function () {
            this.http.get(this.apiUrlBuilder.buildApiUrl('protected/later/x'));
        });
    }

    private sessionUrl(): string {
        return this.apiUrlBuilder.buildApiUrl('protected/user/profile');
    }
}
";
        let parse = parse_file(ctor, "src/friend.service.ts", "src::friend", repo()).unwrap();
        let hit = payload(&parse, "GET", "/protected/friends");
        assert!(
            hit.ends_with(
                r#""raw":"protected/friends","wrapper":"buildApiUrl","wrapper_of":"ApiUrlBuilderService"}"#
            ),
            "wrapper_of follows wrapper: {hit}"
        );
        let p = endpoint_payloads(&parse, endpoint_id(repo(), "GET", "/protected/user/profile"));
        assert_eq!(p[0]["wrapper"], "buildApiUrl");
        assert_eq!(p[0]["wrapper_of"], "ApiUrlBuilderService", "through the URL method");
        let p = endpoint_payloads(&parse, endpoint_id(repo(), "GET", "/protected/later/x"));
        assert_eq!(p[0]["wrapper"], "buildApiUrl");
        assert!(p[0].get("wrapper_of").is_none(), "a `function` rebinds `this`: {}", p[0]);

        let injected = "\
import { Injectable, inject } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { ApiUrlBuilderService } from './api-url-builder.service';

@Injectable({ providedIn: 'root' })
export class FriendService {
    private readonly http = inject(HttpClient);
    private readonly apiUrlBuilder = inject(ApiUrlBuilderService);

    list() {
        return this.http.get(this.apiUrlBuilder.buildApiUrl('protected/friends'));
    }
}
";
        let parse = parse_file(injected, "src/friend.service.ts", "src::friend", repo()).unwrap();
        let p = endpoint_payloads(&parse, endpoint_id(repo(), "GET", "/protected/friends"));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["wrapper"], "buildApiUrl");
        assert_eq!(p[0]["wrapper_of"], "ApiUrlBuilderService", "an inject(T) field");

        let untyped = "\
export class FriendService {
    constructor(private readonly http: any, private readonly api: any) {}
    list() {
        return this.http.get(this.api.buildApiUrl('protected/friends'));
    }
}
";
        let parse = parse_file(untyped, "src/friend.service.ts", "src::friend", repo()).unwrap();
        let hit = payload(&parse, "GET", "/protected/friends");
        assert!(hit.ends_with(r#""wrapper":"buildApiUrl"}"#), "no wrapper_of: {hit}");
    }

    /// CH.3b (e): prefix-like names that are not API prefixes (`cachePrefix`,
    /// a bare `prefix`, `apiBaseUrl`), and a write to `apiPrefix`, key
    /// nothing.
    #[test]
    fn unrelated_prefix_names_ignored() {
        let src = "\
export class Store {
    readonly cachePrefix = 'c:';
    readonly prefix = 'p:';
    apiPrefix = '/api';
    readonly apiBaseUrl = 'http://localhost';

    key(id: string): string {
        return this.cachePrefix + this.prefix + id;
    }

    origin(p: string): string {
        return this.apiBaseUrl + p;
    }

    setPrefix(p: string): void {
        this.apiPrefix = p;
    }
}
";
        let parse = parse_file(src, "src/store.ts", "src::store", repo()).unwrap();
        assert!(
            parse
                .nav
                .nav_facts
                .values()
                .flatten()
                .all(|f| !matches!(f, NavFact::UrlPrefixKey { .. })),
            "{:?}",
            parse.nav.nav_facts
        );
        assert!(
            ["cachePrefix", "prefix", "apiBaseUrl", "logPrefix", "i18nPrefix"]
                .iter()
                .all(|n| !API_PREFIX_NAMES.contains(&prefix_name_key(n).as_str()))
        );
        assert!(
            ["apiPrefix", "API_PREFIX", "api_base_path", "#basePath", "api-root"]
                .iter()
                .all(|n| API_PREFIX_NAMES.contains(&prefix_name_key(n).as_str()))
        );
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

    /// CB.4: the CODE / POSITION / DOC cells of the METHOD `qname`.
    fn method_cells(parse: &FileParse, qname: &str) -> (String, String, Option<String>) {
        let m = id(node_kind::METHOD, qname);
        let node = parse
            .nodes
            .iter()
            .find(|n| n.id == m)
            .unwrap_or_else(|| panic!("METHOD {qname} missing"));
        let text = |kind| {
            node.cells.iter().find_map(|c| match &c.payload {
                CellPayload::Text(t) | CellPayload::Json(t) if c.kind == kind => Some(t.clone()),
                _ => None,
            })
        };
        (
            text(cell_type::CODE).expect("CODE"),
            text(cell_type::POSITION).expect("POSITION"),
            text(cell_type::DOC),
        )
    }

    const DECORATED: &str = "\
import { OnEvent } from \"@nestjs/event-emitter\";

export class Listener {
  @OnEvent('x')
  @Other()
  onX() {}

  plain() {
    return 1;
  }

  /** Handles y. */
  @OnEvent('y')
  onY() {}
}
";

    /// CB.4: tree-sitter-typescript puts a method's decorators beside its
    /// `method_definition`; the method's CODE and POSITION still open at the
    /// first of them.
    #[test]
    fn a_decorated_method_starts_at_its_first_decorator() {
        let parse = parse_file(DECORATED, "src/listener.ts", "src::listener", repo()).unwrap();
        let (code, pos, _) = method_cells(&parse, "src::listener::Listener::onX");
        assert_eq!(
            pos, r#"{"file":"src/listener.ts","start_line":3,"end_line":5}"#,
            "`@OnEvent('x')` is row 3"
        );
        assert!(code.starts_with("@OnEvent('x')"), "CODE: {code}");
        assert!(code.ends_with("onX() {}"), "CODE: {code}");
        assert!(
            code.contains("@Other()"),
            "every decorator of the group: {code}"
        );
    }

    #[test]
    fn an_undecorated_method_is_unchanged() {
        let parse = parse_file(DECORATED, "src/listener.ts", "src::listener", repo()).unwrap();
        let (code, pos, doc) = method_cells(&parse, "src::listener::Listener::plain");
        assert_eq!(
            pos,
            r#"{"file":"src/listener.ts","start_line":7,"end_line":9}"#
        );
        assert!(code.starts_with("plain()"), "CODE: {code}");
        assert_eq!(doc, None);
    }

    #[test]
    fn jsdoc_above_decorators_is_still_the_doc() {
        let parse = parse_file(DECORATED, "src/listener.ts", "src::listener", repo()).unwrap();
        let (code, pos, doc) = method_cells(&parse, "src::listener::Listener::onY");
        assert_eq!(
            pos,
            r#"{"file":"src/listener.ts","start_line":12,"end_line":13}"#
        );
        assert!(
            code.starts_with("@OnEvent('y')"),
            "the JSDoc is DOC, not CODE: {code}"
        );
        assert_eq!(doc.as_deref(), Some("Handles y."));
    }

    // ------------------------------------------------------------------------
    // CG.1: function-valued class fields are METHODs
    // ------------------------------------------------------------------------

    /// The simple names of the cross-file CallSites made from `from`.
    fn call_site_names(parse: &FileParse, from: NodeId) -> Vec<String> {
        parse
            .calls
            .iter()
            .filter(|c| c.from == from)
            .map(|c| match &c.qualifier {
                CallQualifier::Bare(n) | CallQualifier::SelfMethod(n) => n.clone(),
                CallQualifier::Attribute { name, .. }
                | CallQualifier::ComplexReceiver { name, .. } => name.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    /// The `[ts-fields]` counters of one parse, as `(fields, shadowed)`.
    fn fn_field_counts(src: &str, path: &str, module: &str) -> (usize, usize) {
        let (_, stats, ..) = parse_file_stats(src, path, module, repo()).unwrap();
        (stats.fields(), stats.shadowed)
    }

    #[test]
    fn arrow_field_is_a_method_with_its_calls() {
        use glia_code_domain::evidence::{Basis, Evidence};
        let src = "\
class Hero {
  stopAnimation(): void {}
  renderFrame(t: number): void {}
  private handleMotionPreferenceChange = (event: MediaQueryListEvent): void => {
    if (event.matches) {
      this.stopAnimation();
      this.renderFrame(performance.now());
    }
  };
}
";
        let parse = parse_file(src, "src/hero.component.ts", "src::hero", repo()).unwrap();
        let class = id(node_kind::CLASS, "src::hero::Hero");
        let field = id(node_kind::METHOD, "src::hero::Hero::handleMotionPreferenceChange");
        assert!(has_node(&parse, field), "the arrow field is a METHOD");
        assert!(has_edge(&parse, class, field, edge_category::DEFINES));
        for (callee, row) in [("stopAnimation", 5), ("renderFrame", 6)] {
            let to = id(node_kind::METHOD, &format!("src::hero::Hero::{callee}"));
            let edge = parse
                .edges
                .iter()
                .find(|e| e.from == field && e.to == to && e.category == edge_category::CALLS)
                .unwrap_or_else(|| panic!("field -> {callee} resolves in-file"));
            let ev = Evidence::of(edge).expect("intra-file CALLS carries evidence");
            assert_eq!((ev.line, ev.basis), (Some(row), Basis::Site), "{callee}");
            assert_eq!(ev.rule.as_deref(), Some("intra_file"));
        }
        assert!(
            !parse
                .edges
                .iter()
                .any(|e| e.from == class && e.category == edge_category::CALLS),
            "the class takes none of the field body's calls"
        );
        let (code, pos, _) = method_cells(&parse, "src::hero::Hero::handleMotionPreferenceChange");
        assert!(pos.contains("\"start_line\":3,"), "the field's row: {pos}");
        assert!(code.starts_with("private handleMotionPreferenceChange"), "CODE: {code}");
        assert_eq!(fn_field_counts(src, "src/hero.component.ts", "src::hero"), (1, 0));
    }

    #[test]
    fn this_call_binds_to_an_arrow_field() {
        let src = "\
class Hero {
  ngAfterViewInit(): void { this.applyColor(); }
  private applyColor = (): void => {};
}
";
        let parse = parse_file(src, "src/hero.component.ts", "src::hero", repo()).unwrap();
        let init = id(node_kind::METHOD, "src::hero::Hero::ngAfterViewInit");
        let field = id(node_kind::METHOD, "src::hero::Hero::applyColor");
        assert!(
            has_edge(&parse, init, field, edge_category::CALLS),
            "a method declared above the field binds `this.applyColor()` to it"
        );
        assert!(call_site_names(&parse, init).is_empty(), "{:?}", parse.calls);
    }

    #[test]
    fn function_expression_and_generator_fields() {
        let src = "\
class C {
  persist(): void {}
  onSave = function (this: C) { this.persist(); };
  items = function* (this: C) { yield this.persist(); };
}
";
        let parse = parse_file(src, "src/c.ts", "src::c", repo()).unwrap();
        let persist = id(node_kind::METHOD, "src::c::C::persist");
        for field in ["onSave", "items"] {
            let m = id(node_kind::METHOD, &format!("src::c::C::{field}"));
            assert!(has_node(&parse, m), "{field} is a METHOD");
            assert!(has_edge(&parse, m, persist, edge_category::CALLS), "{field} -> persist");
        }
        let (_, stats, ..) = parse_file_stats(src, "src/c.ts", "src::c", repo()).unwrap();
        assert_eq!((stats.arrow, stats.function, stats.calls), (0, 2, 2));
    }

    #[test]
    fn static_and_private_fields() {
        let src = "class Store { notify() {} #tick = () => this.notify(); static create = () => new Store(); }\n";
        let parse = parse_file(src, "src/store.js", "src::store", repo()).unwrap();
        let notify = id(node_kind::METHOD, "src::store::Store::notify");
        let tick = id(node_kind::METHOD, "src::store::Store::#tick");
        let create = id(node_kind::METHOD, "src::store::Store::create");
        assert!(has_node(&parse, tick), "`#tick` keeps its `#`");
        assert!(has_node(&parse, create), "a static arrow field is a METHOD");
        assert!(has_edge(&parse, tick, notify, edge_category::CALLS));
        assert!(
            !parse.edges.iter().any(|e| e.from == create && e.category == edge_category::CALLS),
            "`new Store()` is not a call"
        );
        assert!(call_site_names(&parse, create).is_empty(), "{:?}", parse.calls);
    }

    #[test]
    fn non_function_fields_mint_nothing() {
        let src = "\
import { inject } from '@angular/core';
import { ApiService } from './api.service';

export class K {
  rafId = 0;
  label = 'x';
  state = { n: 0 };
  users$ = this.api.list();
  private api = inject(ApiService);
}
";
        let parse = parse_file(src, "src/k.component.ts", "src::k", repo()).unwrap();
        for field in ["rafId", "label", "state", "users$", "api"] {
            let m = id(node_kind::METHOD, &format!("src::k::K::{field}"));
            assert!(!has_node(&parse, m), "{field} is data, not a METHOD");
        }
        // CH.2: the call-initialised `users$` is a STATE_VAR that owns the
        // `list` call; the literal fields and the inject() field mint none.
        let users = id(node_kind::STATE_VAR, "src::k::K::users$");
        let list = parse
            .calls
            .iter()
            .find(|c| matches!(
                &c.qualifier,
                CallQualifier::ComplexReceiver { name, .. } | CallQualifier::Attribute { name, .. }
                    if name == "list"
            ))
            .unwrap_or_else(|| panic!("the `list` CallSite: {:?}", parse.calls));
        assert_eq!(list.from, users, "the STATE_VAR makes the `list` call");
        for field in ["rafId", "label", "state", "api"] {
            let v = id(node_kind::STATE_VAR, &format!("src::k::K::{field}"));
            assert!(!has_node(&parse, v), "{field} is no STATE_VAR");
        }
        assert_eq!(inject_targets(&parse, "src::k", "src::k::K"), vec!["ApiService".to_string()]);
        assert_eq!(fn_field_counts(src, "src/k.component.ts", "src::k"), (0, 0));
    }

    #[test]
    fn shadowed_field_keeps_the_method() {
        let src = "class K { f = () => a(); f() { b(); } }\n";
        let parse = parse_file(src, "src/k.ts", "src::k", repo()).unwrap();
        let f = id(node_kind::METHOD, "src::k::K::f");
        assert_eq!(parse.nodes.iter().filter(|n| n.id == f).count(), 1, "one node for `f`");
        assert_eq!(call_site_names(&parse, f), vec!["b".to_string()], "the method's body only");
        assert_eq!(fn_field_counts(src, "src/k.ts", "src::k"), (0, 1));

        // A second field of one name keeps the first field's node.
        let twice = "class K { g = () => a(); g = () => b(); }\n";
        let parse = parse_file(twice, "src/k.ts", "src::k", repo()).unwrap();
        let g = id(node_kind::METHOD, "src::k::K::g");
        assert_eq!(parse.nodes.iter().filter(|n| n.id == g).count(), 1);
        assert_eq!(call_site_names(&parse, g), vec!["a".to_string()]);
        assert_eq!(fn_field_counts(twice, "src/k.ts", "src::k"), (1, 1));
    }

    #[test]
    fn arrow_field_http_call_is_the_fields() {
        let src = "\
export class UsersService {
  constructor(private http: HttpClient) {}
  load = () => this.http.get('/api/users');
}
";
        let parse = parse_file(src, "src/users.service.ts", "src::users", repo()).unwrap();
        let ep = endpoint_id(repo(), "GET", "/api/users");
        let load = id(node_kind::METHOD, "src::users::UsersService::load");
        let froms: Vec<NodeId> = parse
            .edges
            .iter()
            .filter(|e| e.to == ep && e.category == edge_category::CALLS)
            .map(|e| e.from)
            .collect();
        assert_eq!(froms, vec![load], "the field METHOD makes the HTTP call");
    }

    // ---- CH.2: call-initialised class fields ----------------------------------

    /// The `[ts-state]` counters of one parse, as `(fields, signal, calls)`.
    fn state_field_counts(src: &str, path: &str, module: &str) -> (usize, usize, usize) {
        let (_, _, _, stats, _) = parse_file_stats(src, path, module, repo()).unwrap();
        (stats.fields, stats.signal, stats.calls)
    }

    /// The POSITION cell of the STATE_VAR `qname`.
    fn state_position(parse: &FileParse, qname: &str) -> String {
        let v = id(node_kind::STATE_VAR, qname);
        let node = parse
            .nodes
            .iter()
            .find(|n| n.id == v)
            .unwrap_or_else(|| panic!("STATE_VAR {qname} missing"));
        node.cells
            .iter()
            .find_map(|c| match &c.payload {
                CellPayload::Json(t) if c.kind == cell_type::POSITION => Some(t.clone()),
                _ => None,
            })
            .expect("POSITION")
    }

    /// The intra-file CALLS edge `from -> to`, its evidence checked: the
    /// call's own 0-based row, a site, the `intra_file` rule.
    fn assert_intra_call(parse: &FileParse, from: NodeId, to: NodeId, row: u32, what: &str) {
        use glia_code_domain::evidence::{Basis, Evidence};
        let edge = parse
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.category == edge_category::CALLS)
            .unwrap_or_else(|| panic!("{what} resolves in-file, edges {:?}", parse.edges));
        let ev = Evidence::of(edge).expect("intra-file CALLS carries evidence");
        assert_eq!((ev.line, ev.basis), (Some(row), Basis::Site), "{what}");
        assert_eq!(ev.rule.as_deref(), Some("intra_file"), "{what}");
    }

    const GRID: &str = "\
import { computed, input, signal } from '@angular/core';

export class Grid {
  readonly rows = input.required<string[]>();
  readonly page = signal(1);
  readonly total = computed(() => this.rows().length * this.page());
  readonly isLong = computed(() => this.longest(this.rows()) > 10);
  longest(rows: string[]): number { return rows.length; }
  next(): void {
    this.page.set(this.page() + 1);
  }
}
";

    #[test]
    fn signal_and_computed_fields_are_state_vars() {
        let parse = parse_file(GRID, "src/grid.component.ts", "src::grid", repo()).unwrap();
        let class = id(node_kind::CLASS, "src::grid::Grid");
        for (field, row) in [("rows", 3), ("page", 4), ("total", 5), ("isLong", 6)] {
            let qname = format!("src::grid::Grid::{field}");
            let v = id(node_kind::STATE_VAR, &qname);
            assert!(has_node(&parse, v), "{field} is a STATE_VAR");
            assert_eq!(parse.nodes.iter().filter(|n| n.id == v).count(), 1, "{field} once");
            assert!(has_edge(&parse, class, v, edge_category::DEFINES), "DEFINES {field}");
            assert!(
                !has_node(&parse, id(node_kind::METHOD, &qname)),
                "{field} is no METHOD"
            );
            let pos = state_position(&parse, &qname);
            assert!(pos.contains(&format!("\"start_line\":{row},")), "{field}: {pos}");
            assert_eq!(parse.nav.kind_by_id.get(&v), Some(&node_kind::STATE_VAR));
            assert_eq!(parse.nav.parent_of.get(&v), Some(&class), "{field}'s nav parent");
        }
        assert_eq!(
            state_field_counts(GRID, "src/grid.component.ts", "src::grid"),
            (4, 4, 8),
            "rows / page / total / isLong are signal factories; calls: 4 factories, \
             rows + page in total, longest + rows in isLong"
        );
    }

    #[test]
    fn computed_body_calls_and_signal_reads_bind() {
        let parse = parse_file(GRID, "src/grid.component.ts", "src::grid", repo()).unwrap();
        let v = |f: &str| id(node_kind::STATE_VAR, &format!("src::grid::Grid::{f}"));
        let m = |f: &str| id(node_kind::METHOD, &format!("src::grid::Grid::{f}"));
        assert_intra_call(&parse, v("total"), v("rows"), 5, "total -> rows");
        assert_intra_call(&parse, v("total"), v("page"), 5, "total -> page");
        assert_intra_call(&parse, v("isLong"), m("longest"), 6, "isLong -> longest (declared below)");
        assert_intra_call(&parse, v("isLong"), v("rows"), 6, "isLong -> rows");
        assert_intra_call(&parse, m("next"), v("page"), 9, "next -> page (a method's signal read)");
        // The factory call is an ordinary library CallSite of the field.
        assert_eq!(call_site_names(&parse, v("total")), vec!["computed".to_string()]);
        assert_eq!(call_site_names(&parse, v("rows")), vec!["required".to_string()]);
        assert_eq!(call_site_names(&parse, m("next")), vec!["set".to_string()]);
        let class = id(node_kind::CLASS, "src::grid::Grid");
        assert!(
            !parse.edges.iter().any(|e| e.from == class && e.category == edge_category::CALLS),
            "the class takes none of the initializers' calls"
        );
        assert!(
            !parse.calls.iter().any(|c| c.from == class),
            "nor their CallSites: {:?}",
            parse.calls
        );
    }

    #[test]
    fn inject_new_and_literal_fields_mint_nothing() {
        let src = "\
import { inject, inject as di } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { BehaviorSubject } from 'rxjs';
import { Store } from './store';

export class K {
  private readonly http = inject(HttpClient);
  private readonly store = di(Store);
  private readonly subject = new BehaviorSubject(0);
  private rafId = 0;
  state = { n: 0 };
}
";
        let parse = parse_file(src, "src/k.component.ts", "src::k", repo()).unwrap();
        for field in ["http", "store", "subject", "rafId", "state"] {
            let qname = format!("src::k::K::{field}");
            assert!(!has_node(&parse, id(node_kind::STATE_VAR, &qname)), "{field} is no STATE_VAR");
            assert!(!has_node(&parse, id(node_kind::METHOD, &qname)), "{field} is no METHOD");
        }
        assert_eq!(
            inject_targets(&parse, "src::k", "src::k::K"),
            vec!["HttpClient".to_string(), "Store".to_string()],
            "both inject() fields keep their INJECTS"
        );
        assert_eq!(state_field_counts(src, "src/k.component.ts", "src::k"), (0, 0, 0));
    }

    #[test]
    fn field_http_call_is_the_state_vars() {
        let src = "\
import { inject } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { toSignal } from '@angular/core/rxjs-interop';

export class ProfileComponent {
  private readonly http = inject(HttpClient);
  readonly profile = toSignal(this.http.get<string>('/api/profile'));
}
";
        let parse =
            parse_file(src, "src/profile.component.ts", "src::profile", repo()).unwrap();
        let ep = endpoint_id(repo(), "GET", "/api/profile");
        assert!(has_node(&parse, ep), "the client call in a field is an ENDPOINT");
        let profile = id(node_kind::STATE_VAR, "src::profile::ProfileComponent::profile");
        let froms: Vec<NodeId> = parse
            .edges
            .iter()
            .filter(|e| e.to == ep && e.category == edge_category::CALLS)
            .map(|e| e.from)
            .collect();
        assert_eq!(froms, vec![profile], "the STATE_VAR makes the HTTP call");
        assert_eq!(
            state_field_counts(src, "src/profile.component.ts", "src::profile"),
            (1, 1, 3),
            "toSignal is a signal factory; calls: toSignal, http.get and its ENDPOINT"
        );
    }

    #[test]
    fn observable_data_field() {
        let src = "\
import { BehaviorSubject } from 'rxjs';

export class FriendService {
  private subject = new BehaviorSubject<number>(0);
  readonly count$ = this.subject.asObservable();
}
";
        let parse = parse_file(src, "src/friend.service.ts", "src::friend", repo()).unwrap();
        let count = id(node_kind::STATE_VAR, "src::friend::FriendService::count$");
        assert!(has_node(&parse, count), "`count$` is a STATE_VAR");
        let site = parse
            .calls
            .iter()
            .find(|c| c.from == count)
            .unwrap_or_else(|| panic!("a CallSite from count$: {:?}", parse.calls));
        assert_eq!(
            site.qualifier,
            CallQualifier::ComplexReceiver {
                receiver: "this.subject".to_string(),
                name: "asObservable".to_string(),
            }
        );
        assert_eq!(site.line, 4, "the call's row");
        assert_eq!(state_field_counts(src, "src/friend.service.ts", "src::friend"), (1, 0, 1));
    }

    #[test]
    fn static_and_private_call_fields() {
        let src = "class Store { static readonly FALLBACK = Object.freeze(['AU']); #ticks = interval(1000); }\n";
        let parse = parse_file(src, "src/store.ts", "src::store", repo()).unwrap();
        let class = id(node_kind::CLASS, "src::store::Store");
        for field in ["FALLBACK", "#ticks"] {
            let v = id(node_kind::STATE_VAR, &format!("src::store::Store::{field}"));
            assert!(has_node(&parse, v), "{field} is a STATE_VAR");
            assert!(has_edge(&parse, class, v, edge_category::DEFINES), "DEFINES {field}");
        }
        let ticks = id(node_kind::STATE_VAR, "src::store::Store::#ticks");
        assert_eq!(call_site_names(&parse, ticks), vec!["interval".to_string()]);
        assert_eq!(state_field_counts(src, "src/store.ts", "src::store"), (2, 0, 2));
    }

    #[test]
    fn wrapped_and_shadowed_call_fields() {
        let src = "\
class W {
  a = signal(1) as WritableSignal<number>;
  b = (load())!;
  c = compute() satisfies Thing;
  d = new Map();
  e = (0 as number);
  f = signal(2);
  f(): number { return 0; }
  g = load();
  g = again();
  h = () => load();
}
";
        let parse = parse_file(src, "src/w.ts", "src::w", repo()).unwrap();
        let v = |f: &str| id(node_kind::STATE_VAR, &format!("src::w::W::{f}"));
        for field in ["a", "b", "c", "g"] {
            assert!(has_node(&parse, v(field)), "{field} is a STATE_VAR");
        }
        for field in ["d", "e", "f", "h"] {
            assert!(!has_node(&parse, v(field)), "{field} is no STATE_VAR");
        }
        assert_eq!(parse.nodes.iter().filter(|n| n.id == v("g")).count(), 1, "one node for `g`");
        assert_eq!(call_site_names(&parse, v("g")), vec!["load".to_string()], "the first `g` only");
        let f = id(node_kind::METHOD, "src::w::W::f");
        assert!(has_node(&parse, f), "the method `f` keeps its node");
        assert!(
            !parse.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Bare(n) if n == "signal" && c.line == 6)),
            "a shadowed field's initializer is not walked: {:?}",
            parse.calls
        );
        assert!(has_node(&parse, id(node_kind::METHOD, "src::w::W::h")), "CG.1 keeps the arrow");
        assert_eq!(state_field_counts(src, "src/w.ts", "src::w"), (4, 1, 4));
    }

    // ---- CH.1: abstract classes ---------------------------------------------

    /// The `[ts-abstract]` counters of one parse, as `(classes, methods)`.
    fn abstract_counts(src: &str, path: &str, module: &str) -> (usize, usize) {
        let (_, _, stats, ..) = parse_file_stats(src, path, module, repo()).unwrap();
        (stats.classes, stats.methods)
    }

    /// The `(bare name, category)` of every heritage / DI ref emitted from `from`.
    fn bare_refs(parse: &FileParse, from: NodeId) -> Vec<(String, EdgeCategoryId)> {
        parse
            .refs
            .iter()
            .filter(|r| r.from == from)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some((n.clone(), r.category)),
                _ => None,
            })
            .collect()
    }

    const BASE_REPO: &str = "\
export abstract class BaseRepo<T> {
  protected load(id: string): T {
    return this.fetchOne(id);
  }
  abstract fetchOne(id: string): T;
  protected abstract label(): string;
  describe(): string {
    return this.label();
  }
}
";

    #[test]
    fn abstract_class_is_a_class_with_methods() {
        use glia_code_domain::evidence::{Basis, Evidence};
        let parse = parse_file(BASE_REPO, "src/base-repo.ts", "src::base-repo", repo()).unwrap();
        let class = id(node_kind::CLASS, "src::base-repo::BaseRepo");
        assert!(has_node(&parse, class), "`export abstract class` is a CLASS");
        let m = |name: &str| id(node_kind::METHOD, &format!("src::base-repo::BaseRepo::{name}"));
        for name in ["load", "fetchOne", "label", "describe"] {
            assert!(has_node(&parse, m(name)), "{name} is a METHOD");
            assert!(has_edge(&parse, class, m(name), edge_category::DEFINES), "DEFINES {name}");
        }
        for (from, to, row) in [("load", "fetchOne", 2), ("describe", "label", 7)] {
            let edge = parse
                .edges
                .iter()
                .find(|e| e.from == m(from) && e.to == m(to) && e.category == edge_category::CALLS)
                .unwrap_or_else(|| panic!("{from} -> {to} resolves in-file, edges {:?}", parse.edges));
            let ev = Evidence::of(edge).expect("intra-file CALLS carries evidence");
            assert_eq!((ev.line, ev.basis), (Some(row), Basis::Site), "{from} -> {to}");
            assert_eq!(ev.rule.as_deref(), Some("intra_file"));
        }
        let (code, pos, _) = method_cells(&parse, "src::base-repo::BaseRepo::fetchOne");
        assert_eq!(code, "abstract fetchOne(id: string): T", "the signature is the CODE");
        assert!(pos.contains("\"start_line\":4,"), "fetchOne sits on row 4: {pos}");
        let (code, _, _) = method_cells(&parse, "src::base-repo::BaseRepo::label");
        assert_eq!(code, "protected abstract label(): string");
        for name in ["fetchOne", "label"] {
            assert!(call_site_names(&parse, m(name)).is_empty(), "{name} has no body to call from");
            assert!(
                !parse.edges.iter().any(|e| e.from == m(name) && e.category == edge_category::CALLS),
                "{name} makes no call"
            );
        }
        assert!(
            call_site_names(&parse, m("load")).is_empty() && call_site_names(&parse, m("describe")).is_empty(),
            "both self-calls bind in-file: {:?}",
            parse.calls
        );
        assert!(!has_node(&parse, id(node_kind::FUNCTION, "src::base-repo::fetchOne")));
        assert_eq!(abstract_counts(BASE_REPO, "src/base-repo.ts", "src::base-repo"), (1, 2));
    }

    #[test]
    fn unexported_and_default_abstract() {
        let src = "abstract class A { abstract run(): void; }\nexport default abstract class B { go() {} }\n";
        let parse = parse_file(src, "src/ab.ts", "src::ab", repo()).unwrap();
        for qname in ["src::ab::A", "src::ab::B"] {
            assert!(has_node(&parse, id(node_kind::CLASS, qname)), "CLASS {qname}");
        }
        assert!(has_node(&parse, id(node_kind::METHOD, "src::ab::A::run")));
        assert!(has_node(&parse, id(node_kind::METHOD, "src::ab::B::go")));
        assert_eq!(abstract_counts(src, "src/ab.ts", "src::ab"), (2, 1));

        // A repeated signature keeps one node; a computed name mints none; a
        // function field named like an abstract member is shadowed by it.
        let odd = "abstract class C {\n  abstract m(): void;\n  abstract m(): void;\n  \
                   abstract ['k'](): void;\n  m2 = () => 1;\n  abstract m2(): number;\n}\n";
        let parse = parse_file(odd, "src/c.ts", "src::c", repo()).unwrap();
        for name in ["m", "m2"] {
            let mid = id(node_kind::METHOD, &format!("src::c::C::{name}"));
            assert_eq!(parse.nodes.iter().filter(|n| n.id == mid).count(), 1, "{name} once");
        }
        let c = id(node_kind::CLASS, "src::c::C");
        let defined = parse
            .edges
            .iter()
            .filter(|e| e.from == c && e.category == edge_category::DEFINES)
            .count();
        assert_eq!(defined, 2, "m and m2 only: `['k']` mints nothing");
        assert_eq!(abstract_counts(odd, "src/c.ts", "src::c"), (1, 2));
        assert_eq!(fn_field_counts(odd, "src/c.ts", "src::c"), (0, 1));

        // A plain class counts nothing.
        assert_eq!(abstract_counts("class P { go() {} }\n", "src/p.ts", "src::p"), (0, 0));
    }

    #[test]
    fn abstract_heritage_refs() {
        let src = "export abstract class Repo extends Base implements IRepo {}\n";
        let parse = parse_file(src, "src/repo.ts", "src::repo", repo()).unwrap();
        let class = id(node_kind::CLASS, "src::repo::Repo");
        assert!(has_node(&parse, class));
        let refs = bare_refs(&parse, class);
        assert!(refs.contains(&("Base".to_string(), edge_category::INHERITS_FROM)), "{refs:?}");
        assert!(refs.contains(&("IRepo".to_string(), edge_category::IMPLEMENTS)), "{refs:?}");
    }

    #[test]
    fn decorated_abstract_service_injects() {
        let src = "import { Injectable } from '@angular/core';\n@Injectable()\n\
                   export abstract class S {\n  constructor(private p: ProductService) {}\n}\n";
        let parse = parse_file(src, "src/s.service.ts", "src::s", repo()).unwrap();
        let class = id(node_kind::CLASS, "src::s::S");
        assert!(has_node(&parse, class));
        assert_eq!(
            bare_refs(&parse, class),
            vec![("ProductService".to_string(), edge_category::INJECTS)]
        );
        // The decorator sits on the parent export_statement; the shape is TsCtor.
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(src, None).unwrap();
        let export = tree.root_node().named_child(1).unwrap();
        let decl = export.child_by_field_name("declaration").unwrap();
        assert_eq!(decl.kind(), "abstract_class_declaration");
        assert_eq!(class_di_shape(decl, src.as_bytes()), Some(DiShape::TsCtor));
    }

    #[test]
    fn nested_abstract_class_calls_stay_its_own() {
        // A class declared inside a function body is skipped by the body's call
        // walk whichever keyword it has (it is not a top-level declaration, so
        // it mints no node either): its field initialiser's call never becomes
        // the function's.
        let src = "export function f() {\n  abstract class Inner { x = g(); }\n  h();\n}\n";
        let parse = parse_file(src, "src/f.ts", "src::f", repo()).unwrap();
        let f = id(node_kind::FUNCTION, "src::f::f");
        assert_eq!(call_site_names(&parse, f), vec!["h".to_string()]);
    }

    // ---- CH.1b: abstract-member facts and `super.m()` -------------------------

    /// Every abstract member carries `NavFact::AbstractMethod` (the graph
    /// crate pairs its overrides with it); a concrete member carries none,
    /// and a repeated signature records the fact once.
    #[test]
    fn abstract_member_carries_the_fact() {
        let parse = parse_file(BASE_REPO, "src/base-repo.ts", "src::base-repo", repo()).unwrap();
        let m = |name: &str| id(node_kind::METHOD, &format!("src::base-repo::BaseRepo::{name}"));
        for name in ["fetchOne", "label"] {
            assert_eq!(
                parse.nav.nav_facts.get(&m(name)),
                Some(&vec![NavFact::AbstractMethod]),
                "{name} is abstract"
            );
        }
        for name in ["load", "describe"] {
            assert!(!parse.nav.nav_facts.contains_key(&m(name)), "{name} is concrete");
        }
        let odd = "abstract class C {\n  abstract m(): void;\n  abstract m(): void;\n  n() {}\n}\n";
        let parse = parse_file(odd, "src/c.ts", "src::c", repo()).unwrap();
        assert_eq!(
            parse.nav.nav_facts.get(&id(node_kind::METHOD, "src::c::C::m")),
            Some(&vec![NavFact::AbstractMethod])
        );
        assert!(!parse.nav.nav_facts.contains_key(&id(node_kind::METHOD, "src::c::C::n")));
    }

    /// `super.m()` is a SuperMethod CallSite at the call's row, never a
    /// ComplexReceiver on `super`; `this.n()` still binds in-file; a
    /// `super(...)` constructor call is no call site at all.
    #[test]
    fn super_call_is_super_method() {
        let src = "class B { m() {} }\nclass C extends B {\n  m() { super.m(); this.n(); }\n  \
                   n() {}\n  constructor() { super(); }\n}\n";
        let parse = parse_file(src, "src/c.ts", "src::c", repo()).unwrap();
        let c_m = id(node_kind::METHOD, "src::c::C::m");
        let c_n = id(node_kind::METHOD, "src::c::C::n");
        let from_c_m: Vec<(&CallQualifier, u32)> =
            parse.calls.iter().filter(|c| c.from == c_m).map(|c| (&c.qualifier, c.line)).collect();
        assert_eq!(from_c_m, vec![(&CallQualifier::SuperMethod("m".to_string()), 2)]);
        assert!(
            !parse.calls.iter().any(|c| matches!(
                &c.qualifier,
                CallQualifier::ComplexReceiver { receiver, .. } if receiver == "super"
            )),
            "{:?}",
            parse.calls
        );
        assert!(has_edge(&parse, c_m, c_n, edge_category::CALLS), "this.n() binds in-file");
        let ctor = id(node_kind::METHOD, "src::c::C::constructor");
        assert!(!parse.calls.iter().any(|c| c.from == ctor), "{:?}", parse.calls);
    }
}
