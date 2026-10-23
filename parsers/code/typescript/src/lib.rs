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

use std::collections::{BTreeSet, HashMap, HashSet};

use glia_code_domain::NavFact;
use glia_code_domain::data_entity;
use glia_code_domain::di_stats::{self, DiShape};
use glia_code_domain::endpoint;
use glia_code_domain::evidence;
use glia_code_domain::wire::{self, ModelField, ModelSchema, Shape, ShapeField, WireFormat};
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

/// [`parse_file_stats`]'s answer: the parse, then each marker's counters in
/// the order the doc there lists them.
type FileStats = (
    FileParse,
    FnFieldStats,
    AbstractStats,
    StateFieldStats,
    LocalStats,
    ReceiverGateStats,
    EndpointArgStats,
);

/// [`parse_file`], plus the file's CG.1 function-field counters (the
/// `[ts-fields]` marker's numbers), its CH.1 abstract-class counters (the
/// `[ts-abstract]` marker's numbers), its CH.2 call-initialised field
/// counters (the `[ts-state]` marker's numbers), its DF.22a local-receiver
/// counters (the `[ts-locals]` marker's numbers), its CH.3c receiver-gate
/// counters (the `[ts-endpoint-gate]` marker's numbers) and its CH.3a
/// URL-argument counters (the `[ts-endpoint-args]` marker's numbers), which
/// the tests read back.
fn parse_file_stats(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileStats, ParseError> {
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
    // DT.1q: the names the file's own `export` declarations export, one fact
    // on the file MODULE (the graph's re-export resolution reads it).
    if !acc.exported.is_empty() {
        let names = acc.exported.iter().cloned().collect();
        acc.nav.record_fact(module_id, NavFact::Exports { names });
    }

    // DT.1k: every import is read now, so the typed GraphQL client calls take
    // their import gates and land on their callables.
    attach_graphql_results(&mut acc);

    // DF.22a: every import is read now, so the `inject(T)`-shaped locals take
    // A7.1's gate, then the file's local-typed receivers are counted.
    record_inject_locals(&mut acc);
    // DT.5k: every local of each scope is known now, so a `const x = f(..)`
    // whose `f` no local shadows takes the call-result type `f()`.
    record_call_locals(&mut acc);
    let ls = local_stats(&acc);
    if ls.typed > 0 {
        eprintln!(
            "[ts-locals] typed receivers={} (param={} annotated={} new={} inject={} alias={} \
             call={}) unknown={} file={file_rel_path}",
            ls.typed, ls.param, ls.annotated, ls.new, ls.inject, ls.alias, ls.call, ls.unknown
        );
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
    // DT.1c: the interfaces and field-only classes given a SCHEMA_FIELDS
    // model schema, per file.
    let ms = acc.models;
    if ms.interfaces + ms.classes > 0 {
        eprintln!(
            "[model-schemas] lang=typescript interfaces={} classes={} fields={} open={} \
             file={file_rel_path}",
            ms.interfaces, ms.classes, ms.fields, ms.open
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
    // CH.3c: `this.<field>.<verb>(<unreadable>)` on a known non-HTTP field
    // type mints nothing; before the `[ts-endpoint-args]` count, which counts
    // only what becomes an ENDPOINT.
    let gate = gate_non_http_receivers(&mut acc);
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
    // DF.20: call-site templates whose base was a config-expression class
    // member (`${this.api}` -> `${environment.apiUrl}`), per file.
    if ea.aliases > 0 {
        eprintln!(
            "[ts-endpoints] member aliases={} file={file_rel_path}",
            ea.aliases
        );
    }
    // DT.4: call-site URLs whose trailing query-string substitution
    // (`${q}`, `const q = s ? `?status=${s}` : ''`) was dropped from the
    // path, per file.
    if ea.query_suffixes > 0 {
        eprintln!(
            "[ts-endpoints] query suffixes={} file={file_rel_path}",
            ea.query_suffixes
        );
    }

    resolve_intra_file(acc).map(|parse| (parse, ff, ab, sf, ls, gate, ea))
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
        stats.aliases += usize::from(cand.alias.is_some());
        stats.query_suffixes += usize::from(cand.query.is_some());
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
    /// DT.1c: the `[model-schemas] lang=typescript` counters.
    models: ModelStats,
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
    /// CH.3c: `(class, field) -> constructor leaf name` of each unannotated
    /// field initialised by `new X(…)` (`timers = new Map<string, number>()`
    /// -> `Map`, `bus = new rx.Subject()` -> `Subject`). Parser-local: read
    /// only by the receiver gate, so A6.2a's receiver binding through
    /// `CodeNav::field_types` is unchanged.
    field_new_types: HashMap<(NodeId, String), String>,
    /// CH.4: `this.<name>` used as a value inside a class member (a method
    /// passed by value: `addEventListener('x', this.onX)`, `subscribe(this.h)`,
    /// `this.render.bind(this)`), as `(from, enclosing class, name, row)`, one
    /// per `(from, name)` in walk order. Resolved into USES edges to the same
    /// class's METHOD in `resolve_intra_file`, once every member is known.
    this_refs: Vec<(NodeId, NodeId, String, u32)>,
    /// CH.4: `(from, name)` -> its index in `this_refs`. The call walk pops
    /// siblings right to left, so a repeat keeps the smaller (first source)
    /// row as the evidence line (LC.3b).
    this_ref_seen: HashMap<(NodeId, String), usize>,
    /// CH.4: the METHODs declared as a `get` / `set` accessor. Reading
    /// `this.isDesktop` runs a getter: a state read, not a method passed by
    /// value, so a by-value candidate naming one mints no USES.
    accessors: HashSet<NodeId>,
    /// DF.22a: `const x = <callee>(T)` locals shaped like `inject(T)`, in
    /// source order. Recorded on `nav.local_types` after the visit loop, once
    /// every import of the file is read: typed when the callee is a named
    /// `inject` import binding (A7.1's gate), else of unknown type.
    local_inject: Vec<LocalInject>,
    /// DF.22a: `(scope, local)` -> what gave the local its first typed record.
    /// Read only by the `[ts-locals]` marker ([`local_stats`]).
    local_src: HashMap<(NodeId, String), LocalSrc>,
    /// DT.1q: every name the file's own `export` declarations export
    /// ([`record_exported_names`]), recorded as one [`NavFact::Exports`] on the
    /// file MODULE after the visit loop.
    exported: BTreeSet<String>,
    /// DT.5g: bare identifiers read as a value ([`record_value_ref`]), one
    /// per `(from, name)` in walk order with every site. Bound in
    /// `resolve_intra_file` once every top-level declaration, import and
    /// local of the file is known.
    value_refs: Vec<ValueRef>,
    /// DT.5g: `(from, name)` -> its index in `value_refs`.
    value_ref_seen: HashMap<(NodeId, String), usize>,
    /// DT.5g: names a nested `function` / `class` declaration binds inside a
    /// walked body, per scope (the call walk skips the declaration itself):
    /// they shadow a top-level symbol of that name, as a local does.
    nested_decls: HashSet<(NodeId, String)>,
    /// DT.5g: this file's top-level CLASSes by name (an `abstract class`
    /// included), the in-file targets of a value reference.
    classes: HashMap<String, NodeId>,
    /// DT.5g: the module-level STATE_VARs G19 minted (`export const X = ..`),
    /// by name: a value reference's in-file target, and the `from` of the
    /// const's own initialiser walk.
    const_vars: HashMap<String, NodeId>,
    /// DT.5g: `this.<name>` read as the object of a further member read
    /// (`this.form.value`) or written (`this.x = v`), as `(from, class, name,
    /// row, write)`, one per `(from, name, write)` at its first source row.
    /// Only an accessor target becomes an edge: a CALLS of the getter
    /// (`accessor_read`) or setter (`accessor_write`). CH.4's `this_refs`
    /// holds the plain value reads.
    accessor_refs: Vec<(NodeId, NodeId, String, u32, bool)>,
    /// DT.5g: `(from, name, write)` -> its index in `accessor_refs`.
    accessor_ref_seen: HashMap<(NodeId, String, bool), usize>,
    /// DT.5g: the `[ts-value-refs]` walk counters.
    value_stats: ValueRefStats,
    /// DT.1k: typed GraphQL client calls ([`try_detect_graphql_result`]), in
    /// walk order. Gated and attached to their callables by
    /// [`attach_graphql_results`] once every import of the file is read.
    graphql_results: Vec<GraphqlResultCand>,
    /// DT.5k: `(class, field)` -> `(factory binding, class)` of each Angular
    /// signal query field (`form = viewChild(SmrFilingForm)`,
    /// `viewChild<KsToggle>('ref')`, `contentChild.required(X)`): what a read
    /// `this.form()` returns. Gated on the factory's import in
    /// `resolve_intra_file`.
    signal_queries: HashMap<(NodeId, String), (String, String)>,
    /// DT.5k: `inject(T)` calls written as a receiver or a member's object
    /// (`inject(CountriesService).list()`), as `(scope, receiver text,
    /// callee, T)`, awaiting the `inject` import gate.
    inject_receivers: Vec<(NodeId, String, String, String)>,
    /// DT.5k: `const x = f(..)` locals, as `(scope, x, f)`, recorded as the
    /// call-result type `f()` once every local of the scope is known.
    call_locals: Vec<(NodeId, String, String)>,
    /// DT.5k: member accesses `<recv>.<name>` that are no callee, one per
    /// `(from, recv, name, write)` at its first source row.
    member_reads: Vec<MemberReadCand>,
    /// DT.5k: `(from, recv, name, write)` -> its index in `member_reads`.
    member_read_seen: HashMap<(NodeId, String, String, bool), usize>,
    /// DT.5k: the `[ts-receivers]` counters.
    receiver_stats: ReceiverStats,
}

/// DT.5k: one member access `<receiver>.<name>` awaiting the receiver check
/// in `resolve_intra_file` ([`Acc::member_reads`]).
struct MemberReadCand {
    from: NodeId,
    class: Option<NodeId>,
    receiver: String,
    shape: RecvShape,
    name: String,
    row: u32,
    write: bool,
}

/// DT.5k: the receiver of a [`MemberReadCand`], by what can type it.
#[derive(Clone, PartialEq, Eq)]
enum RecvShape {
    /// `this.<field>`: the enclosing class's declared field type.
    ThisField(String),
    /// A bare identifier: a typed local or parameter of the scope.
    Local,
    /// Any other expression: a [`NavFact::ReceiverType`] of the scope.
    Expr,
}

/// DT.5k: what one file gave the graph (the `[ts-receivers]` marker).
#[derive(Default, Clone, Copy)]
struct ReceiverStats {
    /// Signal query fields with a class (`query_fields=`).
    query_fields: usize,
    /// `this.<query>()` receivers typed ([`NavFact::ReceiverType`]).
    query_receivers: usize,
    /// `inject(T)` receivers typed.
    inject_receivers: usize,
    /// INJECTS refs / edges from an `inject(T)` that is no field's whole
    /// initialiser.
    inject_refs: usize,
    /// In-file `T.X` static member reads that USE the class `T`.
    static_reads: usize,
    /// `const x = f(..)` locals typed as the call's result.
    call_locals: usize,
    /// [`NavFact::MemberRead`] / [`NavFact::MemberWrite`] facts recorded.
    member_reads: usize,
    /// Accessor METHODs marked ([`NavFact::Getter`] / [`NavFact::Setter`]).
    accessors: usize,
}

impl ReceiverStats {
    fn any(self) -> bool {
        self.query_fields
            + self.query_receivers
            + self.inject_receivers
            + self.inject_refs
            + self.static_reads
            + self.call_locals
            + self.member_reads
            > 0
    }
}

/// DT.5g: a bare identifier read as a value by `from` (an argument, an array
/// element, an object value, a shorthand property, `export default x`).
struct ValueRef {
    from: NodeId,
    name: String,
    /// Each site as `(0-based row, callee)`: the callee identifier when the
    /// site is a call's FIRST argument (`inject(AuthService)` -> `inject`; a
    /// later argument has none, DT.5k). An `inject` import binding's first
    /// argument is DI, never a value reference: a whole-initialiser field is
    /// A7.1's class INJECTS, any other site an INJECTS from its scope
    /// (DT.5k, `resolve_intra_file`); so the USES edge's row (LC.3b) is the
    /// first site that is not one.
    sites: Vec<(u32, Option<String>)>,
}

/// DT.5g: what one file's walks fed the value references (the
/// `[ts-value-refs]` marker's `const_inits=` / `decorators=`).
#[derive(Default, Clone, Copy)]
struct ValueRefStats {
    /// Module-level `const X = <value>` initialisers walked (a function-valued
    /// const is a FUNCTION, walked by `emit_function_value`, and not counted).
    const_inits: usize,
    /// Class decorators whose call arguments were walked as the class's code.
    decorators: usize,
}

/// DF.22a: what typed a local of a TS-family callable, for the `[ts-locals]`
/// marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalSrc {
    /// A formal parameter's annotation (`api: ApiService`).
    Param,
    /// A declarator's annotation (`const t: Repo = …`) or an `x as T` value.
    Annotated,
    /// A `new T(…)` value.
    New,
    /// An `inject(T)` value whose callee passed the `inject` import gate.
    Inject,
    /// A `this.<field>` value: the local aliases the field (`self.<field>`).
    Alias,
    /// DT.5k: a bare call's result (`const store = makeStore()`): the type
    /// `makeStore()`, which the graph reads off the callee's declared return
    /// type.
    Call,
}

/// DF.22a: one `const x = <callee>(T)` local awaiting the `inject` import
/// gate ([`Acc::local_inject`]).
struct LocalInject {
    /// The callable the local belongs to (the call walk's `from`).
    scope: NodeId,
    name: String,
    /// The local name the call goes through (`inject`, or `i` after
    /// `import { inject as i }`).
    callee: String,
    /// The type the call returns when it is `inject`: its `<T>` argument,
    /// else its class argument's simple name.
    ty: String,
}

/// DF.22a: one file's `x.m()` call sites whose receiver `x` is a recorded
/// local of the caller (the `[ts-locals]` marker).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
struct LocalStats {
    /// Sites whose local has a type: the receiver pass looks `m` up on it
    /// (an in-repo type binds; an external one binds nothing).
    typed: usize,
    /// Of `typed`, by what typed the local ([`LocalSrc`]).
    param: usize,
    annotated: usize,
    new: usize,
    inject: usize,
    alias: usize,
    call: usize,
    /// Sites whose local is of unknown type (`""`, a conflict included): it
    /// shadows a same-named field, so nothing binds.
    unknown: usize,
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
    /// CH.3c: the field `x` of a shape-1 call `this.x.method(…)`; None for
    /// shapes 2 and 3. [`gate_non_http_receivers`] looks it up in `class`'s
    /// field types.
    receiver_field: Option<String>,
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
    /// DF.20: the class members (`this.api`) whose config-expression value
    /// [`classify_template`] substituted into `template`, comma-joined in
    /// template order. Serialised as `"alias"` on ENDPOINT_HIT, after
    /// `wrapper_of`.
    alias: Option<String>,
    /// DT.4: the trailing query-string substitution [`classify_template`]
    /// dropped from `path` and `template` ([`query_suffix`]: the `const` it
    /// names, or `inline`). Serialised as `"query"` on ENDPOINT_HIT, last.
    query: Option<String>,
    /// DT.1d: the wire shapes the call site states. `response` and the
    /// request `resolve_intra_file` picks ([`request_of`]) are spliced onto
    /// ENDPOINT_HIT after every other key (`wire::append_hit_fields`).
    wire: WireRead,
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
    /// DF.20: see `EndpointCandidate::alias`.
    alias: Option<String>,
    /// DT.4: see `EndpointCandidate::query`.
    query: Option<String>,
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
            alias: None,
            query: None,
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
    /// DF.20: call sites whose template read a config-expression class
    /// member (the `[ts-endpoints] member aliases=` count); orthogonal to the
    /// arms above.
    aliases: usize,
    /// DT.4: call sites whose trailing query-string substitution was dropped
    /// from the path (the `[ts-endpoints] query suffixes=` count); orthogonal
    /// to the arms above.
    query_suffixes: usize,
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
                // DT.1q: the names this declaration exports, node or not.
                record_exported_names(n, decl, src, acc);
                // G19: surface module-level exported data constants
                // (`export const NAME = ...`) as STATE_VAR nodes. Arrow/function
                // consts are still hoisted to Function nodes by visit_lexical.
                if decl.kind() == "lexical_declaration" {
                    visit_exported_const(decl, src, file_rel, module_qname, module_id, repo, acc);
                }
                visit_top(decl, src, file_rel, module_qname, module_id, repo, acc);
            } else if let Some(value) = n.child_by_field_name("value") {
                // DT.1q: `export default <expr>;`.
                acc.exported.insert(DEFAULT_EXPORT.to_string());
                // DT.5g: `export default bootstrap;` reads `bootstrap` as a
                // value, the MODULE's.
                if value.kind() == "identifier" {
                    push_value_ref(acc, module_id, text(value, src), line_at(value), None);
                }
            } else {
                collect_reexport(n, src, module_id, acc);
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
// Exports and re-exports (DT.1q)
// ============================================================================

/// DT.1q: the export name of `export default ..` (ECMA-262's `"default"`):
/// what a default import (`import X from`) names.
const DEFAULT_EXPORT: &str = "default";

/// DT.1q: insert into `acc.exported` every name `export <decl>` exports.
/// `export default <decl>` exports `default` only (`export default class K`
/// binds `K` in the file but exports no `K`). Otherwise the declaration's
/// own name (a class, abstract class, function, generator, overload
/// signature, interface, type alias, enum), an identifier-named namespace
/// (`export namespace A.B` exports `A`), `export import A = B.C`, each
/// `const` / `let` / `var` binding (every identifier of a destructuring
/// pattern), and an `ambient_declaration`'s inner declaration (`export
/// declare ..`). An ambient `declare module 'x'` exports nothing. Whether a
/// name mints a node is the graph's question, not this one's.
fn record_exported_names(stmt: TsNode, decl: TsNode, src: &[u8], acc: &mut Acc) {
    let mut c = stmt.walk();
    if stmt.children(&mut c).any(|ch| ch.kind() == "default") {
        acc.exported.insert(DEFAULT_EXPORT.to_string());
        return;
    }
    declared_names(decl, src, &mut acc.exported);
}

/// DT.1q: the names declaration `decl` binds at module scope, for
/// [`record_exported_names`].
fn declared_names(decl: TsNode, src: &[u8], out: &mut BTreeSet<String>) {
    match decl.kind() {
        "class_declaration"
        | "abstract_class_declaration"
        | "function_declaration"
        | "generator_function_declaration"
        | "function_signature"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration" => {
            if let Some(name) = child_text(decl, "name", src) {
                out.insert(name.to_string());
            }
        }
        "internal_module" | "module" => {
            let Some(name) = decl.child_by_field_name("name") else { return };
            match name.kind() {
                "identifier" => {
                    out.insert(text(name, src).to_string());
                }
                // `namespace A.B {}` binds `A`.
                "nested_identifier" => {
                    let full = text(name, src);
                    if let Some(first) = full.split('.').next().map(str::trim)
                        && !first.is_empty()
                    {
                        out.insert(first.to_string());
                    }
                }
                // `declare module 'x' {}` binds no name.
                _ => {}
            }
        }
        "import_alias" => {
            let mut c = decl.walk();
            if let Some(id) = decl.named_children(&mut c).find(|ch| ch.kind() == "identifier") {
                out.insert(text(id, src).to_string());
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut c = decl.walk();
            for declarator in decl.named_children(&mut c) {
                if declarator.kind() != "variable_declarator" {
                    continue;
                }
                // Every identifier a destructuring pattern binds (DF.22a's
                // walk): `export const { a, b: c } = o` exports `a c`.
                if let Some(name) = declarator.child_by_field_name("name") {
                    out.extend(pattern_names(name, src).into_iter().map(str::to_string));
                }
            }
        }
        "ambient_declaration" => {
            let mut c = decl.walk();
            for inner in decl.named_children(&mut c) {
                declared_names(inner, src, out);
            }
        }
        _ => {}
    }
}

/// DT.1q: an `export` statement with no declaration and no default value,
/// as [`NavFact`]s on the file MODULE, `line` its 0-based row:
/// `export * as ns from 's'` -> [`NavFact::ReExportNamespace`], each
/// specifier of `export { a as b } from 's'` -> [`NavFact::ReExportName`],
/// of a from-less `export { a as b }` -> [`NavFact::ExportLocal`], and
/// `export * from 's'` -> [`NavFact::ReExportStar`]. `export = x` and
/// `export as namespace X` record nothing. A `type` / `typeof` keyword on
/// the statement or a specifier is ignored: a type-only re-export binds
/// like any other, since heritage and receiver typing read types.
fn collect_reexport(n: TsNode, src: &[u8], module_id: NodeId, acc: &mut Acc) {
    let line = line_at(n);
    let source = n.child_by_field_name("source").map(|s| strip_string_quotes(text(s, src)));
    let mut clause_or_ns = false;
    let mut c = n.walk();
    for child in n.named_children(&mut c) {
        match child.kind() {
            "namespace_export" => {
                clause_or_ns = true;
                let Some(source) = source.clone() else { continue };
                let mut nc = child.walk();
                let Some(name) = child
                    .named_children(&mut nc)
                    .find(|x| matches!(x.kind(), "identifier" | "string"))
                else {
                    continue;
                };
                let exported = strip_string_quotes(text(name, src));
                acc.nav.record_fact(module_id, NavFact::ReExportNamespace { source, exported, line });
            }
            "export_clause" => {
                clause_or_ns = true;
                let mut sc = child.walk();
                for spec in child.named_children(&mut sc) {
                    if spec.kind() != "export_specifier" {
                        continue;
                    }
                    let Some(name_n) = spec.child_by_field_name("name") else { continue };
                    let name = strip_string_quotes(text(name_n, src));
                    let exported = spec
                        .child_by_field_name("alias")
                        .map_or_else(|| name.clone(), |a| strip_string_quotes(text(a, src)));
                    let fact = match &source {
                        Some(source) => NavFact::ReExportName {
                            source: source.clone(),
                            name,
                            exported,
                            line,
                        },
                        None => NavFact::ExportLocal { local: name, exported, line },
                    };
                    acc.nav.record_fact(module_id, fact);
                }
            }
            _ => {}
        }
    }
    if !clause_or_ns && let Some(source) = source {
        acc.nav.record_fact(module_id, NavFact::ReExportStar { source, line });
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
    let mut cells = build_cells(&n, src, file_rel);
    // DT.1c: a field-only class is a model like an interface.
    if let Some(schema) = ts_class_schema(n, src) {
        acc.models.record(&schema, true);
        cells.push(schema.cell());
    }
    acc.nodes.push(Node {
        id: class_id,
        repo,
        confidence: Confidence::Strong,
        cells,
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
    acc.classes.entry(name.to_string()).or_insert(class_id);
    if n.kind() == "abstract_class_declaration" {
        acc.abstract_stats.classes += 1;
    }
    // DT.5g: a class decorator's arguments run as the class's own code:
    // `@Component({ imports: [TimeAgoPipe], providers: [..] })`, `@NgModule({
    // declarations: [..] })`, `@Module({ controllers: [..] })`. Their calls
    // and value references are the class's; the decorator callee itself is
    // no CallSite (a decorator is applied, not called by the source).
    for dec in class_decorators(n) {
        let mut dc = dec.walk();
        let Some(call) = dec.named_children(&mut dc).find(|c| c.kind() == "call_expression") else {
            continue;
        };
        let Some(args) = call.child_by_field_name("arguments") else {
            continue;
        };
        collect_calls_in(args, src, class_id, Some(class_id), acc);
        acc.value_stats.decorators += 1;
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
    // CH.3c: an unannotated `x = new Map<…>()` / `x = new rx.Subject()` field
    // holds a value of the constructed type: the receiver gate's input.
    if value.kind() == "new_expression" {
        if annotated.is_none()
            && let Some(name) = field_name
            && let Some(ctor) = value.child_by_field_name("constructor")
            && matches!(ctor.kind(), "identifier" | "member_expression")
            && let Some(type_name) = heritage_type_name(ctor, src)
        {
            acc.field_new_types
                .insert((class_id, name.to_string()), type_name.to_string());
        }
        return;
    }
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
/// walked for calls, as for a method; their types are the METHOD's locals
/// (DF.22a, [`record_param_types`]).
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
    // DF.22a: the function value's parameters are the field METHOD's locals.
    record_param_types(value, src, method_id, acc);

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

    // DT.5k: an Angular signal query holds the class a read of it returns
    // (`this.form()` for `form = viewChild(SmrFilingForm)`); the factory's
    // import is checked in `resolve_intra_file`.
    if let Some((binding, ty)) = signal_query_type(inner, src) {
        acc.signal_queries.insert(
            (class_id, name.to_string()),
            (binding.to_string(), ty.to_string()),
        );
    }

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

/// DT.5k: `(factory binding, class)` of a single-result signal query call,
/// `viewChild` / `contentChild` or their `.required` forms (the binding is
/// the identifier the factory is reached through; `resolve_intra_file`
/// checks it is the Angular import). The class is, first match wins: the
/// class a `{ read: X }` option names (the query then yields `X`, never its
/// locator or its first type argument); else the `<T>` argument
/// (`viewChild<KsToggle>('pushSwitch')`); else the locator when it names a
/// class (`viewChild(SmrFilingForm)`). A string locator with no `<T>`, a
/// non-class `read` and a non-class `<T>` yield nothing; the list queries
/// (`viewChildren`, `contentChildren`) hold an array, and the gate drops
/// them.
fn signal_query_type<'a>(call: TsNode, src: &'a [u8]) -> Option<(&'a str, &'a str)> {
    let callee = call.child_by_field_name("function")?;
    let binding = signal_factory_name(callee, src)?;
    let upper = |t: &&str| t.starts_with(|c: char| c.is_ascii_uppercase());
    let args = call.child_by_field_name("arguments")?;
    let mut ac = args.walk();
    let mut it = args.named_children(&mut ac).filter(|a| a.kind() != "comment");
    let locator = it.next()?;
    // A `read` option decides first: the query yields its token's type
    // (`viewChild<Cmp, ElementRef>('x', { read: ElementRef })` is an
    // ElementRef, the SECOND type argument), never the locator's.
    if let Some(opts) = it.next().filter(|o| o.kind() == "object") {
        let mut oc = opts.walk();
        let read = opts.named_children(&mut oc).find(|p| {
            p.kind() == "pair"
                && p.child_by_field_name("key").is_some_and(|k| text(k, src) == "read")
        });
        if let Some(pair) = read {
            return pair
                .child_by_field_name("value")
                .filter(|v| matches!(v.kind(), "identifier" | "member_expression"))
                .and_then(|v| heritage_type_name(v, src))
                .filter(upper)
                .map(|t| (binding, t));
        }
    }
    if let Some(ta) = call.child_by_field_name("type_arguments") {
        let mut tc = ta.walk();
        let first = ta.named_children(&mut tc).next()?;
        return class_type_name(first, src).map(|t| (binding, t));
    }
    if !matches!(locator.kind(), "identifier" | "member_expression") {
        return None;
    }
    heritage_type_name(locator, src).filter(upper).map(|t| (binding, t))
}

/// DT.5k: the local names bound to an imported single-result signal query
/// factory (`viewChild` / `contentChild`, or their aliases), A7.1's import
/// gate for `inject`.
fn query_import_bindings(imports: &[ImportStmt]) -> HashSet<&str> {
    imports
        .iter()
        .filter_map(|imp| match &imp.target {
            ImportTarget::Symbol { name, alias, .. }
                if name == "viewChild" || name == "contentChild" =>
            {
                Some(alias.as_deref().unwrap_or(name.as_str()))
            }
            _ => None,
        })
        .collect()
}

/// DT.5k: the field `f` of a signal read receiver `this.f()` (a `!` after
/// the call allowed: `this.f()!`), the receiver text a CallSite or member
/// read keeps for `this.form()?.reset()`. Any argument, chain or other
/// shape names none.
fn signal_read_field(receiver: &str) -> Option<&str> {
    let r = receiver.strip_suffix('!').unwrap_or(receiver);
    let f = r.strip_prefix("this.")?.strip_suffix("()")?;
    let ident = |s: &str| {
        let s = s.strip_prefix('#').unwrap_or(s);
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
    };
    ident(f).then_some(f)
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
    let mut cells = build_cells(&n, src, file_rel);
    // DT.1c: the interface's declared fields, the model a JSON body is
    // compared against. A merged declaration carries its own cell; the
    // models pass unions them.
    if let Some(schema) = ts_interface_schema(n, src) {
        acc.models.record(&schema, false);
        cells.push(schema.cell());
    }
    acc.nodes.push(Node {
        id: iface_id,
        repo,
        confidence: Confidence::Strong,
        cells,
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

// ============================================================================
// DT.1c: declared model fields (SCHEMA_FIELDS, format ts)
// ============================================================================

/// DT.1c: what the file's model schemas hold, for the `[model-schemas]
/// lang=typescript` marker.
#[derive(Default, Clone, Copy)]
struct ModelStats {
    /// INTERFACE declarations given a SCHEMA_FIELDS cell.
    interfaces: usize,
    /// Field-only CLASSes given one.
    classes: usize,
    /// The fields those cells list.
    fields: usize,
    /// The cells marked `open`.
    open: usize,
}

impl ModelStats {
    fn record(&mut self, schema: &ModelSchema, class: bool) {
        if class {
            self.classes += 1;
        } else {
            self.interfaces += 1;
        }
        self.fields += schema.fields.len();
        self.open += usize::from(schema.open);
    }
}

/// Class decorators beside the DI set ([`DI_DECORATORS`],
/// [`NEST_DI_DECORATORS`]) that make a class framework wiring, never a wire
/// model, even when its body is fields only.
const MODULE_DECORATORS: &[&str] = &["NgModule", "Module"];

/// DT.1c: an interface's declared fields as a [`ModelSchema`] (format `ts`),
/// the SCHEMA_FIELDS cell the models pass compares a JSON body against.
///
/// * A `property_signature` is a field: its key ([`ts_property_key`]: an
///   identifier as written, a string unquoted, a number as written), its
///   annotation text whitespace-collapsed (`any` when it has none) and
///   `required` unless it is marked `?` or its type is a top-level union
///   with `undefined` ([`ts_optional_type`]). `array` is read off the type
///   by [`ModelField::new`].
/// * A computed key (`[K]: T`) is skipped and leaves the model `open`, as
///   does an `index_signature`.
/// * Method, call and construct signatures are skipped: they are never on
///   the wire.
/// * `extends` holds the simple base names [`collect_interface_heritage`]
///   reads; a generic or utility base (`Pick<..>`, `Omit<..>`, `Partial<..>`,
///   any `X<..>`) is not listed and leaves the model `open`, since which of
///   its fields arrive depends on the arguments.
///
/// `None` for an interface with no field, no base and nothing open (a
/// method-only service contract, an empty marker): it describes no body.
/// A `type X = {..}` alias mints no node, so it carries no schema.
fn ts_interface_schema(iface: TsNode, src: &[u8]) -> Option<ModelSchema> {
    let mut schema = ModelSchema::new(WireFormat::Ts);
    let mut cursor = iface.walk();
    for clause in iface.named_children(&mut cursor) {
        if clause.kind() != "extends_type_clause" {
            continue;
        }
        let mut tc = clause.walk();
        for ty in clause.children_by_field_name("type", &mut tc) {
            if ty.kind() == "generic_type" {
                schema.open = true;
            } else if let Some(base) = heritage_type_name(ty, src) {
                push_base(&mut schema, base);
            }
        }
    }
    let body = iface.child_by_field_name("body")?;
    let mut bc = body.walk();
    for member in body.named_children(&mut bc) {
        match member.kind() {
            "property_signature" => {
                let Some(name) = member.child_by_field_name("name") else {
                    continue;
                };
                let Some(key) = ts_property_key(name, src) else {
                    schema.open |= name.kind() == "computed_property_name";
                    continue;
                };
                let ty = member
                    .child_by_field_name("type")
                    .and_then(|t| first_named(t));
                let ty_text = ty.map_or_else(|| "any".to_string(), |t| collapse_ws(text(t, src)));
                let optional = has_token(member, "?") || ty.is_some_and(|t| ts_optional_type(t, src));
                schema.fields.push(ModelField::new(&key, &ty_text, !optional));
            }
            "index_signature" => schema.open = true,
            _ => {}
        }
    }
    let describes = !schema.fields.is_empty() || !schema.extends.is_empty() || schema.open;
    describes.then_some(schema)
}

/// DT.1c: a field-only class's declared fields as a [`ModelSchema`] (format
/// `ts`), the same cell an interface carries.
///
/// A class is a model when its body holds only public instance fields and at
/// most one constructor: every `public_field_definition` without `private`
/// / `protected` / `static`, a `#name` or a function value, and nothing else
/// (any other method, accessor, abstract member or static block makes it
/// behaviour, so it gets no cell: a service with a private cache and a
/// `get()` is no model). Constructor parameter properties marked `public` or
/// `readonly` are fields too; a `private` / `protected` one disqualifies the
/// class like a private field. A class carrying a DI or module decorator
/// (`@Component`, `@Injectable`, `@Controller`, `@NgModule`, ...) is
/// framework wiring and gets none. An index signature or a computed key
/// leaves the model `open`.
///
/// A field's type is its annotation text, whitespace-collapsed; an
/// unannotated field's is read off its initialiser ([`ts_initializer_type`]).
/// `required` unless marked `?` (a `?` parameter property) or typed with a
/// top-level `| undefined`. `extends` holds the superclass's simple name; a
/// generic base (`extends Base<T>`) or a computed one (a mixin call) is not
/// listed and leaves the model `open`.
///
/// `None` unless the class lists at least one field.
fn ts_class_schema(class: TsNode, src: &[u8]) -> Option<ModelSchema> {
    let framework = class_decorators(class)
        .into_iter()
        .filter_map(|d| decorator_name(d, src))
        .any(|n| {
            DI_DECORATORS.contains(&n) || NEST_DI_DECORATORS.contains(&n) || MODULE_DECORATORS.contains(&n)
        });
    if framework {
        return None;
    }
    let body = class.child_by_field_name("body")?;
    let mut schema = ModelSchema::new(WireFormat::Ts);
    let mut cursor = class.walk();
    if let Some(heritage) = class.named_children(&mut cursor).find(|c| c.kind() == "class_heritage") {
        let mut hc = heritage.walk();
        for clause in heritage.named_children(&mut hc) {
            if clause.kind() != "extends_clause" {
                continue;
            }
            let mut tc = clause.walk();
            if clause.children_by_field_name("type_arguments", &mut tc).next().is_some() {
                schema.open = true;
                continue;
            }
            let mut vc = clause.walk();
            for value in clause.children_by_field_name("value", &mut vc) {
                match value.kind() {
                    "identifier" | "member_expression" => {
                        if let Some(base) = heritage_type_name(value, src) {
                            push_base(&mut schema, base);
                        }
                    }
                    _ => schema.open = true,
                }
            }
        }
    }
    let mut ctor_seen = false;
    let mut bc = body.walk();
    for member in body.named_children(&mut bc) {
        match member.kind() {
            "comment" | "decorator" => {}
            "public_field_definition" => {
                if !push_class_field(&mut schema, member, src) {
                    return None;
                }
            }
            "method_definition" if !ctor_seen && child_text(member, "name", src) == Some("constructor") => {
                ctor_seen = true;
                if !push_param_properties(&mut schema, member, src) {
                    return None;
                }
            }
            "index_signature" => schema.open = true,
            _ => return None,
        }
    }
    (!schema.fields.is_empty()).then_some(schema)
}

/// One class field into `schema`; `false` when the field makes the class
/// behaviour rather than a model ([`ts_class_schema`]'s rule).
fn push_class_field(schema: &mut ModelSchema, field: TsNode, src: &[u8]) -> bool {
    if !public_member(field, src) || has_token(field, "static") {
        return false;
    }
    let Some(name) = field.child_by_field_name("name") else {
        return true;
    };
    if name.kind() == "private_property_identifier" {
        return false;
    }
    let value = field.child_by_field_name("value");
    if value.is_some_and(|v| matches!(v.kind(), "arrow_function" | "function_expression" | "function")) {
        return false;
    }
    let Some(key) = ts_property_key(name, src) else {
        schema.open |= name.kind() == "computed_property_name";
        return true;
    };
    push_typed_field(schema, &key, field, value, src);
    true
}

/// The constructor's `public` / `readonly` parameter properties into
/// `schema`; `false` when one is `private` / `protected`.
fn push_param_properties(schema: &mut ModelSchema, ctor: TsNode, src: &[u8]) -> bool {
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return true;
    };
    let mut pc = params.walk();
    for param in params.named_children(&mut pc) {
        if !matches!(param.kind(), "required_parameter" | "optional_parameter") {
            continue;
        }
        if !public_member(param, src) {
            return false;
        }
        let property = accessibility(param, src).is_some() || has_token(param, "readonly");
        let Some(pattern) = param.child_by_field_name("pattern") else {
            continue;
        };
        if !property || pattern.kind() != "identifier" {
            continue;
        }
        let value = param.child_by_field_name("value");
        push_typed_field(schema, text(pattern, src), param, value, src);
    }
    true
}

/// A field `key` of `member` (a class field or a parameter property): its
/// annotation, else its initialiser's type; `required` unless `?` or a
/// top-level `| undefined`.
fn push_typed_field(schema: &mut ModelSchema, key: &str, member: TsNode, value: Option<TsNode>, src: &[u8]) {
    let ty = member.child_by_field_name("type").and_then(|t| first_named(t));
    let ty_text = match (ty, value) {
        (Some(t), _) => collapse_ws(text(t, src)),
        (None, Some(v)) => ts_initializer_type(v, src),
        (None, None) => "any".to_string(),
    };
    let optional = has_token(member, "?") || ty.is_some_and(|t| ts_optional_type(t, src));
    schema.fields.push(ModelField::new(key, &ty_text, !optional));
}

/// The member's accessibility modifier text (`public` / `private` /
/// `protected`), if it has one.
fn accessibility<'a>(member: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let mut c = member.walk();
    member
        .named_children(&mut c)
        .find(|ch| ch.kind() == "accessibility_modifier")
        .map(|m| text(m, src).trim())
}

/// The member is not `private` / `protected`.
fn public_member(member: TsNode, src: &[u8]) -> bool {
    !matches!(accessibility(member, src), Some("private" | "protected"))
}

/// The JSON key a `_property_name` node spells: an identifier as written, a
/// string unquoted, a number as written; `None` for a computed key or a
/// `#private` name.
fn ts_property_key(name: TsNode, src: &[u8]) -> Option<String> {
    match name.kind() {
        "property_identifier" | "identifier" | "number" => Some(text(name, src).to_string()),
        "string" => Some(strip_string_quotes(text(name, src))),
        _ => None,
    }
}

/// The type a field's initialiser gives it when it has no annotation: a
/// string / template literal `string`, a number `number`, `true` / `false`
/// `boolean`, `null` `null`, an array literal `any[]`, `new X(..)` the
/// constructed name as written; anything else `any`.
fn ts_initializer_type(value: TsNode, src: &[u8]) -> String {
    match value.kind() {
        "string" | "template_string" => "string".to_string(),
        "number" => "number".to_string(),
        "true" | "false" => "boolean".to_string(),
        "null" => "null".to_string(),
        "array" => "any[]".to_string(),
        "new_expression" => value
            .child_by_field_name("constructor")
            .filter(|c| matches!(c.kind(), "identifier" | "member_expression"))
            .map_or_else(|| "any".to_string(), |c| collapse_ws(text(c, src))),
        _ => "any".to_string(),
    }
}

/// The type is a top-level union one of whose members is `undefined`
/// (`string | undefined`): the field may be absent.
fn ts_optional_type(ty: TsNode, src: &[u8]) -> bool {
    if ty.kind() != "union_type" {
        return false;
    }
    let mut c = ty.walk();
    ty.named_children(&mut c)
        .any(|m| text(m, src).trim() == "undefined" || ts_optional_type(m, src))
}

/// `base` onto `schema.extends`, once.
fn push_base(schema: &mut ModelSchema, base: &str) {
    if !schema.extends.iter().any(|b| b == base) {
        schema.extends.push(base.to_string());
    }
}

/// The first named child of `n` (a `type_annotation`'s type).
fn first_named(n: TsNode) -> Option<TsNode> {
    let mut c = n.walk();
    n.named_children(&mut c).find(|ch| ch.kind() != "comment")
}

/// `s` with every whitespace run collapsed to one space, trimmed.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// CH.4: a `get` / `set` accessor (`get isDesktop(): boolean {…}`, `abstract
/// get label(): string;`): the keyword is an anonymous child of the member, so
/// a method NAMED `get` (`get() {}`, a named `property_identifier`) is none.
fn is_accessor(n: TsNode) -> bool {
    let mut c = n.walk();
    n.children(&mut c)
        .any(|ch| !ch.is_named() && matches!(ch.kind(), "get" | "set"))
}

/// DT.5k: record on an accessor METHOD which keyword declared it, a
/// [`NavFact::Getter`] for `get`, a [`NavFact::Setter`] for `set` (a getter
/// and its setter share one METHOD, which then carries both): the graph
/// binds a member read on another object only to a getter, a write only to a
/// setter.
fn mark_accessor(n: TsNode, method_id: NodeId, acc: &mut Acc) {
    let mut c = n.walk();
    for ch in n.children(&mut c).filter(|ch| !ch.is_named()) {
        let fact = match ch.kind() {
            "get" => NavFact::Getter,
            "set" => NavFact::Setter,
            _ => continue,
        };
        acc.nav.record_fact(method_id, fact);
        acc.receiver_stats.accessors += 1;
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
    if is_accessor(n) {
        acc.accessors.insert(method_id);
        mark_accessor(n, method_id, acc);
    }
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

    // DF.22a: a method's, a constructor's or an accessor's parameters type
    // the receivers of its body (a parameter property records its field's
    // type, so A6.2b's constructor binds keep their target).
    record_param_types(n, src, method_id, acc);
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
    if is_accessor(n) {
        acc.accessors.insert(method_id);
        mark_accessor(n, method_id, acc);
    }
    acc.nav.record(
        method_id,
        name,
        &method_qname,
        node_kind::METHOD,
        Some(class_id),
    );
    // CH.1b: the graph crate pairs a subclass method of this name with it
    // (`emit_override_implements`). A repeated signature returned above, so
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
    let is_const = {
        let mut c = n.walk();
        n.children(&mut c).any(|ch| ch.kind() == "const")
    };
    let mut cursor = n.walk();
    for declarator in n.named_children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let Some(value) = declarator.child_by_field_name("value") else {
            continue;
        };
        let name_n = declarator.child_by_field_name("name");
        if !matches!(value.kind(), "arrow_function" | "function_expression") {
            // DT.5g: a module-level `const X = <value>` runs its initialiser
            // at load: `export const appConfig = { providers: [provideI18n()]
            // }`, `const WIZARD = ROUTES.filter(r => isWizard(r.path))`. Its
            // calls, client HTTP calls and value references are X's STATE_VAR's
            // when G19 minted one (visit_exported_const ran first), else the
            // MODULE's, as a top-level expression statement's are.
            if is_const {
                let from = name_n
                    .filter(|nn| nn.kind() == "identifier")
                    .and_then(|nn| acc.const_vars.get(text(nn, src)).copied())
                    .unwrap_or(module_id);
                collect_calls_in(value, src, from, None, acc);
                acc.value_stats.const_inits += 1;
            }
            continue;
        }
        let Some(name_n) = name_n else {
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
        if let Some(v) = value
            && matches!(v.kind(), "arrow_function" | "function_expression")
        {
            continue;
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
        // DT.5g: the in-file target of a value reference, and the `from` of
        // this const's initialiser walk (visit_lexical, which runs next).
        acc.const_vars.entry(name.to_string()).or_insert(const_id);
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
    // DF.22a: parameters type the receivers of the body.
    record_param_types(n, src, func_id, acc);
    // DT.5k: a declared class return type types a caller's result local.
    record_return_class(n, src, func_id, acc);

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
    // DF.22a: the arrow's / function expression's parameters type the
    // receivers of its body.
    record_param_types(value, src, func_id, acc);
    // DT.5k: a declared class return type types a caller's result local.
    record_return_class(value, src, func_id, acc);

    if let Some(body) = value.child_by_field_name("body") {
        collect_calls_in(body, src, func_id, None, acc);
    }
}

/// DT.5k: record the class a FUNCTION's `return_type` annotation declares
/// ([`annotated_class_type`]: `ns.Foo` / `Foo<T>` / `Foo | null` -> `Foo`) on
/// `CodeNav::return_types`, so a caller's `const x = f()` local is of that
/// class. A primitive, array, union, function or type-predicate annotation,
/// and no annotation, record nothing.
fn record_return_class(func: TsNode, src: &[u8], func_id: NodeId, acc: &mut Acc) {
    if let Some(ty) = func
        .child_by_field_name("return_type")
        .and_then(|t| annotated_class_type(t, src))
    {
        acc.nav.record_return_type(func_id, ty);
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
            // DT.5g: a nested `function f() {}` / `class K {}` binds `f` / `K`
            // in the walked body, shadowing a top-level symbol of that name.
            if matches!(
                kind,
                "function_declaration" | "class_declaration" | "abstract_class_declaration"
            ) && let Some(name) = child_text(node, "name", src)
            {
                acc.nested_decls.insert((from, name.to_string()));
            }
            continue;
        }
        // DT.5g: a bare identifier passed by value.
        if matches!(kind, "identifier" | "shorthand_property_identifier") {
            record_value_ref(node, src, from, acc);
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
            try_detect_graphql_result(node, src, from, acc);
        }
        if kind == "member_expression" {
            record_member_ref(node, src, from, acc);
            record_this_ref(node, src, from, enclosing_class, acc);
            // DT.5k: an `inject(T)` receiver, and a member access a getter
            // or setter of the receiver's class may serve.
            record_inject_receiver(node, src, from, acc);
            record_member_read(node, src, from, enclosing_class, acc);
        }
        if matches!(
            kind,
            "member_expression" | "shorthand_property_identifier_pattern" | "pair_pattern"
        ) {
            record_prefix_read(node, src, from, acc);
        }
        // DF.22a: the bindings of the walked body are locals of `from`, the
        // node its calls are attributed to; a nested callback's parameters
        // and locals included, since its calls are `from`'s too.
        match kind {
            "variable_declarator" => record_declarator_type(node, src, from, acc),
            "arrow_function" | "function_expression" | "generator_function" => {
                record_param_types(node, src, from, acc);
            }
            "for_in_statement" => record_pattern_unknown(node, "left", src, from, acc),
            "catch_clause" => record_pattern_unknown(node, "parameter", src, from, acc),
            _ => {}
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

/// CH.4: record `this.<name>` — object `this`, property a `property_identifier`
/// or `#private` name — read as a VALUE inside a member of `enclosing_class`:
/// a method passed by value (`addEventListener('resize', this.onResize)`,
/// `subscribe(this.handleTick)`, `{ next: this.onNext }`) or bound
/// (`this.render.bind(this)`, `.call` / `.apply`). Skipped: the callee of its
/// call (`this.render()` is a CallSite, so CALLS), the target of an assignment
/// (`this.handler = null` is a write), and the object of a further member read
/// other than `bind` / `call` / `apply` (`this.items.push(x)` reads data). No
/// enclosing class (a module-level walk) records nothing. Whether the name is
/// the class's METHOD (not a data field or a CH.2 STATE_VAR) is decided in
/// `resolve_intra_file`, once every member of the class is known.
///
/// DT.5g: the two skipped reads are kept for the accessor CALLS
/// ([`record_accessor_ref`]): an assignment target is a write (a setter runs),
/// a further member read's object a read (a getter runs). A value read naming
/// an accessor (`${this.apiUrl}/trades`) becomes its `accessor_read` CALLS in
/// `resolve_intra_file`, through CH.4's own list.
fn record_this_ref(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    enclosing_class: Option<NodeId>,
    acc: &mut Acc,
) {
    let Some(class) = enclosing_class else {
        return;
    };
    let (Some(object), Some(property)) = (
        node.child_by_field_name("object"),
        node.child_by_field_name("property"),
    ) else {
        return;
    };
    if object.kind() != "this"
        || !matches!(
            property.kind(),
            "property_identifier" | "private_property_identifier"
        )
    {
        return;
    }
    let name = text(property, src).to_string();
    let row = line_at(node);
    if let Some(parent) = node.parent() {
        let is = |field: &str| parent.child_by_field_name(field) == Some(node);
        match parent.kind() {
            "call_expression" if is("function") => return,
            // DT.5g: a write runs a setter; a further member read
            // (`this.form.value`) runs a getter. Kept for the accessor CALLS
            // only: neither passes a method by value.
            "assignment_expression" | "augmented_assignment_expression" if is("left") => {
                record_accessor_ref(acc, from, class, name, row, true);
                return;
            }
            "member_expression" if is("object") => {
                let via = parent.child_by_field_name("property").map(|p| text(p, src));
                if !matches!(via, Some("bind" | "call" | "apply")) {
                    record_accessor_ref(acc, from, class, name, row, false);
                    return;
                }
            }
            _ => {}
        }
    }
    match acc.this_ref_seen.get(&(from, name.clone())) {
        Some(&i) => {
            if let Some(entry) = acc.this_refs.get_mut(i) {
                entry.3 = entry.3.min(row);
            }
        }
        None => {
            acc.this_ref_seen.insert((from, name.clone()), acc.this_refs.len());
            acc.this_refs.push((from, class, name, row));
        }
    }
}

/// DT.5k: record the object of a member expression that is an
/// `inject(T)`-shaped call ([`inject_shape`]; parentheses and `!` peeled
/// for the test, the receiver text kept as written): the callee
/// `inject(CountriesService).list()` and the read
/// `inject(AuthService).isAuthenticated` hold a `T`. The `inject` import
/// gate is applied in `resolve_intra_file`.
fn record_inject_receiver(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let Some(object) = node.child_by_field_name("object") else {
        return;
    };
    let mut inner = object;
    while matches!(inner.kind(), "parenthesized_expression" | "non_null_expression") {
        let mut c = inner.walk();
        let Some(next) = inner.named_children(&mut c).find(|n| n.kind() != "comment") else {
            return;
        };
        inner = next;
    }
    if inner.kind() != "call_expression" {
        return;
    }
    if let Some((callee, ty)) = inject_shape(inner, src) {
        acc.inject_receivers.push((
            from,
            text(object, src).to_string(),
            callee.to_string(),
            ty.to_string(),
        ));
    }
}

/// DT.5k: record a member access `<recv>.<name>` that is no call's callee
/// (a read, or an assignment's target: `write`), by `from`, once per `(from,
/// recv, name, write)` at its first source row, when `recv` is a shape a
/// type can be known for: `this.<field>`, a bare identifier, or another
/// expression a [`NavFact::ReceiverType`] may type (an `inject(T)` call, a
/// signal read `this.f()`). `this.<name>` itself is DT.5g's
/// ([`record_this_ref`]). Which reads are kept is decided in
/// `resolve_intra_file`, once the file's field, local and receiver types are
/// known.
fn record_member_read(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    enclosing_class: Option<NodeId>,
    acc: &mut Acc,
) {
    let (Some(object), Some(property)) = (
        node.child_by_field_name("object"),
        node.child_by_field_name("property"),
    ) else {
        return;
    };
    if !matches!(
        property.kind(),
        "property_identifier" | "private_property_identifier"
    ) {
        return;
    }
    let mut write = false;
    if let Some(parent) = node.parent() {
        let is = |field: &str| parent.child_by_field_name(field) == Some(node);
        match parent.kind() {
            "call_expression" if is("function") => return,
            "assignment_expression" | "augmented_assignment_expression" if is("left") => {
                write = true;
            }
            _ => {}
        }
    }
    let shape = match object.kind() {
        "this" => return,
        "identifier" => RecvShape::Local,
        "member_expression" => {
            let (Some(o), Some(p)) = (
                object.child_by_field_name("object"),
                object.child_by_field_name("property"),
            ) else {
                return;
            };
            if o.kind() != "this"
                || !matches!(p.kind(), "property_identifier" | "private_property_identifier")
            {
                return;
            }
            RecvShape::ThisField(text(p, src).to_string())
        }
        "call_expression" | "non_null_expression" | "parenthesized_expression" => RecvShape::Expr,
        _ => return,
    };
    let receiver = text(object, src).to_string();
    let name = text(property, src).to_string();
    let row = line_at(node);
    let key = (from, receiver, name, write);
    match acc.member_read_seen.get(&key) {
        Some(&i) => {
            if let Some(r) = acc.member_reads.get_mut(i) {
                r.row = r.row.min(row);
            }
        }
        None => {
            let (from, receiver, name, write) = key;
            acc.member_read_seen.insert(
                (from, receiver.clone(), name.clone(), write),
                acc.member_reads.len(),
            );
            acc.member_reads.push(MemberReadCand {
                from,
                class: enclosing_class,
                receiver,
                shape,
                name,
                row,
                write,
            });
        }
    }
}

/// DT.5g: record a `this.<name>` member read (`write` false) or assignment
/// target (`write` true) of `class`, by `from`, once per `(from, name,
/// write)` at its first source row (the call walk pops siblings right to
/// left). Whether `name` is an accessor is decided in `resolve_intra_file`.
fn record_accessor_ref(
    acc: &mut Acc,
    from: NodeId,
    class: NodeId,
    name: String,
    row: u32,
    write: bool,
) {
    let key = (from, name, write);
    match acc.accessor_ref_seen.get(&key) {
        Some(&i) => {
            if let Some(entry) = acc.accessor_refs.get_mut(i) {
                entry.3 = entry.3.min(row);
            }
        }
        None => {
            let (from, name, write) = key;
            acc.accessor_ref_seen
                .insert((from, name.clone(), write), acc.accessor_refs.len());
            acc.accessor_refs.push((from, class, name, row, write));
        }
    }
}

/// DT.5g: record a bare identifier read as a VALUE by `from`, in one of the
/// positions that hand a symbol on without calling it: a whole argument of
/// a call or a `new` (`sort(byUrgency)`, `provideRouter(routes)`), an array
/// element (`canActivate: [authGuard]`, `imports: [TimeAgoPipe]`), a pair's
/// value (`useClass: AuthInterceptor`) or a shorthand property (`{ authGuard
/// }`). `export default x` records through [`push_value_ref`] in `visit_top`.
/// Not a value reference: a callee (a CallSite), a member's object
/// (`environment.apiUrl`), a type, a destructuring pattern, an assignment
/// target, a key. Whether the name binds (an in-file top-level declaration or
/// a named import, neither shadowed by a local of `from`) is decided in
/// `resolve_intra_file`.
fn record_value_ref(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let Some(parent) = node.parent() else {
        return;
    };
    let callee = match (node.kind(), parent.kind()) {
        ("shorthand_property_identifier", "object") | ("identifier", "array") => None,
        ("identifier", "pair") if parent.child_by_field_name("value") == Some(node) => None,
        // DT.5k: only the first argument is the call's DI token
        // (`inject(Foo, OPTS)` injects Foo; OPTS is a value passed).
        ("identifier", "arguments") => parent
            .named_children(&mut parent.walk())
            .find(|a| a.kind() != "comment")
            .filter(|first| *first == node)
            .and_then(|_| parent.parent())
            .filter(|p| matches!(p.kind(), "call_expression" | "new_expression"))
            .and_then(|p| {
                p.child_by_field_name("function")
                    .or_else(|| p.child_by_field_name("constructor"))
            })
            .filter(|c| c.kind() == "identifier")
            .map(|c| text(c, src).to_string()),
        _ => return,
    };
    push_value_ref(acc, from, text(node, src), line_at(node), callee);
}

/// DT.5g: add a site of `name` read as a value by `from` at `row`, one
/// [`ValueRef`] per `(from, name)` in walk order.
fn push_value_ref(acc: &mut Acc, from: NodeId, name: &str, row: u32, callee: Option<String>) {
    if name.is_empty() {
        return;
    }
    match acc.value_ref_seen.get(&(from, name.to_string())) {
        Some(&i) => {
            if let Some(r) = acc.value_refs.get_mut(i) {
                r.sites.push((row, callee));
            }
        }
        None => {
            acc.value_ref_seen
                .insert((from, name.to_string()), acc.value_refs.len());
            acc.value_refs.push(ValueRef {
                from,
                name: name.to_string(),
                sites: vec![(row, callee)],
            });
        }
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

// ============================================================================
// Local receiver types (DF.22a)
// ============================================================================
//
// The parser records what the AST says about each binding's type on
// `CodeNav::local_types` (LA.35a), the carrier the graph crate's generic
// receiver pass reads innermost-first, before the enclosing class's fields:
// `x.m()` on a typed local binds `m` on that type (rule `receiver_type`); a
// local of unknown type (`""`) shadows a same-named field; `self.<f>` aliases
// field `f`. A binding's scope is the node its calls are attributed to (the
// call walk's `from`): a method, a function, a function field, a
// call-initialised field or, for a top-level statement, the MODULE. A nested
// arrow / function expression shares it, as its calls do.

/// DF.22a: record `name` as a local of `scope` of simple type `ty` (`""` =
/// unknown) and, for a typed record, what typed it (`kind`, first source
/// kept; the `[ts-locals]` marker's only input). `record_local_type`'s rule
/// decides a name bound twice in one scope: the same type keeps it, another
/// type or an unknown makes the local unknown, whatever the order.
fn record_local(acc: &mut Acc, scope: NodeId, name: &str, ty: &str, kind: LocalSrc) {
    acc.nav.record_local_type(scope, name, ty);
    if !ty.is_empty() && !name.is_empty() {
        acc.local_src
            .entry((scope, name.to_string()))
            .or_insert(kind);
    }
}

/// DF.22a: the names a binding pattern binds: an `identifier` or a shorthand
/// property pattern, through object / array / rest / pair / defaulted
/// patterns (`{ d }`, `[a, ...more]`, `{ k: v }`, `{ d = 1 }`). A default
/// value, a pair's key and a member / subscript target (`for (this.x of xs)`)
/// bind nothing.
fn pattern_names<'a>(pattern: TsNode, src: &'a [u8]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut stack = vec![pattern];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => out.push(text(n, src)),
            "object_pattern" | "array_pattern" | "rest_pattern" => {
                let mut cursor = n.walk();
                stack.extend(n.named_children(&mut cursor));
            }
            "pair_pattern" => stack.extend(n.child_by_field_name("value")),
            "assignment_pattern" | "object_assignment_pattern" => {
                stack.extend(n.child_by_field_name("left"));
            }
            _ => {}
        }
    }
    out
}

/// DF.22a: record every name the `field` pattern of `node` binds as a local
/// of unknown type: a `for … in` / `for … of` loop variable, a catch
/// parameter.
fn record_pattern_unknown(node: TsNode, field: &str, src: &[u8], scope: NodeId, acc: &mut Acc) {
    if let Some(pattern) = node.child_by_field_name(field) {
        for name in pattern_names(pattern, src) {
            acc.nav.record_local_type(scope, name, "");
        }
    }
}

/// DF.22a: record every formal parameter of the callable `func` (a method,
/// constructor or accessor, a function declaration, a function field's value,
/// an arrow, a function expression) as a local of `scope`. An identifier
/// parameter takes its annotation's class type ([`annotated_class_type`]:
/// `ns.Foo` / `Foo<T>` / `Foo | null` -> `Foo`), else `""`; a destructuring,
/// rest or defaulted pattern binds each of its names as `""`; a `this`
/// parameter binds nothing. An arrow's bare `x => …` parameter is `""`.
fn record_param_types(func: TsNode, src: &[u8], scope: NodeId, acc: &mut Acc) {
    if let Some(p) = func.child_by_field_name("parameter") {
        for name in pattern_names(p, src) {
            acc.nav.record_local_type(scope, name, "");
        }
        return;
    }
    let Some(params) = func.child_by_field_name("parameters") else {
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
        match pattern.kind() {
            "this" => {}
            "identifier" => {
                let ty = param
                    .child_by_field_name("type")
                    .and_then(|t| annotated_class_type(t, src))
                    .unwrap_or("");
                record_local(acc, scope, text(pattern, src), ty, LocalSrc::Param);
            }
            _ => {
                for name in pattern_names(pattern, src) {
                    acc.nav.record_local_type(scope, name, "");
                }
            }
        }
    }
}

/// DF.22a: record a `variable_declarator`'s binding as a local of `scope`.
/// An identifier name takes, in order: its annotation's class type (an
/// annotation decides, `let s: Store;` with no value included; a
/// non-class annotation is `""`); else its value's: a `new T(…)`'s
/// constructor ([`new_type`]), an `x as T`'s `T`, a `this.<p>` read as the
/// alias `self.<p>`, an `inject(T)`-shaped call queued for the import gate
/// ([`Acc::local_inject`]); anything else (a call result, a literal, an
/// `await`, a chain, no value) is `""`. A destructuring name binds each of
/// its names as `""`.
fn record_declarator_type(decl: TsNode, src: &[u8], scope: NodeId, acc: &mut Acc) {
    let Some(name_node) = decl.child_by_field_name("name") else {
        return;
    };
    if name_node.kind() != "identifier" {
        for name in pattern_names(name_node, src) {
            acc.nav.record_local_type(scope, name, "");
        }
        return;
    }
    let name = text(name_node, src);
    if let Some(ann) = decl.child_by_field_name("type") {
        let ty = annotated_class_type(ann, src).unwrap_or("");
        record_local(acc, scope, name, ty, LocalSrc::Annotated);
        return;
    }
    let Some(value) = decl.child_by_field_name("value") else {
        acc.nav.record_local_type(scope, name, "");
        return;
    };
    match value.kind() {
        "new_expression" => {
            let ty = new_type(value, src).unwrap_or("");
            record_local(acc, scope, name, ty, LocalSrc::New);
        }
        "as_expression" => {
            let mut cursor = value.walk();
            let ty = value
                .named_children(&mut cursor)
                .filter(|c| c.kind() != "comment")
                .nth(1)
                .and_then(|t| class_type_name(t, src))
                .unwrap_or("");
            record_local(acc, scope, name, ty, LocalSrc::Annotated);
        }
        "member_expression" => {
            let alias = value
                .child_by_field_name("object")
                .filter(|o| o.kind() == "this")
                .and(value.child_by_field_name("property"))
                .filter(|p| {
                    matches!(
                        p.kind(),
                        "property_identifier" | "private_property_identifier"
                    )
                })
                .map(|p| format!("self.{}", text(p, src)));
            let ty = alias.as_deref().unwrap_or("");
            record_local(acc, scope, name, ty, LocalSrc::Alias);
        }
        "call_expression" => match inject_shape(value, src) {
            Some((callee, ty)) => acc.local_inject.push(LocalInject {
                scope,
                name: name.to_string(),
                callee: callee.to_string(),
                ty: ty.to_string(),
            }),
            // DT.5k: a bare call `f(..)` holds `f`'s result, typed late
            // ([`record_call_locals`]); a member call's result stays unknown.
            None => match value.child_by_field_name("function") {
                Some(f) if f.kind() == "identifier" => acc.call_locals.push((
                    scope,
                    name.to_string(),
                    text(f, src).to_string(),
                )),
                _ => acc.nav.record_local_type(scope, name, ""),
            },
        },
        _ => acc.nav.record_local_type(scope, name, ""),
    }
}

/// DF.22a: the class a `new T(…)` builds: its constructor's identifier, or a
/// member chain's last property (`new ns.Widget<T>()` -> `Widget`), kept only
/// when it starts with an ASCII uppercase letter (a class name by
/// convention; `new factory()` names none).
fn new_type<'a>(value: TsNode, src: &'a [u8]) -> Option<&'a str> {
    let ctor = value.child_by_field_name("constructor")?;
    let name = match ctor.kind() {
        "identifier" => text(ctor, src),
        "member_expression" => text(ctor.child_by_field_name("property")?, src),
        _ => return None,
    };
    name.starts_with(|c: char| c.is_ascii_uppercase())
        .then_some(name)
}

/// DF.22a: `(callee, type)` of an `inject(T)`-shaped call, as A7.1 reads an
/// `inject(T)` field: an identifier callee and a first argument that is an
/// identifier or a member chain; the type is a class-shaped `<T>` argument
/// (`inject<HttpClient>(TOKEN)`), else the argument's simple name when it
/// starts with an ASCII uppercase letter (`inject(AuthService)`,
/// `inject(ns.Store)`). Whether the callee IS the `inject` import is
/// [`record_inject_locals`]'s late gate.
fn inject_shape<'a>(call: TsNode, src: &'a [u8]) -> Option<(&'a str, &'a str)> {
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "identifier" {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let mut ac = args.walk();
    let arg = args.named_children(&mut ac).find(|a| a.kind() != "comment")?;
    if !matches!(arg.kind(), "identifier" | "member_expression") {
        return None;
    }
    let ty = match call.child_by_field_name("type_arguments") {
        Some(ta) => {
            let mut tc = ta.walk();
            let first = ta.named_children(&mut tc).next()?;
            class_type_name(first, src)?
        }
        None => heritage_type_name(arg, src)
            .filter(|t| t.starts_with(|c: char| c.is_ascii_uppercase()))?,
    };
    Some((text(callee, src), ty))
}

/// DF.22a: record the queued `inject(T)`-shaped locals ([`Acc::local_inject`])
/// in source order, once every import of the file is read: typed when the
/// callee is a named `inject` import binding (`import { inject }`, or
/// `{ inject as i }`: A7.1's gate on `inject(T)` fields), else of unknown
/// type, so a project-local `inject` helper types nothing.
fn record_inject_locals(acc: &mut Acc) {
    let gate: HashSet<String> = inject_import_bindings(&acc.imports)
        .into_iter()
        .map(str::to_string)
        .collect();
    for local in std::mem::take(&mut acc.local_inject) {
        let ty = if gate.contains(&local.callee) {
            local.ty.as_str()
        } else {
            ""
        };
        record_local(acc, local.scope, &local.name, ty, LocalSrc::Inject);
    }
}

/// DT.5k: record the queued `const x = f(..)` locals ([`Acc::call_locals`])
/// as the call-result type `f()` (CodeNav's normalised call form, which the
/// graph types through `f`'s declared return type), once every local of the
/// file is read: a local or parameter of the scope named `f`, or a nested
/// `function f` there, shadows the top-level `f`, and the local is of
/// unknown type.
fn record_call_locals(acc: &mut Acc) {
    for (scope, name, callee) in std::mem::take(&mut acc.call_locals) {
        let shadowed = acc
            .nav
            .local_types
            .get(&scope)
            .is_some_and(|l| l.contains_key(&callee))
            || acc.nested_decls.contains(&(scope, callee.clone()));
        if shadowed || callee == name {
            acc.nav.record_local_type(scope, &name, "");
            continue;
        }
        record_local(acc, scope, &name, &format!("{callee}()"), LocalSrc::Call);
        acc.receiver_stats.call_locals += 1;
    }
}

/// DF.22a: count the file's `x.m()` call sites (an `Attribute` qualifier)
/// whose `x` is a recorded local of the caller, typed (by [`LocalSrc`]) or
/// unknown: the `[ts-locals]` marker's numbers.
fn local_stats(acc: &Acc) -> LocalStats {
    let mut stats = LocalStats::default();
    for uc in &acc.unresolved {
        let CallQualifier::Attribute { base, .. } = &uc.qualifier else {
            continue;
        };
        let Some(ty) = acc
            .nav
            .local_types
            .get(&uc.from)
            .and_then(|locals| locals.get(base.as_str()))
        else {
            continue;
        };
        if ty.is_empty() {
            stats.unknown += 1;
            continue;
        }
        stats.typed += 1;
        match acc.local_src.get(&(uc.from, base.clone())) {
            Some(LocalSrc::Param) => stats.param += 1,
            Some(LocalSrc::Annotated) => stats.annotated += 1,
            Some(LocalSrc::New) => stats.new += 1,
            Some(LocalSrc::Inject) => stats.inject += 1,
            Some(LocalSrc::Alias) => stats.alias += 1,
            Some(LocalSrc::Call) => stats.call += 1,
            None => {}
        }
    }
    stats
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

/// CH.3c: what a client call's callee is reached through.
enum CallBase {
    /// Shape 3, `fetch(…)`: no receiver.
    Fetch,
    /// Shape 1, `this.<field>.<verb>(…)`: the field.
    ThisField(String),
    /// Shape 2, `<alias>.<verb>(…)`: emits only if `alias` is a module-level
    /// import alias.
    Alias(String),
}

/// CH.3c: the declared / constructed / injected types a `this.<field>`
/// receiver of a client HTTP call may hold: Angular's `HttpClient` /
/// `HttpHandler` (and the pre-4.3 `Http`), NestJS's `HttpService`, axios,
/// got and ky instances. A type ending in `HttpClient` or `HttpService` counts
/// too ([`is_http_client_type`]). An interface-typed client (`HttpLike`) gains
/// a row here, never a per-repo rule.
const HTTP_CLIENT_TYPES: &[&str] = &[
    "HttpClient",
    "HttpService",
    "HttpHandler",
    "Http",
    "AxiosInstance",
    "AxiosStatic",
    "Axios",
    "Got",
    "KyInstance",
    "Ky",
];

/// CH.3c: `t` (a simple type name) is an HTTP client type.
fn is_http_client_type(t: &str) -> bool {
    HTTP_CLIENT_TYPES.contains(&t) || t.ends_with("HttpClient") || t.ends_with("HttpService")
}

/// CH.3c: what the receiver gate dropped in one file (the
/// `[ts-endpoint-gate]` marker).
#[derive(Default, Clone, PartialEq, Eq, Debug)]
struct ReceiverGateStats {
    /// `<unresolved>` shape-1 candidates dropped.
    dropped: usize,
    /// Dropped candidates by receiver type; a BTreeMap so the marker is stable.
    by_type: std::collections::BTreeMap<String, usize>,
}

/// CH.3c: drop each shape-1 candidate `this.<field>.<verb>(<arg>)` whose
/// path stayed `<unresolved>` and whose `<field>` has a known type of the
/// enclosing class that is not an HTTP client: a Map / Set / store / in-repo
/// service verb call (`this.timers.get(id)`, `this.drafts.patch(id, {…})`) is
/// no client HTTP call, and the A6.2a receiver pass binds the real callee
/// through the same field type. The type is the one `CodeNav::field_types`
/// holds once A7.1 has run — an `inject(T)` field's `T` whose callee passes
/// the `inject` import gate (recorded later, in `resolve_intra_file`, so
/// re-derived here from the same candidates), else an annotated field or a
/// constructor parameter (A6.2b) — else a `new X(…)` initializer's
/// ([`Acc::field_new_types`]). An unknown type keeps the candidate (fails
/// open), as does every read path (a literal on any receiver is a real call
/// through an in-repo wrapper) and every shape-2 / shape-3 call. Runs before
/// [`endpoint_arg_stats`], so `[ts-endpoint-args]` counts only what becomes
/// an ENDPOINT.
fn gate_non_http_receivers(acc: &mut Acc) -> ReceiverGateStats {
    let inject_bound = inject_import_bindings(&acc.imports);
    let injected: HashMap<(NodeId, &str), &str> = acc
        .inject_fn_candidates
        .iter()
        .filter(|c| inject_bound.contains(c.callee.as_str()))
        .filter_map(|c| {
            let (field, ty) = c.field_type.as_ref()?;
            Some(((c.class_id, field.as_str()), ty.as_str()))
        })
        .collect();
    let mut stats = ReceiverGateStats::default();
    let mut kept = Vec::with_capacity(acc.endpoints.len());
    for cand in std::mem::take(&mut acc.endpoints) {
        let receiver_type = match (cand.class, cand.receiver_field.as_deref()) {
            (Some(class), Some(field)) if cand.path == UNRESOLVED_PATH => injected
                .get(&(class, field))
                .copied()
                .or_else(|| {
                    acc.nav
                        .field_types
                        .get(&class)
                        .and_then(|m| m.get(field))
                        .map(String::as_str)
                })
                .or_else(|| {
                    acc.field_new_types
                        .get(&(class, field.to_string()))
                        .map(String::as_str)
                }),
            _ => None,
        };
        match receiver_type {
            Some(ty) if !is_http_client_type(ty) => {
                stats.dropped += 1;
                *stats.by_type.entry(ty.to_string()).or_default() += 1;
            }
            _ => kept.push(cand),
        }
    }
    acc.endpoints = kept;
    if stats.dropped > 0 {
        let types: Vec<String> = stats
            .by_type
            .iter()
            .map(|(t, k)| format!("{t}:{k}"))
            .collect();
        eprintln!(
            "[ts-endpoint-gate] dropped non-HTTP receivers={} (types={}) file={}",
            stats.dropped,
            types.join(","),
            acc.file_rel
        );
    }
    stats
}

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
        let init = args.named_child(1);
        let method = fetch_method_from_opts(init, src).unwrap_or_else(|| {
            // Method override is opaque (variable, spread, conditional) — drop
            // confidence one tier.
            if init.is_some() {
                arg.confidence = downgrade(arg.confidence);
            }
            "GET".to_string()
        });
        // DT.1d: the init object's `body: JSON.stringify(<expr>)` and the
        // awaited response's `.json() as T`.
        let wire = WireRead {
            fetch: true,
            response: fetch_response(call, src).map(|s| (s, WireSource::Fetch)),
            fetch_request: init.and_then(|i| fetch_request(i, src)),
            ..WireRead::default()
        };
        push_endpoint(
            call,
            from,
            enclosing_class,
            method,
            arg,
            CallBase::Fetch,
            wire,
            acc,
        );
        return;
    }

    // DT.1d: `await recv.verb<T>(..)` parses with the `await` inside the
    // callee ([`awaited_generic_callee`]); its member is the callee, and the
    // hit is located at the member, where the unawaited form's call starts.
    let (func, awaited_generic) = match awaited_generic_callee(call) {
        Some((member, _)) => (member, true),
        None => (func, false),
    };

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
    // whose object is `this`. CH.3c: `<x>` rides on the candidate for the
    // receiver gate.
    let base = match object.kind() {
        "member_expression" => {
            let inner_obj = match object.child_by_field_name("object") {
                Some(o) => o,
                None => return,
            };
            if inner_obj.kind() != "this" {
                return;
            }
            let Some(field) = object.child_by_field_name("property") else {
                return;
            };
            CallBase::ThisField(text(field, src).to_string())
        }
        // Shape 2: <alias>.<method>(...) — object is a plain identifier that
        // must be a module-level import alias. Validation happens in
        // resolve_intra_file once all imports are known.
        "identifier" => CallBase::Alias(text(object, src).to_string()),
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
    let wire = member_call_wire(call, method_lower, args, awaited_generic, src);
    push_endpoint(
        if awaited_generic { func } else { call },
        from,
        enclosing_class,
        method_lower.to_uppercase(),
        arg,
        base,
        wire,
        acc,
    );
}

/// `site` is the node the hit is located at: the call, or the callee member
/// of an awaited generic call (DT.1d), whose call node starts at `await`.
#[allow(clippy::too_many_arguments)]
fn push_endpoint(
    site: TsNode,
    from: NodeId,
    class: Option<NodeId>,
    method: String,
    arg: PathArg,
    base: CallBase,
    wire: WireRead,
    acc: &mut Acc,
) {
    let start = site.start_position();
    let (requires_import_alias, receiver_field) = match base {
        CallBase::Fetch => (None, None),
        CallBase::ThisField(field) => (None, Some(field)),
        CallBase::Alias(alias) => (Some(alias), None),
    };
    let PathArg {
        path,
        template,
        confidence,
        wrapper,
        wrapper_recv,
        read,
        alias,
        query,
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
        receiver_field,
        raw_path,
        template,
        wrapper,
        wrapper_recv,
        class,
        read,
        alias,
        query,
        wire,
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
        "template_string" => classify_template(arg, src, indirect_ok),
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
        // DF.20: a builder's argument substitutes no member: the builder's
        // own transform decides the URL (CH.5b's prefix fold).
        let read = classify_template(literal, src, false);
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
///
/// DF.20: when `aliases_ok` (a direct read: CH.3a's depth one), a
/// substitution `${this.<m>}` whose member [`class_member_value`] reads as a
/// config expression ([`config_expr_text`]) holding at least one path is
/// written into `template` as that expression's normal form instead
/// (`${this.api}/trades` -> `${environment.apiUrl}/trades`), and `alias`
/// names the member. `path` keeps `${…}`, so the identity above holds for
/// every template but such a one: the endpoint fold keys the node once the
/// table resolves the expression, and an expression no span of which
/// resolves folds to nothing, leaving the node as it was. A member that is no
/// config expression (or a literal, which the table's own binding of the
/// member folds, as on HEAD) keeps `${this.<m>}`.
///
/// DT.4: when the template's LAST named child is a substitution
/// [`query_suffix`] reads as a query string (`${q}` with
/// `const q = s ? `?status=${s}` : ''`, or an inline
/// `${t ? '?q=' + t : ''}`), it is written to neither string and `query`
/// names it: a query string is not part of the request path. The identity
/// above still holds (both strings lose the same child), and has_subst /
/// confidence count only the substitutions kept, so `` `/admin/smr${q}` ``
/// reads exactly as the literal `'/admin/smr'` does. A template that is
/// nothing but that substitution keeps it (no path precedes it), as does a
/// `?`-led value anywhere but the end. It reads a declaration, never a value,
/// so it applies with or without `aliases_ok`.
fn classify_template(template: TsNode, src: &[u8], aliases_ok: bool) -> PathArg {
    let mut out = String::new();
    let mut source = String::new();
    let mut has_subst = false;
    let mut aliases: Vec<String> = Vec::new();
    let mut cursor = template.walk();
    let children: Vec<TsNode> = template.named_children(&mut cursor).collect();
    let query = match children.as_slice() {
        [_, .., last] if last.kind() == "template_substitution" => query_suffix(*last, src),
        _ => None,
    };
    let kept = children.len() - usize::from(query.is_some());
    for child in children.into_iter().take(kept) {
        match child.kind() {
            "template_substitution" => {
                has_subst = true;
                out.push_str("${…}");
                match aliases_ok.then(|| member_alias(child, src)).flatten() {
                    Some((alias, normal)) => {
                        source.push_str(&normal);
                        aliases.push(alias);
                    }
                    None => source.push_str(text(child, src)),
                }
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
        alias: (!aliases.is_empty()).then(|| aliases.join(",")),
        query,
    }
}

/// DT.4: what a template's trailing `template_substitution` holds when it is
/// a query string, read off the declaration the site names, never off a
/// value: an identifier whose `const` ([`local_const_value`], CH.3a arm d's
/// rules) [`is_query_string`] -> that identifier; an expression that is
/// itself one (`${t ? '?q=' + t : ''}`) -> `inline`. A parameter, a `let` /
/// `var`, a shadowed name, a call (`${qs(p)}`) or a member (`${this.q}`)
/// is None: no value data-flow.
fn query_suffix(subst: TsNode, src: &[u8]) -> Option<String> {
    let expr = sole_named(subst)?;
    if expr.kind() == "identifier" {
        let value = local_const_value(expr, src)?;
        return is_query_string(value, src).then(|| text(expr, src).to_string());
    }
    is_query_string(expr, src).then(|| "inline".to_string())
}

/// DT.4: true when `expr` is shaped as a URL query string: a `?`-led string
/// or template; a ternary whose two branches are each one or empty (the
/// optional-query idiom: neither runtime value adds to the request path);
/// `( .. )` of one; `'?k=' + v` (a `+` whose left operand is one).
/// `archived ? '/archived' : ''` (a path segment) and `c ? '?a=1' : '/x'` are
/// not.
fn is_query_string(expr: TsNode, src: &[u8]) -> bool {
    match expr.kind() {
        "string" => strip_string_quotes(text(expr, src)).starts_with('?'),
        "template_string" => expr
            .named_child(0)
            .is_some_and(|c| c.kind() == "string_fragment" && text(c, src).starts_with('?')),
        "ternary_expression" => ["consequence", "alternative"].iter().all(|f| {
            expr.child_by_field_name(f)
                .is_some_and(|b| is_query_string(b, src) || is_empty_string(b, src))
        }),
        "parenthesized_expression" => sole_named(expr).is_some_and(|e| is_query_string(e, src)),
        "binary_expression" => {
            expr.child_by_field_name("operator")
                .is_some_and(|op| text(op, src) == "+")
                && expr
                    .child_by_field_name("left")
                    .is_some_and(|l| is_query_string(l, src))
        }
        _ => false,
    }
}

/// DT.4: `''`, `""`, an empty template, or `( .. )` of one.
fn is_empty_string(expr: TsNode, src: &[u8]) -> bool {
    match expr.kind() {
        "string" => strip_string_quotes(text(expr, src)).is_empty(),
        "template_string" => expr.named_child_count() == 0,
        "parenthesized_expression" => sole_named(expr).is_some_and(|e| is_empty_string(e, src)),
        _ => false,
    }
}

/// DF.20: for a `template_substitution` that is exactly `this.<m>`, the
/// alias `this.<m>` and the normal form of the config expression its
/// own-class `readonly` field or one-`return` getter holds
/// ([`class_member_value`], CH.3a's rules: no inherited, reassignable or
/// other-`this` member), when that form reads at least one path.
fn member_alias(subst: TsNode, src: &[u8]) -> Option<(String, String)> {
    let mut cursor = subst.walk();
    let mut named = subst
        .named_children(&mut cursor)
        .filter(|c| c.kind() != "comment");
    let member = named.next()?;
    if named.next().is_some()
        || member.kind() != "member_expression"
        || member.child_by_field_name("object")?.kind() != "this"
    {
        return None;
    }
    let property = member.child_by_field_name("property")?;
    if property.kind() != "property_identifier" {
        return None;
    }
    let normal = config_expr_text(class_member_value(member, src)?, src)?;
    normal
        .contains("${")
        .then(|| (format!("this.{}", text(property, src)), normal))
}

/// DF.20: the template normal form of a CONFIG EXPRESSION, the grammar
/// `glia_code_extractors::constants` reads off one text line for a `const`:
/// (a) a member path of identifiers not rooted at `this` / `super` and
/// reading no `env` / `environ` owner (`environment.apiUrl`) -> `${path}`;
/// (b) a string literal -> its text; (c) a template whose substitutions are
/// all (a) or (e) -> its text, each substitution `${path}`; (d) `x + y` of
/// (a)-(e) -> the concatenation; (e) `<path>.replace('<a>', '<b>')` with two
/// plain string literals -> `${path.replace('a', 'b')}`; a parenthesised one
/// is itself. Anything else (a call, `this.x`, `process.env.X || ..`, a
/// ternary, an escape) is None: no evaluator, no value data-flow.
fn config_expr_text(expr: TsNode, src: &[u8]) -> Option<String> {
    match expr.kind() {
        "member_expression" => config_path(expr, src).map(|p| format!("${{{p}}}")),
        "string" => {
            let mut cursor = expr.walk();
            if expr
                .named_children(&mut cursor)
                .any(|c| c.kind() != "string_fragment")
            {
                return None;
            }
            let body = strip_string_quotes(text(expr, src));
            literal_piece_ok(&body).then_some(body)
        }
        "template_string" => {
            let mut out = String::new();
            let mut cursor = expr.walk();
            for child in expr.named_children(&mut cursor) {
                match child.kind() {
                    "string_fragment" => {
                        let piece = text(child, src);
                        if !literal_piece_ok(piece) {
                            return None;
                        }
                        out.push_str(piece);
                    }
                    "template_substitution" => {
                        let mut inner = child.walk();
                        let mut named = child
                            .named_children(&mut inner)
                            .filter(|c| c.kind() != "comment");
                        let only = named.next()?;
                        if named.next().is_some() {
                            return None;
                        }
                        out.push_str(&path_or_replace(only, src)?);
                    }
                    _ => return None,
                }
            }
            Some(out)
        }
        "binary_expression" => {
            let op = expr.child_by_field_name("operator")?;
            if text(op, src) != "+" {
                return None;
            }
            let left = config_expr_text(expr.child_by_field_name("left")?, src)?;
            let right = config_expr_text(expr.child_by_field_name("right")?, src)?;
            Some(left + &right)
        }
        "parenthesized_expression" => config_expr_text(sole_named(expr)?, src),
        "call_expression" => replace_text(expr, src),
        _ => None,
    }
}

/// DF.20: a member path (a) as `${path}`, or a `.replace` call (e) as its
/// span: what a config template's substitution may be.
fn path_or_replace(expr: TsNode, src: &[u8]) -> Option<String> {
    match expr.kind() {
        "member_expression" => config_path(expr, src).map(|p| format!("${{{p}}}")),
        "call_expression" => replace_text(expr, src),
        _ => None,
    }
}

/// DF.20 (e): `<path>.replace('<a>', '<b>')` (the path optionally
/// parenthesised) with exactly two plain string literals, as
/// `${path.replace('a', 'b')}`.
fn replace_text(call: TsNode, src: &[u8]) -> Option<String> {
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "member_expression"
        || callee
            .child_by_field_name("property")
            .is_none_or(|p| text(p, src) != "replace")
    {
        return None;
    }
    let mut object = callee.child_by_field_name("object")?;
    while object.kind() == "parenthesized_expression" {
        object = sole_named(object)?;
    }
    if object.kind() != "member_expression" {
        return None;
    }
    let path = config_path(object, src)?;
    let args = call
        .child_by_field_name("arguments")
        .filter(|a| a.kind() == "arguments")?;
    let mut cursor = args.walk();
    let lits: Vec<TsNode> = args
        .named_children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .collect();
    let [from, to] = lits.as_slice() else {
        return None;
    };
    let from = replace_arg(*from, src)?;
    let to = replace_arg(*to, src)?;
    Some(format!("${{{path}.replace('{from}', '{to}')}}"))
}

/// DF.20: a `.replace` argument: a plain string literal (no escape) whose
/// text the normal form carries in single quotes and the fold evaluates
/// exactly: no quote, backslash, brace, `$` (a JS replacement pattern) or
/// line break.
fn replace_arg(lit: TsNode, src: &[u8]) -> Option<String> {
    if lit.kind() != "string" {
        return None;
    }
    let mut cursor = lit.walk();
    if lit
        .named_children(&mut cursor)
        .any(|c| c.kind() != "string_fragment")
    {
        return None;
    }
    let body = strip_string_quotes(text(lit, src));
    (!body.contains(['\'', '"', '`', '\\', '{', '}', '$', '\n', '\r'])).then_some(body)
}

/// DF.20: literal text a normal form may carry: nothing that opens a span or
/// hides an escape.
fn literal_piece_ok(piece: &str) -> bool {
    !piece.contains(['$', '`', '\\', '\n', '\r'])
}

/// DF.20 (a): `environment.apiUrl` (or `env?.api.base`) as a `.`-joined
/// path: identifiers and property identifiers only, rooted at an
/// `identifier` (not `this` / `super`), no owner named `env` / `environ`.
fn config_path(member: TsNode, src: &[u8]) -> Option<String> {
    let mut segs: Vec<&str> = Vec::new();
    let mut cur = member;
    loop {
        match cur.kind() {
            "member_expression" => {
                let prop = cur.child_by_field_name("property")?;
                if prop.kind() != "property_identifier" {
                    return None;
                }
                segs.push(text(prop, src));
                cur = cur.child_by_field_name("object")?;
            }
            "identifier" => {
                segs.push(text(cur, src));
                break;
            }
            _ => return None,
        }
    }
    segs.reverse();
    let ident = |s: &str| {
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let (_, owners) = segs.split_last()?;
    let env = owners
        .iter()
        .any(|o| o.eq_ignore_ascii_case("env") || o.eq_ignore_ascii_case("environ"));
    (segs.len() >= 2 && !env && segs.iter().all(|s| ident(s))).then(|| segs.join("."))
}

/// The one named non-comment child of `n`.
fn sole_named(n: TsNode) -> Option<TsNode> {
    let mut cursor = n.walk();
    let mut named = n.named_children(&mut cursor).filter(|c| c.kind() != "comment");
    let only = named.next()?;
    named.next().is_none().then_some(only)
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
///
/// `alias` (DF.20) follows `wrapper_of`, skipped the same way: only a
/// template that read a config-expression class member (`this.api`) names it.
///
/// `query` (DT.4) is written after `alias`, skipped the same way: only a
/// template whose trailing query-string substitution [`classify_template`]
/// dropped names it (the `const` the substitution read, or `inline`).
///
/// `response` and then `request` (DT.1d) are spliced in last through
/// `wire::append_hit_fields`, each only when the site states it: a hit with
/// neither keeps HEAD's bytes. The endpoint fold appends `host` after them.
fn endpoint_hit_cell(cand: &EndpointCandidate, wrapper_of: Option<&str>, request: Option<&Shape>) -> Cell {
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
        #[serde(skip_serializing_if = "Option::is_none")]
        alias: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<&'a str>,
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
        alias: cand.alias.as_deref(),
        query: cand.query.as_deref(),
    })
    .unwrap_or_else(|_| String::from("{}"));
    let response = cand.wire.response.as_ref().map(|(s, _)| s);
    let json = wire::append_hit_fields(&json, response, request);
    Cell {
        kind: cell_type::ENDPOINT_HIT,
        payload: CellPayload::Json(json),
    }
}

// ============================================================================
// DT.1d: client wire shapes
// ============================================================================
//
// What an HTTP call site expects back (`response`) and sends (`request`), as
// the site states it, spliced onto its ENDPOINT_HIT through
// `wire::append_hit_fields`:
//
// - response: the call's first type argument for `get` / `post` / `put` /
//   `patch` / `delete` on shape 1 (`this.<x>.<verb><T>(..)`) and shape 2 (an
//   import alias, `axios.get<T>(..)`); for `fetch`, `(await fetch(..)).json()
//   as T`, or on `const r = await fetch(..)` in the same callable,
//   `(await r.json()) as T`, `r.json() as Promise<T>` or `const d: T = await
//   r.json()`. No type argument, no response key. A typing after the call
//   (ky's `.json<T>()`, an RxJS `map`) is not read.
// - request: the body argument whose position the client fixes
//   ([`request_of`]): Angular HttpClient's and axios's second argument of
//   `post` / `put` / `patch`, ky's and got's `json:` option, fetch's
//   `body: JSON.stringify(<expr>)`. A body is read as an object literal's
//   keys, a declared type (a parameter or local annotation, `x as T`, `x
//   satisfies T`, `new T()`) or, unread, open ([`body_shape`]); FormData /
//   Blob / string bodies are no JSON body and write none.
//
// Type names stay unresolved text (`Order`): the models pass resolves them in
// the hit file's module (DT.1g). Fired-on: `[ts-wire] responses=.. requests=..
// (typed=.. inline=.. literal=.. annotated=.. fetch=..) request_unknown_client=..
// awaited_generic=.. file=..`, per file with a shape or an awaited generic
// endpoint.

/// DT.1d: which reading produced a shape (the `[ts-wire]` counters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireSource {
    /// A type argument other than an inline object type (`get<Order[]>`).
    Typed,
    /// An inline object type argument (`get<{ items: Item[] }>`).
    Inline,
    /// An object-literal body (`{ status: 'cancelled' }`), at the call or in
    /// the local it names.
    Literal,
    /// A body read through a declared type: a parameter or local annotation,
    /// `x as T`, `x satisfies T`, `new T()`.
    Annotated,
    /// A body expression nothing states the type of: an open shape.
    Unread,
    /// A `fetch` call's `JSON.stringify` body or `.json() as T` result.
    Fetch,
}

/// DT.1d: the shapes an HTTP call site states, read during the call walk.
/// The tree is gone by `resolve_intra_file`, which picks the request
/// ([`request_of`]) once the file's imports and field types are known.
#[derive(Debug, Clone, Default)]
struct WireRead {
    /// Shape 3, `fetch(..)`.
    fetch: bool,
    /// The response shape.
    response: Option<(Shape, WireSource)>,
    /// `fetch`: the init object's `body: JSON.stringify(<expr>)`.
    fetch_request: Option<(Shape, WireSource)>,
    /// Shapes 1 / 2: the call has a second argument.
    has_second: bool,
    /// Shapes 1 / 2: the second argument read as a JSON body.
    second: Option<(Shape, WireSource)>,
    /// Shapes 1 / 2: the `json:` property of the second argument's object
    /// literal (ky / got).
    json_option: Option<(Shape, WireSource)>,
    /// The callee was reached through [`awaited_generic_callee`].
    awaited_generic: bool,
}

/// DT.1d: what [`request_of`] found for one emitted call site.
enum RequestRead<'c> {
    /// The client's body position, read (`None` when the call passes no JSON
    /// body there).
    Body(Option<&'c (Shape, WireSource)>),
    /// A `post` / `put` / `patch` with a second argument on a receiver or
    /// library whose body position is not known (`request_unknown_client`).
    UnknownClient,
    /// No body position: a verb the client sends no body with, or no second
    /// argument.
    NoBody,
}

/// DT.1d: the `[ts-wire]` counters of one file's emitted HTTP call sites.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct WireStats {
    responses: usize,
    requests: usize,
    typed: usize,
    inline: usize,
    literal: usize,
    annotated: usize,
    fetch: usize,
    unknown_client: usize,
    awaited_generic: usize,
}

impl WireStats {
    fn record(&mut self, cand: &EndpointCandidate, request: &RequestRead) {
        let mut source = |s: WireSource| match s {
            WireSource::Typed => self.typed += 1,
            WireSource::Inline => self.inline += 1,
            WireSource::Literal => self.literal += 1,
            WireSource::Annotated => self.annotated += 1,
            WireSource::Fetch => self.fetch += 1,
            WireSource::Unread => {}
        };
        if let Some((_, s)) = &cand.wire.response {
            source(*s);
        }
        if let RequestRead::Body(Some((_, s))) = request {
            source(*s);
        }
        self.responses += usize::from(cand.wire.response.is_some());
        self.requests += usize::from(matches!(request, RequestRead::Body(Some(_))));
        self.unknown_client += usize::from(matches!(request, RequestRead::UnknownClient));
        self.awaited_generic += usize::from(cand.wire.awaited_generic);
    }

    fn report(&self, file: &str) {
        #[cfg(test)]
        LAST_WIRE_STATS.with(|w| *w.borrow_mut() = self.clone());
        if self.responses + self.requests + self.awaited_generic == 0 {
            return;
        }
        eprintln!(
            "[ts-wire] responses={} requests={} (typed={} inline={} literal={} annotated={} fetch={}) \
             request_unknown_client={} awaited_generic={} file={file}",
            self.responses,
            self.requests,
            self.typed,
            self.inline,
            self.literal,
            self.annotated,
            self.fetch,
            self.unknown_client,
            self.awaited_generic,
        );
    }
}

// The last file's `[ts-wire]` counters on this thread, which the tests read.
#[cfg(test)]
thread_local! {
    static LAST_WIRE_STATS: std::cell::RefCell<WireStats> = std::cell::RefCell::new(WireStats::default());
}

/// The verbs whose type argument is the response (`head` / `options` carry
/// no body back).
const WIRE_RESPONSE_VERBS: &[&str] = &["get", "post", "put", "patch", "delete"];

/// The declared types of a body that is no JSON value: the call sends the
/// bytes or the form as they are, so it writes no `request`.
const NON_JSON_BODY_TYPES: &[&str] = &[
    "FormData",
    "Blob",
    "File",
    "URLSearchParams",
    "ArrayBuffer",
    "ReadableStream",
    "Uint8Array",
    "string",
    "String",
];

/// How deep an object-literal body's values are followed: the body's own
/// keys, a nested literal's keys and the locals they name, then open.
const BODY_DEPTH: u8 = 2;

/// DT.1d: the callee member and the type arguments of an awaited generic
/// call. tree-sitter-typescript 0.23.2 lists `[call_expression,
/// instantiation_expression, binary_expression, await_expression]` as a
/// conflict (define-grammar.js:77) and resolves `await recv.verb<T>(..)` as
/// `call_expression(function: await_expression(member_expression),
/// type_arguments, arguments)`: the `await` sits INSIDE the callee, so the
/// call's `function` is no member. `None` for every other call, an
/// unawaited generic call (`function: member_expression`) and an awaited
/// call without type arguments (`await_expression(call_expression)`)
/// included. The unary / update conflicts (`!x.get<T>(..)`) are not
/// unwrapped. CallSites keep HEAD's reading ([`extract_call_qualifier`]).
fn awaited_generic_callee<'t>(call: TsNode<'t>) -> Option<(TsNode<'t>, TsNode<'t>)> {
    let func = call.child_by_field_name("function")?;
    if func.kind() != "await_expression" {
        return None;
    }
    let type_args = call.child_by_field_name("type_arguments")?;
    let member = first_named(func)?;
    (member.kind() == "member_expression").then_some((member, type_args))
}

/// DT.1d: the wire read of a shape-1 / shape-2 call `<recv>.<verb>(..)`: the
/// response from its first type argument, and both body readings of its
/// second argument (the argument itself, and its `json:` option), between
/// which [`request_of`] picks.
fn member_call_wire(call: TsNode, verb: &str, args: TsNode, awaited_generic: bool, src: &[u8]) -> WireRead {
    let response = WIRE_RESPONSE_VERBS
        .contains(&verb)
        .then(|| call.child_by_field_name("type_arguments"))
        .flatten()
        .and_then(first_named)
        .map(|t| {
            let shape = ts_type_shape(t, src);
            let source = if matches!(shape, Shape::Object { .. }) {
                WireSource::Inline
            } else {
                WireSource::Typed
            };
            (shape, source)
        });
    let second = args.named_child(1).filter(|a| a.kind() != "comment");
    WireRead {
        fetch: false,
        response,
        fetch_request: None,
        has_second: second.is_some(),
        second: second.and_then(|a| body_shape(a, src)),
        json_option: second
            .and_then(|a| object_property(a, "json", src))
            .and_then(|v| body_shape(v, src)),
        awaited_generic,
    }
}

/// DT.1d: the request of one emitted call site. `receiver_type` is a shape-1
/// receiver field's declared type, `alias_module` the module a shape-2 alias
/// is imported from.
///
/// - `fetch`: the `JSON.stringify` body.
/// - a receiver field typed `HttpClient` (`private http: HttpClient`,
///   `inject(HttpClient)`): the second argument of `post` / `put` / `patch`;
///   `get` / `delete`'s second argument is an options object, never a body.
/// - an alias imported from `axios`: the second argument of `post` / `put` /
///   `patch` (`axios.post(url, data, config)`).
/// - an alias imported from `ky` / `got`: the `json:` property of the second
///   argument's object literal, any verb.
/// - any other receiver or library: none; a `post` / `put` / `patch` with a
///   second argument counts `request_unknown_client`.
fn request_of<'c>(
    cand: &'c EndpointCandidate,
    receiver_type: Option<&str>,
    alias_module: Option<&str>,
) -> RequestRead<'c> {
    let w = &cand.wire;
    if w.fetch {
        return RequestRead::Body(w.fetch_request.as_ref());
    }
    let body_verb = matches!(cand.method.as_str(), "POST" | "PUT" | "PATCH");
    let second = || {
        if body_verb && w.has_second {
            RequestRead::Body(w.second.as_ref())
        } else {
            RequestRead::NoBody
        }
    };
    let unknown = || {
        if body_verb && w.has_second {
            RequestRead::UnknownClient
        } else {
            RequestRead::NoBody
        }
    };
    if cand.receiver_field.is_some() {
        return match receiver_type {
            Some("HttpClient") => second(),
            _ => unknown(),
        };
    }
    match alias_module {
        Some("axios") => second(),
        Some("ky" | "ky-universal" | "got") if w.has_second => RequestRead::Body(w.json_option.as_ref()),
        Some("ky" | "ky-universal" | "got") => RequestRead::NoBody,
        _ => unknown(),
    }
}

/// DT.1d: the module the import binding `binding` comes from (`axios` for
/// `import axios from 'axios'` / `import * as axios from 'axios'` /
/// `import { default as axios } from 'axios'`).
fn import_module<'i>(imports: &'i [ImportStmt], binding: &str) -> Option<&'i str> {
    imports.iter().find_map(|imp| match &imp.target {
        ImportTarget::Symbol {
            module, name, alias, ..
        } if alias.as_deref().unwrap_or(name.as_str()) == binding => Some(module.as_str()),
        ImportTarget::Module {
            path,
            alias: Some(a),
        } if a == binding => Some(path.as_str()),
        _ => None,
    })
}

/// DT.1d: a declared type as a wire [`Shape`].
///
/// - a type name (`Order`, `ns.Order`), an array of one (`Order[]`,
///   `Array<Order>`, `ReadonlyArray<Order>`), a predefined type other than
///   `any` / `unknown` / `object`, a literal type: [`Shape::named`] of the
///   whitespace-collapsed text (one array layer peeled off, `array`);
/// - an object type: [`Shape::Object`] of its property signatures (`?` or a
///   top-level `| undefined` -> not required); an index signature or a
///   computed key leaves it open; method signatures are skipped;
/// - a union: its `null` / `undefined` members dropped, the one left read
///   alone, else [`Shape::Union`] of the rest (`null | undefined` alone is
///   `null`);
/// - parenthesised and `readonly` types: their inner type;
/// - `any` / `unknown` / `object`, every other generic (`Pick<..>`,
///   `Partial<..>`, `ApiResponse<T>`, `Observable<T>`), an intersection, a
///   tuple, a function, a conditional or a mapped type: [`Shape::Open`].
fn ts_type_shape(ty: TsNode, src: &[u8]) -> Shape {
    let written = || collapse_ws(text(ty, src));
    match ty.kind() {
        "type_annotation" | "parenthesized_type" | "readonly_type" => match first_named(ty) {
            Some(inner) => ts_type_shape(inner, src),
            None => Shape::open(&written()),
        },
        "union_type" => {
            let mut members = Vec::new();
            flatten_union(ty, &mut members);
            let kept: Vec<TsNode> = members
                .into_iter()
                .filter(|m| !(m.kind() == "literal_type" && matches!(text(*m, src).trim(), "null" | "undefined")))
                .collect();
            match kept.as_slice() {
                [] => Shape::named("null"),
                [one] => ts_type_shape(*one, src),
                many => Shape::Union(many.iter().map(|m| ts_type_shape(*m, src)).collect()),
            }
        }
        "object_type" => object_type_shape(ty, src),
        "predefined_type" => match text(ty, src).trim() {
            t @ ("any" | "unknown" | "object") => Shape::open(t),
            t => Shape::named(t),
        },
        "type_identifier" | "nested_type_identifier" | "array_type" | "literal_type" => Shape::named(&written()),
        "generic_type" => {
            let name = ty.child_by_field_name("name").map(|n| text(n, src));
            let one_arg = ty
                .child_by_field_name("type_arguments")
                .is_some_and(|a| a.named_children(&mut a.walk()).filter(|c| c.kind() != "comment").count() == 1);
            if one_arg && matches!(name, Some("Array" | "ReadonlyArray")) {
                Shape::named(&written())
            } else {
                Shape::open(&written())
            }
        }
        _ => Shape::open(&written()),
    }
}

/// The members of a (left-nested) `union_type`, in source order.
fn flatten_union<'t>(ty: TsNode<'t>, out: &mut Vec<TsNode<'t>>) {
    let mut cursor = ty.walk();
    for member in ty.named_children(&mut cursor) {
        match member.kind() {
            "union_type" => flatten_union(member, out),
            "comment" => {}
            _ => out.push(member),
        }
    }
}

/// An `object_type` as a [`Shape::Object`] ([`ts_type_shape`]); the key and
/// optionality rules are DT.1c's ([`ts_interface_schema`]).
fn object_type_shape(ty: TsNode, src: &[u8]) -> Shape {
    let mut fields = Vec::new();
    let mut open = false;
    let mut cursor = ty.walk();
    for member in ty.named_children(&mut cursor) {
        match member.kind() {
            "property_signature" => {
                let Some(name) = member.child_by_field_name("name") else {
                    continue;
                };
                let Some(key) = ts_property_key(name, src) else {
                    open |= name.kind() == "computed_property_name";
                    continue;
                };
                let t = member.child_by_field_name("type").and_then(first_named);
                let optional = has_token(member, "?") || t.is_some_and(|t| ts_optional_type(t, src));
                let shape = t.map_or_else(|| Shape::open("any"), |t| ts_type_shape(t, src));
                fields.push(ShapeField::new(&key, !optional, shape));
            }
            "index_signature" => open = true,
            _ => {}
        }
    }
    Shape::Object { fields, open }
}

/// DT.1d: a body argument as a JSON request shape, and how it was read.
///
/// - an object literal: [`Shape::Object`] of its keys (`pair` and shorthand
///   keys; a spread or a computed key leaves it open), each key's value read
///   by [`value_shape`];
/// - an identifier (or a shorthand property): the declared type of the name
///   in its scope ([`declared_shape`]), else open;
/// - `x as T` / `x satisfies T`: `T`; `new T(..)`: `T`;
/// - a string, template, number, boolean, `null` or `undefined`: no JSON
///   body (`None`), as is any shape naming a [`NON_JSON_BODY_TYPES`] type;
/// - anything else (a member read, a call): open, its text as `raw`.
fn body_shape(arg: TsNode, src: &[u8]) -> Option<(Shape, WireSource)> {
    let (shape, source) = match arg.kind() {
        "object" => (object_literal_shape(arg, src, BODY_DEPTH), WireSource::Literal),
        "identifier" | "shorthand_property_identifier" => {
            let name = text(arg, src);
            if name == "undefined" {
                return None;
            }
            declared_shape(arg, name, src, BODY_DEPTH).unwrap_or_else(|| (Shape::open(name), WireSource::Unread))
        }
        "as_expression" | "satisfies_expression" => match asserted_type(arg) {
            Some(t) => (ts_type_shape(t, src), WireSource::Annotated),
            None => return body_shape(first_named(arg)?, src),
        },
        "parenthesized_expression" | "non_null_expression" => return body_shape(first_named(arg)?, src),
        "new_expression" => (constructed_shape(arg, src), WireSource::Annotated),
        "string" | "template_string" | "number" | "true" | "false" | "null" | "undefined" => return None,
        _ => (Shape::open(&collapse_ws(text(arg, src))), WireSource::Unread),
    };
    let non_json = matches!(&shape, Shape::Named { ty, array: false, .. } if NON_JSON_BODY_TYPES.contains(&ty.as_str()));
    (!non_json).then_some((shape, source))
}

/// The type of `x as T` / `x satisfies T` (its second named child); `None`
/// for `x as const`.
fn asserted_type(expr: TsNode) -> Option<TsNode> {
    let mut cursor = expr.walk();
    expr.named_children(&mut cursor)
        .filter(|c| c.kind() != "comment")
        .nth(1)
}

/// `new T(..)` as the constructed type `T` (DT.1c's
/// [`ts_initializer_type`]), open when the constructor is no name.
fn constructed_shape(new_expr: TsNode, src: &[u8]) -> Shape {
    match ts_initializer_type(new_expr, src).as_str() {
        "any" => Shape::open(&collapse_ws(text(new_expr, src))),
        ty => Shape::named(ty),
    }
}

/// An object literal as a [`Shape::Object`] ([`body_shape`]); every listed key
/// is present, so `required`. `depth` bounds how far nested literals and the
/// locals their values name are followed.
fn object_literal_shape(obj: TsNode, src: &[u8], depth: u8) -> Shape {
    let mut fields = Vec::new();
    let mut open = false;
    let mut cursor = obj.walk();
    for member in obj.named_children(&mut cursor) {
        match member.kind() {
            "pair" => {
                let Some(key) = member.child_by_field_name("key").and_then(|k| ts_property_key(k, src)) else {
                    open = true;
                    continue;
                };
                let Some(value) = member.child_by_field_name("value") else {
                    continue;
                };
                if let Some(shape) = value_shape(value, src, depth) {
                    fields.push(ShapeField::new(&key, true, shape));
                }
            }
            "shorthand_property_identifier" => {
                let name = text(member, src);
                if let Some(shape) = value_shape(member, src, depth) {
                    fields.push(ShapeField::new(name, true, shape));
                }
            }
            "spread_element" => open = true,
            _ => {}
        }
    }
    Shape::Object { fields, open }
}

/// One object-literal value as a shape: a literal by its JSON kind (DT.1c's
/// [`ts_initializer_type`]: `string`, `number`, `boolean`, `null`, `any[]`,
/// `new T()` -> `T`), a nested literal by its keys, a name by its declared
/// type, `x as T` / `x satisfies T` by `T`, anything else open. `None` for a
/// value JSON drops (`undefined`, a function).
fn value_shape(value: TsNode, src: &[u8], depth: u8) -> Option<Shape> {
    let shape = match value.kind() {
        "string" | "template_string" | "number" | "true" | "false" | "null" | "array" => {
            Shape::named(&ts_initializer_type(value, src))
        }
        "new_expression" => constructed_shape(value, src),
        "object" if depth > 0 => object_literal_shape(value, src, depth - 1),
        "identifier" | "shorthand_property_identifier" => {
            let name = text(value, src);
            if name == "undefined" {
                return None;
            }
            match declared_shape(value, name, src, depth) {
                Some((shape, _)) => shape,
                None => Shape::open(name),
            }
        }
        "as_expression" | "satisfies_expression" => match asserted_type(value) {
            Some(t) => ts_type_shape(t, src),
            None => return value_shape(first_named(value)?, src, depth),
        },
        "parenthesized_expression" | "non_null_expression" => return value_shape(first_named(value)?, src, depth),
        "arrow_function" | "function_expression" | "generator_function" | "undefined" => return None,
        _ => Shape::open(&collapse_ws(text(value, src))),
    };
    Some(shape)
}

/// Where a name a use reads is declared ([`declared_binding`]).
enum Declared<'t> {
    /// A `required_parameter` / `optional_parameter` (or an arrow's single
    /// parameter) of an enclosing callable.
    Param(TsNode<'t>),
    /// A `variable_declarator` of a `const` / `let` / `var` the scope chain
    /// declares before the use.
    Var(TsNode<'t>),
}

/// DT.1d: the declared shape of the name `name` a use node reads: a
/// parameter's annotation, else a declarator's annotation, `as T` /
/// `satisfies T` value, `new T()` value or (within `depth`) object-literal
/// value. `None` when the name is unbound, bound by a pattern, or declared
/// with nothing that states its type (the caller reads it as open). A
/// declaration is read, never an assignment: no value data-flow.
fn declared_shape(use_node: TsNode, name: &str, src: &[u8], depth: u8) -> Option<(Shape, WireSource)> {
    if depth == 0 {
        return None;
    }
    match declared_binding(use_node, name, src)? {
        Declared::Param(p) => {
            let ty = p.child_by_field_name("type").and_then(first_named)?;
            Some((ts_type_shape(ty, src), WireSource::Annotated))
        }
        Declared::Var(d) => {
            if let Some(ty) = d.child_by_field_name("type").and_then(first_named) {
                return Some((ts_type_shape(ty, src), WireSource::Annotated));
            }
            let value = d.child_by_field_name("value")?;
            match value.kind() {
                "as_expression" | "satisfies_expression" => {
                    Some((ts_type_shape(asserted_type(value)?, src), WireSource::Annotated))
                }
                "new_expression" => Some((constructed_shape(value, src), WireSource::Annotated)),
                "object" => Some((object_literal_shape(value, src, depth - 1), WireSource::Literal)),
                _ => None,
            }
        }
    }
}

/// DT.1d: the declaration of `name` that a use at `use_node` reads, walking
/// the scope chain outwards: each block's (or the module's) last declarator
/// of `name` before the use, then each enclosing callable's parameters. A
/// binding by a destructuring pattern, a loop or catch variable, a hoisted
/// `var`, a function or a class (CH.3a's [`binds_outside_block`]) or one
/// declared only after the use stops the walk (`None`).
fn declared_binding<'t>(use_node: TsNode<'t>, name: &str, src: &[u8]) -> Option<Declared<'t>> {
    let at = use_node.start_byte();
    let mut cur = use_node;
    while let Some(parent) = cur.parent() {
        if matches!(parent.kind(), "statement_block" | "program" | "switch_body") {
            match scope_declarator(parent, name, at, src) {
                ScopeDecl::Unbound => {}
                ScopeDecl::Declarator(d) => return Some(Declared::Var(d)),
                ScopeDecl::Other => return None,
            }
        } else {
            if let Some(p) = param_declaring(parent, name, src) {
                return Some(Declared::Param(p));
            }
            if binds_outside_block(parent, name, src) {
                return None;
            }
        }
        cur = parent;
    }
    None
}

/// How one scope declares a name for [`declared_binding`].
enum ScopeDecl<'t> {
    Unbound,
    Declarator(TsNode<'t>),
    Other,
}

/// The last `variable_declarator` binding `name` (as a plain identifier) in a
/// statement of `scope` that ends before byte `at`; [`ScopeDecl::Other`] for
/// a pattern binding, a function / class declaration of the name, or a
/// binding only at or after the use (the TDZ).
fn scope_declarator<'t>(scope: TsNode<'t>, name: &str, at: usize, src: &[u8]) -> ScopeDecl<'t> {
    let mut seen = false;
    let mut before = None;
    for stmt in scope_statements(scope) {
        let decl = if stmt.kind() == "export_statement" {
            match stmt.child_by_field_name("declaration") {
                Some(d) => d,
                None => continue,
            }
        } else {
            stmt
        };
        let found = match decl.kind() {
            "lexical_declaration" | "variable_declaration" => {
                let mut cursor = decl.walk();
                let declarator = decl.named_children(&mut cursor).find(|d| {
                    d.kind() == "variable_declarator"
                        && d.child_by_field_name("name")
                            .is_some_and(|n| binds_name(n, name, src))
                });
                declarator.map(|d| match d.child_by_field_name("name") {
                    Some(n) if n.kind() == "identifier" => ScopeDecl::Declarator(d),
                    _ => ScopeDecl::Other,
                })
            }
            "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "abstract_class_declaration" => decl
                .child_by_field_name("name")
                .is_some_and(|n| text(n, src) == name)
                .then_some(ScopeDecl::Other),
            _ => None,
        };
        let Some(found) = found else {
            continue;
        };
        seen = true;
        if stmt.end_byte() <= at {
            before = Some(found);
        }
    }
    match before {
        Some(b) => b,
        None if seen => ScopeDecl::Other,
        None => ScopeDecl::Unbound,
    }
}

/// The parameter of the function-like `func` that binds `name` as a plain
/// identifier: a `required_parameter` / `optional_parameter` whose pattern is
/// the name, or an arrow's single `parameter`.
fn param_declaring<'t>(func: TsNode<'t>, name: &str, src: &[u8]) -> Option<TsNode<'t>> {
    if !matches!(
        func.kind(),
        "arrow_function"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "function_expression"
            | "generator_function"
    ) {
        return None;
    }
    if let Some(p) = func.child_by_field_name("parameter") {
        return (p.kind() == "identifier" && text(p, src) == name).then_some(p);
    }
    let params = func.child_by_field_name("parameters")?;
    let mut cursor = params.walk();
    params.named_children(&mut cursor).find(|p| {
        matches!(p.kind(), "required_parameter" | "optional_parameter")
            && p.child_by_field_name("pattern")
                .is_some_and(|pat| pat.kind() == "identifier" && text(pat, src) == name)
    })
}

/// The value of the property `key` of an object literal: a `pair`'s value,
/// or the shorthand identifier itself (`{ json }`).
fn object_property<'t>(obj: TsNode<'t>, key: &str, src: &[u8]) -> Option<TsNode<'t>> {
    if obj.kind() != "object" {
        return None;
    }
    let mut cursor = obj.walk();
    obj.named_children(&mut cursor).find_map(|m| match m.kind() {
        "pair" if m
            .child_by_field_name("key")
            .and_then(|k| ts_property_key(k, src))
            .is_some_and(|k| k == key) =>
        {
            m.child_by_field_name("value")
        }
        "shorthand_property_identifier" if text(m, src) == key => Some(m),
        _ => None,
    })
}

/// DT.1d: a `fetch` init object's `body: JSON.stringify(<expr>)` as the
/// request ([`body_shape`] of `<expr>`); any other body (a string, a
/// FormData, a variable) is none.
fn fetch_request(init: TsNode, src: &[u8]) -> Option<(Shape, WireSource)> {
    let value = object_property(init, "body", src)?;
    if value.kind() != "call_expression" {
        return None;
    }
    let callee = value.child_by_field_name("function")?;
    if callee.kind() != "member_expression" || collapse_ws(text(callee, src)) != "JSON.stringify" {
        return None;
    }
    let arg = value.child_by_field_name("arguments")?.named_child(0)?;
    body_shape(arg, src).map(|(shape, _)| (shape, WireSource::Fetch))
}

/// DT.1d: a `fetch` call's response type: `(await fetch(..)).json() as T`,
/// or, on `const r = await fetch(..)`, the first `r.json()` after the call in
/// the same callable typed by [`json_result_type`].
fn fetch_response(call: TsNode, src: &[u8]) -> Option<Shape> {
    let mut node = call;
    let mut awaited = false;
    while let Some(p) = node.parent() {
        match p.kind() {
            "await_expression" => awaited = true,
            "parenthesized_expression" | "non_null_expression" => {}
            _ => break,
        }
        node = p;
    }
    if !awaited {
        return None;
    }
    let parent = node.parent()?;
    match parent.kind() {
        "member_expression" if parent.child_by_field_name("object") == Some(node) => {
            let json_call = parent.parent()?;
            let is_json = parent
                .child_by_field_name("property")
                .is_some_and(|p| text(p, src) == "json")
                && json_call.kind() == "call_expression"
                && json_call.child_by_field_name("function") == Some(parent);
            if is_json { json_result_type(json_call, src) } else { None }
        }
        "variable_declarator" if parent.child_by_field_name("value") == Some(node) => {
            let name = parent.child_by_field_name("name").filter(|n| n.kind() == "identifier")?;
            let name = text(name, src);
            let scope = enclosing_callable(call);
            let mut stack = vec![scope];
            let mut json_calls = Vec::new();
            while let Some(n) = stack.pop() {
                if n.kind() == "call_expression"
                    && n.start_byte() >= call.end_byte()
                    && let Some(f) = n.child_by_field_name("function")
                    && f.kind() == "member_expression"
                    && f.child_by_field_name("object")
                        .is_some_and(|o| o.kind() == "identifier" && text(o, src) == name)
                    && f.child_by_field_name("property").is_some_and(|p| text(p, src) == "json")
                {
                    json_calls.push(n);
                }
                let mut cursor = n.walk();
                stack.extend(n.named_children(&mut cursor));
            }
            json_calls.sort_by_key(|n| n.start_byte());
            json_calls.into_iter().find_map(|jc| json_result_type(jc, src))
        }
        _ => None,
    }
}

/// The type a `.json()` call's result is given: climbing through `await`,
/// parentheses and `!`, an `as T` / `satisfies T` (`Promise<T>` unwrapped to
/// `T`), or the annotation of the `const` / `let` it initialises.
fn json_result_type(json_call: TsNode, src: &[u8]) -> Option<Shape> {
    let mut node = json_call;
    loop {
        let p = node.parent()?;
        match p.kind() {
            "await_expression" | "parenthesized_expression" | "non_null_expression" => node = p,
            "as_expression" | "satisfies_expression" => {
                if first_named(p) != Some(node) {
                    return None;
                }
                return Some(ts_type_shape(unwrap_promise(asserted_type(p)?, src), src));
            }
            "variable_declarator" => {
                if p.child_by_field_name("value") != Some(node) {
                    return None;
                }
                let ty = p.child_by_field_name("type").and_then(first_named)?;
                return Some(ts_type_shape(unwrap_promise(ty, src), src));
            }
            _ => return None,
        }
    }
}

/// `T` for a `Promise<T>` type node, else the node itself.
fn unwrap_promise<'t>(ty: TsNode<'t>, src: &[u8]) -> TsNode<'t> {
    if ty.kind() == "generic_type"
        && ty.child_by_field_name("name").is_some_and(|n| text(n, src) == "Promise")
        && let Some(arg) = ty.child_by_field_name("type_arguments").and_then(first_named)
    {
        return arg;
    }
    ty
}

/// The innermost function-like node enclosing `n`, else the tree's root.
fn enclosing_callable(n: TsNode) -> TsNode {
    let mut cur = n;
    while let Some(p) = cur.parent() {
        if matches!(
            p.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "function_expression"
                | "generator_function"
                | "arrow_function"
                | "method_definition"
        ) {
            return p;
        }
        cur = p;
    }
    cur
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

// ============================================================================
// DT.1k: GraphQL client result types
// ============================================================================
//
// The TypeScript type a GraphQL client call reads its result as, and the
// document it sends, as a MESSAGE_TYPE cell (`form` `graphql_result`, the
// `document` identifier, the type's `shape`) on the callable issuing the call.
// The models pass (graph/src/resolvers/models.rs, DT.1k) resolves the
// document to its `gql` const, the operation minted in that const's span and
// the schema type each result key's root field returns, and joins the model
// at each key with that type (`graphql_model`).
//
// Callee shapes, the type argument always the call's first (on the call node,
// also for `await recv.m<T>(..)`, [`awaited_generic_callee`]):
//
// - `<recv>.query|mutate|watchQuery|subscribe<T>({ query|mutation|document:
//   DOC, .. })` (Apollo Client, apollo-angular): the object-argument key is
//   the gate, so an RxJS `obs.subscribe<T>(next)` or a SQL client's
//   `db.query<T>(sql)` writes nothing. No import gate: the client is usually
//   a shared instance imported from the app's own module.
// - `useQuery|useLazyQuery|useMutation|useSubscription|useSuspenseQuery<T>(
//   DOC | { query: DOC })`, the hook a named import binds from a GraphQL
//   client module, or `ns.useQuery<T>(..)` through a namespace import of one
//   (graphql-codegen's generated hooks).
// - urql's `<recv>.query|mutation|subscription<T>(DOC, ..)`, in a file
//   importing urql or `@urql/*`.
//
// DOC is an identifier or `ns.NAME`; a document built inline (`gql`...``)
// or by string concatenation names no const and writes nothing.
// Fired-on: `[ts-wire] graphql_results=<n> file=<path>`, per file with one.

/// DT.1k: Apollo / apollo-angular client methods whose options object names
/// the document.
const GQL_CLIENT_METHODS: &[&str] = &["query", "mutate", "watchQuery", "subscribe"];
/// DT.1k: the options keys a client method reads its document from.
const GQL_DOCUMENT_KEYS: &[&str] = &["query", "mutation", "document"];
/// DT.1k: the hooks (Apollo Client React, urql) typed by their result.
const GQL_HOOKS: &[&str] = &["useQuery", "useLazyQuery", "useMutation", "useSubscription", "useSuspenseQuery"];
/// DT.1k: urql client methods taking the document positionally.
const URQL_METHODS: &[&str] = &["query", "mutation", "subscription"];

/// DT.1k: a module a GraphQL client call's hook or client comes from:
/// `@apollo/client` (and its subpaths, `@apollo/client/core`), `apollo-angular`
/// (and subpaths), `urql` and `@urql/*`.
fn is_graphql_client_module(m: &str) -> bool {
    m == "@apollo/client"
        || m.starts_with("@apollo/client/")
        || m == "apollo-angular"
        || m.starts_with("apollo-angular/")
        || is_urql_module(m)
}

/// DT.1k: `urql` or an `@urql/*` package.
fn is_urql_module(m: &str) -> bool {
    m == "urql" || m.starts_with("@urql/")
}

/// DT.1k: which import a candidate needs ([`attach_graphql_results`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum GqlGate {
    /// The options-object key already gated it.
    Options,
    /// A named import of a [`GQL_HOOKS`] hook from a GraphQL client module
    /// binds this local name.
    Hook(String),
    /// A namespace import of a GraphQL client module binds this name.
    Namespace(String),
    /// The file imports urql.
    Urql,
}

/// DT.1k: one typed GraphQL client call, before its gate.
#[derive(Debug, Clone)]
struct GraphqlResultCand {
    from: NodeId,
    gate: GqlGate,
    wire: wire::ServerWire,
}

/// DT.1k: the document a client call names: an identifier (a shorthand
/// option `{ query }` included), or `ns.NAME` on a namespace identifier.
fn gql_document_name(arg: TsNode, src: &[u8]) -> Option<String> {
    match arg.kind() {
        "identifier" | "shorthand_property_identifier" => Some(text(arg, src).to_string()),
        "member_expression" => {
            let (object, property) = (arg.child_by_field_name("object")?, arg.child_by_field_name("property")?);
            (object.kind() == "identifier" && property.kind() == "property_identifier")
                .then(|| format!("{}.{}", text(object, src), text(property, src)))
        }
        _ => None,
    }
}

/// DT.1k: the document an options object names under one of
/// [`GQL_DOCUMENT_KEYS`] (the first key present, in that order).
fn gql_options_document(arg: TsNode, src: &[u8]) -> Option<String> {
    GQL_DOCUMENT_KEYS
        .iter()
        .find_map(|k| object_property(arg, k, src))
        .and_then(|v| gql_document_name(v, src))
}

/// DT.1k: a typed GraphQL client call (see the section doc) as a candidate
/// `graphql_result` site of `from`.
fn try_detect_graphql_result(call: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let (callee, type_args) = match awaited_generic_callee(call) {
        Some(found) => found,
        None => match (call.child_by_field_name("function"), call.child_by_field_name("type_arguments")) {
            (Some(f), Some(t)) => (f, t),
            _ => return,
        },
    };
    let Some(ty) = first_named(type_args) else {
        return;
    };
    let Some(first) = call.child_by_field_name("arguments").and_then(first_named) else {
        return;
    };
    let found = match callee.kind() {
        "member_expression" => {
            let (Some(object), Some(property)) =
                (callee.child_by_field_name("object"), callee.child_by_field_name("property"))
            else {
                return;
            };
            let method = text(property, src);
            if GQL_CLIENT_METHODS.contains(&method)
                && let Some(doc) = gql_options_document(first, src)
            {
                Some((doc, GqlGate::Options))
            } else if URQL_METHODS.contains(&method)
                && let Some(doc) = gql_document_name(first, src)
            {
                Some((doc, GqlGate::Urql))
            } else if GQL_HOOKS.contains(&method) && object.kind() == "identifier" {
                gql_document_name(first, src)
                    .or_else(|| gql_options_document(first, src))
                    .map(|doc| (doc, GqlGate::Namespace(text(object, src).to_string())))
            } else {
                None
            }
        }
        "identifier" => gql_document_name(first, src)
            .or_else(|| gql_options_document(first, src))
            .map(|doc| (doc, GqlGate::Hook(text(callee, src).to_string()))),
        _ => None,
    };
    let Some((document, gate)) = found else {
        return;
    };
    let raw = collapse_ws(text(ty, src));
    let mut w = wire::ServerWire::new(wire::WireForm::GraphqlResult, &raw, line_at(call).saturating_add(1), ts_type_shape(ty, src));
    w.document = Some(document);
    acc.graphql_results.push(GraphqlResultCand { from, gate, wire: w });
}

/// DT.1k: the candidates whose gate the file's imports open, each a
/// MESSAGE_TYPE cell on its callable (sorted by line, one per distinct
/// payload), and the `[ts-wire] graphql_results=` marker.
fn attach_graphql_results(acc: &mut Acc) {
    if acc.graphql_results.is_empty() {
        return;
    }
    let mut hooks: HashSet<&str> = HashSet::new();
    let mut namespaces: HashSet<&str> = HashSet::new();
    let mut urql = false;
    for imp in &acc.imports {
        match &imp.target {
            ImportTarget::Symbol { module, name, alias, .. } if is_graphql_client_module(module) => {
                urql |= is_urql_module(module);
                if GQL_HOOKS.contains(&name.as_str()) {
                    hooks.insert(alias.as_deref().unwrap_or(name.as_str()));
                }
            }
            ImportTarget::Module { path, alias } if is_graphql_client_module(path) => {
                urql |= is_urql_module(path);
                if let Some(a) = alias {
                    namespaces.insert(a.as_str());
                }
            }
            _ => {}
        }
    }
    let mut kept: Vec<(NodeId, wire::ServerWire)> = acc
        .graphql_results
        .iter()
        .filter(|c| match &c.gate {
            GqlGate::Options => true,
            GqlGate::Hook(h) => hooks.contains(h.as_str()),
            GqlGate::Namespace(ns) => namespaces.contains(ns.as_str()),
            GqlGate::Urql => urql,
        })
        .map(|c| (c.from, c.wire.clone()))
        .collect();
    acc.graphql_results.clear();
    kept.sort_by_key(|(_, w)| w.line);
    let mut n = 0usize;
    for (from, w) in kept {
        let cell = w.cell();
        let Some(node) = acc.nodes.iter_mut().find(|node| node.id == from) else {
            continue;
        };
        if !node.cells.contains(&cell) {
            node.cells.push(cell);
            n += 1;
        }
    }
    if n > 0 {
        eprintln!("[ts-wire] graphql_results={n} file={}", acc.file_rel);
    }
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
    record_receiver_types(&mut acc, &mut out);
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

    // CH.4: a method passed by value is USED by the member that passes it, so
    // a handler registered only as a callback has a carry in-edge. Same class
    // by construction (`class_methods` is keyed by the enclosing class, so an
    // inherited method records nothing); METHOD targets only, since CH.2 maps
    // a STATE_VAR there too and a state read is never a USES, and no `get` /
    // `set` accessor (`this.isDesktop` runs a getter: DT.5g's `accessor_read`
    // CALLS below); never a self-reference (`removeEventListener(..,
    // this.onX)` inside `onX`).
    let mut method_uses = 0usize;
    let mut method_targets: HashSet<NodeId> = HashSet::new();
    let mut accessor_calls = AccessorCalls::default();
    for (from, class, name, row) in std::mem::take(&mut acc.this_refs) {
        let Some(&to) = acc.class_methods.get(&(class, name)) else {
            continue;
        };
        if to == from || out.nav.kind_by_id.get(&to) != Some(&node_kind::METHOD) {
            continue;
        }
        if acc.accessors.contains(&to) {
            accessor_calls.add(from, to, row, false);
            continue;
        }
        let ev = evidence::Evidence::emitter(format!("parser:{}", lang_tag(&acc.file_rel)))
            .rule("method_ref")
            .line(row);
        out.edges.push(
            Edge::new(from, to, edge_category::USES, Confidence::Strong).with_cell(ev.to_cell()),
        );
        method_uses += 1;
        method_targets.insert(to);
    }
    if method_uses > 0 {
        eprintln!(
            "[ts-method-refs] uses={} targets={} file={}",
            method_uses,
            method_targets.len(),
            acc.file_rel
        );
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

    // DT.5g: a `this.<accessor>` read or write runs the getter / setter, a
    // CALLS of it (rule `accessor_read` / `accessor_write`): the value reads
    // CH.4's loop set aside, then the member reads and writes. Same class by
    // construction, never a self-reference. After CH.3b's closure, which is
    // read over the call edges it was specified on.
    for (from, class, name, row, write) in std::mem::take(&mut acc.accessor_refs) {
        let Some(&to) = acc.class_methods.get(&(class, name)) else {
            continue;
        };
        if to != from && acc.accessors.contains(&to) {
            accessor_calls.add(from, to, row, write);
        }
    }
    let lang = lang_tag(&acc.file_rel);
    let accessor_edges = accessor_calls.edges.len();
    for (from, to, row, write) in accessor_calls.edges {
        let rule = if write { "accessor_write" } else { "accessor_read" };
        let ev = evidence::Evidence::emitter(format!("parser:{lang}"))
            .rule(rule)
            .line(row);
        out.edges.push(
            Edge::new(from, to, edge_category::CALLS, Confidence::Strong).with_cell(ev.to_cell()),
        );
    }

    // DT.5g: a bare identifier passed by value names the symbol it binds: a
    // USES from the scope that passes it. Not when a local, a parameter or a
    // nested declaration of that scope shadows the name, and not an `inject`
    // import binding's argument (DI). An in-file top-level FUNCTION, CLASS,
    // module STATE_VAR or ENUM is an edge now (rule `value_ref`); a named
    // import binding is a Bare USES ref the graph crate binds through the
    // importer's import bindings (`graph:refs`, rule `import_binding`); one
    // naming a third-party package stays an unresolved ref, as LA.30c's
    // member refs do. A default or namespace import binds a MODULE and is
    // skipped, as is every name neither declared in the file nor imported (a
    // global, an undeclared name), so none of those reaches the persisted
    // unresolved refs. A self-reference is dropped.
    let value_refs = std::mem::take(&mut acc.value_refs);
    let candidates = value_refs.len();
    let (mut in_file, mut imported) = (0usize, 0usize);
    {
        let inject_bindings = inject_import_bindings(&out.imports);
        let named_imports = named_import_bindings(&out.imports);
        for vr in value_refs {
            let is_inject =
                |c: &Option<String>| c.as_deref().is_some_and(|c| inject_bindings.contains(c));
            let shadowed = out
                .nav
                .local_types
                .get(&vr.from)
                .is_some_and(|l| l.contains_key(&vr.name))
                || acc.nested_decls.contains(&(vr.from, vr.name.clone()));
            // DT.5k: an `inject(T)` that is no field's whole initialiser (a
            // guard's `const auth = inject(AuthService)`, an inline
            // `inject(CountriesService).list()`) is DI from the scope that
            // runs it: an INJECTS of `T`, bound like A7.1's field form.
            if !shadowed
                && let Some(row) =
                    vr.sites.iter().filter(|(_, c)| is_inject(c)).map(|(r, _)| *r).min()
            {
                let in_file = acc.classes.get(&vr.name).or_else(|| acc.const_vars.get(&vr.name));
                if let Some(&to) = in_file {
                    // A class's own member injecting it vouches for nothing;
                    // only the INJECTS is skipped, so the scope's non-inject
                    // sites of the name below keep their USES.
                    if !within(&out.nav, vr.from, to) {
                        let ev = evidence::Evidence::emitter(format!("parser:{lang}"))
                            .rule("inject_call")
                            .line(row);
                        out.edges.push(
                            Edge::new(vr.from, to, edge_category::INJECTS, Confidence::Strong)
                                .with_cell(ev.to_cell()),
                        );
                        acc.receiver_stats.inject_refs += 1;
                    }
                } else if named_imports.contains(vr.name.as_str())
                    && let Some(module_id) = acc.module_id
                {
                    out.refs.push(UnresolvedRef {
                        from: vr.from,
                        from_module: module_id,
                        qualifier: CallQualifier::Bare(vr.name.clone()),
                        category: edge_category::INJECTS,
                        line: row,
                    });
                    acc.receiver_stats.inject_refs += 1;
                }
            }
            let Some(row) = vr
                .sites
                .iter()
                .filter(|(_, c)| !is_inject(c))
                .map(|(r, _)| *r)
                .min()
            else {
                continue;
            };
            if shadowed {
                continue;
            }
            let in_file_target = acc
                .module_functions
                .get(&vr.name)
                .or_else(|| acc.classes.get(&vr.name))
                .or_else(|| acc.const_vars.get(&vr.name))
                .or_else(|| acc.enums.get(&vr.name))
                .copied();
            if let Some(to) = in_file_target {
                if to == vr.from {
                    continue;
                }
                let ev = evidence::Evidence::emitter(format!("parser:{lang}"))
                    .rule("value_ref")
                    .line(row);
                out.edges.push(
                    Edge::new(vr.from, to, edge_category::USES, Confidence::Strong)
                        .with_cell(ev.to_cell()),
                );
                in_file += 1;
            } else if named_imports.contains(vr.name.as_str())
                && let Some(module_id) = acc.module_id
            {
                out.refs.push(UnresolvedRef {
                    from: vr.from,
                    from_module: module_id,
                    qualifier: CallQualifier::Bare(vr.name),
                    category: edge_category::USES,
                    line: row,
                });
                imported += 1;
            }
        }
    }
    let vs = acc.value_stats;
    if candidates + vs.const_inits + vs.decorators + accessor_edges > 0 {
        eprintln!(
            "[ts-value-refs] refs={candidates} in_file={in_file} imported={imported} \
             const_inits={} decorators={} accessor_calls={accessor_edges} file={}",
            vs.const_inits, vs.decorators, acc.file_rel
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
        // DT.5k: `T.X` on a CLASS of this file, read outside `T` (its own
        // members reading a static vouch for nothing), USES `T`.
        if let Some(&class) = acc.classes.get(&base) {
            let local = out.nav.local_types.get(&from).is_some_and(|l| l.contains_key(&base));
            if !local && !within(&out.nav, from, class) {
                let ev = evidence::Evidence::emitter(format!("parser:{lang}"))
                    .rule("static_member")
                    .line(line);
                out.edges.push(
                    Edge::new(from, class, edge_category::USES, Confidence::Strong)
                        .with_cell(ev.to_cell()),
                );
                acc.receiver_stats.static_reads += 1;
            }
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

    // DT.5k: a member access on a receiver of known type may run a getter
    // or setter of that type's class: kept as a MemberRead fact for the
    // graph (a MemberWrite for an assignment target), which binds it only
    // to an accessor. A `this.<field>` needs a
    // declared field type (A7.1's `inject(T)` fields are recorded above), a
    // bare name a typed local or parameter of the scope, any other receiver
    // a ReceiverType fact.
    for mr in std::mem::take(&mut acc.member_reads) {
        let typed = match &mr.shape {
            RecvShape::ThisField(f) => mr.class.is_some_and(|c| {
                out.nav.field_types.get(&c).and_then(|t| t.get(f)).is_some_and(|t| !t.is_empty())
            }),
            RecvShape::Local => out
                .nav
                .local_types
                .get(&mr.from)
                .and_then(|l| l.get(&mr.receiver))
                .is_some_and(|t| !t.is_empty()),
            RecvShape::Expr => out.nav.nav_facts.get(&mr.from).is_some_and(|facts| {
                facts.iter().any(|f| {
                    matches!(f, NavFact::ReceiverType { receiver, .. } if *receiver == mr.receiver)
                })
            }),
        };
        if !typed {
            continue;
        }
        let fact = if mr.write {
            NavFact::MemberWrite { receiver: mr.receiver, name: mr.name, line: mr.row }
        } else {
            NavFact::MemberRead { receiver: mr.receiver, name: mr.name, line: mr.row }
        };
        out.nav.record_fact(mr.from, fact);
        acc.receiver_stats.member_reads += 1;
    }

    let mut endpoint_nav_seen: std::collections::HashSet<NodeId> =
        std::collections::HashSet::new();
    let repo = acc.repo.expect("Acc.repo set in parse_file");
    let mut wire_stats = WireStats::default();
    for cand in acc.endpoints {
        if let Some(req) = cand.requires_import_alias.as_ref()
            && !alias_set.contains(req.as_str())
        {
            continue;
        }
        // DT.1d: which argument is the body depends on the client: the
        // receiver field's declared type (A7.1's `inject(T)` fields are on
        // `out.nav` by now) or the module the alias is imported from.
        let receiver_type = cand
            .class
            .zip(cand.receiver_field.as_deref())
            .and_then(|(c, f)| out.nav.field_types.get(&c)?.get(f).map(String::as_str));
        let alias_module = cand
            .requires_import_alias
            .as_deref()
            .and_then(|a| import_module(&out.imports, a));
        let request = request_of(&cand, receiver_type, alias_module);
        wire_stats.record(&cand, &request);
        let request = match request {
            RequestRead::Body(r) => r.map(|(s, _)| s.clone()),
            RequestRead::UnknownClient | RequestRead::NoBody => None,
        };
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
        let cell = endpoint_hit_cell(&cand, wrapper_of.as_deref(), request.as_ref());
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
    wire_stats.report(&acc.file_rel);

    let rs = acc.receiver_stats;
    if rs.any() {
        eprintln!(
            "[ts-receivers] query_fields={} query_receivers={} inject_receivers={} \
             inject_refs={} static_reads={} call_locals={} member_reads={} accessors={} file={}",
            rs.query_fields,
            rs.query_receivers,
            rs.inject_receivers,
            rs.inject_refs,
            rs.static_reads,
            rs.call_locals,
            rs.member_reads,
            rs.accessors,
            acc.file_rel
        );
    }

    Ok(out)
}

/// DT.5k: true when `id` is `class` or a node under it (`parent_of`).
fn within(nav: &CodeNav, mut id: NodeId, class: NodeId) -> bool {
    loop {
        if id == class {
            return true;
        }
        match nav.parent_of.get(&id) {
            Some(&p) => id = p,
            None => return false,
        }
    }
}

/// DT.5k: the receiver texts this file types for the graph, each a
/// [`NavFact::ReceiverType`] on the scope that writes it: a read of a
/// signal query field (`this.form()`, `this.form()!`) whose factory is the
/// imported `viewChild` / `contentChild`, from a call site's receiver or a
/// member read's; and an `inject(T)` receiver whose callee is the `inject`
/// import. Every import is read by now.
fn record_receiver_types(acc: &mut Acc, out: &mut FileParse) {
    let queries = query_import_bindings(&out.imports);
    let injects = inject_import_bindings(&out.imports);
    let mut sites: Vec<(NodeId, Option<NodeId>, &str)> = Vec::new();
    for uc in &acc.unresolved {
        if let CallQualifier::ComplexReceiver { receiver, .. } = &uc.qualifier {
            sites.push((uc.from, uc.enclosing_class, receiver.as_str()));
        }
    }
    for mr in &acc.member_reads {
        if mr.shape == RecvShape::Expr {
            sites.push((mr.from, mr.class, mr.receiver.as_str()));
        }
    }
    for (from, class, receiver) in sites {
        let Some(field) = signal_read_field(receiver) else {
            continue;
        };
        let Some((binding, ty)) = class.and_then(|c| acc.signal_queries.get(&(c, field.to_string())))
        else {
            continue;
        };
        if !queries.contains(binding.as_str()) {
            continue;
        }
        let fact = NavFact::ReceiverType { receiver: receiver.to_string(), ty: ty.clone() };
        if !out.nav.nav_facts.get(&from).is_some_and(|f| f.contains(&fact)) {
            out.nav.record_fact(from, fact);
            acc.receiver_stats.query_receivers += 1;
        }
    }
    acc.receiver_stats.query_fields = acc
        .signal_queries
        .values()
        .filter(|(b, _)| queries.contains(b.as_str()))
        .count();
    for (scope, receiver, callee, ty) in std::mem::take(&mut acc.inject_receivers) {
        if !injects.contains(callee.as_str()) {
            continue;
        }
        let fact = NavFact::ReceiverType { receiver, ty };
        if !out.nav.nav_facts.get(&scope).is_some_and(|f| f.contains(&fact)) {
            out.nav.record_fact(scope, fact);
            acc.receiver_stats.inject_receivers += 1;
        }
    }
}

/// DT.5g: the accessor CALLS of one file, one per `(from, accessor, write)`
/// in first-seen order, each at its smallest source row.
#[derive(Default)]
struct AccessorCalls {
    edges: Vec<(NodeId, NodeId, u32, bool)>,
    seen: HashMap<(NodeId, NodeId, bool), usize>,
}

impl AccessorCalls {
    fn add(&mut self, from: NodeId, to: NodeId, row: u32, write: bool) {
        match self.seen.get(&(from, to, write)) {
            Some(&i) => {
                if let Some(e) = self.edges.get_mut(i) {
                    e.2 = e.2.min(row);
                }
            }
            None => {
                self.seen.insert((from, to, write), self.edges.len());
                self.edges.push((from, to, row, write));
            }
        }
    }
}

/// DT.5g: the local names a named import binds (`a` for `import { a }`, `b`
/// for `import { a as b }`), the import bindings a value reference may name.
/// A default import (`import X from`) and a namespace import (`import * as
/// ns`) bind a MODULE and are left out.
fn named_import_bindings(imports: &[ImportStmt]) -> HashSet<&str> {
    imports
        .iter()
        .filter_map(|imp| match &imp.target {
            ImportTarget::Symbol { name, alias, .. } if name != DEFAULT_EXPORT => {
                Some(alias.as_deref().unwrap_or(name.as_str()))
            }
            _ => None,
        })
        .collect()
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

    /// DF.20: the one ENDPOINT_HIT of the single ENDPOINT with `path`.
    fn sole_hit(parse: &FileParse, method: &str, path: &str) -> serde_json::Value {
        let p = endpoint_payloads(parse, endpoint_id(repo(), method, path));
        assert_eq!(p.len(), 1, "{path}: {p:?}");
        p[0].clone()
    }

    /// DF.20: `${this.api}` whose one-`return` getter returns a config path
    /// is written into the template as that path; the ENDPOINT keeps its
    /// `${…}` path until the engine's fold, and `alias` names the member,
    /// last.
    #[test]
    fn this_member_getter_substitutes() {
        let src = "\
export class TradeService {
    constructor(private http: any) {}
    private get api() { return environment.apiUrl; }
    list() { return this.http.get(`${this.api}/trades`); }
    one(id: string) { return this.http.get(`${this.api}/trades/${id}`); }
}
";
        let (parse, .., stats) =
            parse_file_stats(src, "src/trade.service.ts", "src::trade", repo()).unwrap();
        assert!(method_calls(&parse, "src::trade", "TradeService::list", "GET", "${…}/trades"));
        let hit = sole_hit(&parse, "GET", "${…}/trades");
        assert_eq!(hit["template"], "${environment.apiUrl}/trades");
        assert_eq!(hit["alias"], "this.api");
        assert_eq!(hit["confidence"], "medium");
        let raw: Vec<&str> = parse
            .nodes
            .iter()
            .filter(|n| n.id == endpoint_id(repo(), "GET", "${…}/trades"))
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) if c.kind == cell_type::ENDPOINT_HIT => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            raw.len() == 1 && raw[0].ends_with(r#","alias":"this.api"}"#),
            "alias is last: {raw:?}"
        );
        // A later path parameter stays as written.
        let hit = sole_hit(&parse, "GET", "${…}/trades/${…}");
        assert_eq!(hit["template"], "${environment.apiUrl}/trades/${id}");
        assert_eq!(stats.aliases, 2, "two call sites read a member alias");
    }

    /// DF.20: a `readonly` field holding a template of config paths
    /// substitutes its text, nested `${…}` and all.
    #[test]
    fn readonly_template_field_substitutes() {
        let src = "\
export class NotificationsService {
    private readonly base = `${environment.apiUrl}/notifications`;
    constructor(private http: any) {}
    unread() { return this.http.get(`${this.base}/unread`); }
}
";
        let parse = parse_file(src, "src/n.ts", "src::n", repo()).unwrap();
        let hit = sole_hit(&parse, "GET", "${…}/unread");
        assert_eq!(hit["template"], "${environment.apiUrl}/notifications/unread");
        assert_eq!(hit["alias"], "this.base");
    }

    /// DF.20: `environment.apiUrl.replace('/api', '') + '/admin'` (a field or
    /// a getter) becomes `${environment.apiUrl.replace('/api', '')}/admin`,
    /// whatever the source's quotes and parentheses.
    #[test]
    fn replace_expression_substitutes() {
        let src = "\
export class AdminService {
    private readonly base = environment.apiUrl.replace('/api', '') + '/admin';
    private get adminBase() { return (environment.apiUrl).replace(\"/api\", '') + `/admin`; }
    constructor(private http: any) {}
    stats() { return this.http.get(`${this.base}/stats`); }
    users() { return this.http.get(`${this.adminBase}/users`); }
}
";
        let parse = parse_file(src, "src/admin.ts", "src::admin", repo()).unwrap();
        let hit = sole_hit(&parse, "GET", "${…}/stats");
        assert_eq!(
            hit["template"],
            "${environment.apiUrl.replace('/api', '')}/admin/stats"
        );
        assert_eq!(hit["alias"], "this.base");
        let hit = sole_hit(&parse, "GET", "${…}/users");
        assert_eq!(
            hit["template"],
            "${environment.apiUrl.replace('/api', '')}/admin/users"
        );
        assert_eq!(hit["alias"], "this.adminBase");
    }

    /// DF.20: a member that is no config expression keeps `${this.<m>}` and
    /// names no alias: a call, `this.x`, a non-readonly field, an env
    /// fallback, a ternary, a literal, a `.replace` with a non-literal or a
    /// `$` pattern, a read inside a `function` callback, and a URL builder's
    /// argument.
    #[test]
    fn non_config_member_unchanged() {
        let src = "\
export class S {
    private base = environment.apiUrl;
    private readonly cfgBase = this.cfg.get('x');
    private readonly selfBase = this.root + '/x';
    private readonly envBase = process.env.API_URL || 'http://localhost';
    private readonly tern = environment.production ? '/p' : '/d';
    private readonly lit = '/api/lit';
    private readonly rep = environment.apiUrl.replace(/api/, '');
    private readonly dollar = environment.apiUrl.replace('/api', '$&');
    private readonly env2 = process.env.API + '/x';
    private readonly ok = environment.apiUrl;
    constructor(private http: any, private cfg: any, private urls: any) {}
    a() { return this.http.get(`${this.base}/a`); }
    b() { return this.http.get(`${this.cfgBase}/b`); }
    c() { return this.http.get(`${this.selfBase}/c`); }
    d() { return this.http.get(`${this.envBase}/d`); }
    e() { return this.http.get(`${this.tern}/e`); }
    f() { return this.http.get(`${this.lit}/f`); }
    g() { return this.http.get(`${this.rep}/g`); }
    h() { return this.http.get(`${this.dollar}/h`); }
    i() { return this.http.get(`${this.env2}/i`); }
    j(xs: any[]) { xs.forEach(function () { this.http.get(`${this.ok}/j`); }); }
    k() { return this.http.get(this.urls.buildApiUrl(`${this.ok}/k`)); }
}
";
        let (parse, .., stats) = parse_file_stats(src, "src/s.ts", "src::s", repo()).unwrap();
        for (m, member) in [
            ("a", "base"),
            ("b", "cfgBase"),
            ("c", "selfBase"),
            ("d", "envBase"),
            ("e", "tern"),
            ("f", "lit"),
            ("g", "rep"),
            ("h", "dollar"),
            ("i", "env2"),
            ("j", "ok"),
            ("k", "ok"),
        ] {
            let hit = sole_hit(&parse, "GET", &format!("${{…}}/{m}"));
            assert_eq!(hit["template"], format!("${{this.{member}}}/{m}"), "{m}");
            assert!(hit.get("alias").is_none(), "{m}: {hit}");
        }
        assert_eq!(stats.aliases, 0);
    }

    /// DT.4: the ENDPOINT_HIT JSON texts of the ENDPOINT with `path`.
    fn hit_texts(parse: &FileParse, method: &str, path: &str) -> Vec<String> {
        parse
            .nodes
            .iter()
            .filter(|n| n.id == endpoint_id(repo(), method, path))
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) if c.kind == cell_type::ENDPOINT_HIT => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    /// DT.4: no ENDPOINT of `parse` has a qname in `qnames`.
    fn no_endpoint(parse: &FileParse, qnames: &[&str]) {
        let got = endpoint_qnames(parse);
        for q in qnames {
            assert!(!got.iter().any(|g| g == q), "{q} minted: {got:?}");
        }
    }

    /// DT.4: `const q = s ? `?status=${s}` : ''` read at the site drops the
    /// trailing `${q}` from path and template: the literal path, Strong, no
    /// template, `query` = the const, written last.
    #[test]
    fn trailing_const_query_is_dropped() {
        let src = "\
export class SmrService {
    constructor(private http: any) {}
    list(s: string) {
        const q = s ? `?status=${encodeURIComponent(s)}` : '';
        return this.http.get(`/admin/smr${q}`);
    }
}
";
        let (parse, .., stats) =
            parse_file_stats(src, "src/smr.service.ts", "src::smr", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::smr",
            "SmrService::list",
            "GET",
            "/admin/smr"
        ));
        no_endpoint(&parse, &["endpoint:GET:/admin/smr${…}"]);
        let hit = sole_hit(&parse, "GET", "/admin/smr");
        assert!(hit.get("template").is_none(), "{hit}");
        assert!(hit.get("raw").is_none(), "{hit}");
        assert_eq!(hit["confidence"], "strong");
        assert_eq!(hit["query"], "q");
        let texts = hit_texts(&parse, "GET", "/admin/smr");
        assert!(
            texts[0].ends_with(r#","query":"q"}"#),
            "query is last: {texts:?}"
        );
        assert_eq!(stats.query_suffixes, 1);
    }

    /// DT.4 (examples-glia `${this.base}${query}`): a config-aliased readonly
    /// base keeps DF.20's template and alias; only the query goes.
    #[test]
    fn member_base_with_query() {
        let src = "\
export class OrderService {
    private readonly base = `${environment.apiUrl}/orders`;
    constructor(private http: any) {}
    list(status?: string) {
        const query = status ? `?status=${status}` : '';
        return this.http.get(`${this.base}${query}`);
    }
}
";
        let (parse, .., stats) =
            parse_file_stats(src, "src/order.service.ts", "src::order", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::order",
            "OrderService::list",
            "GET",
            "${…}"
        ));
        no_endpoint(&parse, &["endpoint:GET:${…}${…}"]);
        let hit = sole_hit(&parse, "GET", "${…}");
        assert_eq!(hit["template"], "${environment.apiUrl}/orders");
        assert_eq!(hit["alias"], "this.base");
        assert_eq!(hit["query"], "query");
        assert_eq!(hit["confidence"], "medium");
        let texts = hit_texts(&parse, "GET", "${…}");
        assert!(
            texts[0].ends_with(r#","alias":"this.base","query":"query"}"#),
            "{texts:?}"
        );
        assert_eq!((stats.aliases, stats.query_suffixes), (1, 1));
    }

    /// DT.4 (Kina smr.service.ts): a base built by a call is no config
    /// expression, so `${this.base}` stays; the query still goes.
    #[test]
    fn call_base_with_query() {
        let src = "\
export class SmrAdminService {
    private readonly base = adminBase();
    constructor(private http: any) {}
    list(status?: string) {
        const q = status ? `?status=${encodeURIComponent(status)}` : '';
        return this.http.get(`${this.base}/smr${q}`);
    }
}
";
        let parse = parse_file(src, "src/smr-admin.service.ts", "src::sa", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::sa",
            "SmrAdminService::list",
            "GET",
            "${…}/smr"
        ));
        no_endpoint(&parse, &["endpoint:GET:${…}/smr${…}"]);
        let hit = sole_hit(&parse, "GET", "${…}/smr");
        assert_eq!(hit["template"], "${this.base}/smr");
        assert_eq!(hit["query"], "q");
        assert!(hit.get("alias").is_none(), "{hit}");
    }

    /// DT.4: an inline optional query (`'?q=' + t`, a `?`-led template, a
    /// parenthesised ternary, an empty template branch) is `inline`.
    #[test]
    fn inline_ternary_query() {
        let src = "\
export class S {
    constructor(private http: any) {}
    a(term: string) { return this.http.get(`/admin/smr${term ? '?q=' + term : ''}`); }
    b(p: number) { return this.http.get(`/api/b${(p ? `?page=${p}` : \"\")}`); }
    c(p: number) { return this.http.get(`/api/c${p > 1 ? `?page=${p}` : ``}`); }
    d(x: string) { return this.http.get(`/api/d${x ? '?a=' + x + '&b=' + x : ('')}`); }
}
";
        let (parse, .., stats) = parse_file_stats(src, "src/s.ts", "src::s", repo()).unwrap();
        for (m, path) in [
            ("a", "/admin/smr"),
            ("b", "/api/b"),
            ("c", "/api/c"),
            ("d", "/api/d"),
        ] {
            assert!(
                method_calls(&parse, "src::s", &format!("S::{m}"), "GET", path),
                "{m}"
            );
            let hit = sole_hit(&parse, "GET", path);
            assert_eq!(hit["query"], "inline", "{m}");
            assert!(hit.get("template").is_none(), "{m}: {hit}");
        }
        assert_eq!(stats.query_suffixes, 4);
    }

    /// DT.4: a URL read through a `const` (CH.3a arm d) drops the query its
    /// template ends with too: the read only ever drops text.
    #[test]
    fn indirect_template_query_is_dropped() {
        let src = "\
export class S {
    constructor(private http: any) {}
    list(s: string) {
        const q = s ? `?s=${s}` : '';
        const url = `/api/list${q}`;
        return this.http.get(url);
    }
}
";
        let (parse, .., ea) = parse_file_stats(src, "src/s.ts", "src::s", repo()).unwrap();
        assert!(method_calls(
            &parse,
            "src::s",
            "S::list",
            "GET",
            "/api/list"
        ));
        assert_eq!(sole_hit(&parse, "GET", "/api/list")["query"], "q");
        assert_eq!((ea.local, ea.query_suffixes), (1, 1));
    }

    /// DT.4 CONTROL: a suffix const that is a path segment
    /// (`archived ? '/archived' : ''`) is part of the path: kept.
    #[test]
    fn path_suffix_const_is_kept() {
        let src = "\
export class ProductService {
    constructor(private http: any) {}
    list(archived: boolean) {
        const suffix = archived ? '/archived' : '';
        return this.http.get(`${environment.apiUrl}/products${suffix}`);
    }
}
";
        let (parse, .., stats) = parse_file_stats(src, "src/p.ts", "src::p", repo()).unwrap();
        let hit = sole_hit(&parse, "GET", "${…}/products${…}");
        assert_eq!(hit["template"], "${environment.apiUrl}/products${suffix}");
        assert!(hit.get("query").is_none(), "{hit}");
        assert_eq!(stats.query_suffixes, 0);
    }

    /// DT.4 CONTROL: a ternary with one query branch and one path branch is
    /// not a query: kept.
    #[test]
    fn mixed_branch_is_kept() {
        let src = "\
export class S {
    constructor(private http: any) {}
    m(c: boolean) {
        const s = c ? '?a=1' : '/x';
        return this.http.get(`/api/m${s}`);
    }
}
";
        let parse = parse_file(src, "src/s.ts", "src::s", repo()).unwrap();
        let hit = sole_hit(&parse, "GET", "/api/m${…}");
        assert_eq!(hit["template"], "/api/m${s}");
        assert!(hit.get("query").is_none(), "{hit}");
    }

    /// DT.4 CONTROL: no value data-flow: a parameter (Kina's
    /// `${BACKEND}${urlPath}`), a member, a call and a `?`-led string that is
    /// not a `const` declaration all keep their `${…}`.
    #[test]
    fn parameter_suffix_is_kept() {
        let src = "\
export class S {
    private readonly q = '?a=1';
    constructor(private http: any) {}
    p(url: string, q: string) { return this.http.get(`${url}${q}`); }
    m() { return this.http.get(`/api/m${this.q}`); }
    c(p: any) { return this.http.get(`/api/c${qs(p)}`); }
    o(o: boolean) { return this.http.get(`/api/o${o || '?x'}`); }
}
";
        let (parse, .., stats) = parse_file_stats(src, "src/s.ts", "src::s", repo()).unwrap();
        for (m, path) in [
            ("p", "${…}${…}"),
            ("m", "/api/m${…}"),
            ("c", "/api/c${…}"),
            ("o", "/api/o${…}"),
        ] {
            assert!(
                method_calls(&parse, "src::s", &format!("S::{m}"), "GET", path),
                "{m}"
            );
            assert!(sole_hit(&parse, "GET", path).get("query").is_none(), "{m}");
        }
        assert_eq!(stats.query_suffixes, 0);
    }

    /// DT.4 CONTROL: a trailing path parameter is unchanged (examples-glia
    /// `${this.base}/${id}`), as is a trailing const that is no query.
    #[test]
    fn path_param_unchanged() {
        let src = "\
export class OrderService {
    private readonly base = `${environment.apiUrl}/orders`;
    constructor(private http: any) {}
    get(id: string) { return this.http.get(`${this.base}/${id}`); }
    named() {
        const id = 'latest';
        return this.http.get(`/api/named/${id}`);
    }
}
";
        let parse = parse_file(src, "src/order.service.ts", "src::order", repo()).unwrap();
        let hit = sole_hit(&parse, "GET", "${…}/${…}");
        assert_eq!(hit["template"], "${environment.apiUrl}/orders/${id}");
        assert!(hit.get("query").is_none(), "{hit}");
        let hit = sole_hit(&parse, "GET", "/api/named/${…}");
        assert!(hit.get("query").is_none(), "{hit}");
    }

    /// DT.4 CONTROL: only the template's LAST child is a candidate (a query
    /// ends a URL), and a template that is nothing but the query keeps it.
    #[test]
    fn middle_query_const_is_kept() {
        let src = "\
export class S {
    constructor(private http: any) {}
    mid(c: boolean) {
        const q = c ? '?a=1' : '';
        return this.http.get(`/api/x${q}/y`);
    }
    only(c: boolean) {
        const q = c ? '?a=1' : '';
        return this.http.get(`${q}`);
    }
}
";
        let (parse, .., stats) = parse_file_stats(src, "src/s.ts", "src::s", repo()).unwrap();
        let hit = sole_hit(&parse, "GET", "/api/x${…}/y");
        assert_eq!(hit["template"], "/api/x${q}/y");
        assert!(hit.get("query").is_none(), "{hit}");
        let hit = sole_hit(&parse, "GET", "${…}");
        assert_eq!(hit["template"], "${q}");
        assert!(hit.get("query").is_none(), "{hit}");
        assert_eq!(stats.query_suffixes, 0);
    }

    /// DT.4 CONTROL: only a `const` is read: a `let`, and a module `const`
    /// that a nearer `let` shadows, keep their `${…}`.
    #[test]
    fn let_query_is_kept() {
        let src = "\
const outer = '?z=1';
export class S {
    constructor(private http: any) {}
    l(c: boolean) {
        let q = c ? '?a=1' : '';
        return this.http.get(`/api/l${q}`);
    }
    s(c: boolean) {
        let outer = '?y=1';
        return this.http.get(`/api/s${outer}`);
    }
    t() { return this.http.get(`/api/t${outer}`); }
}
";
        let (parse, .., stats) = parse_file_stats(src, "src/s.ts", "src::s", repo()).unwrap();
        assert!(sole_hit(&parse, "GET", "/api/l${…}").get("query").is_none());
        assert!(sole_hit(&parse, "GET", "/api/s${…}").get("query").is_none());
        // The module `const` itself, unshadowed, is read.
        assert_eq!(sole_hit(&parse, "GET", "/api/t")["query"], "outer");
        assert_eq!(stats.query_suffixes, 1);
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
                unresolved: 1,
                aliases: 0,
                query_suffixes: 0
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
                unresolved: 2,
                aliases: 0,
                query_suffixes: 0
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
        // CH.3b's facts only: DT.5k's MemberRead / MemberWrite facts share the scope.
        let facts = parse.nav.nav_facts.get(&id).cloned().unwrap_or_default();
        facts.into_iter().filter(|f| matches!(f, NavFact::UrlPrefixKey { .. })).collect()
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

    // ========================================================================
    // CH.3c: the receiver gate
    // ========================================================================

    /// CH.3c: every ENDPOINT node of `parse`, as its nav qname.
    fn endpoint_qnames(parse: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = parse
            .nodes
            .iter()
            .filter(|n| parse.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
            .filter_map(|n| parse.nav.qname_by_id.get(&n.id))
            .cloned()
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// CH.3c (a): verb calls on a `new Map()` field and an annotated `Set`
    /// field (Kina's `ToastService::dismiss`, quokka's `typingTimers` /
    /// `desiredRooms`) mint no ENDPOINT.
    #[test]
    fn map_and_set_receivers_mint_nothing() {
        let src = "\
export class ToastService {
    private readonly timers = new Map<string, number>();
    private selected: Set<string> = new Set<string>();
    #pending = new globalThis.Map<string, number>();

    dismiss(id: string) {
        const t = this.timers.get(id);
        this.timers.delete(id);
        this.selected.delete(id);
        this.#pending.delete(id);
        return t;
    }
}
";
        let (parse, .., gate, args) =
            parse_file_stats(src, "src/toast.service.ts", "src::toast", repo()).unwrap();
        assert_eq!(endpoint_qnames(&parse), Vec::<String>::new());
        assert_eq!(gate.dropped, 4);
        assert_eq!(
            gate.by_type.into_iter().collect::<Vec<_>>(),
            vec![("Map".to_string(), 3), ("Set".to_string(), 1)]
        );
        assert_eq!(args.unresolved, 0, "a gated site is no ENDPOINT to count");
    }

    /// CH.3c (b): an `inject(T)` field and a constructor parameter property
    /// of an in-repo service (Kina's `drafts = inject(TradeDraftStore)`,
    /// `tradeService.get(this.id())`) mint no ENDPOINT; the call sites the
    /// A6.2a receiver pass binds remain.
    #[test]
    fn injected_and_ctor_typed_services_mint_nothing() {
        let src = "\
import { Component, inject } from '@angular/core';
import { TradesApi } from './trades.api';

@Component({ selector: 'app-trade', template: '' })
export class TradePage {
    private readonly drafts = inject(TradesApi);
    constructor(private readonly trades: TradesApi) {}
    open(id: string) { return this.trades.get(id); }
    save(id: string) { return this.drafts.patch(id, {}); }
}
";
        let (parse, .., gate, _) =
            parse_file_stats(src, "src/trade.page.ts", "src::trade_page", repo()).unwrap();
        assert_eq!(endpoint_qnames(&parse), Vec::<String>::new());
        assert_eq!(
            gate.by_type.into_iter().collect::<Vec<_>>(),
            vec![("TradesApi".to_string(), 2)]
        );
        let open = id(node_kind::METHOD, "src::trade_page::TradePage::open");
        let save = id(node_kind::METHOD, "src::trade_page::TradePage::save");
        let receiver = |from: NodeId, recv: &str, name: &str| {
            parse.calls.iter().any(|c| {
                c.from == from
                    && matches!(&c.qualifier, CallQualifier::ComplexReceiver { receiver, name: n }
                        if receiver == recv && n == name)
            })
        };
        assert!(receiver(open, "this.trades", "get"), "{:?}", parse.calls);
        assert!(receiver(save, "this.drafts", "patch"), "{:?}", parse.calls);
        assert_eq!(
            field_types(&parse, "src::trade_page::TradePage"),
            pairs(&[("drafts", "TradesApi"), ("trades", "TradesApi")])
        );
    }

    /// CH.3c (c): an HttpClient field, an `any` field, an untyped field, an
    /// `inject(HttpClient)` field, a `new HttpClient(…)` field and a field
    /// typed `…HttpService` keep their `<unresolved>` ENDPOINT.
    #[test]
    fn http_client_and_unknown_receivers_keep() {
        let src = "\
import { inject } from '@angular/core';
import { HttpClient } from '@angular/common/http';

export class S {
    private svc;
    private readonly injected = inject(HttpClient);
    private readonly built = new HttpClient(null);
    private readonly nest: NestHttpService;
    constructor(private http: HttpClient, private client: any) {}
    a(url: string) { return this.http.get(url); }
    b(url: string) { return this.client.get(url); }
    c(url: string) { return this.svc.get(url); }
    d(url: string) { return this.injected.post(url, {}); }
    e(url: string) { return this.built.put(url, {}); }
    f(url: string) { return this.nest.delete(url); }
}
";
        let (parse, .., gate, _) = parse_file_stats(src, "src/s.ts", "src::s", repo()).unwrap();
        assert_eq!(gate, ReceiverGateStats::default());
        for (m, verb) in [
            ("a", "GET"),
            ("b", "GET"),
            ("c", "GET"),
            ("d", "POST"),
            ("e", "PUT"),
            ("f", "DELETE"),
        ] {
            assert!(
                method_calls(&parse, "src::s", &format!("S::{m}"), verb, "<unresolved>"),
                "S::{m} keeps {verb} <unresolved>"
            );
        }
        assert!(is_http_client_type("HttpClient") && is_http_client_type("AxiosInstance"));
        assert!(is_http_client_type("AuthHttpClient") && is_http_client_type("NestHttpService"));
        assert!(!is_http_client_type("Map") && !is_http_client_type("HttpHeaders"));
    }

    /// CH.3c (d): a literal (or any read) path keeps its ENDPOINT on a typed
    /// non-HTTP receiver: `this.api.get('/users')` through an in-repo wrapper
    /// is a real client call.
    #[test]
    fn literal_on_typed_service_keeps() {
        let src = "\
export class UsersPage {
    constructor(private api: ApiService) {}
    load() { return this.api.get('/users'); }
    one(id: string) { return this.api.get(`/users/${id}`); }
}
";
        let (parse, .., gate, _) =
            parse_file_stats(src, "src/users.page.ts", "src::users", repo()).unwrap();
        assert_eq!(gate, ReceiverGateStats::default());
        assert!(method_calls(
            &parse,
            "src::users",
            "UsersPage::load",
            "GET",
            "/users"
        ));
        assert!(method_calls(
            &parse,
            "src::users",
            "UsersPage::one",
            "GET",
            "/users/${…}"
        ));
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
        let (_, _, _, stats, ..) = parse_file_stats(src, path, module, repo()).unwrap();
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

    /// CH.4: the USES edges `parse` holds from `from`, as `(to, evidence)`.
    fn method_uses_from(
        parse: &FileParse,
        from: NodeId,
    ) -> Vec<(NodeId, glia_code_domain::evidence::Evidence)> {
        use glia_code_domain::evidence::Evidence;
        parse
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::USES)
            .map(|e| (e.to, Evidence::of(e).expect("a method_ref USES carries evidence")))
            .collect()
    }

    /// CH.4: an arrow-field METHOD (CG.1) passed to `addEventListener` is USED
    /// by the member passing it: emitter the file's tag, rule `method_ref`, the
    /// site's 0-based row. The field's own `this.render()` stays a CALLS, and
    /// its self-reference (`removeEventListener(.., this.onResize)` inside
    /// `onResize`) mints no self-edge.
    #[test]
    fn arrow_field_passed_by_value_is_used() {
        use glia_code_domain::evidence::Basis;
        let src = "\
export class HeroComponent {
  ngAfterViewInit(): void { window.addEventListener('resize', this.onResize); }
  private onResize = (): void => {
    this.render();
    window.removeEventListener('resize', this.onResize);
  };
  render(): void {}
}
";
        let parse = parse_file(src, "src/hero.component.ts", "src::hero", repo()).unwrap();
        let m = |f: &str| id(node_kind::METHOD, &format!("src::hero::HeroComponent::{f}"));
        let uses = method_uses_from(&parse, m("ngAfterViewInit"));
        assert_eq!(uses.len(), 1, "{uses:?}");
        let (to, ev) = &uses[0];
        assert_eq!(*to, m("onResize"));
        assert_eq!(ev.emitter, "parser:angular");
        assert_eq!(ev.rule.as_deref(), Some("method_ref"));
        assert_eq!((ev.line, ev.basis), (Some(1), Basis::Site));
        assert_intra_call(&parse, m("onResize"), m("render"), 3, "onResize -> render");
        assert!(
            method_uses_from(&parse, m("onResize")).is_empty(),
            "a call is no USES and a self-reference no self-edge: {:?}",
            parse.edges
        );
    }

    /// CH.4: `subscribe(this.h)`, `this.render.bind(this)`, `.call` and a
    /// `#private` method passed by value are each a USES; the data field the
    /// subscription goes through (`this.ticks`) is none.
    #[test]
    fn subscribe_and_bind() {
        let src = "\
export class Hero {
  private readonly ticks = new Subject<number>();
  ngAfterViewInit(): void {
    this.ticks.subscribe(this.handleTick);
    requestAnimationFrame(this.render.bind(this));
    setTimeout(() => this.flush.call(this), 0);
    queueMicrotask(this.#tick);
  }
  handleTick(n: number): void {}
  render(): void {}
  flush(): void {}
  #tick(): void {}
}
";
        let parse = parse_file(src, "src/hero.ts", "src::hero", repo()).unwrap();
        let m = |f: &str| id(node_kind::METHOD, &format!("src::hero::Hero::{f}"));
        let mut got: Vec<(NodeId, Option<u32>, Option<String>, String)> =
            method_uses_from(&parse, m("ngAfterViewInit"))
                .into_iter()
                .map(|(to, ev)| (to, ev.line, ev.rule, ev.emitter))
                .collect();
        got.sort_by_key(|(_, line, ..)| *line);
        let rule = Some("method_ref".to_string());
        let tag = "parser:typescript".to_string();
        assert_eq!(
            got,
            vec![
                (m("handleTick"), Some(3), rule.clone(), tag.clone()),
                (m("render"), Some(4), rule.clone(), tag.clone()),
                (m("flush"), Some(5), rule.clone(), tag.clone()),
                (m("#tick"), Some(6), rule, tag),
            ],
            "edges {:?}",
            parse.edges
        );
    }

    /// CH.4: a call is CALLS, never also a USES.
    #[test]
    fn call_is_not_uses() {
        let src = "class C {\n  render() { this.handleTick(0); }\n  handleTick(n: number) {}\n}\n";
        let parse = parse_file(src, "src/c.ts", "src::c", repo()).unwrap();
        let m = |f: &str| id(node_kind::METHOD, &format!("src::c::C::{f}"));
        assert_intra_call(&parse, m("render"), m("handleTick"), 1, "render -> handleTick");
        assert!(
            !parse.edges.iter().any(|e| e.category == edge_category::USES),
            "{:?}",
            parse.edges
        );
    }

    /// CH.4: another class's member (`this.ticker.stop`), a data field
    /// (`this.rafId`), a CH.2 STATE_VAR (`this.label`, a signal), an inherited
    /// method (`this.handle`, defined on the superclass) passed by value and a
    /// getter read (`this.isDesktop`, a METHOD read as state) mint no USES:
    /// same class, METHOD targets only, no accessor (DT.5g makes the getter
    /// read a CALLS, rule `accessor_read`, never a USES).
    #[test]
    fn other_class_state_and_data_mint_nothing() {
        let src = "\
import { signal } from '@angular/core';
export class Ticker { start(cb: () => void) { cb(); } stop() {} }
export class Hero {
  private readonly ticker = new Ticker();
  private rafId = 0;
  readonly label = signal('x');
  ngAfterViewInit(): void {
    this.ticker.start(this.ticker.stop);
    console.info(this.rafId, this.label);
  }
}
export class Base { handle(e: Event): void {} }
export class Sub extends Base {
  run(): void { window.addEventListener('x', this.handle); }
  get isDesktop(): boolean { return true; }
  layout(): void { if (this.isDesktop) { console.info(this.isDesktop); } }
}
";
        let parse = parse_file(src, "src/hero.component.ts", "src::hero", repo()).unwrap();
        assert!(
            has_node(&parse, id(node_kind::STATE_VAR, "src::hero::Hero::label")),
            "label is a CH.2 STATE_VAR (so class_methods maps it)"
        );
        assert!(
            has_node(&parse, id(node_kind::METHOD, "src::hero::Sub::isDesktop")),
            "the getter is a METHOD (so the accessor filter is what drops it)"
        );
        assert!(
            !parse.edges.iter().any(|e| e.category == edge_category::USES),
            "no USES: {:?}",
            parse.edges
        );
    }

    /// CH.4: two by-value sites of one method in one member are one USES
    /// whose evidence row is the first site's, though the call walk pops
    /// siblings right to left.
    #[test]
    fn dedupe_keeps_first_row() {
        let src = "\
export class Hero {
  toggle(on: boolean): void {
    if (on) {
      window.addEventListener('resize', this.onResize);
    } else {
      window.removeEventListener('resize', this.onResize);
    }
  }
  onResize(): void {}
}
";
        let parse = parse_file(src, "src/hero.ts", "src::hero", repo()).unwrap();
        let m = |f: &str| id(node_kind::METHOD, &format!("src::hero::Hero::{f}"));
        let uses = method_uses_from(&parse, m("toggle"));
        assert_eq!(uses.len(), 1, "{uses:?}");
        assert_eq!(uses[0].0, m("onResize"));
        assert_eq!(uses[0].1.line, Some(3), "the first site's row");
    }

    /// CH.4: assigning to a method's name is a write, never a USES.
    #[test]
    fn assignment_is_not_uses() {
        let src = "\
export class Hero {
  reset(): void {
    this.handler = null;
    this.handler += '';
  }
  handler(): void {}
}
";
        let parse = parse_file(src, "src/hero.ts", "src::hero", repo()).unwrap();
        assert!(
            !parse.edges.iter().any(|e| e.category == edge_category::USES),
            "{:?}",
            parse.edges
        );
    }

    /// DF.22a: the locals `qname`'s scope records (`CodeNav::local_types`),
    /// sorted; empty when the scope records none.
    fn local_types(parse: &FileParse, qname: &str) -> std::collections::BTreeMap<String, String> {
        let ids: Vec<NodeId> = parse
            .nav
            .qname_by_id
            .iter()
            .filter(|(_, q)| q.as_str() == qname)
            .map(|(id, _)| *id)
            .collect();
        assert!(!ids.is_empty(), "no node {qname}");
        ids.iter()
            .filter_map(|id| parse.nav.local_types.get(id))
            .flat_map(|l| l.iter().map(|(k, v)| (k.clone(), v.clone())))
            .collect()
    }

    fn locals(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// DF.22a's `params_and_locals_record_their_types` source: a method with
    /// every parameter and declarator shape, a function and an arrow const.
    const LOCALS_SRC: &str = "\
import { Repo } from './repo';
export class C {
  m(api: ApiService, b?: ns.Bar | null, n: number, { d }: Opts, ...rest: X[]) {
    const t: Repo = make();
    const w = new ns.Widget<string>();
    const r = this.repo;
    const q = other as Queue;
    let u;
    for (const it of list) {}
    try {} catch (e) {}
    list.map((x: Item) => x.go());
  }
}
export function f(svc: Svc) {}
export const g = (h: Helper) => h.go();
";

    /// DF.22a: a parameter's annotation types it (`ns.Bar | null` -> `Bar`);
    /// a primitive, destructured or rest parameter, a loop variable, a catch
    /// parameter and an unannotated, uninitialised `let` are of unknown type;
    /// a declarator takes its annotation over its value, a `new` value's
    /// class, an `as` type or a `this.<f>` alias; a nested arrow's parameter
    /// is the enclosing method's local. A function and an arrow const record
    /// their own parameters.
    #[test]
    fn params_and_locals_record_their_types() {
        let parse = parse_file(LOCALS_SRC, "src/c.ts", "src::c", repo()).unwrap();
        assert_eq!(
            local_types(&parse, "src::c::C::m"),
            locals(&[
                ("api", "ApiService"),
                ("b", "Bar"),
                ("n", ""),
                ("d", ""),
                ("rest", ""),
                ("t", "Repo"),
                ("w", "Widget"),
                ("r", "self.repo"),
                ("q", "Queue"),
                ("u", ""),
                ("it", ""),
                ("e", ""),
                ("x", "Item"),
            ])
        );
        assert_eq!(local_types(&parse, "src::c::f"), locals(&[("svc", "Svc")]));
        assert_eq!(local_types(&parse, "src::c::g"), locals(&[("h", "Helper")]));
    }

    /// DF.22a: a name two bindings of one body give different types (the
    /// method's parameter and a nested arrow's) is of unknown type, and so is
    /// a local holding a function value.
    #[test]
    fn a_name_bound_to_two_types_is_unknown() {
        let src = "export class R { shadow(api: ApiService) { const run = (api: AdminService) => api.purge(); } }\n";
        let parse = parse_file(src, "src/r.ts", "src::r", repo()).unwrap();
        assert_eq!(
            local_types(&parse, "src::r::R::shadow"),
            locals(&[("api", ""), ("run", "")])
        );
    }

    /// DF.22a: `const auth = inject(AuthService)` types `auth` only when the
    /// callee is the `inject` import (A7.1's gate on `inject(T)` fields); a
    /// project-local `inject` helper types nothing.
    #[test]
    fn inject_local_needs_the_inject_import() {
        let body = "export const guard = () => { const auth = inject(AuthService); return auth.check(); };\n";
        let imported = format!("import {{ inject }} from '@angular/core';\n{body}");
        let parse = parse_file(&imported, "src/guard.ts", "src::guard", repo()).unwrap();
        assert_eq!(
            local_types(&parse, "src::guard::guard"),
            locals(&[("auth", "AuthService")])
        );
        let aliased = format!("import {{ inject as di }} from '@angular/core';\n{}", body.replace("inject(", "di("));
        let parse = parse_file(&aliased, "src/guard.ts", "src::guard", repo()).unwrap();
        assert_eq!(
            local_types(&parse, "src::guard::guard"),
            locals(&[("auth", "AuthService")])
        );
        let local_fn = format!("function inject(t: unknown) {{ return t; }}\n{body}");
        let parse = parse_file(&local_fn, "src/guard.ts", "src::guard", repo()).unwrap();
        assert_eq!(local_types(&parse, "src::guard::guard"), locals(&[("auth", "")]));
    }

    /// DF.22a: `[ts-locals]` counts the `x.m()` sites on a recorded local by
    /// what typed it, and the sites on a local of unknown type.
    #[test]
    fn ts_locals_marker_counts() {
        let src = "\
import { Repo } from './repo';
export class C {
  m(api: ApiService, b?: ns.Bar | null, n: number, { d }: Opts, ...rest: X[]) {
    const t: Repo = make();
    const w = new ns.Widget<string>();
    const r = this.repo;
    const q = other as Queue;
    let u;
    api.x(); t.y(); w.z(); r.q(); u.v();
  }
}
";
        let (_, _, _, _, stats, ..) = parse_file_stats(src, "src/c.ts", "src::c", repo()).unwrap();
        assert_eq!(
            stats,
            LocalStats {
                typed: 4,
                param: 1,
                annotated: 1,
                new: 1,
                inject: 0,
                alias: 1,
                call: 0,
                unknown: 1,
            }
        );
        let guard = "import { inject } from '@angular/core';\n\
                     export const guard = () => { const auth = inject(AuthService); return auth.check(); };\n";
        let (_, _, _, _, stats, ..) = parse_file_stats(guard, "src/g.ts", "src::g", repo()).unwrap();
        assert_eq!((stats.typed, stats.inject, stats.unknown), (1, 1, 0));
    }
}

/// DT.1c: the SCHEMA_FIELDS model schemas of interfaces and field-only
/// classes.
#[cfg(test)]
mod model_schema_tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts_models")
    }

    /// Every SCHEMA_FIELDS payload of the `kind` node `qname`.
    fn schemas(parse: &FileParse, kind: glia_core::NodeKindId, qname: &str) -> Vec<String> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        parse
            .nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::SCHEMA_FIELDS)
            .map(|c| match &c.payload {
                CellPayload::Json(j) => j.clone(),
                other => panic!("SCHEMA_FIELDS must be Json, got {other:?}"),
            })
            .collect()
    }

    fn one(parse: &FileParse, kind: glia_core::NodeKindId, qname: &str) -> ModelSchema {
        let got = schemas(parse, kind, qname);
        assert_eq!(got.len(), 1, "{qname}: one SCHEMA_FIELDS cell, got {got:?}");
        ModelSchema::parse(&got[0]).expect("a wire ModelSchema")
    }

    #[test]
    fn interface_fields_keys_optionality_and_openness() {
        let src = "\
export interface Base { createdAt: string }
export interface User extends Base, ns.Named {
  readonly id: string;
  'display-name': string;
  0: number;
  email?: string;
  nick: string | undefined;
  tags: Array<string>;
  meta: {
    a: string;
    b:   number
  };
  untyped;
  save(): void;
  (x: number): string;
  new (x: string): User;
}
export interface Flags { [flag: string]: boolean; on: boolean }
export interface Computed { [KEY]: string; ok: string }
export interface Picked extends Pick<User, 'id'> { extra: number }
export interface Repo { find(id: string): User; }
export interface Empty {}
";
        let parse = parse_file(src, "src/m.ts", "src::m", repo()).unwrap();
        let user = one(&parse, node_kind::INTERFACE, "src::m::User");
        assert_eq!(
            user.to_payload(),
            "{\"extends\":[\"Base\",\"Named\"],\"fields\":[\
             {\"name\":\"id\",\"required\":true,\"type\":\"string\"},\
             {\"name\":\"display-name\",\"required\":true,\"type\":\"string\"},\
             {\"name\":\"0\",\"required\":true,\"type\":\"number\"},\
             {\"name\":\"email\",\"required\":false,\"type\":\"string\"},\
             {\"name\":\"nick\",\"required\":false,\"type\":\"string | undefined\"},\
             {\"array\":true,\"name\":\"tags\",\"required\":true,\"type\":\"Array<string>\"},\
             {\"name\":\"meta\",\"required\":true,\"type\":\"{ a: string; b: number }\"},\
             {\"name\":\"untyped\",\"required\":true,\"type\":\"any\"}],\
             \"format\":\"ts\"}"
        );
        let flags = one(&parse, node_kind::INTERFACE, "src::m::Flags");
        assert!(flags.open && flags.field("on").is_some());
        let computed = one(&parse, node_kind::INTERFACE, "src::m::Computed");
        assert!(computed.open);
        assert_eq!(computed.fields.len(), 1, "the computed key is skipped");
        let picked = one(&parse, node_kind::INTERFACE, "src::m::Picked");
        assert!(picked.open, "a utility base leaves the model open");
        assert!(picked.extends.is_empty(), "a generic base is not listed");
        assert_eq!(picked.fields.len(), 1);
        // A method-only service contract and an empty marker describe no body.
        assert!(schemas(&parse, node_kind::INTERFACE, "src::m::Repo").is_empty());
        assert!(schemas(&parse, node_kind::INTERFACE, "src::m::Empty").is_empty());
        let base = one(&parse, node_kind::INTERFACE, "src::m::Base");
        assert_eq!(base.format, WireFormat::Ts);
    }

    #[test]
    fn a_merged_interface_carries_one_cell_per_declaration() {
        let src = "interface A { x: string }\ninterface A { y?: number }\n";
        let parse = parse_file(src, "src/a.ts", "src::a", repo()).unwrap();
        let got = schemas(&parse, node_kind::INTERFACE, "src::a::A");
        assert_eq!(got.len(), 2, "the pass unions them: {got:?}");
    }

    #[test]
    fn field_only_classes_are_models_and_service_classes_are_not() {
        let src = "\
export class Person {
  name = '';
  count = 0;
  active = true;
  when = new Date();
  list = [];
  age?: number;
  id!: string;
  maybe: string | undefined;
  readonly kind: 'p' = 'p';
  // a comment between members
  constructor(public nickname: string, readonly code?: number, plain: boolean) {}
}
export class Child extends Person { extra: string; }
export class Generic extends Base<string> { x: number; }
export class Store {
  private cache = new Map<string, Person>();
  get(id: string) { return this.cache.get(id); }
}
export class Service { constructor(private http: HttpClient) {} }
export class Hidden { #secret = 1; shown = 2; }
export class Statics { static VERSION = 1; x = 2; }
export class Getter { x = 1; get y() { return this.x; } }
export class Handler { x = 1; onClick = () => this.x; }
export class Protected { protected x = 1; }
export class Bare { constructor() {} }
@Component({ selector: 'app-x' })
export class Widget { @Input() label = ''; }
@Injectable()
export class Api { base = '/api'; }
";
        let parse = parse_file(src, "src/p.ts", "src::p", repo()).unwrap();
        let person = one(&parse, node_kind::CLASS, "src::p::Person");
        assert_eq!(
            person.to_payload(),
            "{\"fields\":[\
             {\"name\":\"name\",\"required\":true,\"type\":\"string\"},\
             {\"name\":\"count\",\"required\":true,\"type\":\"number\"},\
             {\"name\":\"active\",\"required\":true,\"type\":\"boolean\"},\
             {\"name\":\"when\",\"required\":true,\"type\":\"Date\"},\
             {\"array\":true,\"name\":\"list\",\"required\":true,\"type\":\"any[]\"},\
             {\"name\":\"age\",\"required\":false,\"type\":\"number\"},\
             {\"name\":\"id\",\"required\":true,\"type\":\"string\"},\
             {\"name\":\"maybe\",\"required\":false,\"type\":\"string | undefined\"},\
             {\"name\":\"kind\",\"required\":true,\"type\":\"'p'\"},\
             {\"name\":\"nickname\",\"required\":true,\"type\":\"string\"},\
             {\"name\":\"code\",\"required\":false,\"type\":\"number\"}],\
             \"format\":\"ts\"}"
        );
        let child = one(&parse, node_kind::CLASS, "src::p::Child");
        assert_eq!(child.extends, vec!["Person".to_string()]);
        let generic = one(&parse, node_kind::CLASS, "src::p::Generic");
        assert!(generic.open && generic.extends.is_empty());
        for no_model in [
            "Store", "Service", "Hidden", "Statics", "Getter", "Handler", "Protected", "Bare", "Widget", "Api",
        ] {
            assert!(
                schemas(&parse, node_kind::CLASS, &format!("src::p::{no_model}")).is_empty(),
                "{no_model} is no model"
            );
        }
    }

    /// The models-declared-fields fixture's TS file, cell for cell.
    #[test]
    fn fixture_models_declared_fields_web() {
        let src = include_str!(
            "../../../../bench/substrate-gap/fixtures/models-declared-fields/web/src/models/account.ts"
        );
        let parse = parse_file(src, "src/models/account.ts", "src::models::account", repo()).unwrap();
        let q = |n: &str| format!("src::models::account::{n}");
        assert_eq!(
            schemas(&parse, node_kind::INTERFACE, &q("Account")),
            vec![
                "{\"extends\":[\"Audited\"],\"fields\":[\
                 {\"name\":\"id\",\"required\":true,\"type\":\"string\"},\
                 {\"name\":\"display-name\",\"required\":true,\"type\":\"string\"},\
                 {\"name\":\"email\",\"required\":false,\"type\":\"string\"},\
                 {\"array\":true,\"name\":\"roles\",\"required\":true,\"type\":\"string[]\"},\
                 {\"name\":\"owner\",\"required\":true,\"type\":\"Person\"}],\
                 \"format\":\"ts\"}"
                    .to_string()
            ]
        );
        assert_eq!(
            schemas(&parse, node_kind::INTERFACE, &q("Settings")),
            vec![
                "{\"fields\":[{\"name\":\"theme\",\"required\":true,\"type\":\"'light' | 'dark'\"}],\
                 \"format\":\"ts\",\"open\":true}"
                    .to_string()
            ]
        );
        assert_eq!(
            schemas(&parse, node_kind::CLASS, &q("Person")),
            vec![
                "{\"fields\":[{\"name\":\"name\",\"required\":true,\"type\":\"string\"},\
                 {\"name\":\"age\",\"required\":false,\"type\":\"number\"},\
                 {\"name\":\"nickname\",\"required\":true,\"type\":\"string\"}],\"format\":\"ts\"}"
                    .to_string()
            ]
        );
        assert_eq!(schemas(&parse, node_kind::INTERFACE, &q("Audited")).len(), 1);
        assert!(schemas(&parse, node_kind::CLASS, &q("AccountStore")).is_empty());
        // Every payload reads back through the wire vocabulary unchanged.
        for n in &parse.nodes {
            for c in n.cells.iter().filter(|c| c.kind == cell_type::SCHEMA_FIELDS) {
                let schema = ModelSchema::from_cell(c).expect("a wire ModelSchema");
                assert_eq!(schema.cell(), *c);
            }
        }
    }
}

/// DT.1d: client wire shapes on ENDPOINT_HIT.
#[cfg(test)]
mod wire_shape_tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts_wire")
    }

    fn parse(src: &str, path: &str) -> FileParse {
        parse_file(src, path, "src::svc", repo()).unwrap()
    }

    /// The ENDPOINT_HIT payload texts of the ENDPOINT `method path`.
    fn hits(parse: &FileParse, method: &str, path: &str) -> Vec<String> {
        let id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            &format!("endpoint:{method}:{path}"),
        );
        parse
            .nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) if c.kind == cell_type::ENDPOINT_HIT => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn sole(parse: &FileParse, method: &str, path: &str) -> String {
        let h = hits(parse, method, path);
        assert_eq!(h.len(), 1, "{method} {path}: {h:?}");
        h[0].clone()
    }

    /// The `response` / `request` of a hit, as JSON text (`None` when absent).
    fn shapes(payload: &str) -> (Option<String>, Option<String>) {
        let (r, q) = wire::hit_shapes(payload);
        (r.map(|s| s.to_json()), q.map(|s| s.to_json()))
    }

    fn last_stats() -> WireStats {
        LAST_WIRE_STATS.with(|w| w.borrow().clone())
    }

    const NG_HEAD: &str = "import { HttpClient } from '@angular/common/http';\n\
                           import { Injectable } from '@angular/core';\n\
                           @Injectable({ providedIn: 'root' })\n\
                           export class OrderService {\n  constructor(private http: HttpClient) {}\n";

    fn ng(body: &str) -> String {
        format!("{NG_HEAD}{body}}}\n")
    }

    /// The call node of the first statement of `async function f() {..}`'s
    /// body that is a call (`src` holds one such function).
    fn first_call<'t>(tree: &'t tree_sitter::Tree) -> TsNode<'t> {
        let mut stack = vec![tree.root_node()];
        let mut calls = Vec::new();
        while let Some(n) = stack.pop() {
            if n.kind() == "call_expression" {
                calls.push(n);
            }
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
        }
        calls.sort_by_key(|n| n.start_byte());
        calls[0]
    }

    fn ts_tree(src: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        parser.set_language(&lang).unwrap();
        parser.parse(src, None).unwrap()
    }

    /// The parse shape DT.1d relies on, pinned: tree-sitter-typescript
    /// 0.23.2 puts the `await` of `await recv.verb<T>(..)` inside the
    /// callee, with the type arguments on the call; the unawaited and the
    /// untyped forms keep a member callee.
    #[test]
    fn awaited_generic_call_parse_shape_is_pinned() {
        let src = "async function f() { const r = await axios.get<Foo>(u); }";
        let tree = ts_tree(src);
        let call = first_call(&tree);
        assert_eq!(
            call.to_sexp(),
            "(call_expression function: (await_expression (member_expression object: (identifier) \
             property: (property_identifier))) type_arguments: (type_arguments (type_identifier)) \
             arguments: (arguments (identifier)))"
        );
        let (member, targs) = awaited_generic_callee(call).expect("the awaited generic callee");
        assert_eq!(text(member, src.as_bytes()), "axios.get");
        assert_eq!(text(targs, src.as_bytes()), "<Foo>");

        // `.toPromise()` chains: the quirk call is the chain's inner call.
        let src = "async function f() { const r = await this.http.get<Foo>(u).toPromise(); }";
        let tree = ts_tree(src);
        let call = first_call(&tree);
        assert_eq!(call.child_by_field_name("function").unwrap().kind(), "member_expression");
        let inner = call
            .child_by_field_name("function")
            .and_then(|m| m.child_by_field_name("object"))
            .unwrap();
        let (member, _) = awaited_generic_callee(inner).expect("the inner call is the quirk");
        assert_eq!(text(member, src.as_bytes()), "this.http.get");

        for (src, kind) in [
            ("async function f() { const r = axios.get<Foo>(u); }", "member_expression"),
            ("async function f() { const r = await axios.get(u); }", "member_expression"),
        ] {
            let tree = ts_tree(src);
            let call = first_call(&tree);
            assert_eq!(call.child_by_field_name("function").unwrap().kind(), kind, "{src}");
            assert!(awaited_generic_callee(call).is_none(), "{src}");
        }
    }

    /// `get<Order[]>`: one array layer peeled off; no body, no request.
    #[test]
    fn get_typed_array() {
        let p = parse(
            &ng("  list() { return this.http.get<Order[]>('/api/orders'); }\n"),
            "src/order.service.ts",
        );
        let hit = sole(&p, "GET", "/api/orders");
        assert!(hit.ends_with(",\"response\":{\"array\":true,\"type\":\"Order\"}}"), "{hit}");
        assert_eq!(shapes(&hit).1, None);
        let stats = last_stats();
        assert_eq!((stats.responses, stats.requests, stats.typed), (1, 0, 1), "{stats:?}");
    }

    /// examples-glia `place(order: NewOrder)`: the body is the parameter, read
    /// by its annotation; `response` then `request`, last.
    #[test]
    fn post_body_param_annotation() {
        let p = parse(
            &ng("  place(order: NewOrder) { return this.http.post<Order>('/api/orders', order); }\n"),
            "src/order.service.ts",
        );
        let hit = sole(&p, "POST", "/api/orders");
        assert!(
            hit.ends_with(",\"response\":{\"type\":\"Order\"},\"request\":{\"type\":\"NewOrder\"}}"),
            "{hit}"
        );
        let stats = last_stats();
        assert_eq!((stats.requests, stats.annotated), (1, 1), "{stats:?}");
    }

    /// An object-literal body: its keys, each read by its value (a literal by
    /// its JSON kind, a shorthand name by its declared type).
    #[test]
    fn patch_object_literal_keys() {
        let p = parse(
            &ng("  cancel(id: string, reason: string) {\n    \
                 return this.http.patch<Order>(`/api/orders/${id}/status`, { status: 'cancelled', reason, n: 2, ok: true, tags: [], extra: undefined });\n  }\n"),
            "src/order.service.ts",
        );
        let hit = sole(&p, "PATCH", "/api/orders/${…}/status");
        assert_eq!(
            shapes(&hit).1.as_deref(),
            Some(
                "{\"fields\":[{\"name\":\"status\",\"required\":true,\"shape\":{\"type\":\"string\"}},\
                 {\"name\":\"reason\",\"required\":true,\"shape\":{\"type\":\"string\"}},\
                 {\"name\":\"n\",\"required\":true,\"shape\":{\"type\":\"number\"}},\
                 {\"name\":\"ok\",\"required\":true,\"shape\":{\"type\":\"boolean\"}},\
                 {\"name\":\"tags\",\"required\":true,\"shape\":{\"array\":true,\"type\":\"any\"}}]}"
            )
        );
        assert_eq!(last_stats().literal, 1);
        // A spread leaves the literal open; a local object literal is read
        // through its declarator.
        let p = parse(
            &ng("  save(base: Order) {\n    const patch = { ...base, state: 'x' };\n    \
                 return this.http.put('/api/orders/one', patch);\n  }\n"),
            "src/order.service.ts",
        );
        let hit = sole(&p, "PUT", "/api/orders/one");
        assert_eq!(
            shapes(&hit),
            (
                None,
                Some("{\"fields\":[{\"name\":\"state\",\"required\":true,\"shape\":{\"type\":\"string\"}}],\"open\":true}".into())
            )
        );
    }

    /// An inline envelope type argument is an object shape; `?` is not
    /// required.
    #[test]
    fn inline_envelope_type() {
        let p = parse(
            &ng("  offers() { return this.http.get<{ offers: Offer[]; page: number; limit?: number }>('/api/offers'); }\n"),
            "src/order.service.ts",
        );
        let hit = sole(&p, "GET", "/api/offers");
        assert_eq!(
            shapes(&hit).0.as_deref(),
            Some(
                "{\"fields\":[{\"name\":\"offers\",\"required\":true,\"shape\":{\"array\":true,\"type\":\"Offer\"}},\
                 {\"name\":\"page\",\"required\":true,\"shape\":{\"type\":\"number\"}},\
                 {\"name\":\"limit\",\"required\":false,\"shape\":{\"type\":\"number\"}}]}"
            )
        );
        assert_eq!(last_stats().inline, 1);
    }

    /// `T | null` reads as `T`; a union of models stays a union; a `| null`
    /// field of an envelope reads its one model.
    #[test]
    fn union_with_null() {
        let p = parse(
            &ng("  a() { return this.http.get<Order | null>('/api/a'); }\n  \
                 b() { return this.http.get<Order | Refund>('/api/b'); }\n  \
                 c() { return this.http.get<{ users: User[] | null }>('/api/c'); }\n"),
            "src/order.service.ts",
        );
        assert_eq!(shapes(&sole(&p, "GET", "/api/a")).0.as_deref(), Some("{\"type\":\"Order\"}"));
        assert_eq!(
            shapes(&sole(&p, "GET", "/api/b")).0.as_deref(),
            Some("{\"union\":[{\"type\":\"Order\"},{\"type\":\"Refund\"}]}")
        );
        assert_eq!(
            shapes(&sole(&p, "GET", "/api/c")).0.as_deref(),
            Some("{\"fields\":[{\"name\":\"users\",\"required\":true,\"shape\":{\"array\":true,\"type\":\"User\"}}]}")
        );
    }

    /// `any` / `unknown` and every generic but `Array<X>` are open, their text
    /// kept as `raw`; `Array<X>` / `ReadonlyArray<X>` are arrays of `X`.
    #[test]
    fn any_response_is_open() {
        let p = parse(
            &ng("  a() { return this.http.get<any>('/api/a'); }\n  \
                 b() { return this.http.get<unknown>('/api/b'); }\n  \
                 c() { return this.http.get<Partial<Order>>('/api/c'); }\n  \
                 d() { return this.http.get<Array<Order>>('/api/d'); }\n  \
                 e() { return this.http.get<ReadonlyArray<Order>>('/api/e'); }\n"),
            "src/order.service.ts",
        );
        let resp = |path: &str| shapes(&sole(&p, "GET", path)).0;
        assert_eq!(resp("/api/a").as_deref(), Some("{\"open\":true,\"raw\":\"any\"}"));
        assert_eq!(resp("/api/b").as_deref(), Some("{\"open\":true,\"raw\":\"unknown\"}"));
        assert_eq!(resp("/api/c").as_deref(), Some("{\"open\":true,\"raw\":\"Partial<Order>\"}"));
        assert_eq!(resp("/api/d").as_deref(), Some("{\"array\":true,\"type\":\"Order\"}"));
        assert_eq!(resp("/api/e").as_deref(), Some("{\"array\":true,\"type\":\"Order\"}"));
    }

    /// `await axios.get<T>(..)`, `(await axios.post<T>(..)).data`, `await
    /// this.http.get<T>(..).toPromise()` and `const x = await
    /// this.http.get<T>(..)` mint their ENDPOINTs (none did before DT.1d),
    /// located at the callee as the unawaited form is.
    #[test]
    fn awaited_generic_axios_mints_endpoint() {
        let src = "import axios from 'axios';\n\
                   export async function load() {\n  \
                   const r = await axios.get<Foo>('/api/foo');\n  \
                   const s = (await axios.post<Bar>('/api/bar', { a: 1 })).data;\n  \
                   const t = await axios.get<Array<Foo>>('/api/foos');\n  \
                   return [r, s, t];\n}\n";
        let p = parse(src, "src/load.ts");
        let foo = sole(&p, "GET", "/api/foo");
        let v: serde_json::Value = serde_json::from_str(&foo).unwrap();
        assert_eq!((v["line"].as_u64(), v["col"].as_u64()), (Some(3), Some(19)), "col at `axios`: {foo}");
        assert_eq!(shapes(&foo).0.as_deref(), Some("{\"type\":\"Foo\"}"));
        let bar = sole(&p, "POST", "/api/bar");
        assert_eq!(
            shapes(&bar),
            (
                Some("{\"type\":\"Bar\"}".into()),
                Some("{\"fields\":[{\"name\":\"a\",\"required\":true,\"shape\":{\"type\":\"number\"}}]}".into())
            )
        );
        assert_eq!(shapes(&sole(&p, "GET", "/api/foos")).0.as_deref(), Some("{\"array\":true,\"type\":\"Foo\"}"));
        assert_eq!(last_stats().awaited_generic, 3);
        let load = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "src::svc::load");
        let foo_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/api/foo");
        assert!(
            p.edges
                .iter()
                .any(|e| e.from == load && e.to == foo_id && e.category == edge_category::CALLS),
            "load CALLS the awaited endpoint"
        );

        let p = parse(
            &ng("  async one() { return await this.http.get<Order>('/api/one').toPromise(); }\n  \
                 async two() { const x = await this.http.get<Order[]>('/api/two'); return x; }\n"),
            "src/order.service.ts",
        );
        assert_eq!(shapes(&sole(&p, "GET", "/api/one")).0.as_deref(), Some("{\"type\":\"Order\"}"));
        assert_eq!(
            shapes(&sole(&p, "GET", "/api/two")).0.as_deref(),
            Some("{\"array\":true,\"type\":\"Order\"}")
        );
        assert_eq!(last_stats().awaited_generic, 2);
    }

    /// fetch: the init's `body: JSON.stringify(<expr>)` is the request, and
    /// the awaited response's `.json()` typed by `as T`, `as Promise<T>` or a
    /// declarator annotation is the response.
    #[test]
    fn fetch_stringify_body_and_json_as() {
        let src = "export async function addComment(ticketId: string, body: string): Promise<Comment> {\n  \
                   const res = await fetch(`/api/tickets/${ticketId}/comments`, {\n    method: 'POST',\n    \
                   headers: { 'Content-Type': 'application/json' },\n    body: JSON.stringify({ body }),\n  });\n  \
                   return (await res.json()) as Comment;\n}\n\
                   export async function one(): Promise<Order> {\n  return (await fetch('/api/one')).json() as Order;\n}\n\
                   export async function two(o: NewOrder) {\n  const r = await fetch('/api/two', { method: 'PUT', body: JSON.stringify(o) });\n  \
                   const d: Order[] = await r.json();\n  return d;\n}\n\
                   export async function three() {\n  const r = await fetch('/api/three');\n  return r.json() as Promise<Stats>;\n}\n\
                   export async function four(fd: FormData) {\n  await fetch('/api/four', { method: 'POST', body: fd });\n}\n";
        let p = parse(src, "src/comments.ts");
        let hit = sole(&p, "POST", "/api/tickets/${…}/comments");
        assert!(
            hit.ends_with(
                ",\"response\":{\"type\":\"Comment\"},\"request\":{\"fields\":[{\"name\":\"body\",\"required\":true,\
                 \"shape\":{\"type\":\"string\"}}]}}"
            ),
            "{hit}"
        );
        assert_eq!(shapes(&sole(&p, "GET", "/api/one")), (Some("{\"type\":\"Order\"}".into()), None));
        assert_eq!(
            shapes(&sole(&p, "PUT", "/api/two")),
            (Some("{\"array\":true,\"type\":\"Order\"}".into()), Some("{\"type\":\"NewOrder\"}".into()))
        );
        assert_eq!(shapes(&sole(&p, "GET", "/api/three")), (Some("{\"type\":\"Stats\"}".into()), None));
        // A FormData body is no JSON body: the hit keeps HEAD's bytes.
        assert_eq!(shapes(&sole(&p, "POST", "/api/four")), (None, None));
        let stats = last_stats();
        assert_eq!((stats.responses, stats.requests, stats.fetch), (4, 2, 6), "{stats:?}");
    }

    /// ky / got: the body is the `json:` option of the second argument, any
    /// verb; axios's second argument of `post` / `put` / `patch`.
    #[test]
    fn ky_json_option_is_the_body() {
        let src = "import ky from 'ky';\nimport got from 'got';\nimport axios from 'axios';\n\
                   export function a(name: string) { return ky.post('/api/a', { json: { name } }); }\n\
                   export function b(p: Profile) { return got.put('/api/b', { json: p, timeout: 3 }); }\n\
                   export function c() { return ky.post('/api/c', { body: 'raw' }); }\n\
                   export function d(o: NewOrder) { return axios.post<Order>('/api/d', o); }\n\
                   export function e(params: Q) { return axios.get<Order>('/api/e', { params }); }\n";
        let p = parse(src, "src/clients.ts");
        assert_eq!(
            shapes(&sole(&p, "POST", "/api/a")).1.as_deref(),
            Some("{\"fields\":[{\"name\":\"name\",\"required\":true,\"shape\":{\"type\":\"string\"}}]}")
        );
        assert_eq!(shapes(&sole(&p, "PUT", "/api/b")).1.as_deref(), Some("{\"type\":\"Profile\"}"));
        assert_eq!(shapes(&sole(&p, "POST", "/api/c")), (None, None));
        assert_eq!(
            shapes(&sole(&p, "POST", "/api/d")),
            (Some("{\"type\":\"Order\"}".into()), Some("{\"type\":\"NewOrder\"}".into()))
        );
        // axios `get`'s second argument is its config, never a body.
        assert_eq!(shapes(&sole(&p, "GET", "/api/e")), (Some("{\"type\":\"Order\"}".into()), None));
        assert_eq!(last_stats().unknown_client, 0);
    }

    /// A receiver whose body position is unknown (an in-repo wrapper, an
    /// alias from a local module) records the response it is typed with but
    /// no request, and counts `request_unknown_client`.
    #[test]
    fn custom_wrapper_receiver_records_no_request() {
        let src = "import { Injectable } from '@angular/core';\nimport { api } from './api';\n\
                   @Injectable()\nexport class S {\n  constructor(private client: ApiClient) {}\n  \
                   a(o: NewOrder) { return this.client.post<Order>('/api/a', o); }\n  \
                   b(o: NewOrder) { return api.post<Order>('/api/b', o); }\n}\n";
        let p = parse(src, "src/s.service.ts");
        assert_eq!(shapes(&sole(&p, "POST", "/api/a")), (Some("{\"type\":\"Order\"}".into()), None));
        assert_eq!(shapes(&sole(&p, "POST", "/api/b")), (Some("{\"type\":\"Order\"}".into()), None));
        assert_eq!(last_stats().unknown_client, 2);
    }

    /// An `inject(HttpClient)` field is an HttpClient receiver too; its
    /// `get` / `delete` options object is never a body; a string or
    /// FormData body writes no request.
    #[test]
    fn inject_http_client_and_non_json_bodies() {
        let src = "import { HttpClient } from '@angular/common/http';\nimport { inject, Injectable } from '@angular/core';\n\
                   @Injectable()\nexport class S {\n  private readonly http = inject(HttpClient);\n  \
                   a(o: NewOrder) { return this.http.post<Order>('/api/a', o); }\n  \
                   b(params: P) { return this.http.get<Order>('/api/b', { params }); }\n  \
                   c(token: string) { return this.http.post('/api/c', token); }\n  \
                   d(file: File) { const fd = new FormData(); fd.append('f', file); return this.http.post('/api/d', fd); }\n  \
                   e() { return this.http.post('/api/e', this.form.value); }\n}\n";
        let p = parse(src, "src/s.service.ts");
        assert_eq!(shapes(&sole(&p, "POST", "/api/a")).1.as_deref(), Some("{\"type\":\"NewOrder\"}"));
        assert_eq!(shapes(&sole(&p, "GET", "/api/b")), (Some("{\"type\":\"Order\"}".into()), None));
        assert_eq!(shapes(&sole(&p, "POST", "/api/c")), (None, None));
        assert_eq!(shapes(&sole(&p, "POST", "/api/d")), (None, None));
        assert_eq!(
            shapes(&sole(&p, "POST", "/api/e")).1.as_deref(),
            Some("{\"open\":true,\"raw\":\"this.form.value\"}")
        );
    }

    /// An untyped call with no body writes no shape: its payload is HEAD's,
    /// byte for byte, and the file prints no `[ts-wire]` line.
    #[test]
    fn untyped_call_writes_no_shape() {
        let p = parse(
            &ng("  a() { return this.http.get('/api/plain'); }\n  \
                 b(params: P) { return this.http.get(`/api/q/${params.id}`, { params }); }\n"),
            "src/order.service.ts",
        );
        assert_eq!(
            sole(&p, "GET", "/api/plain"),
            "{\"method\":\"GET\",\"path\":\"/api/plain\",\"file\":\"src/order.service.ts\",\"line\":6,\"col\":16,\"confidence\":\"strong\"}"
        );
        assert_eq!(
            sole(&p, "GET", "/api/q/${…}"),
            "{\"method\":\"GET\",\"path\":\"/api/q/${…}\",\"file\":\"src/order.service.ts\",\"line\":7,\"col\":25,\
             \"confidence\":\"medium\",\"template\":\"/api/q/${params.id}\"}"
        );
        assert_eq!(last_stats(), WireStats::default());
    }

    /// Every shape this parser writes reads back through the wire vocabulary
    /// and re-serialises to the same bytes (the sorted-key rule the endpoint
    /// fold's `serde_json::Value` round trip relies on).
    #[test]
    fn hit_shapes_round_trip() {
        let p = parse(
            &ng("  a(o: NewOrder) { return this.http.post<{ items: Item[]; next?: string | null; [k: string]: unknown }>('/api/a', { o, n: 1 }); }\n"),
            "src/order.service.ts",
        );
        let hit = sole(&p, "POST", "/api/a");
        let v: serde_json::Value = serde_json::from_str(&hit).unwrap();
        for key in ["response", "request"] {
            let raw = serde_json::to_string(&v[key]).unwrap();
            let shape = Shape::parse(&raw).expect("a wire Shape");
            assert_eq!(shape.to_json(), raw, "{key}");
            assert!(hit.contains(&raw), "{key}: {hit}");
        }
    }
}

#[cfg(test)]
mod reexport_tests {
    //! DT.1q: the export facts the graph's re-export resolution reads.
    use super::*;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts-reexports")
    }

    /// The facts recorded on the file MODULE `src::<stem>` of `src`.
    fn facts(src: &str, path: &str) -> Vec<NavFact> {
        let stem = path.trim_start_matches("src/").rsplit_once('.').map_or(path, |(s, _)| s);
        let qname = format!("src::{stem}");
        let parse = parse_file(src, path, &qname, repo()).expect("parse");
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, &qname);
        parse.nav.nav_facts.get(&module).cloned().unwrap_or_default()
    }

    fn star(source: &str, line: u32) -> NavFact {
        NavFact::ReExportStar { source: source.into(), line }
    }

    fn named(source: &str, name: &str, exported: &str, line: u32) -> NavFact {
        NavFact::ReExportName { source: source.into(), name: name.into(), exported: exported.into(), line }
    }

    fn local(local: &str, exported: &str, line: u32) -> NavFact {
        NavFact::ExportLocal { local: local.into(), exported: exported.into(), line }
    }

    fn exports(names: &[&str]) -> NavFact {
        NavFact::Exports { names: names.iter().map(|n| n.to_string()).collect() }
    }

    #[test]
    fn reexport_statements_are_recorded() {
        let src = "\
export * from './models';
export * as fmt from './util/format';
export { A, B as C } from \"./ab\";
export type { T } from './types';
export { default as D, E as default } from './d';
export { \"a-b\" as ab } from './kebab';
";
        assert_eq!(
            facts(src, "src/index.ts"),
            [
                star("./models", 0),
                NavFact::ReExportNamespace { source: "./util/format".into(), exported: "fmt".into(), line: 1 },
                named("./ab", "A", "A", 2),
                named("./ab", "B", "C", 2),
                named("./types", "T", "T", 3),
                // tree-sitter lexes `default` in a specifier as an identifier.
                named("./d", "default", "D", 4),
                named("./d", "E", "default", 4),
                named("./kebab", "a-b", "ab", 5),
            ]
        );
    }

    #[test]
    fn from_less_export_clause_records_export_local() {
        let src = "\
import { User } from './user.model';
const KEY = 'k';
export { User as Customer, KEY };
export type { User };
export = KEY;
";
        assert_eq!(
            facts(src, "src/order.model.ts"),
            [local("User", "Customer", 2), local("KEY", "KEY", 2), local("User", "User", 3)]
        );
    }

    #[test]
    fn exported_declarations_record_one_exports_fact() {
        let src = "\
export class A {}
export abstract class B {}
export function f() {}
export function g(a: string): void;
export function g(a: any) {}
export function sig(a: string): void;
export declare function df(): void;
export const c1 = 1, c2 = () => 2;
export const { d1, d2: e2, ...rest } = obj;
export let l = 0;
export interface I {}
export type T = string;
export enum E { X }
export namespace N.Inner { export const z = 1; }
export declare const decl: number;
export default class K {}
class Private {}
function helper() {}
";
        let got = facts(src, "src/decls.ts");
        assert_eq!(
            got,
            [exports(&[
                "A", "B", "E", "I", "N", "T", "c1", "c2", "d1", "decl", "default", "df", "e2", "f", "g",
                "l", "rest", "sig",
            ])]
        );
        // The non-exported class and `K` (exported as `default` only) are
        // absent; the namespace's inner export is no module export.
        let NavFact::Exports { names } = &got[0] else { unreachable!() };
        for absent in ["Private", "helper", "K", "z", "Inner"] {
            assert!(!names.iter().any(|n| n == absent), "{absent}");
        }
    }

    #[test]
    fn export_default_expression_exports_default() {
        assert_eq!(facts("export default { name: 'x' };\n", "src/cfg.ts"), [exports(&["default"])]);
        assert_eq!(facts("export default function () {}\n", "src/fn.ts"), [exports(&["default"])]);
        // `export as namespace` and a plain file record nothing.
        assert!(facts("export as namespace Lib;\n", "src/umd.ts").is_empty());
        assert!(facts("const x = 1;\n", "src/plain.ts").is_empty());
    }

    #[test]
    fn a_js_path_records_the_same_facts() {
        let src = "export * from './badge';\nexport function local() {}\n";
        let ts = facts(src, "src/shared.ts");
        assert_eq!(ts, [star("./badge", 0), exports(&["local"])]);
        assert_eq!(facts(src, "src/shared.js"), ts);
        assert_eq!(facts(src, "src/shared.jsx"), ts);
    }
}

#[cfg(test)]
mod value_ref_tests {
    //! DT.5g: module-level const initialisers and class decorator arguments
    //! are walked, a bare identifier passed by value is a USES of the symbol
    //! it binds (rule `value_ref` in-file, a Bare USES ref when imported), and
    //! a `this.<accessor>` read or write is a CALLS of the accessor.
    use super::*;
    use glia_code_domain::evidence::{Basis, Evidence};

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts-value-refs")
    }

    fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn parse(src: &str, path: &str, qname: &str) -> FileParse {
        parse_file(src, path, qname, repo()).expect("parse")
    }

    /// The Bare USES refs `parse` holds from `from`, as `(name, row)`.
    fn uses_refs(parse: &FileParse, from: NodeId) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = parse
            .refs
            .iter()
            .filter(|r| r.from == from && r.category == edge_category::USES)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some((n.clone(), r.line)),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// Every Bare USES ref's name, whoever holds it.
    fn all_uses_ref_names(parse: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = parse
            .refs
            .iter()
            .filter(|r| r.category == edge_category::USES)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// The `(to, rule, row)` of every `category` edge from `from` whose
    /// evidence names `rule`.
    fn ruled(parse: &FileParse, from: NodeId, category: glia_core::EdgeCategoryId, rule: &str) -> Vec<(NodeId, u32)> {
        parse
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == category)
            .filter_map(|e| {
                let ev = Evidence::of(e)?;
                (ev.rule.as_deref() == Some(rule)).then(|| {
                    assert_eq!(ev.basis, Basis::Site, "{rule} is a site");
                    (e.to, ev.line.expect("a site row"))
                })
            })
            .collect()
    }

    fn bare_calls_from(parse: &FileParse, from: NodeId) -> Vec<String> {
        let mut out: Vec<String> = parse
            .calls
            .iter()
            .filter(|c| c.from == from)
            .filter_map(|c| match &c.qualifier {
                CallQualifier::Bare(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    const APP_CONFIG: &str = "\
import { ApplicationConfig } from '@angular/core';
import { HTTP_INTERCEPTORS, provideHttpClient, withInterceptors } from '@angular/common/http';
import { provideRouter } from '@angular/router';
import { routes } from './app.routes';
import { jwtInterceptor } from './core/interceptors/jwt.interceptor';
import { AuthInterceptor } from './core/interceptors/auth.interceptor';
import { provideI18n } from './core/i18n';

export const appConfig: ApplicationConfig = {
  providers: [
    provideRouter(routes),
    provideHttpClient(withInterceptors([jwtInterceptor])),
    { provide: HTTP_INTERCEPTORS, useClass: AuthInterceptor, multi: true },
    provideI18n(),
  ],
};
";

    /// An exported object const's calls are its G19 STATE_VAR's, and the
    /// names it passes by value are Bare USES refs from it (an external
    /// import included: the graph crate leaves that one unbound).
    #[test]
    fn exported_const_initialiser_is_the_state_vars() {
        let p = parse(APP_CONFIG, "src/app/app.config.ts", "src::app::app.config");
        let cfg = id(node_kind::STATE_VAR, "src::app::app.config::appConfig");
        assert_eq!(
            bare_calls_from(&p, cfg),
            ["provideHttpClient", "provideI18n", "provideRouter", "withInterceptors"]
        );
        assert_eq!(
            uses_refs(&p, cfg),
            [
                ("AuthInterceptor".to_string(), 12),
                ("HTTP_INTERCEPTORS".to_string(), 12),
                ("jwtInterceptor".to_string(), 11),
                ("routes".to_string(), 10),
            ]
        );
        let module = id(node_kind::MODULE, "src::app::app.config");
        assert!(bare_calls_from(&p, module).is_empty(), "{:?}", p.calls);
    }

    /// A non-exported const has no STATE_VAR: its initialiser's calls and
    /// value references are the MODULE's, as a top-level statement's are; a
    /// callback parameter (`r`) is a local and binds nothing.
    #[test]
    fn non_exported_const_initialiser_is_the_modules() {
        let src = "\
import { LISTING_ROUTES } from './listing.routes';
function isListingWizard(path: string): boolean { return path.startsWith('w'); }
function byPath(a: any, b: any): number { return 0; }
const LISTING_WIZARD_ROUTES = LISTING_ROUTES.filter(r => isListingWizard(r.path)).sort(byPath);
let later = isListingWizard('x');
";
        let p = parse(src, "src/app/listing.ts", "src::app::listing");
        let module = id(node_kind::MODULE, "src::app::listing");
        let wizard = id(node_kind::FUNCTION, "src::app::listing::isListingWizard");
        let by_path = id(node_kind::FUNCTION, "src::app::listing::byPath");
        let calls = ruled(&p, module, edge_category::CALLS, "intra_file");
        assert_eq!(calls, [(wizard, 3)], "one const call; a `let` is not walked");
        assert_eq!(ruled(&p, module, edge_category::USES, "value_ref"), [(by_path, 3)]);
        assert!(all_uses_ref_names(&p).is_empty(), "LISTING_ROUTES is a callee's object: {:?}", p.refs);
    }

    /// A route table's guard is a USES ref from the `routes` STATE_VAR; a
    /// shorthand property `{ authGuard }` is one too; the lazy route's arrow
    /// parameter `m` binds nothing.
    #[test]
    fn route_guard_and_shorthand_are_refs() {
        let src = "\
import { Routes } from '@angular/router';
import { authGuard, adminGuard } from './core/guards/auth.guard';

export const routes: Routes = [
  { path: 'trades', canActivate: [authGuard], loadComponent: () => import('./home/home.page').then((m) => m.HomePage) },
];
export const guards = { adminGuard };
";
        let p = parse(src, "src/app/app.routes.ts", "src::app::app.routes");
        let routes = id(node_kind::STATE_VAR, "src::app::app.routes::routes");
        let guards = id(node_kind::STATE_VAR, "src::app::app.routes::guards");
        assert_eq!(uses_refs(&p, routes), [("authGuard".to_string(), 4)]);
        assert_eq!(uses_refs(&p, guards), [("adminGuard".to_string(), 6)]);
        assert_eq!(all_uses_ref_names(&p), ["adminGuard", "authGuard"]);
    }

    /// `export default bootstrap` reads the in-file function as a value: a
    /// USES from the MODULE, rule `value_ref`. A default or namespace import
    /// passed by value binds a MODULE and records nothing.
    #[test]
    fn export_default_identifier_and_default_imports() {
        let src = "\
import Widget from './widget';
import * as ns from './ns';
function bootstrap() { register(Widget, ns); }
export default bootstrap;
";
        let p = parse(src, "src/main.ts", "src::main");
        let module = id(node_kind::MODULE, "src::main");
        let boot = id(node_kind::FUNCTION, "src::main::bootstrap");
        assert_eq!(ruled(&p, module, edge_category::USES, "value_ref"), [(boot, 3)]);
        assert!(all_uses_ref_names(&p).is_empty(), "{:?}", p.refs);
        let main = p.nav.nav_facts.get(&module).cloned().unwrap_or_default();
        assert!(
            main.contains(&NavFact::Exports { names: [DEFAULT_EXPORT.to_string()].into() }),
            "DT.1q's default export is kept: {main:?}"
        );
    }

    /// A class decorator's arguments are the class's own code: standalone
    /// `imports`, `providers` and `useClass` are refs from the CLASS, a call
    /// in them (`forwardRef`) is its CallSite, the decorator callee is none,
    /// and the class naming itself (`useExisting`) mints no self-edge.
    #[test]
    fn class_decorator_arguments_are_the_classes() {
        let src = "\
import { Component, forwardRef } from '@angular/core';
import { NG_VALUE_ACCESSOR } from '@angular/forms';
import { TimeAgoPipe } from '../shared/time-ago.pipe';
import { KsInputDirective } from '../shared/ks-input.directive';
import { TradeService, MockTradeService } from '../core/trade.service';

@Component({
  selector: 'ks-home',
  standalone: true,
  imports: [TimeAgoPipe, KsInputDirective],
  providers: [
    { provide: TradeService, useClass: MockTradeService },
    { provide: NG_VALUE_ACCESSOR, useExisting: forwardRef(() => HomePage), multi: true },
    { provide: 'self', useExisting: HomePage },
  ],
  template: '<p></p>',
})
export class HomePage {}
";
        let p = parse(src, "src/app/home/home.page.ts", "src::app::home::home.page");
        let page = id(node_kind::CLASS, "src::app::home::home.page::HomePage");
        let names: Vec<String> = uses_refs(&p, page).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            ["KsInputDirective", "MockTradeService", "NG_VALUE_ACCESSOR", "TimeAgoPipe", "TradeService"]
        );
        assert_eq!(bare_calls_from(&p, page), ["forwardRef"], "the decorator callee is no CallSite");
        assert!(!p.edges.iter().any(|e| e.from == page && e.to == page), "{:?}", p.edges);
    }

    /// A parameter, a local, a nested declaration of the passing scope and
    /// an `inject(T)` argument shadow or bind nothing; a function passed by
    /// value is a USES edge, the comparator control.
    #[test]
    fn shadows_and_inject_bind_nothing() {
        let src = "\
import { inject } from '@angular/core';
import { AuthService } from './auth.service';
interface Item { urgency: number }
export function byUrgency(a: Item, b: Item): number { return b.urgency - a.urgency; }
export function byName(a: Item, b: Item): number { return a.urgency - b.urgency; }
function sortWith(byName: (a: Item, b: Item) => number, items: Item[]): Item[] {
  return [...items].sort(byName);
}
function sortLocal(items: Item[]): Item[] {
  const byUrgency = (a: Item, b: Item) => 0;
  return items.sort(byUrgency);
}
function sortNested(items: Item[]): Item[] {
  function byName(a: Item, b: Item) { return 0; }
  return items.sort(byName);
}
export const authGuard = () => { const auth = inject(AuthService); return auth.ok(); };
export function sorted(items: Item[]): Item[] { return [...items].sort(byUrgency); }
";
        let p = parse(src, "src/app/home/home.page.ts", "src::app::home::home.page");
        let f = |n: &str| id(node_kind::FUNCTION, &format!("src::app::home::home.page::{n}"));
        for scope in ["sortWith", "sortLocal", "sortNested", "authGuard"] {
            assert!(
                !p.edges.iter().any(|e| e.from == f(scope) && e.category == edge_category::USES),
                "{scope}: {:?}",
                p.edges
            );
        }
        assert!(all_uses_ref_names(&p).is_empty(), "inject(AuthService) is DI: {:?}", p.refs);
        assert_eq!(ruled(&p, f("sorted"), edge_category::USES, "value_ref"), [(f("byUrgency"), 17)]);
    }

    /// A `this.<getter>` read (a value read in a template, a member read's
    /// object) is a CALLS of the getter, rule `accessor_read`, one per scope
    /// at its first row; a setter write is a CALLS, rule `accessor_write`; a
    /// plain field write and a never-read getter bind nothing.
    #[test]
    fn accessor_reads_and_writes_are_calls() {
        let src = "\
export class TradeService {
  private base = '';
  private get apiUrl() { return this.base; }
  private get unused() { return 1; }
  get form() { return { value: 1 }; }
  set label(v: string) { this.base = v; }

  list() {
    const a = `${this.apiUrl}/trades`;
    const b = `${this.apiUrl}/other`;
    return this.form.value;
  }
  rename(v: string) {
    this.label = v;
    this.base = v;
  }
}
";
        let p = parse(src, "src/app/core/trade.service.ts", "src::app::core::trade.service");
        let m = |n: &str| id(node_kind::METHOD, &format!("src::app::core::trade.service::TradeService::{n}"));
        let mut reads = ruled(&p, m("list"), edge_category::CALLS, "accessor_read");
        reads.sort_by_key(|(_, row)| *row);
        assert_eq!(reads, [(m("apiUrl"), 8), (m("form"), 10)]);
        assert_eq!(ruled(&p, m("rename"), edge_category::CALLS, "accessor_write"), [(m("label"), 13)]);
        assert!(
            !p.edges.iter().any(|e| e.to == m("unused") && e.category == edge_category::CALLS),
            "{:?}",
            p.edges
        );
        assert!(
            !p.edges.iter().any(|e| e.from == m("label") && e.category == edge_category::CALLS),
            "a plain field write binds nothing: {:?}",
            p.edges
        );
        assert!(
            !p.edges.iter().any(|e| e.category == edge_category::USES),
            "a getter read is never a USES: {:?}",
            p.edges
        );
    }

    /// CH.4 is unchanged: a method passed by value is a USES `method_ref`,
    /// and a getter read beside it is a CALLS, never a USES.
    #[test]
    fn method_by_value_stays_a_method_ref() {
        let src = "\
export class Hero {
  get isDesktop(): boolean { return true; }
  init(): void { window.addEventListener('resize', this.onResize); if (this.isDesktop) {} }
  onResize(): void {}
}
";
        let p = parse(src, "src/hero.ts", "src::hero");
        let m = |n: &str| id(node_kind::METHOD, &format!("src::hero::Hero::{n}"));
        assert_eq!(ruled(&p, m("init"), edge_category::USES, "method_ref"), [(m("onResize"), 2)]);
        assert_eq!(ruled(&p, m("init"), edge_category::CALLS, "accessor_read"), [(m("isDesktop"), 2)]);
    }
}

/// DT.1k: typed GraphQL client calls as `graphql_result` MESSAGE_TYPE cells.
#[cfg(test)]
mod graphql_result_tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts_gql")
    }

    fn parse(src: &str) -> FileParse {
        parse_file(src, "src/inventory.ts", "src::inventory", repo()).unwrap()
    }

    /// The `graphql_result` sites on the node `kind` `qname`.
    fn results(p: &FileParse, kind: glia_core::NodeKindId, qname: &str) -> Vec<wire::ServerWire> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        p.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .filter_map(wire::ServerWire::from_cell)
            .filter(|w| w.form == wire::WireForm::GraphqlResult)
            .collect()
    }

    /// Every `graphql_result` site of the file, on any node.
    fn every(p: &FileParse) -> usize {
        p.nodes
            .iter()
            .flat_map(|n| n.cells.iter())
            .filter_map(wire::ServerWire::from_cell)
            .filter(|w| w.form == wire::WireForm::GraphqlResult)
            .count()
    }

    fn func(name: &str) -> String {
        format!("src::inventory::{name}")
    }

    /// The fixture's two Apollo client calls, awaited, in a file that imports
    /// no GraphQL module (the client is the app's own instance): each callable
    /// carries its result type, its document and the call's 1-based line.
    #[test]
    fn awaited_client_query_and_mutate() {
        let src = "import { client } from './client';\n\
                   import { LIST_ITEMS, ADJUST_STOCK } from './documents';\n\
                   import { InventoryItem } from './types';\n\
                   export async function emptyShelves(): Promise<InventoryItem[]> {\n\
                   \x20 const { data } = await client.query<{ items: InventoryItem[] }>({ query: LIST_ITEMS, variables: { first: 100 } });\n\
                   \x20 return data.items;\n}\n\
                   export async function adjust(id: string): Promise<number> {\n\
                   \x20 const { data } = await client.mutate<{ adjustStock: InventoryItem }>({ mutation: ADJUST_STOCK, variables: { id } });\n\
                   \x20 return data!.adjustStock.onHand;\n}\n";
        let p = parse(src);
        let q = results(&p, node_kind::FUNCTION, &func("emptyShelves"));
        assert_eq!(q.len(), 1, "{q:?}");
        assert_eq!(q[0].document.as_deref(), Some("LIST_ITEMS"));
        assert_eq!(q[0].line, 5);
        assert_eq!(q[0].raw, "{ items: InventoryItem[] }");
        assert_eq!(
            q[0].shape.to_json(),
            Shape::Object {
                fields: vec![ShapeField::new("items", true, Shape::named("InventoryItem[]"))],
                open: false,
            }
            .to_json()
        );
        let payload = q[0].to_payload();
        assert!(payload.contains("\"form\":\"graphql_result\""), "{payload}");
        let m = results(&p, node_kind::FUNCTION, &func("adjust"));
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].document.as_deref(), Some("ADJUST_STOCK"));
        assert!(m[0].to_payload().contains("\"document\":\"ADJUST_STOCK\""));
        assert_eq!(every(&p), 2);
    }

    /// apollo-angular, unawaited, in a class method: `watchQuery` / `mutate` /
    /// `subscribe` with the document under `query`, `mutation` or a
    /// shorthand key; `ns.DOC` is kept as written.
    #[test]
    fn apollo_angular_methods_on_a_class() {
        let src = "import { Apollo } from 'apollo-angular';\n\
                   import * as docs from './documents';\n\
                   export class Stock {\n\
                   \x20 constructor(private apollo: Apollo) {}\n\
                   \x20 list() { return this.apollo.watchQuery<{ items: Item[] }>({ query: docs.GET_ITEMS }).valueChanges; }\n\
                   \x20 bump(query: any) { return this.apollo.subscribe<{ bumped: Item }>({ query }); }\n\
                   \x20 save() { return this.apollo.mutate<{ save: Item }>({ mutation: SAVE }); }\n\
                   }\n";
        let p = parse(src);
        let m = |n: &str| results(&p, node_kind::METHOD, &format!("src::inventory::Stock::{n}"));
        let docs = |n: &str| m(n).iter().map(|w| w.document.clone().unwrap_or_default()).collect::<Vec<_>>();
        assert_eq!(docs("list"), ["docs.GET_ITEMS"]);
        assert_eq!(docs("bump"), ["query"]);
        assert_eq!(docs("save"), ["SAVE"]);
    }

    /// Hooks need their import: a named `useQuery` / `useMutation` (aliased
    /// too) from `@apollo/client` or urql, or `ns.useQuery` on a namespace
    /// import of one; react-query's `useQuery` writes nothing.
    #[test]
    fn hooks_take_the_import_gate() {
        let src = "import { useQuery, useMutation as useM } from '@apollo/client';\n\
                   import * as Apollo from '@apollo/client/react';\n\
                   export function List() { const { data } = useQuery<{ items: Item[] }>(GET_ITEMS); return data; }\n\
                   export function Save() { const [run] = useM<{ save: Item }>(SAVE); return run; }\n\
                   export function Gen() { return Apollo.useQuery<GetItemsQuery>(GetItemsDocument); }\n";
        let p = parse(src);
        for (f, doc) in [("List", "GET_ITEMS"), ("Save", "SAVE"), ("Gen", "GetItemsDocument")] {
            let r = results(&p, node_kind::FUNCTION, &func(f));
            assert_eq!(r.iter().map(|w| w.document.as_deref()).collect::<Vec<_>>(), [Some(doc)], "{f}");
        }
        let other = "import { useQuery } from '@tanstack/react-query';\n\
                     export function List() { return useQuery<Item[]>(KEY); }\n\
                     export function State() { return useState<Item[]>(INITIAL); }\n";
        assert_eq!(every(&parse(other)), 0, "no GraphQL hook import");
        let urql = "import { useQuery } from 'urql';\n\
                    export function List() { const [res] = useQuery<{ items: Item[] }>({ query: GET_ITEMS }); return res; }\n";
        let r = results(&parse(urql), node_kind::FUNCTION, &func("List"));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].document.as_deref(), Some("GET_ITEMS"));
    }

    /// The options-key gate keeps RxJS `subscribe<T>(next)` and a SQL
    /// client's `query<T>(sql)` out; urql's positional `client.query<T>(DOC)`
    /// needs an urql import; an untyped call, an inline document and a
    /// non-identifier document write nothing.
    #[test]
    fn non_graphql_calls_write_nothing() {
        let src = "import { Observable } from 'rxjs';\n\
                   export function a(obs: Observable<Item>) { obs.subscribe<Item>((x) => x); }\n\
                   export async function b(db: any) { return db.query<Row>(SQL); }\n\
                   export async function c() { return client.query({ query: LIST_ITEMS }); }\n\
                   export async function d() { return client.query<{ items: Item[] }>({ query: gql`query { items { id } }` }); }\n\
                   export async function e() { return client.query<{ items: Item[] }>({ variables: { q } }); }\n\
                   export async function f() { return client.query<{ items: Item[] }>({ query: docs[0] }); }\n";
        assert_eq!(every(&parse(src)), 0);
        let urql = "import { createClient } from '@urql/core';\n\
                    export async function g() { return client.query<{ items: Item[] }>(GET_ITEMS, { first: 1 }).toPromise(); }\n";
        let r = results(&parse(urql), node_kind::FUNCTION, &func("g"));
        assert_eq!(r.iter().map(|w| w.document.as_deref()).collect::<Vec<_>>(), [Some("GET_ITEMS")]);
        let no_urql = "export async function g() { return client.query<{ items: Item[] }>(GET_ITEMS).toPromise(); }\n";
        assert_eq!(every(&parse(no_urql)), 0, "positional needs urql");
    }

    /// A named result type stays a name for the models pass to bind; two
    /// calls of one callable are two sites, sorted by line, each once.
    #[test]
    fn named_result_and_two_calls() {
        let src = "export async function both() {\n\
                   \x20 await client.query<StockList>({ query: STOCK_LIST });\n\
                   \x20 await client.mutate<{ restock: StockItem }>({ mutation: RESTOCK });\n}\n";
        let p = parse(src);
        let r = results(&p, node_kind::FUNCTION, &func("both"));
        assert_eq!(r.iter().map(|w| (w.line, w.ty.as_str())).collect::<Vec<_>>(), [(2, "StockList"), (3, "")]);
        assert_eq!(r[0].shape.to_json(), Shape::named("StockList").to_json());
    }
}

#[cfg(test)]
mod typed_receiver_tests {
    //! DT.5k: what the parser states about a receiver the graph could not
    //! type: an Angular signal query read (`this.form()`), an inline
    //! `inject(T)` (a receiver and DI from the scope that runs it), an
    //! in-file static read `T.X`, a call-result local (`const s = make()`)
    //! with its callee's declared return type, a member access on a typed
    //! receiver, and the `get` / `set` accessors it may run.
    use super::*;
    use glia_code_domain::evidence::Evidence;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://ts-typed-receivers")
    }

    fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    fn parse(src: &str, path: &str, qname: &str) -> FileParse {
        parse_file(src, path, qname, repo()).expect("parse")
    }

    /// The 0-based row of the first source line containing `needle`.
    fn row_of(src: &str, needle: &str) -> u32 {
        src.lines().position(|l| l.contains(needle)).expect("needle in src") as u32
    }

    /// `scope`'s ReceiverType facts as `(receiver, ty)`, sorted.
    fn receiver_types(p: &FileParse, scope: NodeId) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = p
            .nav
            .nav_facts
            .get(&scope)
            .into_iter()
            .flatten()
            .filter_map(|f| match f {
                NavFact::ReceiverType { receiver, ty } => Some((receiver.clone(), ty.clone())),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// Every ReceiverType fact of the parse, whoever holds it.
    fn all_receiver_types(p: &FileParse) -> usize {
        p.nav
            .nav_facts
            .values()
            .flatten()
            .filter(|f| matches!(f, NavFact::ReceiverType { .. }))
            .count()
    }

    /// `scope`'s member access facts as `(receiver, name, row, write)`,
    /// sorted.
    fn member_facts(p: &FileParse, scope: NodeId) -> Vec<(String, String, u32, bool)> {
        let mut out: Vec<(String, String, u32, bool)> = p
            .nav
            .nav_facts
            .get(&scope)
            .into_iter()
            .flatten()
            .filter_map(|f| match f {
                NavFact::MemberRead { receiver, name, line } => {
                    Some((receiver.clone(), name.clone(), *line, false))
                }
                NavFact::MemberWrite { receiver, name, line } => {
                    Some((receiver.clone(), name.clone(), *line, true))
                }
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    fn quad(r: &str, n: &str, row: u32, write: bool) -> (String, String, u32, bool) {
        (r.to_string(), n.to_string(), row, write)
    }

    fn pair(r: &str, t: &str) -> (String, String) {
        (r.to_string(), t.to_string())
    }

    /// The Bare refs of `category` from `from`, as `(name, row)`, sorted.
    fn bare_refs(p: &FileParse, from: NodeId, category: glia_core::EdgeCategoryId) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = p
            .refs
            .iter()
            .filter(|r| r.from == from && r.category == category)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some((n.clone(), r.line)),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// The `(from, to, row)` of every `category` edge whose evidence rule is
    /// `rule`.
    fn ruled(p: &FileParse, category: glia_core::EdgeCategoryId, rule: &str) -> Vec<(NodeId, NodeId, u32)> {
        p.edges
            .iter()
            .filter(|e| e.category == category)
            .filter_map(|e| {
                let ev = Evidence::of(e)?;
                (ev.rule.as_deref() == Some(rule)).then(|| (e.from, e.to, ev.line.expect("a site row")))
            })
            .collect()
    }

    const QUERIES: &str = "\
import { Component, ViewContainerRef, contentChild, contentChildren, viewChild, viewChildren } from '@angular/core';
import { Form } from './form';
import { Toggle } from './toggle';
import { Zone } from './zone';
import { Panel } from './panel';
import { Item } from './item';

@Component({ selector: 'app-p', standalone: true, template: '' })
export class P {
  private readonly a = viewChild(Form);
  private readonly b = viewChild<Toggle>('b');
  private readonly c = viewChild.required(Zone);
  private readonly d = contentChild(Panel);
  private readonly e = contentChild.required<Item>('e');
  private readonly f = viewChild(Zone, { read: ViewContainerRef });
  private readonly g = viewChild<Zone, ViewContainerRef>('g', { read: ViewContainerRef });
  private readonly h = viewChildren(Form);
  private readonly i = contentChildren(Panel);
  private readonly j = viewChild('j');
  private readonly k = viewChild(Zone, { read: 'el' });

  run(): void {
    this.a()?.reset();
    this.b()?.sync(true);
    this.c().clear();
    this.d()?.open();
    this.e().pick();
    this.f()?.clear();
    this.g()?.clear();
    this.h().forEach((x) => x.reset());
    this.i().map((x) => x);
    this.j()?.clear();
    this.k()?.clear();
    this.a()!.submit();
  }
}
";

    /// Each single-result query names the class its read returns, on the
    /// method that reads it: the locator, the `<T>` argument, the `.required`
    /// forms; a `read` option wins over both (also as a second type
    /// argument). A list query, a string locator with no `<T>` and a
    /// non-class `read` type nothing; nor does a project-local `viewChild`.
    /// An aliased import types.
    #[test]
    fn signal_query_fields_type_their_reads() {
        let p = parse(QUERIES, "src/p.page.ts", "src::p.page");
        let run = id(node_kind::METHOD, "src::p.page::P::run");
        assert_eq!(
            receiver_types(&p, run),
            [
                pair("this.a()", "Form"),
                pair("this.a()!", "Form"),
                pair("this.b()", "Toggle"),
                pair("this.c()", "Zone"),
                pair("this.d()", "Panel"),
                pair("this.e()", "Item"),
                pair("this.f()", "ViewContainerRef"),
                pair("this.g()", "ViewContainerRef"),
            ]
        );
        assert_eq!(all_receiver_types(&p), 8, "nothing for h / i / j / k");

        let local = "\
import { Form } from './form';
function viewChild<T>(t: T): () => T { return () => t; }
export class Q {
  private readonly a = viewChild(Form);
  run(): void { this.a()?.reset(); }
}
";
        assert_eq!(all_receiver_types(&parse(local, "src/q.ts", "src::q")), 0);

        let aliased = "\
import { viewChild as vc } from '@angular/core';
import { Form } from './form';
export class R {
  private readonly a = vc(Form);
  run(): void { this.a()?.reset(); }
}
";
        let p = parse(aliased, "src/r.ts", "src::r");
        assert_eq!(
            receiver_types(&p, id(node_kind::METHOD, "src::r::R::run")),
            [pair("this.a()", "Form")]
        );
    }

    /// `inject(T).m()` and `inject<Api>(TOKEN).m()` type their receiver on
    /// the scope; a project-local `inject` types nothing.
    #[test]
    fn inline_inject_receivers() {
        let src = "\
import { inject } from '@angular/core';
import { Api, Store, TOKEN } from './api';
export function f(): void {
  inject(Store).load();
  inject<Api>(TOKEN).get();
}
";
        let p = parse(src, "src/f.ts", "src::f");
        assert_eq!(
            receiver_types(&p, id(node_kind::FUNCTION, "src::f::f")),
            [pair("inject(Store)", "Store"), pair("inject<Api>(TOKEN)", "Api")]
        );

        let local = "\
import { Store } from './api';
function inject<T>(t: T): T { return t; }
export function g(): void { inject(Store).load(); }
";
        assert_eq!(all_receiver_types(&parse(local, "src/g.ts", "src::g")), 0);
    }

    /// An `inject(T)` that is no field's whole initialiser is DI from the
    /// scope that runs it: a Bare INJECTS ref for an import, an INJECTS edge
    /// (rule `inject_call`) for an in-file class. Only the first argument is
    /// the token; a whole-initialiser field stays A7.1's class ref; a class
    /// injecting itself, a shadowed name and a non-import `inject` inject
    /// nothing.
    #[test]
    fn inline_inject_is_injects() {
        let src = "\
import { inject } from '@angular/core';
import { AuthService } from './auth';
import { OPTS } from './opts';
export function guard(): boolean {
  const a = inject(AuthService);
  return a.ok;
}
export function optional(): boolean {
  const b = inject(AuthService, OPTS);
  return b.ok;
}
";
        let p = parse(src, "src/guard.ts", "src::guard");
        let guard = id(node_kind::FUNCTION, "src::guard::guard");
        let optional = id(node_kind::FUNCTION, "src::guard::optional");
        assert_eq!(
            bare_refs(&p, guard, edge_category::INJECTS),
            [("AuthService".to_string(), row_of(src, "inject(AuthService);"))]
        );
        assert!(bare_refs(&p, guard, edge_category::USES).is_empty(), "the token is no value");
        let row = row_of(src, "inject(AuthService, OPTS)");
        assert_eq!(
            bare_refs(&p, optional, edge_category::INJECTS),
            [("AuthService".to_string(), row)]
        );
        assert_eq!(bare_refs(&p, optional, edge_category::USES), [("OPTS".to_string(), row)]);

        let field = "\
import { Component, inject } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { AuthService } from './auth';
import { CountriesService } from './countries';
export class C {
  private readonly auth = inject(AuthService);
  readonly countries = toSignal(inject(CountriesService).list());
}
";
        let p = parse(field, "src/c.ts", "src::c");
        let class = id(node_kind::CLASS, "src::c::C");
        let countries = id(node_kind::STATE_VAR, "src::c::C::countries");
        let mut injects: Vec<(NodeId, String)> = p
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some((r.from, n.clone())),
                _ => None,
            })
            .collect();
        injects.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            injects,
            [(class, "AuthService".to_string()), (countries, "CountriesService".to_string())],
            "the whole-initialiser field is A7.1's class ref only"
        );

        let in_file = "\
import { inject } from '@angular/core';
class Local { ping(): void {} }
export class Self {
  static make(): unknown {
    const all = [Self];
    return inject(Self) ?? all;
  }
}
export function h(): void { inject(Local).ping(); }
";
        let p = parse(in_file, "src/h.ts", "src::h");
        let h = id(node_kind::FUNCTION, "src::h::h");
        let local = id(node_kind::CLASS, "src::h::Local");
        let class = id(node_kind::CLASS, "src::h::Self");
        let make = id(node_kind::METHOD, "src::h::Self::make");
        assert_eq!(
            ruled(&p, edge_category::INJECTS, "inject_call"),
            [(h, local, row_of(in_file, "inject(Local)"))],
            "a class's own member injecting it vouches for nothing"
        );
        assert_eq!(
            ruled(&p, edge_category::USES, "value_ref"),
            [(make, class, row_of(in_file, "[Self]"))],
            "the same member's by-value read keeps its USES"
        );

        let shadowed = "\
import { inject } from '@angular/core';
import { AuthService, Token } from './auth';
export function s(AuthService: Token): void { inject(AuthService); }
";
        let p = parse(shadowed, "src/s.ts", "src::s");
        let s = id(node_kind::FUNCTION, "src::s::s");
        assert!(bare_refs(&p, s, edge_category::INJECTS).is_empty());
        assert!(bare_refs(&p, s, edge_category::USES).is_empty());

        let not_angular = "\
import { AuthService } from '../core/auth.service';
function inject<T>(token: T): T { return token; }
export function probe(): boolean { return inject(AuthService) !== null; }
";
        let p = parse(not_angular, "src/probe.ts", "src::probe");
        let probe = id(node_kind::FUNCTION, "src::probe::probe");
        assert!(p.refs.iter().all(|r| r.category != edge_category::INJECTS));
        assert_eq!(
            bare_refs(&p, probe, edge_category::USES),
            [("AuthService".to_string(), row_of(not_angular, "inject(AuthService)"))]
        );
    }

    /// An in-file `T.X` read from outside `T` USES the class (rule
    /// `static_member`); `T`'s own members and a local shadow read nothing;
    /// an imported base keeps LA.30c's Attribute ref.
    #[test]
    fn static_member_reads() {
        let src = "\
import { Imported } from './imported';
export class Limits {
  static readonly MAX = 3;
  static clamp(n: number): number { return Math.min(n, Limits.MAX); }
}
export function f(): number { return Limits.MAX; }
export function g(): number { const Limits = { MAX: 1 }; return Limits.MAX; }
export function h(): number { return Imported.MAX; }
";
        let p = parse(src, "src/limits.ts", "src::limits");
        let f = id(node_kind::FUNCTION, "src::limits::f");
        let limits = id(node_kind::CLASS, "src::limits::Limits");
        assert_eq!(
            ruled(&p, edge_category::USES, "static_member"),
            [(f, limits, row_of(src, "function f()"))]
        );
        let h = id(node_kind::FUNCTION, "src::limits::h");
        let attrs: Vec<(NodeId, String, String)> = p
            .refs
            .iter()
            .filter(|r| r.category == edge_category::USES)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Attribute { base, name } => Some((r.from, base.clone(), name.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(attrs, [(h, "Imported".to_string(), "MAX".to_string())]);
    }

    /// `const s = make()` records the call form `make()`; a member call's
    /// result, a parameter or nested function named like the callee and a
    /// self-named callee record the empty type. `[ts-locals]` counts the
    /// call-typed sites. A class-shaped return annotation is a return type.
    #[test]
    fn call_result_locals() {
        let src = "\
function make(): Store { return new Store(); }
export class C {
  run(): void {
    const s = make();
    const t = this.make();
    const u = a.b();
    s.go(); t.go(); u.go();
  }
  make(): Store { return new Store(); }
}
export function p(make: () => Store): void { const v = make(); v.go(); }
export function q(): void { function make(): Store { return new Store(); } const w = make(); w.go(); }
export function r(): void { const make = make(); make.go(); }
";
        let (p, _, _, _, stats, ..) = parse_file_stats(src, "src/c.ts", "src::c", repo()).unwrap();
        let local = |scope: NodeId, name: &str| {
            p.nav.local_types.get(&scope).and_then(|l| l.get(name)).cloned()
        };
        let run = id(node_kind::METHOD, "src::c::C::run");
        assert_eq!(local(run, "s").as_deref(), Some("make()"));
        assert_eq!(local(run, "t").as_deref(), Some(""));
        assert_eq!(local(run, "u").as_deref(), Some(""));
        assert_eq!(local(id(node_kind::FUNCTION, "src::c::p"), "v").as_deref(), Some(""));
        assert_eq!(local(id(node_kind::FUNCTION, "src::c::q"), "w").as_deref(), Some(""));
        assert_eq!(local(id(node_kind::FUNCTION, "src::c::r"), "make").as_deref(), Some(""));
        assert_eq!((stats.call, stats.typed), (1, 1), "{stats:?}");

        let returns = "\
export function a(): Store { return x; }
export const b = (): Store => x;
export function c(): Store | null { return null; }
export function d(): ns.Foo { return x; }
export function e(): Box<number> { return x; }
export function f(): Promise<Store> { return x; }
export function g(): string { return ''; }
export function h() { return x; }
export function i(): Store[] { return []; }
export function j(): A | B { return x; }
";
        let p = parse(returns, "src/r.ts", "src::r");
        let ret = |name: &str| {
            p.nav.return_types.get(&id(node_kind::FUNCTION, &format!("src::r::{name}"))).cloned()
        };
        let got: Vec<(&str, Option<String>)> =
            ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"].into_iter().map(|n| (n, ret(n))).collect();
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            got,
            [
                ("a", some("Store")),
                ("b", some("Store")),
                ("c", some("Store")),
                ("d", some("Foo")),
                ("e", some("Box")),
                ("f", some("Promise")),
                ("g", None),
                ("h", None),
                ("i", None),
                ("j", None),
            ]
        );
    }

    const READS: &str = "\
import { Component, inject, viewChild } from '@angular/core';
import { Auth } from './auth';
import { Form } from './form';
import { environment } from './env';

@Component({ selector: 'app-c', standalone: true, template: '' })
export class C {
  private readonly injected = inject(Auth);
  private annotated: Auth;
  private loose;
  private readonly form = viewChild(Form);

  constructor(private readonly auth: Auth) {}

  run(local: Auth, other): void {
    const x = this.auth.isIn;
    const y = this.injected.isIn;
    const z = this.annotated.isIn;
    const w = local.isIn;
    const v = inject(Auth).isIn;
    const d = this.form()?.dirty;
    this.auth.session = 't';
    const n1 = other.isIn;
    const n2 = this.loose.isIn;
    const n3 = environment.apiUrl;
    const n4 = this.isIn;
    this.auth.check();
    const again = this.auth.isIn;
  }
}
";

    /// A member access that is no callee is a MemberRead (an assignment
    /// target a MemberWrite) when the parser types its receiver: a declared
    /// field (constructor parameter, `inject(T)` field, annotation), a typed
    /// local, a ReceiverType expression; one per key, at its first row.
    /// Nothing for an untyped local or field, an import base, `this.x`, a
    /// callee.
    #[test]
    fn member_reads_record_typed_receivers() {
        let p = parse(READS, "src/c.component.ts", "src::c.component");
        let run = id(node_kind::METHOD, "src::c.component::C::run");
        let at = |needle: &str| row_of(READS, needle);
        let mut want = vec![
            quad("this.auth", "isIn", at("const x ="), false),
            quad("this.injected", "isIn", at("const y ="), false),
            quad("this.annotated", "isIn", at("const z ="), false),
            quad("local", "isIn", at("const w ="), false),
            quad("inject(Auth)", "isIn", at("const v ="), false),
            quad("this.form()", "dirty", at("const d ="), false),
            quad("this.auth", "session", at("this.auth.session ="), true),
        ];
        want.sort();
        assert_eq!(member_facts(&p, run), want);
    }

    /// `get` / `set` declare a Getter / Setter on their METHOD (a pair, both;
    /// `abstract get`, a Getter); a method named `get` declares neither.
    #[test]
    fn accessor_facts() {
        let src = "\
export abstract class A {
  get g(): number { return 1; }
  set s(v: number) {}
  get both(): number { return 1; }
  set both(v: number) {}
  abstract get ab(): number;
  get(): number { return 0; }
  set(): void {}
}
";
        let p = parse(src, "src/a.ts", "src::a");
        let facts = |m: &str| -> Vec<NavFact> {
            p.nav
                .nav_facts
                .get(&id(node_kind::METHOD, &format!("src::a::A::{m}")))
                .into_iter()
                .flatten()
                .filter(|f| matches!(f, NavFact::Getter | NavFact::Setter))
                .cloned()
                .collect()
        };
        assert_eq!(facts("g"), [NavFact::Getter]);
        assert_eq!(facts("s"), [NavFact::Setter]);
        assert_eq!(facts("both"), [NavFact::Getter, NavFact::Setter]);
        assert_eq!(facts("ab"), [NavFact::Getter]);
        assert!(facts("get").is_empty());
        assert!(facts("set").is_empty());
    }
}
