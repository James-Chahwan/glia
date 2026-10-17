use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

use glia_code_domain::NavFact;
pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
// A3.5: `is_http_client_receiver` / `ident_before` were defined here first and
// now live in code_domain::endpoint, shared with ts_routes' Express scan. An
// empty receiver (a `..get` cascade) is not a client, so it still falls
// through to ROUTE emission in `scan_dart_routes`.
use glia_code_domain::endpoint::{
    ClientEndpoint, HitExtras, client_url_split, ident_before, is_http_client_receiver,
    normalise_client_path, push_client_endpoint_with,
};
use glia_code_domain::wire::{self, ModelField, ModelSchema, Shape, ShapeField, WireFormat};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
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

    // CH.5c: the receivers bound to a Dio, read before the body walks (the
    // client-call detector) and the route scan consult them.
    acc.dio_receivers = collect_dio_receivers(source, root, src);
    // DH.15: read before the class walk, which emits INJECTS only in a file
    // importing injectable.
    acc.injectable_import = imports_injectable(root, src);
    // DT.5e: read before the class walk, which tags a widget / State
    // subclass a COMPONENT only in a file importing a Flutter library.
    acc.flutter_import = imports_any(root, src, FLUTTER_IMPORT_PREFIXES);
    // DT.1l: read before the class walk, which writes no model schema in a
    // generated file.
    acc.generated = dart_generated(file_rel_path, source);
    visit_top(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    // DT.5e: every member is declared now, so a name passed by value can be
    // checked against the METHOD / FUNCTION it would name.
    resolve_name_refs(&mut acc, module_qname, repo);
    // DT.5h: every closure parameter is typed now, so a member read on a
    // local is kept only when the walk typed that local.
    finish_member_reads(&mut acc);
    if let Some(line) = acc.di.marker(file_rel_path) {
        eprintln!("{line}");
    }
    if let Some(line) = acc.flutter.marker(file_rel_path) {
        eprintln!("{line}");
    }
    let shelf = scan_dart_routes(source, repo, module_id, &mut acc);
    if let Some(line) = shelf.marker(file_rel_path) {
        eprintln!("{line}");
    }
    if let Some(line) = acc.dio_bound.marker(acc.dio_receivers.len(), file_rel_path) {
        eprintln!("{line}");
    }
    if let Some(line) = acc.wire.marker(file_rel_path) {
        eprintln!("{line}");
    }

    // CH.5a: the client base URL inputs of the endpoint fold (CH.5c), as
    // build-time facts on this file's MODULE, and their fired_on marker.
    let base_stats = collect_dio_base_facts(source, root, module_id, &mut acc);
    if let Some(line) = base_stats.marker(file_rel_path) {
        eprintln!("{line}");
    }

    // LA.34 fired_on marker: this file had an unqualified call that Dart's
    // lexical scope decided (a class member or a local binding); DT.5e: or a
    // closure walked for its member, a `const` / `new` creation site, or a
    // method / function passed by name; DT.5h: or a getter / setter named, a
    // constructor tear-off, a field initialiser walked, a member read kept
    // for the graph or an element-typed closure parameter.
    let s = &acc.bare_calls;
    if s.fired() {
        eprintln!(
            "[dart-calls] self={} bare={} local_skip={} closures={} creations={} method_refs={} \
             accessor_refs={} tear_offs={} field_inits={} member_reads={} elements={} \
             file={file_rel_path}",
            s.self_calls,
            s.bare,
            s.local_skip,
            s.closures,
            s.creations,
            s.method_refs,
            s.accessor_refs,
            s.tear_offs,
            s.field_inits,
            s.member_reads,
            s.elements
        );
    }

    // CB.9 stopgap marker (GLIA_DART_DEBUG=1): `Type.member(..)` calls on a
    // type this file declares, bound through the receiver-type pass.
    if dart_debug_enabled() && acc.type_receivers > 0 {
        eprintln!(
            "[dart-type-receivers] recorded={} file={file_rel_path}",
            acc.type_receivers
        );
    }

    // LA.37a fired_on marker (GLIA_DART_DEBUG=1): this file declared top-level
    // functions / getters / setters, whose sibling bodies were walked, or
    // (CB.17) top-level variables whose initialisers were walked.
    let t = &acc.top_level;
    if dart_debug_enabled() && t.bodies + t.bodyless + t.initialisers > 0 {
        eprintln!(
            "[dart-top-level] bodies={} accessors={} bodyless={} initialisers={} \
             file={file_rel_path}",
            t.bodies, t.accessors, t.bodyless, t.initialisers
        );
    }

    // LA.37b / CB.9 / CB.17 fired_on marker (GLIA_DART_DEBUG=1): this file
    // declared member containers beyond a plain class (CB.17: an unnamed
    // extension on a type declared elsewhere is one), getter / setter bodies,
    // or the members CB.9 made METHODs (constructors / factories, operators,
    // bodiless members) and enum constants it made ATTRIBUTEs.
    let m = &acc.members;
    if dart_debug_enabled() && m.fired() {
        eprintln!(
            "[dart-members] mixins={} extensions={} extension_types={} unnamed_ext_containers={} \
             enum_members={} accessors={} ctors={} operators={} abstract={} enum_constants={} \
             file={file_rel_path}",
            m.mixins,
            m.extensions,
            m.extension_types,
            m.unnamed_ext_containers,
            m.enum_members,
            m.accessors,
            m.ctors,
            m.operators,
            m.bodiless,
            m.enum_constants
        );
    }

    Ok(FileParse {
        nodes: acc.nodes,
        edges: acc.edges,
        imports: acc.imports,
        calls: acc.calls,
        refs: acc.refs,
        nav: acc.nav,
        // DT.5h: the getter / setter METHODs and FUNCTIONs, the
        // property-style channel the Dart graph build reads a member read
        // through (a getter read is a CALLS, any other METHOD read a tear-off).
        properties: acc.accessor_ids,
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
    /// Dedup for client-HTTP ENDPOINT nodes (Pattern A) — one node per
    /// (method, path) even if the same endpoint is called twice in a file.
    endpoint_seen: HashSet<NodeId>,
    /// LA.34: how this file's unqualified calls inside class members were
    /// classified, for the `[dart-calls]` marker.
    bare_calls: BareCallStats,
    /// LA.37a: the declaration ids this file has already pushed as a Node. A
    /// top-level getter / setter pair shares one qname and so one NodeId: the
    /// first declaration in source order pushes the Node, its DEFINES edge and
    /// its nav record; the second only walks its body under the same id.
    /// Lookup-only, never iterated into output.
    declared_ids: HashSet<NodeId>,
    /// LA.37a: this file's top-level declarations, for the `[dart-top-level]`
    /// marker.
    top_level: TopLevelStats,
    /// LA.37b: this file's member containers and member bodies, for the
    /// `[dart-members]` marker.
    members: MemberStats,
    /// CB.9 stopgap ([`record_type_receiver`]): the class / mixin / enum /
    /// extension-type names this file declares. Lookup-only.
    type_names: HashSet<String>,
    /// CB.9 stopgap: `Type.member(..)` call sites whose same-file type base
    /// was recorded as a typed name of the caller's scope, for the
    /// `[dart-type-receivers]` marker.
    type_receivers: usize,
    /// CH.5c: the names this file binds to a Dio client
    /// ([`collect_dio_receivers`]). Lookup-only.
    dio_receivers: HashSet<String>,
    /// CH.5c: client calls / route-scan hits on a [`Acc::dio_receivers`]
    /// name the name heuristic misses, for the `[dart-dio-receivers]` marker.
    dio_bound: DioBoundStats,
    /// DH.15: this file imports `package:injectable/` ([`imports_injectable`]),
    /// so an injectable annotation on one of its classes is get_it's.
    injectable_import: bool,
    /// DH.15: injectable classes and their INJECTS refs, for the `[dart-di]`
    /// marker.
    di: DiStats,
    /// DT.5e: this file imports a [`FLUTTER_IMPORT_PREFIXES`] library, so a
    /// class extending a [`FLUTTER_COMPONENT_BASES`] name is Flutter's.
    flutter_import: bool,
    /// DT.5e: the widget / State classes tagged COMPONENT, for the
    /// `[dart-flutter]` marker.
    flutter: FlutterStats,
    /// DT.5e: the getter / setter METHODs and FUNCTIONs this file declares,
    /// which a name passed by value reads rather than uses. Lookup-only.
    accessor_ids: HashSet<NodeId>,
    /// DT.5e: the names passed by value ([`record_name_ref`]), in walk order,
    /// bound once the whole file is declared ([`resolve_name_refs`]).
    name_refs: Vec<NameRef>,
    /// DT.5h: the `Type.member` / `x.member` reads no call follows
    /// ([`push_chain_reads`]), in walk order, recorded for the graph once the
    /// walk has typed every closure parameter ([`finish_member_reads`]).
    member_reads: Vec<PendingRead>,
    /// DT.1l: the file is generated ([`dart_generated`]): it writes no model
    /// schema.
    generated: bool,
    /// DT.1l: the models and Dio shapes this file wrote, for the
    /// `[dart-wire]` marker.
    wire: DartWireStats,
    /// DT.1l: how many Dio calls each callable body holds, keyed by the body's
    /// tree-sitter node id ([`dio_calls_in`]). Lookup-only.
    dio_calls: HashMap<usize, usize>,
}

/// DT.5e: an identifier naming a member or same-file function by value
/// (`onPressed: _save`; DT.5h: in any value position, and `this.x` with no
/// call after it), as the scope of its body classified it.
struct NameRef {
    /// The member / function / variable whose body holds the identifier.
    from: NodeId,
    target: NameTarget,
    /// The identifier's 0-based row.
    line: u32,
    /// DT.5h: the identifier is the whole left side of an assignment
    /// (`_mode = 1`, `_n += 1`, [`is_assignment_target`]): a setter runs.
    write: bool,
}

/// DT.5h: one member read no call follows, its receiver as the graph will
/// type it ([`push_chain_reads`]).
struct PendingRead {
    /// The scope whose body holds the read.
    from: NodeId,
    /// A capitalised library-scope name (a type), `this.<field>` for a
    /// member of the enclosing type, or a local's name.
    receiver: String,
    /// The member read.
    name: String,
    /// The member selector's 0-based row.
    line: u32,
    /// The receiver is a local: kept only when the walk typed it.
    local: bool,
}

enum NameTarget {
    /// A member of the enclosing class: the qname of the METHOD it would be.
    Member(String),
    /// Neither a local nor a member: a library-scope name.
    Library(String),
}

/// DT.5e: the classes [`visit_class`] tagged a Flutter COMPONENT.
#[derive(Default)]
struct FlutterStats {
    /// Subclasses of a widget base (everything but a State).
    widgets: usize,
    /// Subclasses of `State` / `ConsumerState`.
    states: usize,
}

impl FlutterStats {
    fn marker(&self, file_rel: &str) -> Option<String> {
        let n = self.widgets + self.states;
        (n > 0).then(|| {
            format!(
                "[dart-flutter] components={n} (widgets={} states={}) file={file_rel}",
                self.widgets, self.states
            )
        })
    }
}

#[derive(Default)]
struct MemberStats {
    /// `mixin M { }` declarations emitted as a CLASS.
    mixins: usize,
    /// Named extensions (a CLASS of their own) plus unnamed extensions whose
    /// members hang on a type this file declares.
    extensions: usize,
    /// `extension type T(..) { }` declarations emitted as a CLASS.
    extension_types: usize,
    /// CB.17: unnamed extensions on a type this file does not declare, each a
    /// CLASS container `<module>::extension<T>` ([`visit_extension`]).
    unnamed_ext_containers: usize,
    /// Enum members that declared a METHOD (constructors included).
    enum_members: usize,
    /// Member getter / setter bodies, each credited to its METHOD.
    accessors: usize,
    /// CB.9: constructor / factory members, each a METHOD `<T>::<T>` or
    /// `<T>::<name>`, with a body or without.
    ctors: usize,
    /// CB.9: operator members, each a METHOD `<T>::operator<op>`.
    operators: usize,
    /// CB.9: bodiless non-constructor members (abstract or `external`
    /// methods, getters, setters and operators), each a METHOD with no calls.
    /// The marker's `abstract=` field.
    bodiless: usize,
    /// CB.9: enum constants, each an ATTRIBUTE `<Enum>::<constant>`.
    enum_constants: usize,
}

impl MemberStats {
    fn fired(&self) -> bool {
        self.mixins
            + self.extensions
            + self.extension_types
            + self.unnamed_ext_containers
            + self.enum_members
            + self.accessors
            + self.ctors
            + self.operators
            + self.bodiless
            + self.enum_constants
            > 0
    }

    /// Count one declared member of `kind`; `bodiless` when it came from a
    /// `declaration` (a member with no `function_body`).
    fn count(&mut self, kind: MemberKind, bodiless: bool) {
        match kind {
            MemberKind::Ctor => self.ctors += 1,
            MemberKind::Operator => self.operators += 1,
            MemberKind::Method | MemberKind::Accessor => {}
        }
        if bodiless && kind != MemberKind::Ctor {
            self.bodiless += 1;
        }
    }
}

#[derive(Default)]
struct TopLevelStats {
    /// Top-level signatures whose sibling `function_body` was walked.
    bodies: usize,
    /// Top-level `getter_signature` + `setter_signature` declarations.
    accessors: usize,
    /// Top-level signatures with no body (`external`).
    bodyless: usize,
    /// CB.17: top-level variables (`const` / `final` G19 lists and `var` /
    /// typed / `late` lists) whose initialiser was walked for calls from
    /// their STATE_VAR.
    initialisers: usize,
}

/// `GLIA_DART_DEBUG=1` turns on the `[dart-top-level]` and `[dart-members]`
/// markers, read once. Off by default: nearly every Dart file declares a
/// top-level function or a getter, and parsers run per file inside a
/// panic-suppressed loop that must stay quiet on a normal build.
fn dart_debug_enabled() -> bool {
    static DART_DEBUG: OnceLock<bool> = OnceLock::new();
    *DART_DEBUG
        .get_or_init(|| std::env::var("GLIA_DART_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0"))
}

#[derive(Default)]
struct BareCallStats {
    /// `<id>(..)` naming a member of the enclosing class -> SelfMethod.
    self_calls: usize,
    /// `<id>(..)` naming nothing the class or the member binds -> Bare.
    bare: usize,
    /// `<id>(..)` naming a parameter or local -> no call site.
    local_skip: usize,
    /// DT.5e: closures (`function_expression`) walked for their enclosing
    /// member, function or variable.
    closures: usize,
    /// DT.5e: `const T(..)` / `new T(..)` / `const T.named(..)` call sites.
    creations: usize,
    /// DT.5e: names passed by value that became a USES edge.
    method_refs: usize,
    /// DT.5h: getter / setter names read or written that became a CALLS
    /// edge (rule `accessor_read` / `accessor_write`).
    accessor_refs: usize,
    /// DT.5h: constructor tear-offs (`T.new`, `T<A>.new`) recorded as a
    /// call site of T.
    tear_offs: usize,
    /// DT.5h: class field initialisers walked for the owner type's calls.
    field_inits: usize,
    /// DT.5h: member reads recorded as a `NavFact::MemberRead`.
    member_reads: usize,
    /// DT.5h: closure parameters typed as a field's element type.
    elements: usize,
}

impl BareCallStats {
    /// The `[dart-calls]` marker fires when any count but `bare` moved.
    fn fired(&self) -> bool {
        self.self_calls
            + self.local_skip
            + self.closures
            + self.creations
            + self.method_refs
            + self.accessor_refs
            + self.tear_offs
            + self.field_inits
            + self.member_reads
            + self.elements
            > 0
    }
}

fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // LA.37b: the types this file declares and its library-level names, for
    // an unnamed extension that hangs its members on a same-file type.
    let file_types = FileTypes {
        types: declared_types(node, src, parent_qname, repo),
        library: library_names(node, src),
    };
    acc.type_names = file_types.types.keys().cloned().collect();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_or_export" => collect_import(child, src, parent_qname, acc),
            "class_declaration" => {
                visit_class(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "enum_declaration" => {
                visit_enum(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            // LA.37b: a mixin and an extension type are CLASS nodes owning
            // their members, like a class.
            "mixin_declaration" | "extension_type_declaration" => {
                visit_container(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            "extension_declaration" => {
                visit_extension(child, &file_types, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            // LA.37a: a top-level function / getter / setter's body is the
            // signature's SIBLING under the program root, not its child. The
            // signature claims it here, so a `function_body` child falls
            // through to `_ => {}`.
            "function_signature" | "getter_signature" | "setter_signature" => {
                let body = sibling_body(child);
                visit_function(child, body, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            // G19 — library-level `const`/`final NAME = expr;`. The hidden
            // `_top_level_definition` rule inlines the keyword + this list as
            // direct children of the program root.
            "static_final_declaration_list" => {
                visit_top_level_consts(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            // CB.17 — library-level `var a = .., b = f();`, and the typed /
            // `late` / `late final` forms, which the grammar puts in this
            // list (beside their keyword / type tokens), not the G19 one.
            "initialized_identifier_list" => {
                visit_top_level_vars(child, src, file_rel, parent_qname, parent_id, repo, acc);
            }
            _ => {}
        }
    }
}

fn visit_class(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = find_identifier(node, src) else {
        return;
    };
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, &qname);

    // DT.1l: where the CLASS node sits, for its SCHEMA_FIELDS cell.
    let class_ix = acc.nodes.len();
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
    acc.nav.record(id, &name, &qname, node_kind::CLASS, Some(parent_id));
    // LA.37b: a later container of the same name adds no second Node.
    acc.declared_ids.insert(id);

    // DT.5e: a Flutter widget / State subclass is a framework component, so
    // a COMPONENT overlay of its qname, which graph::roles folds into ROLE
    // COMPONENT on this CLASS (as angular.rs tags an `@Component`).
    if acc.flutter_import
        && let Some(base) = flutter_component_base(node, src)
    {
        let overlay = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::COMPONENT, &qname);
        acc.nodes.push(Node {
            id: overlay,
            repo,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav.record(overlay, &name, &qname, node_kind::COMPONENT, Some(parent_id));
        if FLUTTER_STATE_BASES.contains(&base) {
            acc.flutter.states += 1;
        } else {
            acc.flutter.widgets += 1;
        }
    }

    // G12.5 — heritage: `extends Y` → INHERITS_FROM (superclass);
    // `implements I` and `with M` → IMPLEMENTS (interface/mixin). A6.5: emitted
    // as refs from this file's MODULE (`parent_id`), bound by the graph crate.
    visit_class_heritage(node, src, id, parent_id, acc);

    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() == "class_body" {
            // LA.34: the names this class declares, which an unqualified call
            // in any of its members reaches before library scope.
            let members = container_member_names(node, child, src);
            let elems = collect_dart_field_elem_types(child, src);
            let owner = Owner {
                qname: &qname,
                id,
                members: &members,
                elems: &elems,
            };
            visit_members(child, src, file_rel, &owner, repo, acc);
            // LA.23e: declared / constructor-initialised field types, for
            // A6.2a's receiver-type pass.
            collect_dart_field_types(child, src, id, acc);
            // DH.15: an injectable class's constructor parameters, a field
            // formal typed through the field types just recorded.
            if acc.injectable_import && is_injectable_class(node, src) {
                emit_injects(child, src, id, parent_id, acc);
            }
            // DT.1l: a model class's JSON fields.
            if let Some(schema) = dart_model_schema(node, child, src, acc) {
                acc.nodes[class_ix].cells.push(schema.cell());
            }
        }
    }
}

// ============================================================================
// LA.37b: member containers - mixins, extensions, extension types, enums
// ============================================================================
//
// One member walker ([`visit_members`]) serves every body that holds
// `class_member`s: class_body (class, mixin, extension type), extension_body
// and enum_body. Each container is the OWNER its member METHODs hang on:
//
//   mixin M { }              CLASS <module>::M (it carries implementation, and
//                            SelfMethod resolution walks to CLASS / STRUCT /
//                            ENUM only)
//   extension E on T { }     CLASS <module>::E (the extension's own identity:
//                            explicit application `E(t).m()` names it, and two
//                            extensions on one type stay distinct)
//   extension type X(..) { } CLASS <module>::X
//   extension on T { }       T's own node when this file declares T (an
//                            unnamed extension is library-private and its
//                            members act as T's members here); otherwise
//                            (CB.17) its own CLASS <module>::extension<T>, T the
//                            on-type's simple name - never a node for T itself
//   enum E { ..; m() {} }    the ENUM
//
// Mixin heritage (`on` / `implements`) is not emitted here.

/// The type that owns a member (its METHODs hang on `id`; CB.9: every member
/// body is credited to its own METHOD, never to the owner), and LA.34's names
/// an unqualified call in a member body reaches before library scope.
struct Owner<'a> {
    qname: &'a str,
    id: NodeId,
    members: &'a HashSet<String>,
    /// DT.5h: the element type of each field declared `List<T>` / `Set<T>` /
    /// `Iterable<T>` ([`collect_dart_field_elem_types`]), which a
    /// one-parameter closure over the field's elements types its parameter
    /// with ([`type_element_closure`]).
    elems: &'a HashMap<String, String>,
}

/// A type this file declares (class, mixin, enum, extension type), keyed by
/// name in [`FileTypes::types`].
struct LocalType<'t> {
    id: NodeId,
    qname: String,
    decl: TsNode<'t>,
    body: Option<TsNode<'t>>,
}

/// What an unnamed extension needs from the rest of its file.
struct FileTypes<'t> {
    /// [`declared_types`]: the same-file types it can hang its members on.
    types: HashMap<String, LocalType<'t>>,
    /// [`library_names`]: this file's library-level names.
    library: HashSet<String>,
}

/// Every class / mixin / enum / extension-type declaration under the program
/// root, by name, with the NodeId its visitor mints. The first declaration of
/// a name wins. Lookup-only, never iterated into output.
fn declared_types<'t>(
    root: TsNode<'t>,
    src: &[u8],
    module_qname: &str,
    repo: RepoId,
) -> HashMap<String, LocalType<'t>> {
    let mut out = HashMap::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        let (name, kind) = match child.kind() {
            // visit_class reads the name as the first identifier child.
            "class_declaration" => (find_identifier(child, src), node_kind::CLASS),
            "mixin_declaration" | "extension_type_declaration" => {
                (decl_name(child, src), node_kind::CLASS)
            }
            "enum_declaration" => (decl_name(child, src), node_kind::ENUM),
            _ => continue,
        };
        let Some(name) = name else {
            continue;
        };
        let qname = format!("{module_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
        out.entry(name).or_insert(LocalType {
            id,
            qname,
            decl: child,
            body: child.child_by_field_name("body"),
        });
    }
    out
}

/// The names this file declares at library level: top-level functions,
/// getters, setters and variables. Inside an extension, Dart's lexical scope
/// reaches these BEFORE the on-type's members (which only an implicit `this`
/// reaches), so an on-type member of the same name is not in the extension's
/// member scope.
fn library_names(root: TsNode, src: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "function_signature" | "getter_signature" | "setter_signature" => {
                if let Some(n) = child.child_by_field_name("name") {
                    names.insert(text_of(n, src).to_string());
                }
            }
            "initialized_identifier_list" | "static_final_declaration_list" => {
                let mut items = child.walk();
                for item in child.named_children(&mut items) {
                    if let Some(n) = item.child_by_field_name("name") {
                        names.insert(text_of(n, src).to_string());
                    }
                }
            }
            _ => {}
        }
    }
    names
}

/// A mixin / extension / extension-type / enum declaration's name: its `name`
/// field (an extension type's is an `extension_type_name` wrapping the
/// identifier), else the first identifier child.
fn decl_name(node: TsNode, src: &[u8]) -> Option<String> {
    let Some(name) = node.child_by_field_name("name") else {
        return find_identifier(node, src);
    };
    if name.kind() == "extension_type_name" {
        return find_identifier(name, src);
    }
    Some(text_of(name, src).to_string())
}

/// LA.34's member set for any container: the names its body's
/// `class_member`s declare, plus an enum's constants and an extension type's
/// representation field - every name an unqualified call in one of its
/// members binds to before library scope.
fn container_member_names(decl: TsNode, body: TsNode, src: &[u8]) -> HashSet<String> {
    let mut names = class_member_names(body, src);
    let mut cursor = body.walk();
    for constant in body.named_children(&mut cursor) {
        if constant.kind() == "enum_constant"
            && let Some(n) = constant.child_by_field_name("name")
        {
            names.insert(text_of(n, src).to_string());
        }
    }
    if let Some(rep) = decl.child_by_field_name("representation")
        && let Some(n) = rep.child_by_field_name("name")
    {
        names.insert(text_of(n, src).to_string());
    }
    names
}

/// A mixin, a named extension or an extension type: a CLASS
/// `<module>::<Name>` + DEFINES + nav, owning the members of its body.
fn visit_container(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let (Some(name), Some(body)) = (decl_name(node, src), node.child_by_field_name("body")) else {
        return;
    };
    match node.kind() {
        "mixin_declaration" => acc.members.mixins += 1,
        "extension_declaration" => acc.members.extensions += 1,
        _ => acc.members.extension_types += 1,
    }
    emit_container(node, body, &name, src, file_rel, parent_qname, parent_id, repo, acc);
}

/// The CLASS `<module>::<name>` a member container declaration `node` mints
/// (its Node once per file, DEFINES from the module, nav), and the walk of
/// its `body`'s members under it. `name` is the declared name, or CB.17's
/// `extension<T>` for an unnamed extension on a type declared elsewhere.
#[allow(clippy::too_many_arguments)]
fn emit_container(
    node: TsNode,
    body: TsNode,
    name: &str,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, &qname);
    if acc.declared_ids.insert(id) {
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
        acc.nav.record(id, name, &qname, node_kind::CLASS, Some(parent_id));
    }
    let members = container_member_names(node, body, src);
    let elems = collect_dart_field_elem_types(body, src);
    let owner = Owner {
        qname: &qname,
        id,
        members: &members,
        elems: &elems,
    };
    visit_members(body, src, file_rel, &owner, repo, acc);
    // LA.23e: a mixin's or extension type's fields (an extension's static
    // ones), for A6.2a's receiver-type pass.
    collect_dart_field_types(body, src, id, acc);
}

/// An extension declaration. A named one is its own CLASS
/// ([`visit_container`]). An unnamed one on a type this file declares hangs
/// its members on that type's node (LA.37b). CB.17: an unnamed one on any
/// other type (declared in another file, import-prefixed, a core type, a
/// type parameter, a function or record type) is its own CLASS container
/// `<module>::extension<T>` ([`extension_display_type`]) owning its members,
/// which see only the extension's own members before library scope: the
/// on-type's are unknown here. `<` / `>` never occur in a Dart identifier, so
/// no declared type can take that qname; two unnamed extensions on one type
/// in one file share it (the first declaration's cells, both bodies' members).
/// The qname is not Dart's positional `_extension#0`, which moves under edits.
#[allow(clippy::too_many_arguments)]
fn visit_extension(
    node: TsNode,
    file_types: &FileTypes,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    if node.child_by_field_name("name").is_some() {
        visit_container(node, src, file_rel, parent_qname, parent_id, repo, acc);
        return;
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let Some(ty) = extension_on_type(node, src).and_then(|t| file_types.types.get(t)) else {
        // A parse error can leave the `on` clause empty: no type, no name.
        let Some(on) = extension_display_type(node, src) else {
            return;
        };
        acc.members.unnamed_ext_containers += 1;
        let name = format!("extension<{on}>");
        emit_container(node, body, &name, src, file_rel, parent_qname, parent_id, repo, acc);
        return;
    };
    // The extension's own members, then the on-type's that no library-level
    // name shadows (an implicit `this` reaches those only after library
    // scope). Imported library names are not known here.
    let mut members: HashSet<String> = ty
        .body
        .map(|b| container_member_names(ty.decl, b, src))
        .unwrap_or_default()
        .into_iter()
        .filter(|n| !file_types.library.contains(n))
        .collect();
    members.extend(class_member_names(body, src));
    acc.members.extensions += 1;
    // DT.5h: an extension declares no instance field, so no element type.
    let no_elems = HashMap::new();
    let owner = Owner {
        qname: &ty.qname,
        id: ty.id,
        members: &members,
        elems: &no_elems,
    };
    visit_members(body, src, file_rel, &owner, repo, acc);
}

/// The simple name of the type an extension is `on`, when it is a plain type
/// name, optionally generic or nullable (`on Api`, `on Api<T>`, `on Api?`).
/// An import-prefixed (`on p.Api`), function or record type -> None.
fn extension_on_type<'a>(node: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let mut cursor = node.walk();
    let parts: Vec<TsNode> = node.children_by_field_name("class", &mut cursor).collect();
    if parts.iter().any(|p| p.kind() == ".") {
        return None;
    }
    let head = parts.first().filter(|p| p.kind() == "type_identifier")?;
    let text = text_of(*head, src);
    Some(text.split('<').next().unwrap_or(text).trim())
}

/// CB.17: the `T` of an unnamed extension's container `extension<T>` - the
/// on-type's simple name with its import prefix, type arguments and `?`
/// dropped (`on m.Money` -> `Money`, `on List<int>?` -> `List`, `on T` ->
/// `T`), `Function` for a function type, `Record` for a record type, `void`
/// for `void`. None when the declaration names no type (a parse error).
fn extension_display_type(node: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let parts: Vec<TsNode> = node.children_by_field_name("class", &mut cursor).collect();
    match parts.first()?.kind() {
        "function_type" | "Function" => return Some("Function".to_string()),
        "record_type" => return Some("Record".to_string()),
        "void_type" => return Some("void".to_string()),
        _ => {}
    }
    // The type_identifiers directly in the field are the prefix and the
    // name (`m` `.` `Money`); type arguments are one nested node.
    let name = parts.iter().rev().find(|p| p.kind() == "type_identifier")?;
    Some(text_of(*name, src).trim().to_string()).filter(|n| !n.is_empty())
}

/// Walk one body's `class_member`s (class_body, extension_body, enum_body)
/// under `owner`. Returns how many declared a METHOD.
fn visit_members(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    owner: &Owner,
    repo: RepoId,
    acc: &mut Acc,
) -> usize {
    let mut declared = 0;
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if member.kind() == "class_member" && visit_class_member(member, src, file_rel, owner, repo, acc)
        {
            declared += 1;
        }
    }
    declared
}

/// What one `class_member` declares (CB.9). Every kind is a METHOD
/// `<owner>::<name>`; the kind picks the name ([`member_name`]) and the
/// `[dart-members]` counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemberKind {
    /// A method, instance or static (`function_signature`).
    Method,
    /// A getter or setter: a getter + setter pair is one METHOD.
    Accessor,
    /// A generative, `const`, factory or redirecting-factory constructor.
    Ctor,
    /// `operator <op>`.
    Operator,
}

/// The member a `class_member`'s `method_signature` (a member with a body) or
/// `declaration` (a bodiless one) declares, and its kind (CB.9):
///
///   `m(..)` / `get g` / `set s(..)`   its `name`
///   `C(..)`, `C.new(..)`, `factory C(..)`   `C`, the class name (the Java /
///                                     C# constructor qname `<T>::<T>`)
///   `C.name(..)`, `factory C.name(..)`, `const C.name(..)`   `name`: Dart
///                                     calls it `C.name(..)`, like a static
///                                     method, and forbids a static member of
///                                     the same name
///   `operator +`, `operator []=`      `operator+`, `operator[]=`: the token
///                                     verbatim, no space (unary and binary
///                                     `-` are one `operator-`)
///
/// None for a field declaration, which declares no METHOD.
fn member_name(sig: TsNode, src: &[u8]) -> Option<(String, MemberKind)> {
    let mut cursor = sig.walk();
    for part in sig.named_children(&mut cursor) {
        let kind = match part.kind() {
            "function_signature" => MemberKind::Method,
            "getter_signature" | "setter_signature" => MemberKind::Accessor,
            "constructor_signature"
            | "constant_constructor_signature"
            | "factory_constructor_signature"
            | "redirecting_factory_constructor_signature" => {
                return constructor_name(part, src).map(|n| (n, MemberKind::Ctor));
            }
            "operator_signature" => {
                let op: String = text_of(part.child_by_field_name("operator")?, src)
                    .split_whitespace()
                    .collect();
                return (!op.is_empty()).then(|| (format!("operator{op}"), MemberKind::Operator));
            }
            _ => continue,
        };
        let name = part
            .child_by_field_name("name")
            .map(|n| text_of(n, src).to_string())
            .or_else(|| find_identifier(part, src))?;
        return Some((name, kind));
    }
    None
}

/// A constructor signature's member name. tree-sitter-dart 0.1.0's `name`
/// field spans several nodes - the class `identifier`, then optionally `.`
/// and an `identifier` or the `new` keyword - so it is read with
/// `children_by_field_name` (`child_by_field_name` sees only the first):
/// `C` / `C.new` -> `C`, `C.name` -> `name`.
fn constructor_name(sig: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = sig.walk();
    let parts: Vec<TsNode> = sig.children_by_field_name("name", &mut cursor).collect();
    let class = parts.first().filter(|p| p.kind() == "identifier")?;
    let name = match parts.last() {
        Some(last) if last.id() != class.id() && last.kind() == "identifier" => *last,
        _ => *class,
    };
    Some(text_of(name, src).to_string()).filter(|n| !n.is_empty())
}

/// G12.5: class heritage. The `superclass` field holds `extends <type>` plus an
/// optional `with` mixin clause (or, in the mixin-only form, just `with`). The
/// `interfaces` field holds the `implements` clause.
///   - `extends Y`  → INHERITS_FROM (class → superclass)
///   - `with M`     → IMPLEMENTS    (class → mixin)
///   - `implements I` → IMPLEMENTS  (class → interface)
///
/// A6.5: each is a Bare [`UnresolvedRef`] from `module_id` (the file's MODULE),
/// never an edge. A Dart class is keyed `<module>::<Name>`, so the parser cannot
/// name the target node; the graph crate's `resolve_refs` binds a same-file
/// base through the module's own symbols and a cross-file one by its
/// repo-unique name. An external base (`extends StatelessWidget`) stays an
/// unresolved ref and emits no edge.
fn visit_class_heritage(node: TsNode, src: &[u8], id: NodeId, module_id: NodeId, acc: &mut Acc) {
    if let Some(superclass) = node.child_by_field_name("superclass") {
        // The `extends <type>` head sits directly under `superclass` (its `type`
        // field is the hidden `_type_not_void`); mixins (`with`) nest as a
        // `mixins` child holding one or more types.
        for head in heritage_type_heads(superclass, src) {
            emit_heritage_ref(head, edge_category::INHERITS_FROM, id, module_id, line_at(superclass), acc);
        }
        let mut sc_cursor = superclass.walk();
        for child in superclass.named_children(&mut sc_cursor) {
            if child.kind() == "mixins" {
                emit_mixin_or_interface_refs(child, src, id, module_id, acc);
            }
        }
    }
    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        emit_mixin_or_interface_refs(interfaces, src, id, module_id, acc);
    }
}

/// Emit an IMPLEMENTS ref per type in a `mixins` (`with`) or `interfaces`
/// (`implements`) clause.
fn emit_mixin_or_interface_refs(
    clause: TsNode,
    src: &[u8],
    id: NodeId,
    module_id: NodeId,
    acc: &mut Acc,
) {
    for head in heritage_type_heads(clause, src) {
        emit_heritage_ref(head, edge_category::IMPLEMENTS, id, module_id, line_at(clause), acc);
    }
}

/// A6.5: the class-type heads directly under one heritage clause, as source
/// text. The grammar inlines `_type_name` (`type_identifier ('.'
/// type_identifier)?`), so a library-prefixed `p.Base` arrives as TWO sibling
/// `type_identifier`s split by an anonymous `.`: they are one head, spanned
/// here so [`emit_heritage_ref`] reduces it to `Base`. Read one identifier at a
/// time, the prefix `p` would become a ref that binds by name to whatever the
/// repo calls `p`. Generics (`type_arguments`), a nested `mixins` clause and a
/// function / record type (never a class, so never a Dart supertype) are not
/// heads.
fn heritage_type_heads<'a>(clause: TsNode, src: &'a [u8]) -> Vec<&'a str> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut after_dot = false;
    let mut cursor = clause.walk();
    for child in clause.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => match spans.last_mut() {
                Some(span) if after_dot => span.1 = child.end_byte(),
                _ => spans.push((child.start_byte(), child.end_byte())),
            },
            "." => {
                after_dot = true;
                continue;
            }
            _ => {}
        }
        after_dot = false;
    }
    spans
        .into_iter()
        .filter_map(|(start, end)| std::str::from_utf8(src.get(start..end)?).ok())
        .collect()
}

// ============================================================================
// DT.5e: Flutter widget / State classes are framework components
// ============================================================================
//
// Flutter builds a widget the framework renders, and creates (createState) and
// drives (initState / build / dispose) the State a StatefulWidget names: no
// source line calls them, as no line calls an Angular `@Component`. A class
// whose `extends` head is one of these bases, in a file importing a Flutter
// library, gets a COMPONENT overlay folded into ROLE COMPONENT on its CLASS:
// an entry of the one entry rule, and the owner role the code domain's
// `flutter` INVOKED_MEMBERS row reads. The base is matched by simple name
// (generic arguments and a `p.` prefix stripped), so a user class called
// `State` in a Flutter file would match too. A user-defined base widget
// (`class X extends BaseScreen`, BaseScreen extends StatelessWidget in the
// repo) is not followed: X is no COMPONENT, BaseScreen is.

/// The Flutter, Riverpod and flutter_hooks widget and State base classes.
const FLUTTER_COMPONENT_BASES: &[&str] = &[
    "StatelessWidget",
    "StatefulWidget",
    "State",
    "ConsumerWidget",
    "ConsumerStatefulWidget",
    "ConsumerState",
    "HookWidget",
    "HookConsumerWidget",
    "StatefulHookWidget",
    "StatefulHookConsumerWidget",
    "InheritedWidget",
    "InheritedNotifier",
    "InheritedModel",
];

/// The [`FLUTTER_COMPONENT_BASES`] whose subclasses are a widget's State
/// (the `[dart-flutter]` marker's `states=`); every other base is a widget.
const FLUTTER_STATE_BASES: &[&str] = &["State", "ConsumerState"];

/// A file importing one of these defines Flutter widgets.
const FLUTTER_IMPORT_PREFIXES: &[&str] = &[
    "package:flutter/",
    "package:flutter_riverpod/",
    "package:hooks_riverpod/",
    "package:flutter_hooks/",
];

/// The [`FLUTTER_COMPONENT_BASES`] entry a `class_declaration` extends: its
/// `extends` head (the `type_identifier`s directly under `superclass`; a
/// `with` clause nests in `mixins`, generic arguments in `type_arguments`),
/// reduced to its simple name.
fn flutter_component_base(class: TsNode, src: &[u8]) -> Option<&'static str> {
    let superclass = class.child_by_field_name("superclass")?;
    let head = heritage_type_heads(superclass, src).into_iter().next()?;
    let simple = head.rsplit('.').next().unwrap_or(head).trim();
    FLUTTER_COMPONENT_BASES.iter().copied().find(|b| *b == simple)
}

fn emit_heritage_ref(
    raw: &str,
    category: glia_core::EdgeCategoryId,
    from_id: NodeId,
    module_id: NodeId,
    line: u32,
    acc: &mut Acc,
) {
    // Strip generic args (`Comparable<Foo>` → `Comparable`) and take the trailing
    // simple name (`pkg.Base` → `Base`). Graph crate resolves the target node.
    let base = raw.split('<').next().unwrap_or(raw).trim();
    let simple = base.rsplit(['.', ':']).next().unwrap_or(base).trim();
    if simple.is_empty() {
        return;
    }
    acc.refs.push(UnresolvedRef {
        from: from_id,
        from_module: module_id,
        qualifier: CallQualifier::Bare(simple.to_string()),
        category,
        line,
    });
}

/// One `class_member` of any container (LA.37b, CB.9). Every member with a
/// name declares the METHOD `<owner>::<name>` ([`member_name`]) once per file
/// (LA.37a's `declared_ids`), so a getter + setter pair - or an unnamed
/// extension member re-declaring its on-type's - is one Node and one DEFINES
/// edge. A `method_signature` is followed by its `function_body`; a
/// `declaration` is a bodiless member (abstract, `external`, a constructor
/// ending in `;`, a redirecting factory) or a field, which declares nothing
/// here. A constructor's initializer list (`: x = f(v), super(g(v))`) and
/// redirection (`: this(h())`) run as part of it: their calls, and every
/// body call, come from the member's own METHOD, never from the owner type.
/// Every unqualified call goes through LA.34's scope (`owner.members`, then
/// the member's parameters and locals). Returns true when the member
/// declared a METHOD.
fn visit_class_member(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    owner: &Owner,
    repo: RepoId,
    acc: &mut Acc,
) -> bool {
    let mut member: Option<(NodeId, MemberKind)> = None;
    // LA.34: the signature that declared it, whose parameters are locals.
    let mut signature: Option<TsNode> = None;
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        match child.kind() {
            "method_signature" | "declaration" => {
                let Some((name, kind)) = member_name(child, src) else {
                    // DT.5h: a field declaration declares no METHOD; its
                    // initialisers run as the owner type's code.
                    if child.kind() == "declaration" {
                        visit_field_initializers(child, owner, src, repo, file_rel, acc);
                    }
                    continue;
                };
                let id = declare_member(node, &name, owner, repo, file_rel, src, acc);
                acc.members.count(kind, child.kind() == "declaration");
                if kind == MemberKind::Accessor {
                    acc.accessor_ids.insert(id);
                }
                signature = Some(child);
                member = Some((id, kind));
                visit_initializers(child, id, owner, src, repo, file_rel, acc);
            }
            "function_body" => {
                // Every body follows the signature that names its member; a
                // body whose signature named nothing (a parse error) is
                // credited to no node.
                let Some((id, kind)) = member else {
                    continue;
                };
                if kind == MemberKind::Accessor {
                    acc.members.accessors += 1;
                }
                let scope = CallScope {
                    owner: Some(owner.qname),
                    members: owner.members,
                    elems: owner.elems,
                    locals: local_names(signature, child, src),
                };
                collect_calls_in(child, src, id, &scope, repo, file_rel, acc);
            }
            _ => {}
        }
    }
    member.is_some()
}

/// The METHOD `<owner>::<name>` for one `class_member` (`node`): its Node
/// (the member's CODE / POSITION), the owner's DEFINES edge and its nav
/// record, pushed once per file (LA.37a's `declared_ids`). Returns its id.
fn declare_member(
    node: TsNode,
    name: &str,
    owner: &Owner,
    repo: RepoId,
    file_rel: &str,
    src: &[u8],
    acc: &mut Acc,
) -> NodeId {
    let qname = format!("{}::{name}", owner.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);
    if acc.declared_ids.insert(id) {
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: entity_cells(&node, src, file_rel),
        });
        acc.edges.push(Edge {
            from: owner.id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, name, &qname, node_kind::METHOD, Some(owner.id));
    }
    id
}

/// CB.9: a constructor's `initializers` (`Money.zero() : cents = round2(0)`,
/// `super(g(v))`, `assert(..)`) and `redirection` (`: this(h())`), children
/// of its `method_signature` or `declaration`, walked for calls from the
/// constructor METHOD `from`, the constructor's parameters as locals.
fn visit_initializers(
    sig: TsNode,
    from: NodeId,
    owner: &Owner,
    src: &[u8],
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut cursor = sig.walk();
    for part in sig.named_children(&mut cursor) {
        if !matches!(part.kind(), "initializers" | "redirection") {
            continue;
        }
        let scope = CallScope {
            owner: Some(owner.qname),
            members: owner.members,
            elems: owner.elems,
            locals: local_names(Some(sig), part, src),
        };
        collect_calls_in(part, src, from, &scope, repo, file_rel, acc);
    }
}

/// DT.5h: a class field `declaration`'s initialisers (`static const _tabs =
/// [_Tab(..)]`, `final _ctrl = Ctrl()`, `late final _r = Repo.open()`): a
/// code written in a type body is the type's (the DT.5c rule), so each
/// `initialized_identifier` / `static_final_declaration` with a `value` is
/// walked for calls, names and client endpoints FROM the owner type
/// (`owner.id`, the CLASS / ENUM / mixin), under the owner's member scope,
/// a closure's parameters as locals and the field's own name not one.
fn visit_field_initializers(
    decl: TsNode,
    owner: &Owner,
    src: &[u8],
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut lists = decl.walk();
    for list in decl.named_children(&mut lists) {
        if !matches!(list.kind(), "initialized_identifier_list" | "static_final_declaration_list") {
            continue;
        }
        let mut items = list.walk();
        for item in list.named_children(&mut items) {
            if !matches!(item.kind(), "initialized_identifier" | "static_final_declaration")
                || item.child_by_field_name("value").is_none()
            {
                continue;
            }
            let mut locals = local_names(None, item, src);
            if let Some(name) = item.child_by_field_name("name") {
                locals.remove(text_of(name, src));
            }
            let scope = CallScope {
                owner: Some(owner.qname),
                members: owner.members,
                elems: owner.elems,
                locals,
            };
            collect_calls_in(item, src, owner.id, &scope, repo, file_rel, acc);
            acc.bare_calls.field_inits += 1;
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
    let Some(name_node) = node.child_by_field_name("name").or_else(|| {
        let mut c = node.walk();
        node.named_children(&mut c).find(|ch| ch.kind() == "identifier")
    }) else {
        return;
    };
    let name = text_of(name_node, src);
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ENUM, &qname);

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
        .record(id, name, &qname, node_kind::ENUM, Some(parent_id));
    acc.declared_ids.insert(id);

    // LA.37b: an enhanced enum's members hang on the ENUM (LA.30a: an ENUM
    // owns its METHOD children), its constants are members too.
    if let Some(body) = node.child_by_field_name("body") {
        let members = container_member_names(node, body, src);
        let elems = collect_dart_field_elem_types(body, src);
        let owner = Owner {
            qname: &qname,
            id,
            members: &members,
            elems: &elems,
        };
        let mut cursor = body.walk();
        for constant in body.named_children(&mut cursor) {
            if constant.kind() == "enum_constant" {
                visit_enum_constant(constant, src, file_rel, &owner, repo, acc);
            }
        }
        acc.members.enum_members += visit_members(body, src, file_rel, &owner, repo, acc);
        collect_dart_field_types(body, src, id, acc);
    }
}

/// CB.9: one enum constant -> the ATTRIBUTE `<Enum>::<constant>` under its
/// ENUM (HAS_ATTRIBUTE, nav parent the ENUM): the Rust-variant / TypeScript /
/// Java enum-member shape that graph/src/calls.rs `enum_member` binds
/// `Enum.member` uses against. An enhanced-enum constant's arguments
/// (`aud(1)`, `usd.named(2)`) are walked for calls from the ATTRIBUTE, under
/// the enum's member scope.
fn visit_enum_constant(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    owner: &Owner,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{}::{name}", owner.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ATTRIBUTE, &qname);
    if acc.declared_ids.insert(id) {
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: entity_cells(&node, src, file_rel),
        });
        acc.edges.push(Edge {
            from: owner.id,
            to: id,
            category: edge_category::HAS_ATTRIBUTE,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, name, &qname, node_kind::ATTRIBUTE, Some(owner.id));
    }
    acc.members.enum_constants += 1;
    let mut cursor = node.walk();
    for args in node.named_children(&mut cursor) {
        if !matches!(args.kind(), "arguments" | "argument_part") {
            continue;
        }
        // DT.5e: a closure in the arguments binds its parameters.
        let scope = CallScope {
            owner: Some(owner.qname),
            members: owner.members,
            elems: owner.elems,
            locals: local_names(None, args, src),
        };
        collect_calls_in(args, src, id, &scope, repo, file_rel, acc);
    }
}

/// tree-sitter-dart 0.1.0 puts a top-level function's body BESIDE its
/// signature: the body is the signature's next named sibling after any
/// comments. Anything else there (the next declaration after an `external`
/// signature, which has no body) means no body, so a body-less function
/// cannot steal its neighbour's.
fn sibling_body(sig: TsNode) -> Option<TsNode> {
    let mut next = sig.next_named_sibling();
    while let Some(n) = next {
        if n.kind() == "comment" {
            next = n.next_named_sibling();
            continue;
        }
        return (n.kind() == "function_body").then_some(n);
    }
    None
}

/// A top-level function, getter or setter (`node` is its signature) and, when
/// it has one, the sibling `body` it owns (LA.37a): the FUNCTION's CODE /
/// POSITION span signature + body, and the body's calls and client ENDPOINTs
/// are credited to it under Dart's lexical scope - the function's parameters
/// and locals first (LA.34's [`local_names`]), then library scope. There is no
/// class scope here, so every other unqualified call stays Bare.
#[allow(clippy::too_many_arguments)]
fn visit_function(
    node: TsNode,
    body: Option<TsNode>,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = find_method_name(node, src) else {
        return;
    };
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    if matches!(node.kind(), "getter_signature" | "setter_signature") {
        acc.top_level.accessors += 1;
        acc.accessor_ids.insert(id);
    }
    // The second half of a getter / setter pair is the same FUNCTION: it adds
    // no Node, DEFINES edge or nav record, only its body's calls.
    if acc.declared_ids.insert(id) {
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: span_cells(&node, &body.unwrap_or(node), src, file_rel),
        });
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

    let Some(body) = body else {
        acc.top_level.bodyless += 1;
        return;
    };
    acc.top_level.bodies += 1;
    let no_members = HashSet::new();
    let no_elems = HashMap::new();
    let scope = CallScope {
        owner: None,
        members: &no_members,
        elems: &no_elems,
        locals: local_names(Some(node), body, src),
    };
    collect_calls_in(body, src, id, &scope, repo, file_rel, acc);
}

/// G19: library-level `const`/`final` constants. The list holds one
/// `static_final_declaration` per declarator (`name = value`). Emits a STATE_VAR
/// node + DEFINES edge module→const for each ([`emit_top_level_var`]), and
/// (CB.17) walks its initialiser for calls from it.
///
/// Noise gate: skip when undocumented AND the initializer is a primitive literal
/// (number / string / bool). Documented or non-trivial initializers are kept.
fn visit_top_level_consts(
    list: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // The leading `///` doc precedes the `const`/`final` keyword, which is a
    // prev-sibling of this list (the `_top_level_definition` rule is hidden).
    // Anchor doc detection at that keyword so `leading_doc` reaches the comment.
    let doc_anchor = const_keyword_sibling(list).unwrap_or(list);
    let doc = glia_doc::leading_doc(&doc_anchor, src);
    let mut cursor = list.walk();
    for decl in list.named_children(&mut cursor) {
        if decl.kind() == "static_final_declaration" {
            emit_top_level_var(decl, doc.as_deref(), src, file_rel, parent_qname, parent_id, repo, acc);
        }
    }
}

/// CB.17: a library-level `var a = .., b = f();` - or a typed (`int n;`),
/// `late` or `late final` one, which the grammar also shapes as an
/// `initialized_identifier_list` beside its keyword / type tokens. One
/// STATE_VAR per `initialized_identifier` (`name`, optional `value`), the
/// same emission, noise gate and initialiser walk as G19's
/// [`visit_top_level_consts`]; the doc is the `///` above the declaration's
/// first token ([`declaration_head`]).
fn visit_top_level_vars(
    list: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let doc = glia_doc::leading_doc(&declaration_head(list), src);
    let mut cursor = list.walk();
    for decl in list.named_children(&mut cursor) {
        if decl.kind() == "initialized_identifier" {
            emit_top_level_var(decl, doc.as_deref(), src, file_rel, parent_qname, parent_id, repo, acc);
        }
    }
}

/// The first token of the top-level variable declaration `list` belongs to:
/// walks back over the keywords and the type (`late final Map<K, V>? x`,
/// `p.Type x`) to the token after the previous declaration, so the doc
/// comment above the declaration is the one found. `list` itself when
/// nothing precedes it.
fn declaration_head(list: TsNode) -> TsNode {
    let mut head = list;
    while let Some(prev) = head.prev_sibling() {
        let part_of_head = matches!(
            prev.kind(),
            "var"
                | "final"
                | "const"
                | "late"
                | "external"
                | "static"
                | "covariant"
                | "type_identifier"
                | "type_arguments"
                | "?"
                | "."
                | "function_type"
                | "record_type"
                | "void_type"
                | "inferred_type"
                | "nullable_type"
                | "Function"
        );
        if !part_of_head {
            break;
        }
        head = prev;
    }
    head
}

/// One library-level variable `decl` (a `static_final_declaration` or an
/// `initialized_identifier`, both `name` + optional `value`): the STATE_VAR
/// `<module>::<name>` (G19) - CODE / POSITION of the declarator, `doc` the
/// declaration's - its DEFINES edge from the module and its nav record,
/// pushed once per file. Noise gate: undocumented + a primitive-literal
/// initialiser emits nothing. CB.17: the initialiser is walked for calls and
/// client ENDPOINTs FROM the STATE_VAR, under library scope (no members;
/// DT.5e: its closures are entered, as in every body walk, and their
/// parameters are locals).
#[allow(clippy::too_many_arguments)]
fn emit_top_level_var(
    decl: TsNode,
    doc: Option<&str>,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = decl.child_by_field_name("name") else {
        return;
    };
    let value = decl.child_by_field_name("value");
    // Noise gate: undocumented + literal-primitive initializer → skip.
    if doc.is_none() && value.is_some_and(|v| is_primitive_literal(v.kind())) {
        return;
    }
    let name = text_of(name_node, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);

    if acc.declared_ids.insert(id) {
        // entity_cells gives CODE + POSITION (+ DOC when leading_doc sees it
        // from the node itself). A top-level variable carries the doc above
        // its keyword, so splice in the doc resolved from there when present.
        let mut cells = entity_cells(&decl, src, file_rel);
        if let Some(d) = doc
            && !cells.iter().any(|c| c.kind == cell_type::DOC)
        {
            cells.push(Cell {
                kind: cell_type::DOC,
                payload: CellPayload::Text(d.to_string()),
            });
        }
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells,
        });
        acc.edges.push(Edge {
            from: parent_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav
            .record(id, name, &qname, node_kind::STATE_VAR, Some(parent_id));
    }

    // The value is a primary + its SIBLING selectors (`seed` `()`), all
    // children of `decl` beside the name, so the chain walk runs over `decl`:
    // the name is followed by `=`, never a selector, and emits nothing.
    if value.is_none() {
        return;
    }
    acc.top_level.initialisers += 1;
    let no_members = HashSet::new();
    // DT.5e: a closure in the initialiser (`StateNotifierProvider((ref) =>
    // ..)`) binds its parameters; the variable's own name is no local.
    // The value is several `value` siblings (a primary and its selectors), so
    // the bindings are read over the whole declarator.
    let mut locals = local_names(None, decl, src);
    locals.remove(name);
    let no_elems = HashMap::new();
    let scope = CallScope {
        owner: None,
        members: &no_members,
        elems: &no_elems,
        locals,
    };
    collect_calls_in(decl, src, id, &scope, repo, file_rel, acc);
}

/// Walk prev-siblings of a top-level declaration list to the `const`/`final`/
/// `late` keyword token, used as the doc-comment anchor.
fn const_keyword_sibling(list: TsNode) -> Option<TsNode> {
    let mut prev = list.prev_sibling();
    let mut hops = 0u32;
    while let Some(n) = prev {
        hops += 1;
        if hops > 8 {
            break;
        }
        match n.kind() {
            "const" | "final" | "late" => return Some(n),
            // Skip an optional type annotation / `augment` marker between the
            // keyword and the list.
            _ => prev = n.prev_sibling(),
        }
    }
    None
}

/// True for Dart primitive/atom literal initializer node kinds.
fn is_primitive_literal(kind: &str) -> bool {
    matches!(
        kind,
        "decimal_integer_literal"
            | "hex_integer_literal"
            | "decimal_floating_point_literal"
            | "string_literal"
            | "true"
            | "false"
            | "null_literal"
    )
}

fn find_identifier<'a>(node: TsNode<'a>, src: &'a [u8]) -> Option<String> {
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() == "identifier" {
            return Some(text_of(child, src).to_string());
        }
    }
    None
}

fn find_method_name<'a>(node: TsNode<'a>, src: &'a [u8]) -> Option<String> {
    if let Some(name) = find_identifier(node, src) {
        return Some(name);
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() == "function_signature" {
            return find_identifier(child, src);
        }
    }
    None
}

/// CB.17 (D6): one `import` directive, read from the AST - the
/// `import_specification` under `import_or_export` / `library_import`, with
/// its `uri`, its `alias` field (`as p`, `deferred as p`) and its `show` /
/// `hide` combinators:
///
///   `import 'a.dart' as p ..;`         Module { a.dart, Some(p) }: the
///                                      prefix is what the code writes
///                                      (`p.f()`), whatever it shows
///   `import 'a.dart' show X, y;`       one Symbol { a.dart, name } per name
///                                      the combinators leave visible
///   `import 'a.dart';` / `hide Z;`     Module { a.dart, None }
///
/// The uri is the string's text without its quotes; a conditional import's
/// (`'a.dart' if (dart.library.io) 'b.dart'`) is its default, the first
/// `uri`. `export` directives are not read.
fn collect_import(node: TsNode, src: &[u8], from_module: &str, acc: &mut Acc) {
    let Some(spec) = named_child_of_kind(node, "library_import")
        .and_then(|lib| named_child_of_kind(lib, "import_specification"))
    else {
        return;
    };
    let Some(path) = spec.child_by_field_name("uri").and_then(|u| import_uri(u, src)) else {
        return;
    };
    let alias = spec
        .child_by_field_name("alias")
        .map(|a| text_of(a, src).to_string())
        .filter(|a| !a.is_empty());
    let line = line_at(node);
    let mut push = |target: ImportTarget| {
        acc.imports.push(ImportStmt {
            from_module: from_module.to_string(),
            target,
            line,
        });
    };
    match (alias, shown_names(spec, src)) {
        (None, Some(names)) if !names.is_empty() => {
            for name in names {
                push(ImportTarget::Symbol {
                    module: path.clone(),
                    name,
                    alias: None,
                    level: 0,
                });
            }
        }
        (alias, _) => push(ImportTarget::Module { path, alias }),
    }
}

/// The first named child of `node` of `kind`.
fn named_child_of_kind<'a>(node: TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|c| c.kind() == kind)
}

/// An import's uri string without quotes: the `uri` itself, or a
/// `configurable_uri`'s first (default) `uri`. None for an empty string.
fn import_uri(uri: TsNode, src: &[u8]) -> Option<String> {
    let uri = if uri.kind() == "configurable_uri" {
        named_child_of_kind(uri, "uri")?
    } else {
        uri
    };
    let text = text_of(uri, src)
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .to_string();
    (!text.is_empty()).then_some(text)
}

/// The names an import's combinators leave visible, in source order, when
/// some `show` limits them: each `show` keeps only the names it lists (so
/// two narrow each other), each `hide` drops its names from a `show` list.
/// None when no `show` appears: every public name is visible, `hide` or not.
fn shown_names(spec: TsNode, src: &[u8]) -> Option<Vec<String>> {
    let mut shown: Option<Vec<String>> = None;
    let mut cursor = spec.walk();
    for comb in spec.named_children(&mut cursor) {
        if comb.kind() != "combinator" {
            continue;
        }
        let mut ids = comb.walk();
        let names: Vec<String> = comb
            .named_children(&mut ids)
            .filter(|i| i.kind() == "identifier")
            .map(|i| text_of(i, src).to_string())
            .collect();
        match comb.child(0).map(|t| t.kind()) {
            Some("show") => {
                shown = Some(match shown {
                    Some(prev) => prev.into_iter().filter(|n| names.contains(n)).collect(),
                    None => names,
                });
            }
            Some("hide") => {
                if let Some(list) = shown.as_mut() {
                    list.retain(|n| !names.contains(n));
                }
            }
            _ => {}
        }
    }
    shown
}

/// Walk a body for Pattern A endpoints and the call sites of every selector
/// chain in it (LA.23e), its unqualified calls classified by `scope`
/// (LA.34), all credited to `from` - the body's own METHOD / FUNCTION (a
/// constructor / factory / operator is its own METHOD since CB.9, which also
/// walks a constructor's initializer list and an enum constant's arguments
/// through here), or the STATE_VAR whose initialiser it is (CB.17).
///
/// DT.5e: the walk enters every closure (`function_expression`, an arrow or
/// a block body: a `builder: (ctx) { .. }`, an `onTap: () => ..`), so its
/// calls and endpoints are `from`'s, as the TS parser credits an anonymous
/// callback's calls to the enclosing node; `scope.locals` already holds the
/// closures' parameters. Local functions were always walked (their name is a
/// local). A `const T(..)` / `new T(..)` is a call of T
/// ([`push_creation_call`]); DT.5h: so is a constructor tear-off `T.new`
/// ([`push_tearoff_call`]), and every identifier the walk visits that names
/// a member or a same-file function is recorded for a reference
/// ([`record_name_ref`]): an `identifier_dollar_escaped` is `$name` in an
/// interpolation, its text the bare name.
fn collect_calls_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    scope: &CallScope,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        // Pattern A: client HTTP call (`dio.get('/x')`) → ENDPOINT node so the
        // HttpStackResolver can pair it with a server ROUTE.
        try_detect_dart_endpoint(n, src, from, repo, file_rel, acc);
        push_selector_chain_calls(n, src, from, scope, acc);
        match n.kind() {
            "function_expression" => acc.bare_calls.closures += 1,
            "const_object_expression" | "new_expression" => {
                push_creation_call(n, src, from, scope, acc);
            }
            "constructor_tearoff" => push_tearoff_call(n, src, from, scope, acc),
            "identifier" | "identifier_dollar_escaped" => {
                record_name_ref(n, src, from, scope, acc);
            }
            _ => {}
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
}

/// DT.5e: a `const_object_expression` / `new_expression` (grammar fields
/// `type`, the optional `constructor` and `arguments`) is a call of its class:
/// `const T(..)` / `new T(..)` -> Bare(T), `const T.named(..)` ->
/// Attribute { T, named }, the qualifiers `T(..)` / `T.named(..)` already get
/// from the selector chain scan. Classified by `scope` and, for a named
/// constructor of a same-file type, recorded for the receiver-type pass
/// ([`record_type_receiver`]), as a `T.named(..)` is.
///
/// tree-sitter-dart 0.1.0 reads `A.b` after `const` / `new` as one prefixed
/// type name (`type_identifier` `.` `type_identifier`, the grammar's
/// `_type_name`), never as a type and its `constructor`: only a third part
/// (`const p.T.named()`) lands in the `constructor` field. Dart's naming
/// convention splits the two ([`is_dart_class_name`]): a capitalised last part
/// is the class behind a library prefix (`const p.Chip()` -> Bare(Chip)), a
/// lowercase one is a named constructor of the class before it
/// (`const Dot.small()` -> Attribute { Dot, small }).
fn push_creation_call(n: TsNode, src: &[u8], from: NodeId, scope: &CallScope, acc: &mut Acc) {
    let mut cursor = n.walk();
    let parts: Vec<&str> = n
        .children_by_field_name("type", &mut cursor)
        .filter(|k| k.kind() == "type_identifier")
        .map(|k| text_of(k, src).trim())
        .filter(|t| !t.is_empty())
        .collect();
    let ctor = n
        .child_by_field_name("constructor")
        .map(|c| text_of(c, src).trim())
        .filter(|c| !c.is_empty());
    let (class, ctor) = match (parts.as_slice(), ctor) {
        ([.., class], Some(ctor)) => (*class, Some(ctor)),
        ([class, named], None) if is_dart_class_name(class) && !is_dart_class_name(named) => {
            (*class, Some(*named))
        }
        ([.., class], None) => (*class, None),
        ([], _) => return,
    };
    let qualifier = match ctor {
        Some(ctor) => CallQualifier::Attribute {
            base: class.to_string(),
            name: ctor.to_string(),
        },
        None => CallQualifier::Bare(class.to_string()),
    };
    if let Some(qualifier) = scope.classify(qualifier, &mut acc.bare_calls) {
        record_type_receiver(&qualifier, from, scope, acc);
        acc.calls.push(CallSite {
            from,
            qualifier,
            line: line_at(n),
        });
        acc.bare_calls.creations += 1;
    }
}

/// DT.5h: a `constructor_tearoff` (`T.new`, `p.T.new`: a declaration's or
/// an argument's value) builds a T: the call site Bare(<its last
/// type_identifier>), the qualifier [`push_creation_call`] builds for
/// `const T()`, classified by `scope`, at the tear-off's row.
fn push_tearoff_call(n: TsNode, src: &[u8], from: NodeId, scope: &CallScope, acc: &mut Acc) {
    let mut cursor = n.walk();
    let Some(class) = n
        .children(&mut cursor)
        .filter(|k| k.kind() == "type_identifier")
        .map(|k| text_of(k, src).trim())
        .filter(|t| !t.is_empty())
        .last()
    else {
        return;
    };
    if let Some(qualifier) = scope.classify(CallQualifier::Bare(class.to_string()), &mut acc.bare_calls)
    {
        acc.calls.push(CallSite {
            from,
            qualifier,
            line: line_at(n),
        });
        acc.bare_calls.tear_offs += 1;
    }
}

/// DT.5e / DT.5h: an `identifier` the body walk visits that is a name
/// reference ([`is_name_reference`]: no label, selector or cascade member,
/// declaration name, constructor name or callee) names something by value:
/// a conditional's branch or condition, an `??` operand, a list / map /
/// record element, an initialiser, a return value, an assert, a `case`
/// constant, an interpolation (`'$_title'`), a whole argument
/// (`onPressed: _save`). Classified by `scope` ([`CallScope::lookup`],
/// Dart's lexical order) and kept for [`resolve_name_refs`]: a local binds
/// nothing (`final onTap = ..; onTap: onTap`); a member is the METHOD
/// `<owner>::<name>` it would be; a library-scope name is the top-level
/// FUNCTION of this file it would be. An assignment's whole left side
/// ([`is_assignment_target`]) is a write.
fn record_name_ref(id: TsNode, src: &[u8], from: NodeId, scope: &CallScope, acc: &mut Acc) {
    if !is_name_reference(id, src) {
        return;
    }
    let name = text_of(id, src).trim();
    if name.is_empty() || name == "_" {
        return;
    }
    let target = match scope.lookup(name) {
        NameScope::Local => return,
        NameScope::Member => {
            let Some(owner) = scope.owner else { return };
            NameTarget::Member(format!("{owner}::{name}"))
        }
        NameScope::Library => NameTarget::Library(name.to_string()),
    };
    acc.name_refs.push(NameRef {
        from,
        target,
        line: line_at(id),
        write: is_assignment_target(id),
    });
}

/// DT.5h: the parents under which an identifier names no value: a label
/// (`onPressed:`, a record or object-pattern field), a statement label and
/// a `break` / `continue` target, a member selector (`x._save`, the chain
/// scan's: [`push_chain_reads`]), a cascade selector (`..text = v`,
/// `..addListener(f)` set or call a member of the cascade's target, never of
/// this class), an annotation, a symbol literal's part and a dot shorthand
/// (`.red` names a static member of the context type). Every shape probed on
/// tree-sitter-dart 0.1.0.
const NON_REFERENCE_PARENTS: &[&str] = &[
    "label",
    "labeled_statement",
    "break_statement",
    "continue_statement",
    "unconditional_assignable_selector",
    "conditional_assignable_selector",
    "cascade_selector",
    "annotation",
    "symbol_literal",
    "static_member_shorthand",
];

/// DT.5h: whether the identifier `id` is a reference to a value: not under
/// a [`NON_REFERENCE_PARENTS`] parent, not its parent's `name` field (a
/// declaration, a field initializer's field, a for-in or pattern variable)
/// or `constructor` field (`const p.T.named()`), and not a callee (its next
/// named sibling, comments skipped, a `selector` holding an argument part or
/// type arguments: `_save()` and `_build<T>()` stay call sites).
fn is_name_reference(id: TsNode, src: &[u8]) -> bool {
    let Some(parent) = id.parent() else {
        return true;
    };
    if NON_REFERENCE_PARENTS.contains(&parent.kind()) {
        return false;
    }
    for field in ["name", "constructor"] {
        let mut cursor = parent.walk();
        if parent
            .children_by_field_name(field, &mut cursor)
            .any(|k| k.id() == id.id())
        {
            return false;
        }
    }
    let mut next = id.next_named_sibling();
    while let Some(n) = next.filter(|n| n.kind() == "comment") {
        next = n.next_named_sibling();
    }
    !next.is_some_and(|sel| {
        sel.kind() == "selector"
            && matches!(selector_part(sel, src), SelectorPart::Call | SelectorPart::TypeArgs)
    })
}

/// DT.5h: the identifier IS the whole `left` side of an assignment
/// (`_mode = 1`, `_n += 1`, `_x ??= v`): an `assignable_expression` holding
/// it alone. The head of `_obj.x = v` is a read of `_obj`.
fn is_assignment_target(id: TsNode) -> bool {
    let Some(left) = id.parent().filter(|p| p.kind() == "assignable_expression") else {
        return false;
    };
    let mut cursor = left.walk();
    if left.named_children(&mut cursor).any(|k| k.id() != id.id() && k.kind() != "comment") {
        return false;
    }
    left.parent()
        .filter(|a| a.kind() == "assignment_expression")
        .and_then(|a| a.child_by_field_name("left"))
        .is_some_and(|l| l.id() == left.id())
}

/// DT.5e: bind every [`NameRef`] of the file once all of it is declared.
///
/// - A member: an edge to the METHOD `<owner>::<name>` when this file
///   declared it (a field, which `members` holds too, declares none).
/// - A library-scope name: an edge to this file's top-level FUNCTION
///   `<module>::<name>` when it declared one.
///
/// DT.5h: a getter / setter target ([`Acc::accessor_ids`]) runs: a CALLS
/// edge, rule `accessor_read`, or `accessor_write` for an assignment's left
/// side (a getter + setter pair is one METHOD, so a write counts too). Any
/// other METHOD / FUNCTION is torn off: a USES edge, rule `method_ref`.
///
/// Nothing else binds: a variable, class or other type, a self-reference (a
/// method passing itself, `removeListener(_onChange)` inside `_onChange`),
/// and a name this file does not declare. The graph binds a Bare ref to
/// whatever symbol an import names: on quokka-stack every imported name
/// passed by value that bound was a Riverpod provider variable
/// (`ref.watch(cartProvider)`), so an imported name is left alone rather
/// than made a USES of a STATE_VAR. Each edge carries an EVIDENCE cell,
/// emitter `parser:dart`, at the identifier's row; one edge per (from, to,
/// read | write), at its earliest row.
fn resolve_name_refs(acc: &mut Acc, module_qname: &str, repo: RepoId) {
    // The walk is a stack, so walk order is not source order: the earliest
    // row of each (from, target, write) wins.
    let mut refs = std::mem::take(&mut acc.name_refs);
    refs.sort_by_key(|r| (r.line, r.write));
    let mut seen: HashSet<(NodeId, NodeId, bool)> = HashSet::new();
    for r in refs {
        let to = match &r.target {
            NameTarget::Member(qname) => {
                NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, qname)
            }
            NameTarget::Library(name) => NodeId::from_parts(
                GRAPH_TYPE,
                repo,
                node_kind::FUNCTION,
                &format!("{module_qname}::{name}"),
            ),
        };
        if to == r.from || !acc.declared_ids.contains(&to) {
            continue;
        }
        let accessor = acc.accessor_ids.contains(&to);
        let write = accessor && r.write;
        if !seen.insert((r.from, to, write)) {
            continue;
        }
        let (category, rule) = match (accessor, write) {
            (true, true) => (edge_category::CALLS, "accessor_write"),
            (true, false) => (edge_category::CALLS, "accessor_read"),
            (false, _) => (edge_category::USES, "method_ref"),
        };
        let ev = glia_code_domain::evidence::Evidence::emitter("parser:dart")
            .rule(rule)
            .line(r.line);
        acc.edges.push(Edge::new(r.from, to, category, Confidence::Strong).with_cell(ev.to_cell()));
        if accessor {
            acc.bare_calls.accessor_refs += 1;
        } else {
            acc.bare_calls.method_refs += 1;
        }
    }
}

/// DT.5h: record each [`PendingRead`] as a `NavFact::MemberRead` on its
/// scope, in (row, receiver, name) order (the walk is a stack). A read on a
/// local is kept only when the walk typed that local
/// (`CodeNav::local_types`, a closure parameter of
/// [`type_element_closure`]): an untyped local would otherwise be typed
/// through a same-named field by the graph's receiver typing (`final dynamic
/// _friends` beside the field `_friends`).
fn finish_member_reads(acc: &mut Acc) {
    let mut reads = std::mem::take(&mut acc.member_reads);
    reads.sort_by(|a, b| (a.line, &a.receiver, &a.name).cmp(&(b.line, &b.receiver, &b.name)));
    for r in reads {
        let typed = || {
            acc.nav
                .local_types
                .get(&r.from)
                .and_then(|t| t.get(&r.receiver))
                .is_some_and(|t| !t.is_empty())
        };
        if r.local && !typed() {
            continue;
        }
        let fact = NavFact::MemberRead {
            receiver: r.receiver,
            name: r.name,
            line: r.line,
        };
        if acc.nav.nav_facts.get(&r.from).is_some_and(|f| f.contains(&fact)) {
            continue;
        }
        acc.nav.record_fact(r.from, fact);
        acc.bare_calls.member_reads += 1;
    }
}

// ============================================================================
// LA.23e: receiver call sites over tree-sitter-dart's selector chains
// ============================================================================
//
// tree-sitter-dart 0.1.0 has no `selector_expression` / call node. A postfix
// expression is a primary followed by SIBLING `selector` nodes inside whatever
// node holds it (an `expression_statement`, `return_statement`, `argument`,
// `await_expression`, `initialized_identifier`, a `=>` `function_body`, ...):
//
//   repo.find(id)       identifier  selector(.find)  selector(argument_part)
//   this.repo.find(id)  this        selector(.repo)  selector(.find)  selector(argument_part)
//
// `this` is an ANONYMOUS node, so the scan walks every child, not only the
// named ones. One call site per `argument_part`:
//
//   f()          -> Bare(f)             this.m()    -> SelfMethod(m)
//   x.m()        -> Attribute{x, m}     x?.m()      -> Attribute{x, m}
//   this.f.m()   -> ComplexReceiver{"this.f", m}
//   a.b().c()    -> Attribute{a, b}, then ComplexReceiver{"a.b()", c}
//
// A ComplexReceiver's receiver is the primary's and the preceding selectors'
// texts, concatenated (so layout whitespace between chained selectors drops).
// `super.m()`, cascades (`..m()`), `new` / `const` constructions, a
// parenthesised primary (`(a).m()`) and the second call of `f()()` emit
// nothing here.

/// What one `selector` in a postfix chain does.
enum SelectorPart<'a> {
    /// `.name` / `?.name`: a member access.
    Member(&'a str),
    /// `(args)`: invokes what the chain has built so far.
    Call,
    /// `<T>`: generic arguments of the call that follows. Transparent.
    TypeArgs,
    /// `!`, `[i]`, anything else: the receiver is no longer a plain name.
    Opaque,
}

fn selector_part<'a>(sel: TsNode<'a>, src: &'a [u8]) -> SelectorPart<'a> {
    let Some(inner) = sel.named_child(0) else {
        return SelectorPart::Opaque; // the `!` null assertion has no named child
    };
    match inner.kind() {
        "argument_part" => SelectorPart::Call,
        "type_arguments" => SelectorPart::TypeArgs,
        "unconditional_assignable_selector" | "conditional_assignable_selector" => {
            // `.name` / `?.name`, not an index `[i]` / `?[i]` (whose named child
            // can be an identifier too): the leading token decides.
            let dotted = inner
                .child(0)
                .is_some_and(|t| matches!(t.kind(), "." | "?."));
            match inner.named_child(0) {
                Some(id) if dotted && id.kind() == "identifier" => {
                    SelectorPart::Member(text_of(id, src))
                }
                _ => SelectorPart::Opaque,
            }
        }
        _ => SelectorPart::Opaque,
    }
}

/// Emit the call sites of every `identifier` / `this` primary among `n`'s
/// children that is followed by one or more `selector` siblings. An
/// unqualified call goes through `scope` (LA.34): it may become a
/// SelfMethod, or no call site at all.
fn push_selector_chain_calls(
    n: TsNode,
    src: &[u8],
    from: NodeId,
    scope: &CallScope,
    acc: &mut Acc,
) {
    let mut cursor = n.walk();
    let kids: Vec<TsNode> = n.children(&mut cursor).collect();
    let mut i = 0;
    while let Some(primary) = kids.get(i).copied() {
        if !matches!(primary.kind(), "identifier" | "this") {
            i += 1;
            continue;
        }
        let selectors: Vec<TsNode> = kids
            .iter()
            .skip(i + 1)
            .take_while(|k| k.kind() == "selector")
            .copied()
            .collect();
        i += 1 + selectors.len();
        for (qualifier, line) in chain_call_sites(primary, &selectors, src) {
            if let Some(qualifier) = scope.classify(qualifier, &mut acc.bare_calls) {
                record_type_receiver(&qualifier, from, scope, acc);
                acc.calls.push(CallSite { from, qualifier, line });
            }
        }
        push_chain_reads(primary, &selectors, src, from, scope, acc);
        type_element_closure(primary, &selectors, src, from, scope, acc);
    }
}

/// DT.5h: the member reads of one primary + selector chain: each `.name`
/// (`SelectorPart::Member`) no call or type arguments follow.
///
/// - `this.<name>` (the first selector): a [`NameRef`] to the member METHOD
///   `<owner>::<name>`, [`record_name_ref`]'s rules (a getter runs, a method
///   is torn off, a field binds nothing).
/// - `T.new` / `T<A>.new` (only type arguments before `.new`, a capitalised
///   primary: the grammar's shape in a declaration or a return) builds a T:
///   the call site Bare(T) through `scope.classify`.
/// - Any other first selector on an identifier primary: a [`PendingRead`]
///   with the receiver `<head>` for a local, `this.<head>` for a member of
///   the enclosing type, `<head>` for a capitalised library-scope name (a
///   type: `Env.apiBaseUrl`, `Friend.fromJson`); a lowercase library-scope
///   name (an inherited `widget` / `state`, an import prefix) is not read.
/// - A later selector (`a.b.c`) is not read.
///
/// An assignment target's trailing member is a bare
/// `unconditional_assignable_selector`, never a `selector`, so a write
/// through a receiver (`obj.x = v`, `this.x = v`) is not read here. In an
/// argument the grammar parses `f(Box<int>.new)` as a relational expression
/// ending in a dot shorthand: not read. A plain `T.new` argument is a
/// `constructor_tearoff` ([`push_tearoff_call`]).
fn push_chain_reads(
    primary: TsNode,
    selectors: &[TsNode],
    src: &[u8],
    from: NodeId,
    scope: &CallScope,
    acc: &mut Acc,
) {
    let parts: Vec<SelectorPart> = selectors.iter().map(|s| selector_part(*s, src)).collect();
    let is_this = primary.kind() == "this";
    let head = text_of(primary, src).trim();
    for (j, part) in parts.iter().enumerate() {
        let SelectorPart::Member(name) = part else {
            continue;
        };
        if matches!(parts.get(j + 1), Some(SelectorPart::Call | SelectorPart::TypeArgs)) {
            continue;
        }
        let line = selectors.get(j).map_or(line_at(primary), |sel| line_at(*sel));
        if *name == "new" {
            let only_type_args = parts[..j].iter().all(|p| matches!(p, SelectorPart::TypeArgs));
            if !is_this && only_type_args && is_dart_class_name(head) {
                let bare = CallQualifier::Bare(head.to_string());
                if let Some(qualifier) = scope.classify(bare, &mut acc.bare_calls) {
                    acc.calls.push(CallSite {
                        from,
                        qualifier,
                        line: line_at(primary),
                    });
                    acc.bare_calls.tear_offs += 1;
                }
            }
            continue;
        }
        if j > 0 {
            continue;
        }
        if is_this {
            if let Some(owner) = scope.owner {
                acc.name_refs.push(NameRef {
                    from,
                    target: NameTarget::Member(format!("{owner}::{name}")),
                    line,
                    write: false,
                });
            }
            continue;
        }
        let (receiver, local) = match scope.lookup(head) {
            NameScope::Local => (head.to_string(), true),
            NameScope::Member => (format!("this.{head}"), false),
            NameScope::Library if is_dart_class_name(head) => (head.to_string(), false),
            NameScope::Library => continue,
        };
        acc.member_reads.push(PendingRead {
            from,
            receiver,
            name: name.to_string(),
            line,
            local,
        });
    }
}

/// DT.5h: the collections whose element type a field declaration keeps
/// ([`declared_element_type`]).
const ELEMENT_COLLECTIONS: &[&str] = &["List", "Set", "Iterable"];

/// DT.5h: the collection methods whose callback receives one element.
const ELEMENT_CALLBACKS: &[&str] = &[
    "map",
    "forEach",
    "where",
    "any",
    "every",
    "firstWhere",
    "lastWhere",
    "singleWhere",
    "expand",
    "takeWhile",
    "skipWhile",
    "indexWhere",
    "removeWhere",
    "retainWhere",
];

/// DT.5h: `<field>.<cb>((p) => ..)` / `this.<field>.<cb>((p) => ..)` over a
/// field of the enclosing type declared `List<T>` / `Set<T>` / `Iterable<T>`
/// ([`Owner::elems`]; a bare `<field>` only when no local of that name
/// shadows it), `cb` an [`ELEMENT_CALLBACKS`] member followed by its call,
/// the call's first positional argument a closure with exactly one untyped
/// parameter `p`: records `p` as a local of type T in `from`'s scope
/// (`CodeNav::record_local_type`), so `p.m()` binds T's METHOD `m` through
/// the graph's receiver typing, when the closure owns the name
/// ([`closure_owns_receiver`]). A typed parameter (`(Item j) => ..`) is not
/// recorded.
fn type_element_closure(
    primary: TsNode,
    selectors: &[TsNode],
    src: &[u8],
    from: NodeId,
    scope: &CallScope,
    acc: &mut Acc,
) {
    let parts: Vec<SelectorPart> = selectors.iter().map(|s| selector_part(*s, src)).collect();
    let (field, cb_at) = match primary.kind() {
        "identifier" => {
            let field = text_of(primary, src).trim();
            if scope.lookup(field) != NameScope::Member {
                return;
            }
            (field, 0)
        }
        "this" => match parts.first() {
            Some(SelectorPart::Member(field)) => (*field, 1),
            _ => return,
        },
        _ => return,
    };
    let Some(elem) = scope.elems.get(field) else {
        return;
    };
    let (Some(SelectorPart::Member(cb)), Some(SelectorPart::Call)) =
        (parts.get(cb_at), parts.get(cb_at + 1))
    else {
        return;
    };
    if !ELEMENT_CALLBACKS.contains(cb) {
        return;
    }
    let Some(closure) = selectors.get(cb_at + 1).and_then(|sel| first_positional_closure(*sel)) else {
        return;
    };
    let Some(param) = sole_untyped_parameter(closure, src) else {
        return;
    };
    if !closure_owns_receiver(closure, param, src) {
        return;
    }
    acc.nav.record_local_type(from, param, elem);
    acc.bare_calls.elements += 1;
}

/// The `function_expression` a call selector passes as its first positional
/// argument (a `named_argument` is skipped, comments too), or None.
fn first_positional_closure(call: TsNode) -> Option<TsNode> {
    let part = call.named_child(0).filter(|p| p.kind() == "argument_part")?;
    let mut cursor = part.walk();
    let args = part.named_children(&mut cursor).find(|k| k.kind() == "arguments")?;
    let mut cursor = args.walk();
    let first = args.named_children(&mut cursor).find(|k| {
        k.kind() == "argument" && k.named_child(0).is_some_and(|v| v.kind() != "named_argument")
    })?;
    let mut cursor = first.walk();
    let kids: Vec<TsNode> = first
        .named_children(&mut cursor)
        .filter(|k| k.kind() != "comment")
        .collect();
    match kids.as_slice() {
        [closure] if closure.kind() == "function_expression" => Some(*closure),
        _ => None,
    }
}

/// The one parameter of a closure, when its list holds exactly one
/// positional `formal_parameter` that is only an identifier (no type, no
/// `final`), and it is no wildcard.
fn sole_untyped_parameter<'a>(closure: TsNode<'a>, src: &'a [u8]) -> Option<&'a str> {
    let params = closure.child_by_field_name("parameters")?;
    let mut cursor = params.walk();
    let list: Vec<TsNode> = params
        .named_children(&mut cursor)
        .filter(|k| k.kind() != "comment")
        .collect();
    let [param] = list.as_slice() else {
        return None;
    };
    if param.kind() != "formal_parameter" {
        return None;
    }
    let mut cursor = param.walk();
    let kids: Vec<TsNode> = param.children(&mut cursor).collect();
    let [id] = kids.as_slice() else {
        return None;
    };
    let name = text_of(*id, src).trim();
    (id.kind() == "identifier" && !name.is_empty() && name != "_").then_some(name)
}

/// DT.5h: whether the closure parameter `param` is the only `param` its
/// enclosing declaration reads members of: the closure's body binds no
/// second `param` ([`local_names`]), and no identifier `param` followed by a
/// member selector lies in the enclosing declaration (the class member, the
/// top-level function body or variable holding it) outside the closure. A
/// local type is recorded per scope, so a second same-named receiver
/// elsewhere in the method (`for (final i in [1]) { i.isEven }` beside
/// `.map((i) => ..)`) loses the edge rather than guess.
fn closure_owns_receiver(closure: TsNode, param: &str, src: &[u8]) -> bool {
    if let Some(body) = closure.child_by_field_name("body")
        && local_names(None, body, src).contains(param)
    {
        return false;
    }
    let mut decl = closure;
    while let Some(parent) = decl.parent() {
        if matches!(
            parent.kind(),
            "class_body" | "extension_body" | "enum_body" | "extension_type_body" | "source_file"
        ) {
            break;
        }
        decl = parent;
    }
    let inside = closure.start_byte()..closure.end_byte();
    let mut stack = vec![decl];
    while let Some(n) = stack.pop() {
        if n.id() == closure.id() {
            continue;
        }
        if n.kind() == "identifier" && !inside.contains(&n.start_byte()) && text_of(n, src) == param {
            let mut next = n.next_named_sibling();
            while let Some(c) = next.filter(|c| c.kind() == "comment") {
                next = c.next_named_sibling();
            }
            if next.is_some_and(|sel| {
                sel.kind() == "selector" && matches!(selector_part(sel, src), SelectorPart::Member(_))
            }) {
                return false;
            }
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    true
}

/// CB.9 STOPGAP - `Money.zero()` / `Money.parse(..)` / `Color.pick()` on a
/// type THIS file declares. The call site is `Attribute { base: "Money", .. }`
/// and graph/src/calls.rs `resolve_attribute_target` binds an Attribute base
/// only through the caller module's import bindings: a Dart library's own
/// declarations are never bound there (nor are an `import 'x.dart'`'s
/// names), so a named constructor, factory or static member called on its
/// class never binds. Here the parser records, for the caller's scope, that
/// the name `Money` denotes the type `Money` (`CodeNav::record_local_type`,
/// the A6.2a / LA.35a receiver table), so the graph's receiver-type pass
/// resolves `Money` through the caller module's own symbols and binds
/// `zero` in that CLASS / ENUM's `class_methods` - the member METHOD this
/// packet declares. Only a type this file declares, and only when no local
/// and no member of the enclosing type in `scope` shadows the name.
///
/// REMOVAL PATH: once the graph binds a Dart library-scope type as an
/// Attribute base itself (resolve_attribute_target falling back to the
/// caller module's own type symbols, and to the names a Dart
/// `import 'x.dart'` brings in), delete this function, its one call in
/// [`push_selector_chain_calls`], `Acc::type_names` / `Acc::type_receivers`,
/// the `[dart-type-receivers]` marker and the
/// `same_file_type_member_calls_record_the_type` test.
fn record_type_receiver(qualifier: &CallQualifier, from: NodeId, scope: &CallScope, acc: &mut Acc) {
    let CallQualifier::Attribute { base, .. } = qualifier else {
        return;
    };
    if !acc.type_names.contains(base) || scope.locals.contains(base) || scope.members.contains(base) {
        return;
    }
    acc.nav.record_local_type(from, base, base);
    acc.type_receivers += 1;
}

/// The call sites of one primary + selector chain, in source order.
/// Each call with its 0-based row (LC.3b): the member selector's for
/// `x.m()`, the primary's for a bare `f()`.
fn chain_call_sites(
    primary: TsNode,
    selectors: &[TsNode],
    src: &[u8],
) -> Vec<(CallQualifier, u32)> {
    let head = text_of(primary, src);
    let is_this = primary.kind() == "this";
    let mut out = Vec::new();
    // The member access awaiting its call: (selector index, name).
    let mut pending: Option<(usize, &str)> = None;
    // True while nothing but `<T>` follows the primary, so `f()` / `f<T>()`
    // calls the primary itself.
    let mut bare = true;
    for (j, sel) in selectors.iter().enumerate() {
        match selector_part(*sel, src) {
            SelectorPart::Member(name) => {
                pending = Some((j, name));
                bare = false;
            }
            SelectorPart::TypeArgs => {}
            SelectorPart::Opaque => {
                pending = None;
                bare = false;
            }
            SelectorPart::Call => {
                let line = pending
                    .and_then(|(at, _)| selectors.get(at))
                    .map_or(line_at(primary), |sel| line_at(*sel));
                let site = match pending.take() {
                    // `this.m()` / `x.m()`: the member hangs off the primary.
                    Some((0, name)) if is_this => Some(CallQualifier::SelfMethod(name.to_string())),
                    Some((0, name)) => Some(CallQualifier::Attribute {
                        base: head.to_string(),
                        name: name.to_string(),
                    }),
                    // `this.f.m()`, `a.b().c()`, `x!.m()`: everything before
                    // the member is the receiver.
                    Some((at, name)) => {
                        let mut receiver = head.to_string();
                        for s in selectors.iter().take(at) {
                            receiver.push_str(text_of(*s, src));
                        }
                        Some(CallQualifier::ComplexReceiver {
                            receiver,
                            name: name.to_string(),
                        })
                    }
                    None if bare && !is_this => Some(CallQualifier::Bare(head.to_string())),
                    None => None,
                };
                out.extend(site.map(|q| (q, line)));
                bare = false;
            }
        }
    }
    out
}

// ============================================================================
// LA.34: Dart lexical scope for an unqualified call inside a class member
// ============================================================================
//
// Dart resolves a bare `f(..)` against the local scope first (the member's
// parameters and everything bound in its body), then the enclosing class's
// own members (instance and static), then library scope; inherited members
// come after library scope. The graph's Bare arm only knows library scope
// (imports, the file MODULE's symbols), so the parser, which holds this
// file's class body and member body, decides the first two steps:
//
//   local      -> no call site (a parameter or closure, never a class or
//                 library function)
//   member     -> SelfMethod(f): graph resolve_calls binds it against the
//                 enclosing CLASS's methods, ahead of a same-named top-level
//                 function
//   otherwise  -> Bare(f), as LA.23e emitted it
//
// Inherited members stay Bare (no heritage walk). The local set is per member
// body and conservative: a name bound anywhere in it suppresses every bare
// call of that name in the body, so a shadow in one block can lose an edge in
// a sibling block, never add a wrong one.

/// The names an unqualified call inside one class member can reach before
/// library scope.
struct CallScope<'a> {
    /// DT.5e: the qname of the type whose members `members` lists (the
    /// owner a member's METHOD hangs on); None outside a type.
    owner: Option<&'a str>,
    /// Everything the enclosing class declares: methods, getters, setters and
    /// fields, instance or static ([`class_member_names`]).
    members: &'a HashSet<String>,
    /// DT.5h: the enclosing type's [`Owner::elems`] (empty outside a type).
    elems: &'a HashMap<String, String>,
    /// The member's parameters and every name bound in its body
    /// ([`local_names`]).
    locals: HashSet<String>,
}

/// Where Dart's lexical order finds one unqualified name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameScope {
    /// A parameter or a name bound in the body.
    Local,
    /// A member of the enclosing type.
    Member,
    /// Neither: library scope (the file, its imports), or inherited.
    Library,
}

impl CallScope<'_> {
    /// Where an unqualified `name` binds: locals first, then the enclosing
    /// type's members, then library scope.
    fn lookup(&self, name: &str) -> NameScope {
        if self.locals.contains(name) {
            NameScope::Local
        } else if self.members.contains(name) {
            NameScope::Member
        } else {
            NameScope::Library
        }
    }

    /// Apply Dart's lexical order to one call site. Only `Bare` changes:
    /// `this.m()` and receiver calls pass through as LA.23e built them.
    fn classify(&self, q: CallQualifier, stats: &mut BareCallStats) -> Option<CallQualifier> {
        let CallQualifier::Bare(name) = q else {
            return Some(q);
        };
        match self.lookup(&name) {
            NameScope::Local => {
                stats.local_skip += 1;
                None
            }
            NameScope::Member => {
                stats.self_calls += 1;
                Some(CallQualifier::SelfMethod(name))
            }
            NameScope::Library => {
                stats.bare += 1;
                Some(CallQualifier::Bare(name))
            }
        }
    }
}

/// The names a class body declares, read from each `class_member`'s
/// `method_signature` (a member with a body) or `declaration` (abstract /
/// external members and fields): method, getter and setter names, and field
/// names. A field of function type is called as `f()` just like a method, and
/// Dart binds that call to the field, so a field name must shadow a top-level
/// function too. Constructor and operator names are not members: a bare
/// `Calc(..)` inside `Calc` is a constructor call and stays Bare.
fn class_member_names(class_body: TsNode, src: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut members = class_body.walk();
    for member in class_body.named_children(&mut members) {
        if member.kind() != "class_member" {
            continue;
        }
        let mut parts = member.walk();
        for part in member.named_children(&mut parts) {
            if !matches!(part.kind(), "method_signature" | "declaration") {
                continue;
            }
            let mut decls = part.walk();
            for decl in part.named_children(&mut decls) {
                match decl.kind() {
                    "function_signature" | "getter_signature" | "setter_signature" => {
                        if let Some(n) = decl.child_by_field_name("name") {
                            names.insert(text_of(n, src).to_string());
                        }
                    }
                    "initialized_identifier_list" | "static_final_declaration_list" => {
                        let mut items = decl.walk();
                        for item in decl.named_children(&mut items) {
                            if let Some(n) = item.child_by_field_name("name") {
                                names.insert(text_of(n, src).to_string());
                            }
                        }
                    }
                    "identifier_list" => {
                        let mut items = decl.walk();
                        for item in decl.named_children(&mut items) {
                            if item.kind() == "identifier" {
                                names.insert(text_of(item, src).to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    names
}

/// The names local to one member: the parameters of its `method_signature`
/// and every name bound anywhere in `body` - local variables (each declarator
/// of `var a = 1, b = 2;`), local functions, for-in loop variables, catch
/// parameters, pattern variables, and the parameters of closures and local
/// functions.
fn local_names(signature: Option<TsNode>, body: TsNode, src: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(sig) = signature {
        // A class member passes its `method_signature` / `declaration`
        // wrapper; a top-level function / setter (LA.37a) passes the
        // signature itself. LA.37b / CB.9: a constructor / factory / operator
        // body and a constructor's initializer list are walked too, and its
        // parameters are locals (`this.x` field formals bind nothing here).
        let parts: Vec<TsNode> = if matches!(sig.kind(), "function_signature" | "setter_signature") {
            vec![sig]
        } else {
            let mut c = sig.walk();
            sig.named_children(&mut c).collect()
        };
        for part in parts {
            if !matches!(
                part.kind(),
                "function_signature"
                    | "setter_signature"
                    | "constructor_signature"
                    | "constant_constructor_signature"
                    | "factory_constructor_signature"
                    | "operator_signature"
            ) {
                continue;
            }
            let mut p = part.walk();
            for params in part.children_by_field_name("parameters", &mut p) {
                if params.kind() == "formal_parameter_list" {
                    param_names(params, src, &mut names);
                }
            }
        }
    }
    let insert_field = |n: TsNode, field: &str, names: &mut HashSet<String>| {
        if let Some(id) = n.child_by_field_name(field) {
            names.insert(text_of(id, src).to_string());
        }
    };
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        match n.kind() {
            // A closure's or local function's parameters; their own nested
            // function-typed parameter lists bind nothing here.
            "formal_parameter_list" => {
                param_names(n, src, &mut names);
                continue;
            }
            // `var x = ..`, `final T x = ..`, and each further declarator.
            "initialized_variable_definition" | "initialized_identifier" => {
                insert_field(n, "name", &mut names);
            }
            // A local function's name (in a body, a `function_signature`
            // only heads a `local_function_declaration`).
            "function_signature" => insert_field(n, "name", &mut names),
            // `for (final x in xs)`: the declared loop variable.
            "for_statement" => insert_field(n, "name", &mut names),
            "catch_clause" => {
                insert_field(n, "exception", &mut names);
                insert_field(n, "stack_trace", &mut names);
            }
            // `case (var a, int b)`, `var (x, y) = ..` with typed parts.
            "variable_pattern" => insert_field(n, "name", &mut names),
            // `var (a, b) = ..`, `for (final (k, v) in ..)`: an untyped
            // pattern variable is a plain identifier inside the pattern.
            "record_pattern" | "list_pattern" | "map_pattern" | "object_pattern"
                if n
                    .parent()
                    .is_some_and(|p| matches!(p.kind(), "pattern_variable_declaration" | "for_statement")) =>
            {
                pattern_identifiers(n, src, &mut names);
            }
            _ => {}
        }
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
    }
    names
}

/// The parameter names of one `formal_parameter_list`: positional, `[..]`
/// optional and `{..}` named, each `formal_parameter`'s `name`.
///
/// A typed parameter (`int x`, `int Function(int) f`) carries a `name` field;
/// an untyped one (`(x) => ..`) and the old function-typed form
/// (`int f(int x)`) hold the name as their only direct `identifier` child.
fn param_names(list: TsNode, src: &[u8], names: &mut HashSet<String>) {
    let mut c = list.walk();
    for p in list.named_children(&mut c) {
        match p.kind() {
            "formal_parameter" => {
                let name = p.child_by_field_name("name").or_else(|| {
                    let mut k = p.walk();
                    p.named_children(&mut k).find(|n| n.kind() == "identifier")
                });
                if let Some(id) = name {
                    names.insert(text_of(id, src).to_string());
                }
            }
            "optional_formal_parameters" => param_names(p, src, names),
            _ => {}
        }
    }
}

/// Every identifier bound by a declaration pattern: the plain identifiers in
/// it, descending through nested patterns but not into a `label` (`x:` of a
/// record / object field) or a type. The wildcard `_` binds nothing.
fn pattern_identifiers(pattern: TsNode, src: &[u8], names: &mut HashSet<String>) {
    let mut stack = vec![pattern];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" {
            let name = text_of(n, src);
            if name != "_" {
                names.insert(name.to_string());
            }
            continue;
        }
        if matches!(n.kind(), "label" | "type_identifier" | "type_arguments") {
            continue;
        }
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
    }
}

// ============================================================================
// LA.23e: declared field types -> CodeNav::field_types
// ============================================================================
//
// Read by A6.2a's receiver-type pass in the graph crate: `repo.find()` /
// `this.repo.find()` inside a method binds `find` on the type of the enclosing
// class's field `repo`. A field's type comes from its declaration (`final T x;`,
// `late T x;`, `T? x;`, `T x = ...;`, `static final T x = ...;`, `p.T x;`) or,
// untyped, from a constructor-call initialiser (`final x = T();`,
// `var x = T.named();`, `final x = p.T();`, `const T()`, `new T()`).
// Constructor field formals (`UserService(this.repo)`) add nothing: the
// field's own declaration carries the type.

/// Core-library types: a field of one of these never names a repo class.
const DART_BUILTIN_TYPES: &[&str] = &[
    "int", "double", "String", "bool", "num", "dynamic", "Object", "List", "Map", "Set", "Future",
    "Stream", "Iterable", "Function", "FutureOr", "Null", "Never",
];

/// What a field `declaration` says about its type.
enum DeclaredType {
    /// A named repo-class candidate (`final UserRepo repo;`).
    Named(String),
    /// No type written (`final x = ...`, `var x = ...`): the initialiser decides.
    Untyped,
    /// A builtin, `void`, function or record type: nothing to bind.
    Rejected,
}

/// The type written in a field `declaration`, read from the children before
/// its identifier list. tree-sitter-dart inlines the type as siblings:
/// `type_identifier` (twice with a `.` for an import prefix `p.T`), then an
/// optional `type_arguments` and `?`. The last `type_identifier` is the simple
/// name, so generics, prefixes and nullability drop.
fn dart_declared_type(decl: TsNode, src: &[u8]) -> DeclaredType {
    let mut name: Option<&str> = None;
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => name = Some(text_of(child, src)),
            "function_type" | "record_type" | "void_type" => return DeclaredType::Rejected,
            "initialized_identifier_list" | "static_final_declaration_list" => break,
            _ => {}
        }
    }
    match name {
        None => DeclaredType::Untyped,
        Some(t) if DART_BUILTIN_TYPES.contains(&t) => DeclaredType::Rejected,
        Some(t) => DeclaredType::Named(t.to_string()),
    }
}

/// A capitalised identifier (`UserRepo`, `_RepoImpl`): a class name by Dart
/// convention, as opposed to a function or an import prefix.
fn is_dart_class_name(name: &str) -> bool {
    name.trim_start_matches('_')
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
}

/// The class a field initialiser constructs, for an untyped field. `item` is
/// an `initialized_identifier` / `static_final_declaration`: the field's
/// `identifier`, `=`, then the initialiser expression inlined as siblings.
fn dart_initialiser_type(item: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = item.walk();
    let kids: Vec<TsNode> = item.named_children(&mut cursor).collect();
    // kids[0] is the field name; the initialiser starts at kids[1].
    let init = kids.get(1).copied()?;
    let name = match init.kind() {
        // `const T()` / `new T()` / `const p.T()`: the last capitalised type.
        "const_object_expression" | "new_expression" => {
            let mut c = init.walk();
            let last = init
                .named_children(&mut c)
                .filter(|k| k.kind() == "type_identifier")
                .map(|k| text_of(k, src))
                .filter(|t| is_dart_class_name(t))
                .last();
            last?.to_string()
        }
        // `T(...)`, `T<A>(...)`, `T.named(...)`, `p.T(...)`, then optional
        // cascades (`T()..init()` still yields the T).
        "identifier" => {
            let head = text_of(init, src);
            let rest: Vec<TsNode> = kids.iter().skip(2).copied().collect();
            let selectors: Vec<TsNode> = rest
                .iter()
                .take_while(|k| k.kind() == "selector")
                .copied()
                .collect();
            if rest
                .iter()
                .skip(selectors.len())
                .any(|k| k.kind() != "cascade_section")
            {
                return None;
            }
            let mut member: Option<&str> = None;
            let mut members = 0usize;
            let mut called = false;
            for sel in &selectors {
                if called {
                    return None; // `T().build()`: the result is something else
                }
                match selector_part(*sel, src) {
                    SelectorPart::Member(m) => {
                        member = Some(m);
                        members += 1;
                    }
                    SelectorPart::TypeArgs => {}
                    SelectorPart::Call => called = true,
                    SelectorPart::Opaque => return None,
                }
            }
            if !called || members > 1 {
                return None;
            }
            match member {
                // `T(...)` / `T<A>(...)`
                None if is_dart_class_name(head) => head.to_string(),
                // `T.named(...)`: a named constructor or static factory
                Some(_) if is_dart_class_name(head) => head.to_string(),
                // `p.T(...)`: an import-prefixed class
                Some(m) if is_dart_class_name(m) => m.to_string(),
                _ => return None,
            }
        }
        _ => return None,
    };
    (!DART_BUILTIN_TYPES.contains(&name.as_str())).then_some(name)
}

/// Record every typed or constructor-initialised field of the class whose
/// body is `class_body` into `acc.nav.field_types` under `class_id`.
fn collect_dart_field_types(class_body: TsNode, src: &[u8], class_id: NodeId, acc: &mut Acc) {
    let mut members = class_body.walk();
    for member in class_body.named_children(&mut members) {
        if member.kind() != "class_member" {
            continue;
        }
        let mut decls = member.walk();
        for decl in member.named_children(&mut decls) {
            if decl.kind() != "declaration" {
                continue;
            }
            let declared = dart_declared_type(decl, src);
            let mut lists = decl.walk();
            for list in decl.named_children(&mut lists) {
                let item_kind = match list.kind() {
                    "initialized_identifier_list" => "initialized_identifier",
                    "static_final_declaration_list" => "static_final_declaration",
                    _ => continue,
                };
                let mut items = list.walk();
                for item in list.named_children(&mut items) {
                    if item.kind() != item_kind {
                        continue;
                    }
                    let Some(field) = item.named_child(0).filter(|n| n.kind() == "identifier")
                    else {
                        continue;
                    };
                    let ty = match &declared {
                        DeclaredType::Named(t) => Some(t.clone()),
                        DeclaredType::Untyped => dart_initialiser_type(item, src),
                        DeclaredType::Rejected => None,
                    };
                    if let Some(t) = ty {
                        acc.nav.record_field_type(class_id, text_of(field, src), &t);
                    }
                }
            }
        }
    }
}

/// DT.5h: the element type of every field of a container body declared
/// `List<T>` / `Set<T>` / `Iterable<T>` (nullable, `late`, `final`, `static`
/// alike; [`declared_element_type`]), by field name. Never the field's own
/// type: `_items.contains(x)` must never bind a method of T. Lookup-only.
fn collect_dart_field_elem_types(body: TsNode, src: &[u8]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut members = body.walk();
    for member in body.named_children(&mut members) {
        if member.kind() != "class_member" {
            continue;
        }
        let mut decls = member.walk();
        for decl in member.named_children(&mut decls) {
            if decl.kind() != "declaration" {
                continue;
            }
            let Some(elem) = declared_element_type(decl, src) else {
                continue;
            };
            let mut lists = decl.walk();
            for list in decl.named_children(&mut lists) {
                let mut items = list.walk();
                for item in list.named_children(&mut items) {
                    let name = match (list.kind(), item.kind()) {
                        ("initialized_identifier_list", "initialized_identifier")
                        | ("static_final_declaration_list", "static_final_declaration") => {
                            item.child_by_field_name("name")
                        }
                        ("identifier_list", "identifier") => Some(item),
                        _ => None,
                    };
                    if let Some(n) = name {
                        out.insert(text_of(n, src).to_string(), elem.clone());
                    }
                }
            }
        }
    }
    out
}

/// DT.5h: T of a field `declaration` typed `List<T>` / `Set<T>` /
/// `Iterable<T>` ([`ELEMENT_COLLECTIONS`]), T one non-builtin simple name
/// ([`type_argument_heads`]; a prefix and `?` dropped). A function, record
/// or `void` type, any other collection and an untyped field -> None.
fn declared_element_type(decl: TsNode, src: &[u8]) -> Option<String> {
    let mut collection: Option<&str> = None;
    let mut args: Option<TsNode> = None;
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => {
                collection = Some(text_of(child, src).trim());
                args = None;
            }
            "type_arguments" => args = Some(child),
            "function_type" | "record_type" | "void_type" => return None,
            "initialized_identifier_list" | "static_final_declaration_list" | "identifier_list" => {
                break;
            }
            _ => {}
        }
    }
    if !ELEMENT_COLLECTIONS.contains(&collection?) {
        return None;
    }
    match type_argument_heads(args?, src).as_slice() {
        [t] if !t.is_empty() => Some(t.clone()),
        _ => None,
    }
}

/// DT.5h: each top-level argument of a `type_arguments` node as its simple
/// name (`<p.Item?>` -> `Item`), `""` for a builtin ([`DART_BUILTIN_TYPES`]),
/// generic (`Map<K, V>`), function, record or `void` argument.
fn type_argument_heads(type_arguments: TsNode, src: &[u8]) -> Vec<String> {
    let mut heads = Vec::new();
    // The current argument's simple name; `Some("")` once it is not simple.
    let mut current: Option<String> = None;
    let mut cursor = type_arguments.walk();
    for child in type_arguments.children(&mut cursor) {
        match child.kind() {
            "<" | "." | "?" | "comment" => {}
            "," | ">" => {
                if let Some(head) = current.take() {
                    heads.push(head);
                }
            }
            "type_identifier" => {
                if current.as_deref() != Some("") {
                    current = Some(text_of(child, src).trim().to_string());
                }
            }
            _ => current = Some(String::new()),
        }
    }
    if let Some(head) = current {
        heads.push(head);
    }
    heads
        .into_iter()
        .map(|h| if DART_BUILTIN_TYPES.contains(&h.as_str()) { String::new() } else { h })
        .collect()
}

// ============================================================================
// DH.15: injectable constructor injection -> INJECTS refs
// ============================================================================
//
// injectable (pub.dev/packages/injectable, the get_it code generator)
// registers a class annotated `@injectable` / `@lazySingleton` /
// `@singleton` (or their `@Injectable(..)` / `@LazySingleton(..)` /
// `@Singleton(..)` constructor forms) and builds it by resolving each
// constructor parameter by TYPE, so the parameter type is a fact of the
// source, the TS / Java / C# constructor-DI rule:
//
//   @injectable
//   class HomeController {
//     final ApiService api;
//     HomeController(this.api);       INJECTS Bare(ApiService) from the CLASS
//   }
//
// A field formal (`this.api`, `{required this.api}`) takes the field's type
// from [`collect_dart_field_types`]; a typed parameter (`ApiService api`,
// `required p.Api<T>? api`, `Api this.api`) its own head, the prefix,
// generic arguments and `?` dropped. Core-library types are never
// registered services; a `@factoryParam` / `@ignoreParam` parameter is the
// caller's argument, never a container lookup. Every constructor
// (generative, `const`, factory, redirecting factory) contributes; a type
// two of them inject is one ref. Refs bind in the graph crate through
// `resolve_refs`' INJECTS chain (import binding, the module's own symbols,
// then the repo-unique name), so a package type (`Dio`) binds nothing. A
// class without the annotation, or a file not importing injectable, emits
// none: plain constructor parameters are not DI.

/// The class annotations that make injectable register (and so build) the
/// class. A form ending in `(` matches its constructor call with any
/// arguments; any other form matches exactly.
const INJECTABLE_ANNOTATIONS: &[&str] = &[
    "@injectable",
    "@Injectable(",
    "@lazySingleton",
    "@LazySingleton(",
    "@singleton",
    "@Singleton(",
];

/// Parameter annotations injectable reads as "not from the container".
const INJECTABLE_SKIP_PARAMS: &[&str] = &["factoryParam", "ignoreParam"];

/// Core-library types no injectable registration provides, beyond
/// [`DART_BUILTIN_TYPES`].
const INJECT_CORE_TYPES: &[&str] = &["Duration", "DateTime"];

/// The constructor signature kinds whose parameters injectable resolves.
const DART_CTOR_SIGNATURES: &[&str] = &[
    "constructor_signature",
    "constant_constructor_signature",
    "factory_constructor_signature",
    "redirecting_factory_constructor_signature",
];

#[derive(Default)]
struct DiStats {
    /// Classes carrying an [`INJECTABLE_ANNOTATIONS`] annotation in a file
    /// importing injectable.
    classes: usize,
    /// INJECTS refs emitted from them.
    injects: usize,
}

impl DiStats {
    /// The DH.15 fired_on line, when this file declared an injectable class.
    fn marker(&self, file_rel: &str) -> Option<String> {
        (self.classes > 0).then(|| {
            format!(
                "[dart-di] injectable classes={} injects={} path={file_rel}",
                self.classes, self.injects
            )
        })
    }
}

/// Whether the file imports any `package:injectable/` library.
fn imports_injectable(root: TsNode, src: &[u8]) -> bool {
    imports_any(root, src, &["package:injectable/"])
}

/// Whether the file imports a library whose uri starts with one of
/// `prefixes`.
fn imports_any(root: TsNode, src: &[u8], prefixes: &[&str]) -> bool {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|c| c.kind() == "import_or_export")
        .filter_map(|c| {
            named_child_of_kind(c, "library_import")
                .and_then(|lib| named_child_of_kind(lib, "import_specification"))
                .and_then(|spec| spec.child_by_field_name("uri"))
                .and_then(|u| import_uri(u, src))
        })
        .any(|uri| prefixes.iter().any(|p| uri.starts_with(p)))
}

/// Whether a `class_declaration` carries an [`INJECTABLE_ANNOTATIONS`]
/// annotation (its `annotation` children, whitespace dropped).
fn is_injectable_class(class: TsNode, src: &[u8]) -> bool {
    let mut cursor = class.walk();
    class
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "annotation")
        .any(|a| {
            let text: String = text_of(a, src).split_whitespace().collect();
            INJECTABLE_ANNOTATIONS.iter().any(|form| {
                if form.ends_with('(') {
                    text.starts_with(form)
                } else {
                    text == *form
                }
            })
        })
}

/// One INJECTS ref per distinct injected type of the injectable class `id`
/// whose body is `class_body`, from `id` in `module_id`, in source order.
fn emit_injects(class_body: TsNode, src: &[u8], id: NodeId, module_id: NodeId, acc: &mut Acc) {
    let fields = acc.nav.field_types.get(&id);
    let mut found: Vec<(String, u32)> = Vec::new();
    let mut members = class_body.walk();
    for member in class_body.named_children(&mut members) {
        if member.kind() != "class_member" {
            continue;
        }
        let mut parts = member.walk();
        for part in member.named_children(&mut parts) {
            if !matches!(part.kind(), "method_signature" | "declaration") {
                continue;
            }
            let mut sigs = part.walk();
            for sig in part.named_children(&mut sigs) {
                if !DART_CTOR_SIGNATURES.contains(&sig.kind()) {
                    continue;
                }
                if let Some(params) = sig.child_by_field_name("parameters") {
                    ctor_param_types(params, src, fields, &mut found);
                }
            }
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    acc.di.classes += 1;
    for (ty, line) in found {
        if DART_BUILTIN_TYPES.contains(&ty.as_str())
            || INJECT_CORE_TYPES.contains(&ty.as_str())
            || !seen.insert(ty.clone())
        {
            continue;
        }
        acc.di.injects += 1;
        acc.refs.push(UnresolvedRef {
            from: id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(ty),
            category: edge_category::INJECTS,
            line,
        });
    }
}

/// The injected type head and row of each parameter of one
/// `formal_parameter_list` (positional, `[..]` and `{..}`), pushed to `out`.
/// `fields` is the class's [`collect_dart_field_types`] map, which types a
/// field formal (`this.x`) that writes none of its own.
///
/// tree-sitter-dart 0.1.0 puts a parameter's annotation inside its
/// `formal_parameter` when the parameter opens its list, but as the
/// preceding SIBLING after a `,` (`{Clock? c, @factoryParam Session? s}`),
/// so both places are read.
fn ctor_param_types(
    list: TsNode,
    src: &[u8],
    fields: Option<&HashMap<String, String>>,
    out: &mut Vec<(String, u32)>,
) {
    let mut skip_next = false;
    let mut cursor = list.walk();
    for p in list.named_children(&mut cursor) {
        match p.kind() {
            "annotation" => skip_next |= is_skip_annotation(p, src),
            "formal_parameter" => {
                let skip = std::mem::take(&mut skip_next) || {
                    let mut k = p.walk();
                    p.named_children(&mut k)
                        .any(|c| c.kind() == "annotation" && is_skip_annotation(c, src))
                };
                if skip {
                    continue;
                }
                if let Some(ty) = formal_param_type(p, src, fields) {
                    out.push((ty, line_at(p)));
                }
            }
            "optional_formal_parameters" => ctor_param_types(p, src, fields, out),
            _ => {}
        }
    }
}

/// A parameter annotation naming [`INJECTABLE_SKIP_PARAMS`] (`@factoryParam`).
fn is_skip_annotation(annotation: TsNode, src: &[u8]) -> bool {
    annotation
        .child_by_field_name("name")
        .is_some_and(|n| INJECTABLE_SKIP_PARAMS.contains(&text_of(n, src)))
}

/// The type head of one `formal_parameter`. A field formal
/// (`constructor_param`: `this.x`, `Api this.x`) and a super formal
/// (`super_formal_parameter`: `Api super.x`) write their type, if any,
/// inside that child; an untyped field formal is typed by its field, an
/// untyped super formal by nothing this class says.
fn formal_param_type(
    param: TsNode,
    src: &[u8],
    fields: Option<&HashMap<String, String>>,
) -> Option<String> {
    let mut cursor = param.walk();
    let formal = param
        .named_children(&mut cursor)
        .find(|c| matches!(c.kind(), "constructor_param" | "super_formal_parameter"));
    let Some(formal) = formal else {
        return param_type_head(param, src);
    };
    if let Some(ty) = param_type_head(formal, src) {
        return Some(ty);
    }
    if formal.kind() != "constructor_param" {
        return None;
    }
    let mut k = formal.walk();
    let field = formal
        .named_children(&mut k)
        .filter(|c| c.kind() == "identifier")
        .last()?;
    fields?.get(text_of(field, src)).cloned()
}

/// The type a parameter node writes among its direct children: tree-sitter-
/// dart inlines it as siblings of the name, `type_identifier` (twice with a
/// `.` for an import prefix `p.T`), then an optional `type_arguments` and
/// `?`, so the last `type_identifier` is the simple name. The grammar wraps
/// a named field formal's `required` keyword in a `type_identifier`, which
/// is no type. None for an untyped parameter and for a function, record or
/// `void` type, or the old function-typed form (`int f(int x)`), none of
/// which a container provides.
fn param_type_head(node: TsNode, src: &[u8]) -> Option<String> {
    let mut head: Option<&str> = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "type_identifier" => {
                let t = text_of(child, src);
                if t != "required" {
                    head = Some(t);
                }
            }
            "function_type" | "record_type" | "void_type" | "formal_parameter_list" => {
                return None;
            }
            _ => {}
        }
    }
    head.map(str::to_string)
}

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

const HTTP_VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options"];

/// Pattern A: detect a client HTTP call `<recv>.<verb>('<path>', ...)` and emit a
/// shared ENDPOINT node. tree-sitter-dart shapes `dio.get('/x')` as a node whose
/// first named children are `identifier(receiver)`, `selector(.verb)`,
/// `selector((args))`; [`dart_client_call`] also reads the two shapes a generic
/// call `dio.get<T>('/x')` takes (CH.5a). `<recv>` must name an HTTP client
/// (dio / http / *client / api) — server routes (`router.get`, shelf cascades)
/// are handled by `scan_dart_routes`, which skips these client receivers so no
/// phantom ROUTE.
fn try_detect_dart_endpoint(
    n: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let Some(call) = dio_call(n, src, &acc.dio_receivers) else {
        return;
    };
    // A11.5: the authority of the pre-normalisation literal, as `host`. Only
    // the host half is read: the path stays A3.3's normaliser's, so no qname
    // moves. `$base/users` has no authority; `https://$host/x` has a
    // placeholder one, which names no service.
    let (host, _) = client_url_split(&call.raw);
    let pos = n.start_position();
    let ep = ClientEndpoint {
        method: call.method.clone(),
        path: call.path.clone(),
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: Confidence::Strong,
    };
    let extras = HitExtras {
        raw: call.changed.then_some(call.raw.as_str()),
        host: host.as_deref(),
    };
    let before = acc.nodes.len();
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
    acc.dio_bound.endpoints += usize::from(call.by_binding);
    // DT.1l: the site that wrote the (method, path)'s ENDPOINT_HIT (the node
    // just pushed) also states its shapes; a later site of the same endpoint
    // in this file writes no hit of its own.
    if acc.nodes.len() > before {
        splice_dio_shapes(n, &call, before, src, acc);
    }
}

/// One client call [`try_detect_dart_endpoint`] emits an ENDPOINT for.
struct DioCall<'t> {
    /// Upper-case HTTP verb.
    method: String,
    /// The path literal as written (interpolations as `${…}`).
    raw: String,
    /// The normalised request path.
    path: String,
    /// The normaliser rewrote `raw`.
    changed: bool,
    /// The receiver is a client only through the file's Dio bindings (CH.5c).
    by_binding: bool,
    /// The call's `arguments` (none for the relational-misparse shape, whose
    /// one argument is the path literal).
    args: Option<TsNode<'t>>,
}

/// The client call `<recv>.<verb>('<path>', ...)` held by `n`
/// ([`dart_client_call`]), when `<recv>` names an HTTP client (dio / http /
/// *client / api, or a name the file binds to a Dio) and the path is a
/// request path. The one detector both the ENDPOINT walk and DT.1l's call
/// count ([`dio_calls_in`]) read.
fn dio_call<'t>(n: TsNode<'t>, src: &[u8], dio_receivers: &HashSet<String>) -> Option<DioCall<'t>> {
    let (recv, verb_sel, sl, args) = dart_client_call(n, src)?;
    // CH.5c: a receiver the file binds to a Dio is a client whatever its name.
    let recv_name = text_of(recv, src);
    let by_binding = !is_http_client_receiver(recv_name);
    if by_binding && !dio_receivers.contains(recv_name) {
        return None;
    }
    // Verb is the first identifier under the `.verb` selector.
    let method = first_identifier_text(verb_sel, src)?.to_ascii_lowercase();
    if !HTTP_VERBS.contains(&method.as_str()) {
        return None;
    }
    // A3.3: normalise BEFORE the guard, so `dio.post('https://api/users')`
    // becomes `/users` instead of being dropped as "not a path".
    let raw = dart_string_path(sl, src);
    let (path, changed) = normalise_client_path(&raw);
    if !is_dart_request_path(&path) {
        return None; // not a request path (full-URL var, non-path first arg, etc.)
    }
    Some(DioCall {
        method: method.to_ascii_uppercase(),
        raw,
        path,
        changed,
        by_binding,
        args,
    })
}

/// [`dart_client_call`]'s parts: the receiver, the `.verb` selector, the path
/// literal and the call's `arguments`.
type ClientCallParts<'t> = (TsNode<'t>, TsNode<'t>, TsNode<'t>, Option<TsNode<'t>>);

/// The receiver `identifier`, the `.verb` selector and the first argument's
/// `string_literal` of a call `<recv>.<verb>(<args>)` held by `n`, in the three
/// shapes tree-sitter-dart 0.1.0 gives it:
///
/// - plain, `dio.get('/x', ..)`: `n`'s named children are `identifier`,
///   `selector(.get)`, `selector((args))`; the path is the first string
///   literal in the argument selector.
/// - generic selector chain, `dio.post<void>('/x', data: d)` (CH.5a): one or
///   more `selector(<T>)` sit between the verb and the argument selector and
///   are skipped. The parser picks this shape whenever the call has two or
///   more arguments, or the type argument is not also an expression
///   (`<String?>`, `<List<T>>` outside an `await`).
/// - generic relational misparse, `await dio.get<dynamic>('/x')` (CH.5a): with
///   one argument and an expression-like type argument the GLR parse prefers
///   `((await dio.get) < dynamic) > ('/x')`, a `relational_expression` whose
///   left side is another one holding the receiver and `.verb` before its `<`
///   operator, and whose right side is a `parenthesized_expression` holding the
///   argument, or a one-field `record_literal` when the argument has a
///   trailing comma (`('/x',)`). Real Dart never compares a comparison's
///   result with `>`, so the shape names a generic call; its first argument
///   must be the literal itself.
///
/// The fourth part is the call's `arguments` node (DT.1l reads its `data:`
/// request), absent for the misparse shape.
///
/// None for anything else: the caller then mints nothing, as before.
fn dart_client_call<'t>(n: TsNode<'t>, src: &[u8]) -> Option<ClientCallParts<'t>> {
    let mut c = n.walk();
    let kids: Vec<TsNode> = n.named_children(&mut c).collect();
    if n.kind() == "relational_expression" {
        return dart_generic_misparse(&kids, src).map(|(r, v, sl)| (r, v, sl, None));
    }
    if kids.len() < 3 || kids[0].kind() != "identifier" || kids[1].kind() != "selector" {
        return None;
    }
    let args_sel = kids[2..].iter().copied().find(|k| {
        !(k.kind() == "selector" && matches!(selector_part(*k, src), SelectorPart::TypeArgs))
    })?;
    if args_sel.kind() != "selector" || !matches!(selector_part(args_sel, src), SelectorPart::Call)
    {
        return None;
    }
    let sl = first_descendant_of_kind(args_sel, "string_literal")?;
    let args = first_descendant_of_kind(args_sel, "arguments");
    Some((kids[0], kids[1], sl, args))
}

/// [`dart_client_call`]'s relational-misparse shape, over the outer
/// `relational_expression`'s named children `kids`: `[inner, ">", (literal),
/// ..]` or `[inner, ">", (literal, ..), ..]`, `inner` = `[<head>, "<",
/// <type>..]`, `<head>` = `identifier`, `selector(.verb)`, possibly wrapped in
/// `unary_expression` / `await_expression`.
fn dart_generic_misparse<'t>(
    kids: &[TsNode<'t>],
    src: &[u8],
) -> Option<(TsNode<'t>, TsNode<'t>, TsNode<'t>)> {
    let kids: Vec<TsNode> = kids
        .iter()
        .copied()
        .filter(|k| k.kind() != "comment")
        .collect();
    let [inner, close, paren, ..] = kids.as_slice() else {
        return None;
    };
    if inner.kind() != "relational_expression"
        || close.kind() != "relational_operator"
        || text_of(*close, src) != ">"
        || !matches!(paren.kind(), "parenthesized_expression" | "record_literal")
    {
        return None;
    }
    let mut c = paren.walk();
    let sl = paren
        .named_children(&mut c)
        .find(|k| k.kind() != "comment")?;
    if sl.kind() != "string_literal" {
        return None;
    }
    let mut c = inner.walk();
    let inner_kids: Vec<TsNode> = inner
        .named_children(&mut c)
        .filter(|k| k.kind() != "comment")
        .collect();
    let open_at = inner_kids
        .iter()
        .position(|k| k.kind() == "relational_operator")?;
    if text_of(inner_kids[open_at], src) != "<" || open_at + 1 >= inner_kids.len() {
        return None;
    }
    let mut head: Vec<TsNode> = inner_kids[..open_at].to_vec();
    while let [only] = head.as_slice()
        && matches!(only.kind(), "unary_expression" | "await_expression")
    {
        let only = *only;
        let mut c = only.walk();
        head = only
            .named_children(&mut c)
            .filter(|k| k.kind() != "comment")
            .collect();
    }
    let [recv, verb_sel] = head.as_slice() else {
        return None;
    };
    if recv.kind() != "identifier"
        || verb_sel.kind() != "selector"
        || !matches!(selector_part(*verb_sel, src), SelectorPart::Member(_))
    {
        return None;
    }
    Some((*recv, *verb_sel, sl))
}

/// A normalised client path worth an ENDPOINT: absolute (`/users`), or an
/// interpolated base followed by a path (`$baseUrl/users` → `${…}/users`),
/// which the HTTP resolver's BaseFold tier pairs.
///
/// A path that is ONLY an interpolation (`'$url'` → `${…}`) is still rejected:
/// it names a whole URL held in a variable, and it would normalise to `/{}`,
/// which BaseFold folds to the root `/` — a guaranteed false pairing.
fn is_dart_request_path(path: &str) -> bool {
    path.starts_with('/') || path.starts_with("${…}/")
}

/// First descendant `identifier` text, depth-first.
fn first_identifier_text(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() == "identifier" {
        return Some(text_of(node, src).to_string());
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if let Some(t) = first_identifier_text(child, src) {
            return Some(t);
        }
    }
    None
}

fn first_descendant_of_kind<'a>(node: TsNode<'a>, kind: &str) -> Option<TsNode<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if let Some(found) = first_descendant_of_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

/// Reconstruct a Dart string-literal path, replacing every `$id` / `${expr}`
/// interpolation with `${…}` so it normalises like a TS template path
/// (`normalise_http_path` collapses any segment containing `${` to `{}`).
fn dart_string_path(string_literal: TsNode, src: &[u8]) -> String {
    fn rec(n: TsNode, src: &[u8], out: &mut String) {
        let mut c = n.walk();
        for child in n.named_children(&mut c) {
            let k = child.kind();
            if k == "template_substitution" {
                out.push_str("${…}");
            } else if k.starts_with("template_chars") {
                out.push_str(text_of(child, src));
            } else {
                rec(child, src, out);
            }
        }
    }
    let mut out = String::new();
    rec(string_literal, src, &mut out);
    if out.is_empty() {
        out = text_of(string_literal, src)
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string();
    }
    out
}

// ============================================================================
// CH.5a: Dio client base URLs and URL-shaped value literals
// ============================================================================
//
// quokka_android builds its client as `Dio(BaseOptions(baseUrl: Env.apiBaseUrl))`
// and calls root-relative paths (`dio.get('/protected/friends')`) against
// `/api/protected/..` server routes. The engine cannot see the AST, so the
// parse records the two inputs the endpoint fold (CH.5c) needs as build-time
// facts on the file's MODULE, never stored:
//
//   NavFact::ClientBase   every `BaseOptions(baseUrl: X)` (a call, `const` or
//                         `new`) and every `<recv>.options.baseUrl = X`: the
//                         expression text X and its 0-based row
//   NavFact::ValueLiteral every getter returning a single string literal and
//                         every top-level / static `const` / `final` initialised
//                         to one, when the literal is URL-shaped (contains
//                         `://` or starts with `/`): `Env.apiBaseUrl` ->
//                         `${…}://${…}/api`

/// What [`collect_dio_base_facts`] recorded in one file, for the
/// `[dart-dio-base]` fired_on marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DioBaseStats {
    /// `NavFact::ClientBase` facts found (equal ones are stored once).
    bases: usize,
    /// Of `bases`, those whose expression ends with `.baseUrl`: a base copied
    /// from an existing request or client (`retry.baseUrl`).
    copies: usize,
    /// `NavFact::ValueLiteral` facts found.
    literals: usize,
}

impl DioBaseStats {
    /// The `[dart-dio-base]` stderr line for `file_rel`, None when the file
    /// recorded nothing.
    fn marker(&self, file_rel: &str) -> Option<String> {
        (self.bases + self.literals > 0).then(|| {
            format!(
                "[dart-dio-base] bases={} (copy={}) literals={} file={file_rel}",
                self.bases, self.copies, self.literals
            )
        })
    }
}

/// Record this file's [`NavFact::ClientBase`] and [`NavFact::ValueLiteral`]
/// facts on `module_id`. A file that never spells `baseUrl` holds no client
/// base, so its full-tree walk is skipped.
fn collect_dio_base_facts(
    source: &str,
    root: TsNode,
    module_id: NodeId,
    acc: &mut Acc,
) -> DioBaseStats {
    let (bases, copies) = if source.contains("baseUrl") {
        collect_client_bases(root, source.as_bytes(), module_id, acc)
    } else {
        (0, 0)
    };
    let literals = collect_value_literals(root, source.as_bytes(), module_id, acc);
    DioBaseStats {
        bases,
        copies,
        literals,
    }
}

/// Every client base URL in the file, depth-first in source order over the
/// whole tree (not the body walks, which entered no closure before DT.5e:
/// quokka's Dio is built inside a top-level provider closure). Returns
/// (bases, copies).
///
/// Shape A: `BaseOptions(.., baseUrl: X, ..)`, an `identifier` followed by its
/// call selector, or a `const` / `new` construction of it. Shape B: an
/// assignment `<recv>.options.baseUrl = X` (or `??=`).
fn collect_client_bases(
    root: TsNode,
    src: &[u8],
    module_id: NodeId,
    acc: &mut Acc,
) -> (usize, usize) {
    let mut bases = 0;
    let mut copies = 0;
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if let Some((expr, row)) = client_base_at(n, src) {
            bases += 1;
            if expr.ends_with(".baseUrl") {
                copies += 1;
            }
            acc.nav.record_fact(
                module_id,
                NavFact::ClientBase {
                    via: "dio".into(),
                    expr,
                    line: u32::try_from(row).unwrap_or(u32::MAX),
                },
            );
        }
        let mut cursor = n.walk();
        let kids: Vec<TsNode> = n.named_children(&mut cursor).collect();
        stack.extend(kids.into_iter().rev());
    }
    (bases, copies)
}

/// The base-URL expression text and 0-based row when `n` is a
/// [`collect_client_bases`] shape.
fn client_base_at(n: TsNode, src: &[u8]) -> Option<(String, usize)> {
    match n.kind() {
        "identifier" if text_of(n, src) == "BaseOptions" => {
            let call = n.next_named_sibling()?;
            if call.kind() != "selector" || !matches!(selector_part(call, src), SelectorPart::Call)
            {
                return None;
            }
            let args = first_descendant_of_kind(call, "arguments")?;
            Some((base_url_argument(args, src)?, n.start_position().row))
        }
        "const_object_expression" | "new_expression" => {
            let ty = n.child_by_field_name("type")?;
            if text_of(ty, src) != "BaseOptions" {
                return None;
            }
            let args = n.child_by_field_name("arguments")?;
            Some((base_url_argument(args, src)?, n.start_position().row))
        }
        "assignment_expression" => {
            let left = n.child_by_field_name("left")?;
            let right = n.child_by_field_name("right")?;
            let target: String = text_of(left, src).split_whitespace().collect();
            if !target.ends_with(".options.baseUrl") {
                return None;
            }
            // The operator is the anonymous token right after the left side.
            let op = left.next_sibling()?;
            if !matches!(op.kind(), "=" | "??=") {
                return None;
            }
            let expr = text_of(right, src).trim();
            (!expr.is_empty()).then(|| (expr.to_string(), n.start_position().row))
        }
        _ => None,
    }
}

/// The expression text of the `baseUrl:` named argument among `args` (an
/// `arguments` node), trimmed: the source from the first node after the
/// label to the end of the argument.
fn base_url_argument(args: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        let Some(named) = (arg.kind() == "argument")
            .then(|| arg.named_child(0))
            .flatten()
            .filter(|k| k.kind() == "named_argument")
        else {
            continue;
        };
        let mut c = named.walk();
        let parts: Vec<TsNode> = named.named_children(&mut c).collect();
        let [label, value, ..] = parts.as_slice() else {
            continue;
        };
        if label.kind() != "label" || find_identifier(*label, src).as_deref() != Some("baseUrl") {
            continue;
        }
        let expr = src
            .get(value.start_byte()..named.end_byte())
            .and_then(|b| std::str::from_utf8(b).ok())?
            .trim();
        return (!expr.is_empty()).then(|| expr.to_string());
    }
    None
}

/// Every URL-shaped value literal the file declares (see the section head),
/// at library level and in each named class, mixin, enum, extension and
/// extension type. Returns how many were found.
fn collect_value_literals(root: TsNode, src: &[u8], module_id: NodeId, acc: &mut Acc) -> usize {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "getter_signature" => {
                if let Some(v) = getter_literal(child, sibling_body(child), src) {
                    found.push(v);
                }
            }
            "static_final_declaration_list" => found.extend(const_literals(child, src)),
            "class_declaration"
            | "mixin_declaration"
            | "enum_declaration"
            | "extension_declaration"
            | "extension_type_declaration" => {
                let (Some(owner), Some(body)) = (
                    container_name(child, src),
                    child.child_by_field_name("body"),
                ) else {
                    continue;
                };
                let mut members = body.walk();
                for member in body.named_children(&mut members) {
                    if member.kind() != "class_member" {
                        continue;
                    }
                    for (name, value) in member_literals(member, src) {
                        found.push((format!("{owner}.{name}"), value));
                    }
                }
            }
            _ => {}
        }
    }
    let count = found.len();
    for (name, value) in found {
        acc.nav
            .record_fact(module_id, NavFact::ValueLiteral { name, value });
    }
    count
}

/// A container's own name (its `name` field), None for an unnamed extension,
/// whose static members nothing outside it can name.
fn container_name(decl: TsNode, src: &[u8]) -> Option<String> {
    let name = decl.child_by_field_name("name")?;
    if name.kind() == "extension_type_name" {
        return find_identifier(name, src);
    }
    Some(text_of(name, src).to_string())
}

/// The URL-shaped literals one `class_member` declares, by member name: a
/// getter (`method_signature` holding a `getter_signature`, its sibling
/// `function_body`) or a `static const` / `static final` list.
fn member_literals(member: TsNode, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = member.walk();
    for part in member.named_children(&mut cursor) {
        match part.kind() {
            "method_signature" => {
                let mut c = part.walk();
                let getter = part
                    .named_children(&mut c)
                    .find(|k| k.kind() == "getter_signature");
                if let Some(getter) = getter
                    && let Some(v) = getter_literal(getter, sibling_body(part), src)
                {
                    out.push(v);
                }
            }
            "declaration" => {
                let mut c = part.walk();
                for list in part.named_children(&mut c) {
                    if list.kind() == "static_final_declaration_list" {
                        out.extend(const_literals(list, src));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// A getter whose body yields one string literal: `=> '<lit>'`, or a block
/// with exactly one `return` (returns inside nested closures and local
/// functions not counted) of a string literal. (name, value) when the value is
/// URL-shaped.
fn getter_literal(sig: TsNode, body: Option<TsNode>, src: &[u8]) -> Option<(String, String)> {
    let name = text_of(sig.child_by_field_name("name")?, src).to_string();
    let body = body?;
    let mut cursor = body.walk();
    let inner = body
        .named_children(&mut cursor)
        .find(|k| k.kind() != "comment")?;
    let literal = if inner.kind() == "block" {
        let mut returns = Vec::new();
        collect_returns(inner, &mut returns);
        let [ret] = returns.as_slice() else {
            return None;
        };
        let mut c = ret.walk();
        let values: Vec<TsNode> = ret
            .named_children(&mut c)
            .filter(|k| k.kind() != "comment")
            .collect();
        let [value] = values.as_slice() else {
            return None;
        };
        *value
    } else {
        inner
    };
    (literal.kind() == "string_literal")
        .then(|| url_shaped(dart_string_path(literal, src)))
        .flatten()
        .map(|value| (name, value))
}

/// Every `return_statement` under `n`, not entering a nested closure, local
/// function or class.
fn collect_returns<'t>(n: TsNode<'t>, out: &mut Vec<TsNode<'t>>) {
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        match child.kind() {
            "return_statement" => out.push(child),
            "function_expression"
            | "function_body"
            | "local_function_declaration"
            | "lambda_expression"
            | "class_definition"
            | "class_declaration" => {}
            _ => collect_returns(child, out),
        }
    }
}

/// The URL-shaped string-literal initialisers of a `static_final_declaration_list`
/// (library-level `const` / `final`, or a member's `static const` / `static
/// final`), by declared name.
fn const_literals(list: TsNode, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = list.walk();
    for decl in list.named_children(&mut cursor) {
        if decl.kind() != "static_final_declaration" {
            continue;
        }
        let (Some(name), Some(value)) = (
            decl.child_by_field_name("name"),
            decl.child_by_field_name("value"),
        ) else {
            continue;
        };
        if value.kind() != "string_literal" {
            continue;
        }
        if let Some(v) = url_shaped(dart_string_path(value, src)) {
            out.push((text_of(name, src).to_string(), v));
        }
    }
    out
}

/// `value` when it is URL-shaped (contains `://` or starts with `/`), which
/// keeps env / l10n strings out of the cache sidecar.
fn url_shaped(value: String) -> Option<String> {
    (value.contains("://") || value.starts_with('/')).then_some(value)
}

// ============================================================================
// CH.5c: receivers bound to a Dio client
// ============================================================================
//
// The client-call detector and the route scan both decide "client or server
// router" by the receiver's NAME (`is_http_client_receiver`: dio / http /
// *client / *api). quokka_android's token refresh builds a one-off client
// `final fresh = Dio(BaseOptions(..))..interceptors.add(..)` and calls
// `fresh.post('/auth/refresh')`: no client name, so the scan minted a phantom
// server ROUTE `POST /auth/refresh` and no ENDPOINT. The file's own
// declarations say what `fresh` is, so they are read first.

/// What the binding set reclassified in one file, for the
/// `[dart-dio-receivers]` fired_on marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DioBoundStats {
    /// Client ENDPOINT call sites minted through the binding set only.
    endpoints: usize,
    /// Route-scan hits skipped through the binding set only (each would
    /// have been a phantom server ROUTE).
    route_skips: usize,
}

impl DioBoundStats {
    /// `[dart-dio-receivers] bound=<n> endpoints=<e> route_skips=<r> file=<f>`,
    /// None when the binding set changed nothing in the file.
    fn marker(&self, bound: usize, file_rel: &str) -> Option<String> {
        (self.endpoints + self.route_skips > 0).then(|| {
            format!(
                "[dart-dio-receivers] bound={bound} endpoints={} route_skips={} file={file_rel}",
                self.endpoints, self.route_skips
            )
        })
    }
}

/// How one declaration binds its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// Declared `Dio` / `Dio?`, or initialised with a `Dio(..)` construction
    /// (`new` / `const` too, and a cascade `Dio(..)..x` after it).
    Dio,
    /// Declared another type, or initialised with another class's
    /// construction (`Router()`).
    Other,
}

/// Every name the file binds to a Dio client: a local (`final` / `var` /
/// typed), a field or top-level variable, a getter declared `Dio`, or a
/// parameter declared `Dio`. One name space per file, not per scope, so a
/// name the file ALSO binds to something else (a `Router()`, a `String`) is
/// left out and the name heuristic alone decides it, as before. A file that
/// never spells `Dio` binds nothing, so its walk is skipped.
fn collect_dio_receivers(source: &str, root: TsNode, src: &[u8]) -> HashSet<String> {
    if !source.contains("Dio") {
        return HashSet::new();
    }
    let mut seen: HashMap<String, (bool, bool)> = HashMap::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if let Some((name, binding)) = dio_binding_at(n, src) {
            let slot = seen.entry(name).or_default();
            match binding {
                Binding::Dio => slot.0 = true,
                Binding::Other => slot.1 = true,
            }
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    seen.into_iter()
        .filter(|(_, (dio, other))| *dio && !*other)
        .map(|(name, _)| name)
        .collect()
}

/// The name `n` declares and how, when `n` is a binding shape that says:
///
/// - `initialized_variable_definition` (a local): `[type] name value..`
/// - `initialized_identifier` / `static_final_declaration` (a field or a
///   top-level variable): `name value..`, its type the `type_identifier`
///   just before its list (inside a `declaration`, or beside the list at
///   library level)
/// - `getter_signature` / `formal_parameter`: `[type] name`
fn dio_binding_at(n: TsNode, src: &[u8]) -> Option<(String, Binding)> {
    if !matches!(
        n.kind(),
        "initialized_variable_definition"
            | "initialized_identifier"
            | "static_final_declaration"
            | "getter_signature"
            | "formal_parameter"
    ) {
        return None;
    }
    let mut cursor = n.walk();
    let kids: Vec<TsNode> = n
        .named_children(&mut cursor)
        .filter(|k| k.kind() != "comment")
        .collect();
    let at = kids.iter().position(|k| k.kind() == "identifier")?;
    let name = text_of(kids[at], src).to_string();
    let own_type = kids[..at]
        .iter()
        .rev()
        .find(|k| k.kind() == "type_identifier")
        .copied();
    let declared = match n.kind() {
        "initialized_variable_definition" | "getter_signature" | "formal_parameter" => own_type,
        _ => n
            .parent()
            .and_then(|list| list.prev_named_sibling())
            .filter(|t| t.kind() == "type_identifier"),
    };
    if let Some(t) = declared {
        let binding = if text_of(t, src) == "Dio" {
            Binding::Dio
        } else {
            Binding::Other
        };
        return Some((name, binding));
    }
    if matches!(n.kind(), "getter_signature" | "formal_parameter") {
        return None;
    }
    construction_binding(kids.get(at + 1..)?, src).map(|b| (name, b))
}

/// The class an initialiser constructs: `X(..)` (an `identifier` and its
/// call selector, a cascade possibly after), `new X(..)` or `const X(..)`.
/// Dio when X is `Dio`, Other for another capitalised class, None for
/// anything else (a call result, a literal, a provider read).
fn construction_binding(value: &[TsNode], src: &[u8]) -> Option<Binding> {
    let class = match value {
        [ident, sel, ..] if ident.kind() == "identifier" && sel.kind() == "selector" => {
            if !matches!(selector_part(*sel, src), SelectorPart::Call) {
                return None;
            }
            text_of(*ident, src)
        }
        [expr, ..] if matches!(expr.kind(), "new_expression" | "const_object_expression") => {
            text_of(expr.child_by_field_name("type")?, src)
        }
        _ => return None,
    };
    if class == "Dio" {
        Some(Binding::Dio)
    } else if class.starts_with(|c: char| c.is_ascii_uppercase()) {
        Some(Binding::Other)
    } else {
        None
    }
}

// ============================================================================
// Dart route extraction (v0.4.11a R-dart)
// ============================================================================
//
// Two framework surfaces covered via text scan (robust to tree-sitter-dart's
// no-field-name quirk):
//
//   go_router navigation:  GoRoute(path: '/users', ...)  → page:/users (ANY)
//   shelf / shelf_router:  router.get('/users', handler) → GET /users
//                          ..post('/x', h)  (cascade)    → POST /x
//
// Shape B ROUTE nodes (METHOD <path> qname + Text ROUTE_METHOD cell) for the
// server routes; go_router pages take the `page:<path>` qname (LB.4c).
//
// DH.15: a server route's argument #1 is its handler (`..get('/users/<id>',
// getUser)`): a HANDLED_BY ref from the ROUTE, `Bare(getUser)` or
// `Attribute { base, name }` for `handlers.getUser`, bound by the graph
// crate's `resolve_refs` (the module's own symbols first). A closure or a
// call names no handler. go_router pages get none: their `builder:` is a
// closure and nav links are LA.6's.

fn scan_dart_routes(source: &str, repo: RepoId, module_id: NodeId, acc: &mut Acc) -> ShelfRouteStats {
    // Track emitted routes to dedup — a file may hit the same path twice
    // between the tree walk and the text scan.
    let mut seen = std::collections::HashSet::new();
    let mut stats = ShelfRouteStats::default();
    // DH.15: one HANDLED_BY ref per (route, handler) a file registers.
    let mut handlers: HashSet<(NodeId, String)> = HashSet::new();

    // go_router — look for `GoRoute(path:` token.
    let needle = "GoRoute(";
    let mut idx = 0;
    while let Some(pos) = source[idx..].find(needle) {
        let start = idx + pos + needle.len();
        if let Some(path) = extract_kwarg_string(&source[start..], "path") {
            // A3.4: go_router is BROWSER/app navigation, not a server
            // endpoint — mark it so the HTTP route index skips it.
            let _ = emit_dart_route("ANY", &path, repo, acc, &mut seen, true);
        }
        idx = start;
    }

    // shelf-style `.get('/...' / .post('/...' / etc. — SERVER routes only.
    // A client HTTP call (`dio.get('/x')`) has the same textual shape but is an
    // outbound ENDPOINT, handled by `try_detect_dart_endpoint`; skip it here (by
    // receiver name, or CH.5c: a receiver the file binds to a Dio) so it is
    // not mis-emitted as a phantom server ROUTE.
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let needle = format!(".{method}(");
        let mut idx = 0;
        while let Some(pos) = source[idx..].find(&needle) {
            let dot_at = idx + pos;
            let after = &source[dot_at + needle.len()..];
            if let Some((path, lit_end)) = first_string_literal_dart_end(after)
                && path.starts_with('/')
            {
                let recv = ident_before(source, dot_at);
                if !is_http_client_receiver(recv) {
                    if acc.dio_receivers.contains(recv) {
                        acc.dio_bound.route_skips += 1;
                    } else {
                        let verb = method.to_ascii_uppercase();
                        let known = seen.len();
                        let route_id = emit_dart_route(&verb, &path, repo, acc, &mut seen, false);
                        if seen.len() > known {
                            stats.routes += 1;
                        }
                        // DH.15: argument #1 of the same call is the handler.
                        if let Some(handler) = after.get(lit_end..).and_then(dart_route_handler) {
                            let key = match &handler {
                                CallQualifier::Bare(n) => n.clone(),
                                CallQualifier::Attribute { base, name } => format!("{base}.{name}"),
                                _ => String::new(),
                            };
                            if handlers.insert((route_id, key)) {
                                stats.handlers += 1;
                                acc.refs.push(UnresolvedRef {
                                    from: route_id,
                                    from_module: module_id,
                                    qualifier: handler,
                                    category: edge_category::HANDLED_BY,
                                    line: glia_code_domain::line_of(source, dot_at),
                                });
                            }
                        }
                    }
                }
            }
            idx = dot_at + needle.len();
        }
    }
    stats
}

/// DH.15: the shelf server routes a file registered and the HANDLED_BY refs
/// read from them, for the `[dart-routes]` marker.
#[derive(Default)]
struct ShelfRouteStats {
    /// Distinct server ROUTE nodes the method scan emitted.
    routes: usize,
    /// HANDLED_BY refs from them.
    handlers: usize,
}

impl ShelfRouteStats {
    /// The DH.15 fired_on line, when this file registered a server route.
    fn marker(&self, file_rel: &str) -> Option<String> {
        (self.routes > 0).then(|| {
            format!(
                "[dart-routes] shelf routes={} handlers={} path={file_rel}",
                self.routes, self.handlers
            )
        })
    }
}

/// DH.15: the handler a shelf route call names as its argument #1, read
/// from `rest`, the source right after the path literal. shelf_router's
/// `router.get('/users/<id>', getUser)` passes a function: an identifier is
/// `Bare(name)`, a `base.name` tear-off (`handlers.getUser`) an
/// `Attribute`. The argument must end at the next `,` or `)`, so a closure
/// (`(Request r) => ..`), a call (`makeHandler()`) or any other expression
/// names nothing; so does a call with no second argument, or a literal.
fn dart_route_handler(rest: &str) -> Option<CallQualifier> {
    let rest = rest.trim_start().strip_prefix(',')?.trim_start();
    let (first, rest) = split_dart_ident(rest)?;
    let (second, rest) = match rest.strip_prefix('.') {
        Some(after_dot) => {
            let (name, rest) = split_dart_ident(after_dot)?;
            (Some(name), rest)
        }
        None => (None, rest),
    };
    if !matches!(rest.trim_start().chars().next(), Some(',' | ')')) {
        return None;
    }
    match second {
        None if matches!(first, "null" | "true" | "false" | "this" | "super") => None,
        None => Some(CallQualifier::Bare(first.to_string())),
        Some(name) => Some(CallQualifier::Attribute {
            base: first.to_string(),
            name: name.to_string(),
        }),
    }
}

/// A Dart identifier (`[A-Za-z_$][A-Za-z0-9_$]*`) at the start of `s`, and
/// the rest of `s` after it.
fn split_dart_ident(s: &str) -> Option<(&str, &str)> {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(s.len());
    let ident = s.get(..end)?;
    if ident.is_empty() || ident.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some((ident, s.get(end..)?))
}

fn extract_kwarg_string(s: &str, key: &str) -> Option<String> {
    // Looks for `path: '/x'` or `path: "/x"` allowing whitespace.
    let mut i = 0;
    while let Some(pos) = s[i..].find(key) {
        let start = i + pos + key.len();
        let rest = s[start..].trim_start();
        if let Some(after_colon) = rest.strip_prefix(':')
            && let Some(lit) = first_string_literal_dart(after_colon)
        {
            return Some(lit);
        }
        i = start;
    }
    None
}

fn first_string_literal_dart(s: &str) -> Option<String> {
    first_string_literal_dart_end(s).map(|(lit, _)| lit)
}

/// [`first_string_literal_dart`], plus the byte offset in `s` just past the
/// literal's closing quote (DH.15: where a route call's next argument
/// starts).
fn first_string_literal_dart_end(s: &str) -> Option<(String, usize)> {
    let trimmed = s.trim_start();
    let lead = s.len() - trimmed.len();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let quote = match bytes[0] {
        b'\'' | b'"' => bytes[0],
        _ => return None,
    };
    let rest = &trimmed[1..];
    let end = rest.find(quote as char)?;
    let lit = &rest[..end];
    if lit.is_empty() || lit.len() > 256 {
        return None;
    }
    // lead + the opening quote + the literal + the closing quote.
    Some((lit.to_string(), lead + 1 + end + 1))
}

/// Emit one ROUTE node. `nav` marks a *browser/app navigation* target
/// (go_router) as opposed to a real server route (shelf): A3.4 adds an ORIGIN
/// `provenance: nav_route` cell so the graph crate's HTTP route index skips it
/// and a same-app `dio.get('/users')` cannot pair to the app's own navigation
/// table. The node itself survives — only the pairing is suppressed.
///
/// LB.4c: a nav page lives in its own qname namespace — qname `page:<path>`,
/// display name `<path>` — so it never shares a NodeId with a server route on
/// the same path (`<METHOD> <path>`). ROUTE_METHOD stays the passed method.
///
/// Dart has no stats channel out of `parse_file`, so nav routes marked here are
/// not in the engine's `[extract] nav-routes marked: N` count; they ARE in that
/// line's `(page-qnamed P)` figure, which the engine counts off the parses'
/// `page:` qnames. The graph-side `[http] nav-routes excluded from route index`
/// marker covers them too.
///
/// Returns the route's id, emitted here or already seen in this file: DH.15's
/// HANDLED_BY ref starts from it.
fn emit_dart_route(
    method: &str,
    path: &str,
    repo: RepoId,
    acc: &mut Acc,
    seen: &mut std::collections::HashSet<(String, String)>,
    nav: bool,
) -> NodeId {
    let route_qname = if nav {
        format!("page:{path}")
    } else {
        format!("{method} {path}")
    };
    let display_name = if nav { path } else { route_qname.as_str() };
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_qname);
    let key = (method.to_string(), route_qname.clone());
    if !seen.insert(key) {
        return route_id;
    }
    let mut cells = vec![Cell {
        kind: cell_type::ROUTE_METHOD,
        payload: CellPayload::Text(method.to_string()),
    }];
    if nav {
        cells.push(Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(r#"{"provenance":"nav_route"}"#.to_string()),
        });
    }
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Medium,
        cells,
    });
    acc.nav
        .record(route_id, display_name, &route_qname, node_kind::ROUTE, None);
    route_id
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

/// [`entity_cells`] for a declaration whose body is a sibling of its
/// signature (LA.37a: a top-level function): CODE is the source from the
/// start of `first` to the end of `last`, POSITION spans both, DOC is the
/// `///` above `first`.
fn span_cells(first: &TsNode, last: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    // Tree-sitter byte offsets sit on char boundaries; `get` + `from_utf8`
    // still cannot panic, and fall back to the first node's own text.
    let code = src
        .get(first.start_byte()..last.end_byte())
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or_else(|| text_of(*first, src));
    let mut cells = vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(code.to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(glia_doc::position_json_span(first, last, file_rel)),
        },
    ];
    if let Some(doc) = glia_doc::leading_doc(first, src) {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    cells
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
// DT.1l: Dart wire models (SCHEMA_FIELDS) and Dio call shapes (ENDPOINT_HIT)
// ============================================================================
//
// The client half of the models pass (DT.1g) for Dart, written in the
// `wire` vocabulary (code-domain/src/wire.rs, format `dart`):
//
// - **Models.** A CLASS with a hand-written `factory C.fromJson(<p>)` whose
//   first parameter is a `Map` carries SCHEMA_FIELDS: its JSON names are the
//   string-literal subscripts `<p>['k']` the factory reads, in source order,
//   deduped (Dart's wire names, never its field names: `j['display_name']`
//   fills `displayName`). A key's `type` is the model a `C.fromJson(<p>['k'])`
//   / `.map(C.fromJson)` reaches (`C`, `List<C>`), else the `as T` cast on the
//   subscript, else the declared type of the field the named argument
//   assigns the subscript to (directly: through casts, parentheses, `!` and
//   `??` only, never a conversion call), else `dynamic`; `field` is that
//   named argument when it differs; `required` holds when a non-nullable
//   cast of it runs whenever the factory does (not in a branch, a loop or a
//   closure: the factory throws without the key), or the directly assigned
//   field is non-nullable with no `??` default. The schema is `open`
//   when `<p>` escapes the literal reads (passed whole, spread, a computed
//   subscript, iterated). A `@JsonSerializable` class instead lists its
//   instance fields: `@JsonKey(name:)` renames, the class's `fieldRename`
//   (snake / kebab / pascal / screamingSnake) applied otherwise,
//   `@JsonKey(includeFromJson: false)` / `ignore: true` skipped, `required`
//   when non-nullable with no `defaultValue` and no constructor default (or
//   `@JsonKey(required: true)`). A generated file ([`dart_generated`]:
//   protobuf, build_runner output, a generator banner) writes none: a
//   protobuf `fromJson($core.String i, ..)` is no map reader. Zero keys, no
//   cell.
// - **Responses.** A Dio call's response is read inside its one callable
//   body ([`enclosing_callable`]), only when that body holds exactly one Dio
//   call (else counted `skipped_multi_call`): `<res>.data` for the call's
//   result variable, or `(await <call>).data`, then the locals bound from it
//   (`!`, `?? ..`, `as Map<..>`, subscripts, `whereType` / `cast` / `toList`
//   / `where`, `first` / `last` / `single` / `[0]` an element). A
//   string-literal subscript adds an envelope key, `C.fromJson(x)` names the
//   model, `.map(C.fromJson)` / `.map((e) => C.fromJson(e))` an array of it,
//   and several alternatives a Union ([`ReadTree::shape`]). A shape read only
//   through a helper (`_parse(res.data)`) is not followed.
// - **Requests.** A Dio call's `data:` argument: a map literal is an Object of
//   its string keys (a collection-`if` key is not required, a spread or
//   computed key leaves it open), `x.toJson()` the declared type of `x`;
//   anything else writes no request.
//
// The shapes are spliced onto the ENDPOINT_HIT the site wrote, after every
// other key, through `wire::append_hit_fields`; a hit with neither keeps its
// bytes. The endpoint fold appends a folded `host` after them. Type names stay
// unresolved text: the models pass binds them in the hit file's module
// (DT.1g). fired_on: `[dart-wire] models=<m> (fromjson=<f> serializable=<s>)
// skipped_generated=<g> responses=<r> requests=<q> skipped_multi_call=<k>
// file=<path>`, per file with m+r+q+g > 0.

/// DT.1l: the `[dart-wire]` counters of one file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct DartWireStats {
    /// Classes whose schema was read from a `fromJson` factory's keys.
    fromjson: usize,
    /// `@JsonSerializable` classes whose schema was read from their fields.
    serializable: usize,
    /// `fromJson` factories and `@JsonSerializable` classes in a generated
    /// file, which write no schema.
    skipped_generated: usize,
    /// ENDPOINT_HITs that gained a `response`.
    responses: usize,
    /// ENDPOINT_HITs that gained a `request`.
    requests: usize,
    /// ENDPOINT_HITs whose callable holds more than one Dio call, so no
    /// response was read.
    skipped_multi_call: usize,
}

impl DartWireStats {
    fn marker(&self, file_rel: &str) -> Option<String> {
        #[cfg(test)]
        LAST_DART_WIRE.with(|w| *w.borrow_mut() = self.clone());
        let models = self.fromjson + self.serializable;
        (models + self.responses + self.requests + self.skipped_generated > 0).then(|| {
            format!(
                "[dart-wire] models={models} (fromjson={} serializable={}) skipped_generated={} \
                 responses={} requests={} skipped_multi_call={} file={file_rel}",
                self.fromjson,
                self.serializable,
                self.skipped_generated,
                self.responses,
                self.requests,
                self.skipped_multi_call
            )
        })
    }
}

// The last file's `[dart-wire]` counters on this thread, which the tests read.
#[cfg(test)]
thread_local! {
    static LAST_DART_WIRE: std::cell::RefCell<DartWireStats> =
        std::cell::RefCell::new(DartWireStats::default());
}

/// Generated Dart file names: the protobuf plugin's files and build_runner
/// output (json_serializable / freezed / auto_route / mockito). The same
/// endings as the extractors' generated-file classifier (DE.7), which this
/// parser cannot depend on without a new crate edge.
const DART_GENERATED_SUFFIXES: &[&str] = &[
    ".pb.dart",
    ".pbjson.dart",
    ".pbenum.dart",
    ".pbserver.dart",
    ".pbgrpc.dart",
    ".g.dart",
    ".freezed.dart",
    ".gr.dart",
    ".mocks.dart",
];

/// How many lines from the top [`dart_generated`] reads for a banner.
const DART_BANNER_LINES: usize = 40;

/// DT.1l: the file is machine-written: a [`DART_GENERATED_SUFFIXES`] name, or
/// a `//` comment among its first [`DART_BANNER_LINES`] lines saying
/// `generated code` (build_runner's banner), or comment lines saying
/// `generated` beside `do not modify` / `do not edit` (protoc's
/// `Generated code. Do not modify.`).
fn dart_generated(file_rel: &str, source: &str) -> bool {
    let base = file_rel.rsplit(['/', '\\']).next().unwrap_or(file_rel).to_ascii_lowercase();
    if DART_GENERATED_SUFFIXES.iter().any(|s| base.ends_with(s)) {
        return true;
    }
    let mut generated = false;
    let mut keep_off = false;
    for line in source.lines().take(DART_BANNER_LINES) {
        let line = line.trim_start_matches('\u{feff}').trim();
        if !(line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')) {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains("generated code") && lower.contains("modify") {
            return true;
        }
        generated |= lower.contains("generated");
        keep_off |= lower.contains("do not modify") || lower.contains("do not edit");
        if generated && keep_off {
            return true;
        }
    }
    false
}

/// DT.1l: the SCHEMA_FIELDS of the class `class` (body `body`): its
/// `@JsonSerializable` fields, else its `fromJson` factory's keys (see the
/// section head). `None` for a class that is neither, a generated file's
/// model (counted) and a schema with no field.
fn dart_model_schema(class: TsNode, body: TsNode, src: &[u8], acc: &mut Acc) -> Option<ModelSchema> {
    let serializable = class_annotation(class, "JsonSerializable", src);
    let factory = from_json_factory(body, src);
    if serializable.is_none() && factory.is_none() {
        return None;
    }
    if acc.generated {
        acc.wire.skipped_generated += 1;
        return None;
    }
    let schema = match (serializable, factory) {
        (Some(annotation), _) => {
            let schema = serializable_schema(annotation, body, src);
            acc.wire.serializable += usize::from(!schema.fields.is_empty());
            schema
        }
        (None, Some(f)) => {
            let schema = from_json_schema(&f, &class_fields(body, src), src)?;
            acc.wire.fromjson += usize::from(!schema.fields.is_empty());
            schema
        }
        (None, None) => return None,
    };
    (!schema.fields.is_empty()).then_some(schema)
}

/// A class's `fromJson` factory: its body and its first parameter.
struct FromJson<'t> {
    body: TsNode<'t>,
    /// The first parameter's name, when it can hold the decoded JSON object
    /// ([`MAP_PARAM_TYPES`] or untyped). A `String` (protobuf's
    /// `fromJson($core.String i, ..)`) or any other type is no map reader.
    map_param: Option<String>,
}

/// The first-parameter types a `fromJson` factory reads a decoded JSON object
/// through: `Map<..>`, and the `dynamic` / `Object` a factory that also takes
/// a list declares (`ActivityPage.fromJson(dynamic raw)`).
const MAP_PARAM_TYPES: &[&str] = &["Map", "dynamic", "Object"];

/// The `factory <C>.fromJson(..)` member of a class body.
fn from_json_factory<'t>(body: TsNode<'t>, src: &[u8]) -> Option<FromJson<'t>> {
    let mut members = body.walk();
    for member in body.named_children(&mut members) {
        if member.kind() != "class_member" {
            continue;
        }
        let kids = named_kids(member);
        let Some(sig) = kids
            .iter()
            .filter(|k| k.kind() == "method_signature")
            .find_map(|m| named_kids(*m).into_iter().find(|p| p.kind() == "factory_constructor_signature"))
        else {
            continue;
        };
        if constructor_name(sig, src).as_deref() != Some("fromJson") {
            continue;
        }
        let Some(fn_body) = kids.iter().copied().find(|k| k.kind() == "function_body") else {
            continue;
        };
        let first = sig
            .child_by_field_name("parameters")
            .and_then(|list| first_formal_parameter(list));
        let map_param = first.and_then(|p| {
            let name = p.child_by_field_name("name")?;
            let map_like = match param_type_head(p, src).as_deref() {
                Some(head) => MAP_PARAM_TYPES.contains(&head),
                None => named_kids(p).len() == 1,
            };
            map_like.then(|| text_of(name, src).to_string())
        });
        return Some(FromJson { body: fn_body, map_param });
    }
    None
}

/// The first `formal_parameter` of a parameter list, positional or not.
fn first_formal_parameter(list: TsNode) -> Option<TsNode> {
    let mut cursor = list.walk();
    for p in list.named_children(&mut cursor) {
        match p.kind() {
            "formal_parameter" => return Some(p),
            "optional_formal_parameters" => return first_formal_parameter(p),
            _ => {}
        }
    }
    None
}

/// The schema a `fromJson(Map ..)` factory reads (section head); `None` when
/// its first parameter is no `Map`.
fn from_json_schema(f: &FromJson, fields: &[(String, String)], src: &[u8]) -> Option<ModelSchema> {
    let param = f.map_param.as_deref()?;
    let mut flow = JsonFlow::new(src, FlowRoot::Param(param.to_string()));
    flow.walk(f.body);
    let tree = ReadTree::build(&flow.reads, &flow.key_at);
    let mut schema = ModelSchema::new(WireFormat::Dart);
    schema.open = flow.escaped;
    for (key, _, t) in &tree.keys {
        let assigned: Vec<(String, bool)> = flow
            .key_sites
            .iter()
            .filter(|(k, _)| k == key)
            .filter_map(|(_, sel)| assigned_field(*sel, f.body, fields, src))
            .collect();
        let label = assigned.first().map(|(l, _)| l.clone());
        let declared = assigned
            .iter()
            .find(|(_, direct)| *direct)
            .and_then(|(l, _)| fields.iter().find(|(n, _)| n == l))
            .map(|(_, ty)| ty.clone());
        let ty = t
            .models
            .first()
            .cloned()
            .or_else(|| t.each.as_ref().and_then(|e| e.models.first()).map(|m| format!("List<{m}>")))
            .or_else(|| t.casts.first().cloned())
            .or_else(|| declared.clone())
            .unwrap_or_else(|| "dynamic".to_string());
        let required = t.required() || declared.as_deref().is_some_and(|d| !dart_nullable(d) && !t.defaulted);
        let mut field = ModelField::new(key, &ty, required);
        field.field = label.filter(|l| l != key);
        schema.fields.push(field);
    }
    Some(schema)
}

/// The class field a subscript's value is assigned to: the label of the
/// nearest `named_argument` holding it (below the factory body `top`), when
/// it names one of the class's `fields`; and whether the subscript IS the
/// assigned value (only casts, parentheses, `!` and a `??` default between),
/// so the field's declared type describes the wire value. Through a call or
/// a conversion (`joinStatusFrom(j['s'])`, `(j['n'] as num?)?.toInt()`) the
/// field is still named, its type is not.
fn assigned_field(sel: TsNode, top: TsNode, fields: &[(String, String)], src: &[u8]) -> Option<(String, bool)> {
    let only_bangs = |n: TsNode| {
        let mut next = n.next_named_sibling();
        while let Some(k) = next {
            if k.kind() == "selector" && text_of(k, src).trim() != "!" {
                return false;
            }
            next = k.next_named_sibling();
        }
        true
    };
    let mut direct = only_bangs(sel);
    let mut at = sel;
    while let Some(p) = at.parent() {
        if p.id() == top.id() {
            return None;
        }
        match p.kind() {
            "named_argument" => {
                let label = p.named_child(0).filter(|l| l.kind() == "label")?;
                let name = text_of(label, src).trim_end_matches(':').trim();
                return fields
                    .iter()
                    .any(|(n, _)| n == name)
                    .then(|| (name.to_string(), direct));
            }
            "type_cast_expression" | "if_null_expression" => {}
            "parenthesized_expression" => direct &= only_bangs(p),
            _ => direct = false,
        }
        at = p;
    }
    None
}

/// A type text that admits null: `T?`, `dynamic`, `Null`, `Object?`.
fn dart_nullable(ty: &str) -> bool {
    let t = ty.trim();
    t.ends_with('?') || matches!(t, "dynamic" | "Null" | "void")
}

/// The class's annotation `@<name>` / `@<name>(..)`.
fn class_annotation<'t>(class: TsNode<'t>, name: &str, src: &[u8]) -> Option<TsNode<'t>> {
    named_kids(class).into_iter().find(|a| {
        a.kind() == "annotation" && a.child_by_field_name("name").is_some_and(|n| text_of(n, src) == name)
    })
}

/// An annotation's named arguments: label -> its value nodes.
fn annotation_args<'t>(annotation: TsNode<'t>, src: &[u8]) -> Vec<(String, Vec<TsNode<'t>>)> {
    let Some(args) = named_kids(annotation)
        .into_iter()
        .find(|k| k.kind() == "annotation_arguments")
    else {
        return Vec::new();
    };
    named_kids(args)
        .into_iter()
        .filter(|a| a.kind() == "argument")
        .filter_map(|a| a.named_child(0).filter(|n| n.kind() == "named_argument"))
        .filter_map(|na| {
            let kids = named_kids(na);
            let (label, value) = kids.split_first()?;
            (label.kind() == "label")
                .then(|| (text_of(*label, src).trim_end_matches(':').trim().to_string(), value.to_vec()))
        })
        .collect()
}

/// The instance fields of a class body, in source order: `(name, type text)`.
/// A static or untyped field is left out.
fn class_fields(body: TsNode, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for member in named_kids(body) {
        if member.kind() != "class_member" {
            continue;
        }
        out.extend(member_fields(member, src));
    }
    out
}

/// The instance fields one `class_member` declares: `(name, type text)`.
/// Static fields, untyped fields and non-field members give none.
fn member_fields(member: TsNode, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for decl in named_kids(member) {
        if decl.kind() != "declaration" {
            continue;
        }
        let mut c = decl.walk();
        if decl.children(&mut c).any(|t| t.kind() == "static") {
            continue;
        }
        let Some(list) = named_kids(decl)
            .into_iter()
            .find(|k| k.kind() == "initialized_identifier_list")
        else {
            continue;
        };
        let Some(ty) = type_text(decl, list.id(), src) else {
            continue;
        };
        for item in named_kids(list) {
            if item.kind() != "initialized_identifier" {
                continue;
            }
            if let Some(name) = item.child_by_field_name("name") {
                out.push((text_of(name, src).to_string(), ty.clone()));
            }
        }
    }
    out
}

/// The type a declaration-like node writes among its children before the
/// child `stop` (a name or an identifier list): its `type_identifier`s (the
/// grammar's `required` keyword excluded), `type_arguments`, function /
/// record / void types and the `.` / `?` tokens between them, concatenated
/// (`p.Member?`, `List<String>`). `None` when none is written.
fn type_text(node: TsNode, stop: usize, src: &[u8]) -> Option<String> {
    let mut out = String::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.id() == stop {
            break;
        }
        match child.kind() {
            "type_identifier" if text_of(child, src) != "required" => out.push_str(text_of(child, src)),
            "type_arguments" | "function_type" | "record_type" | "void_type" => {
                out.push_str(&text_of(child, src).split_whitespace().collect::<Vec<_>>().join(" "));
            }
            "?" | "." if !out.is_empty() => out.push_str(child.kind()),
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The type an `as T` `type_cast` names.
fn cast_type(cast: TsNode, src: &[u8]) -> Option<String> {
    type_text(cast, usize::MAX, src)
}

/// A `@JsonSerializable` class's schema: its instance fields (section head).
fn serializable_schema(annotation: TsNode, body: TsNode, src: &[u8]) -> ModelSchema {
    let rename = annotation_args(annotation, src)
        .into_iter()
        .find(|(l, _)| l == "fieldRename")
        .and_then(|(_, v)| v.iter().rev().find_map(|n| first_identifier_text(*n, src).filter(|_| n.kind() == "selector")))
        .unwrap_or_default();
    let defaults = constructor_defaults(body, src);
    let mut schema = ModelSchema::new(WireFormat::Dart);
    for member in named_kids(body) {
        if member.kind() != "class_member" {
            continue;
        }
        let key = named_kids(member)
            .into_iter()
            .find(|a| a.kind() == "annotation" && a.child_by_field_name("name").is_some_and(|n| text_of(n, src) == "JsonKey"))
            .map(|a| annotation_args(a, src))
            .unwrap_or_default();
        let flag = |name: &str, want: &str| {
            key.iter()
                .any(|(l, v)| l == name && v.first().is_some_and(|n| text_of(*n, src) == want))
        };
        if flag("includeFromJson", "false") || flag("ignore", "true") {
            continue;
        }
        let renamed = key
            .iter()
            .find(|(l, _)| l == "name")
            .and_then(|(_, v)| v.first().and_then(|n| literal_text(*n, src)));
        let has_default = key.iter().any(|(l, _)| l == "defaultValue");
        for (name, ty) in member_fields(member, src) {
            let json = renamed.clone().unwrap_or_else(|| field_rename(&name, &rename));
            let required = flag("required", "true")
                || (!dart_nullable(&ty) && !has_default && !defaults.contains(&name));
            let mut field = ModelField::new(&json, &ty, required);
            field.field = Some(name).filter(|n| *n != json);
            schema.fields.push(field);
        }
    }
    schema
}

/// The field formals (`this.x`) any constructor of the class gives a default
/// value (`{this.x = 1}`). Lookup-only.
fn constructor_defaults(body: TsNode, src: &[u8]) -> HashSet<String> {
    fn walk(list: TsNode, src: &[u8], out: &mut HashSet<String>) {
        let kids = named_kids(list);
        let optional = list.kind() == "optional_formal_parameters";
        for (i, p) in kids.iter().enumerate() {
            match p.kind() {
                "optional_formal_parameters" => walk(*p, src, out),
                "formal_parameter" => {
                    let defaulted = optional
                        && kids
                            .get(i + 1)
                            .is_some_and(|n| !matches!(n.kind(), "formal_parameter" | "annotation"));
                    let field = named_kids(*p)
                        .into_iter()
                        .find(|c| c.kind() == "constructor_param")
                        .and_then(|c| named_kids(c).into_iter().rfind(|i| i.kind() == "identifier"));
                    if let (true, Some(f)) = (defaulted, field) {
                        out.insert(text_of(f, src).to_string());
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = HashSet::new();
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "formal_parameter_list" {
            walk(n, src, &mut out);
            continue;
        }
        if matches!(n.kind(), "function_body" | "class_body") && n.id() != body.id() {
            continue;
        }
        stack.extend(named_kids(n));
    }
    out
}

/// json_serializable's `FieldRename` applied to a field name: `snake`
/// (`orderId` -> `order_id`), `kebab` (`order-id`), `pascal` (`OrderId`),
/// `screamingSnake` (`ORDER_ID`); `none` or anything else keeps it.
fn field_rename(name: &str, rename: &str) -> String {
    let split = |sep: char| {
        let mut out = String::with_capacity(name.len() + 4);
        for (i, c) in name.chars().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                out.push(sep);
            }
            out.push(c.to_ascii_lowercase());
        }
        out
    };
    match rename {
        "snake" => split('_'),
        "kebab" => split('-'),
        "screamingSnake" => split('_').to_ascii_uppercase(),
        "pascal" => {
            let mut c = name.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        }
        _ => name.to_string(),
    }
}

/// The text of a string literal without interpolation or escapes; `None`
/// for any other node, and for an empty literal.
fn literal_text(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() != "string_literal" {
        return None;
    }
    let mut out = String::new();
    for part in named_kids(node) {
        for piece in named_kids(part) {
            if !piece.kind().starts_with("template_chars") {
                return None;
            }
            out.push_str(text_of(piece, src));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A node's named children, comments dropped.
fn named_kids(node: TsNode) -> Vec<TsNode> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|k| k.kind() != "comment")
        .collect()
}

// ---- the JSON value flow ----------------------------------------------------

/// One step from a JSON value to a part of it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Step {
    /// A string-literal subscript: the value's key.
    Key(String),
    /// An element of an array value (`.map`, `[0]`, `.first`).
    Each,
}

/// What one read did with the value at its path.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Terminal {
    /// `C.fromJson(v)`: the value is a `C`.
    Model(String),
    /// `v as T`.
    Cast(String),
    /// Read, with neither.
    Plain,
}

/// One read of a JSON value: where (`path`), what was done with it, whether
/// a `??` default covers it, and whether it is `firm`: a non-nullable cast
/// that runs whenever the body does ([`JsonFlow::guarded`]), so the body
/// throws when the part is absent.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Read {
    path: Vec<Step>,
    terminal: Terminal,
    defaulted: bool,
    firm: bool,
}

/// Where a [`JsonFlow`]'s JSON comes from.
enum FlowRoot<'t> {
    /// A `fromJson` factory's map parameter: the object itself.
    Param(String),
    /// A Dio call: `.data` of its result variable (`var`), or of the
    /// awaited call itself.
    Dio { var: Option<String>, call: TsNode<'t> },
}

/// What an expression evaluates to.
enum Flow {
    /// A part of the JSON value, at `path`.
    Json(Vec<Step>, bool),
    /// The Dio `Response` before its `.data`.
    Response,
}

/// How deep `.map((e) => ..)` callbacks nest before the flow stops following.
const FLOW_DEPTH: u8 = 6;

/// The iterable members that keep the value they are called on, for the
/// flow: a filter, a cast or a copy of the same elements.
const PASS_THROUGH: &[&str] = &["whereType", "cast", "toList", "toSet", "where"];

/// DT.1l: the JSON reads of one callable body, from one [`FlowRoot`]:
/// locals bound to a part of the value, and every read of a part (a key, a
/// cast, a model). Single-pass in source order; the maps are only probed.
struct JsonFlow<'t, 's> {
    src: &'s [u8],
    root: FlowRoot<'t>,
    /// Local name -> the part it holds. Lookup-only.
    locals: HashMap<String, Vec<Step>>,
    /// Every read, in evaluation order (a re-evaluated read repeats).
    reads: Vec<Read>,
    /// A keyed path -> the least byte offset of a subscript reaching it, the
    /// keys' source order. Lookup-only.
    key_at: HashMap<Vec<Step>, usize>,
    /// The subscript selectors that read a top-level key, with the key.
    key_sites: Vec<(String, TsNode<'t>)>,
    /// A [`FlowRoot::Param`] was used other than through literal subscripts.
    escaped: bool,
    depth: u8,
    /// The body the first [`JsonFlow::walk`] visits.
    top: Option<TsNode<'t>>,
}

impl<'t, 's> JsonFlow<'t, 's> {
    fn new(src: &'s [u8], root: FlowRoot<'t>) -> Self {
        JsonFlow {
            src,
            root,
            locals: HashMap::new(),
            reads: Vec::new(),
            key_at: HashMap::new(),
            key_sites: Vec::new(),
            escaped: false,
            depth: 0,
            top: None,
        }
    }

    /// Visit `top` in source order: bind each local whose initialiser is a
    /// part of the value, and evaluate every selector chain, cast and `??`
    /// (re-evaluating a nested one repeats its reads, which
    /// [`ReadTree::build`] folds).
    fn walk(&mut self, top: TsNode<'t>) {
        self.top.get_or_insert(top);
        let mut stack = vec![top];
        while let Some(n) = stack.pop() {
            match n.kind() {
                "initialized_variable_definition" => self.bind(n),
                "type_cast_expression" | "if_null_expression" | "relational_expression" => {
                    self.eval_node(n);
                }
                "argument" | "named_argument" | "spread_element" => self.escape_whole(n),
                _ => {}
            }
            let kids = named_kids(n);
            let mut i = 0;
            while i < kids.len() {
                let len = kids[i + 1..].iter().take_while(|k| k.kind() == "selector").count();
                if kids[i].kind() != "selector" && len > 0 {
                    self.eval_parts(&kids[i..=i + len]);
                }
                i += 1 + len;
            }
            stack.extend(kids.into_iter().rev());
        }
    }

    /// `<p>` passed whole (`f(json)`, `Base(raw: json)`) or spread
    /// (`...json`): the factory reads keys this body does not list.
    fn escape_whole(&mut self, n: TsNode) {
        let kids = named_kids(n);
        let value = match kids.as_slice() {
            [only] => *only,
            [label, only] if label.kind() == "label" => *only,
            _ => return,
        };
        if let FlowRoot::Param(p) = &self.root
            && value.kind() == "identifier"
            && text_of(value, self.src) == p
        {
            self.escaped = true;
        }
    }

    fn bind(&mut self, def: TsNode<'t>) {
        let Some(name) = def.child_by_field_name("name").map(|n| text_of(n, self.src).to_string()) else {
            return;
        };
        let mut c = def.walk();
        let value: Vec<TsNode> = def
            .children_by_field_name("value", &mut c)
            .filter(|k| k.kind() != "comment")
            .collect();
        if value.is_empty() || self.is_root_name(&name) {
            return;
        }
        match self.eval_parts(&value) {
            Some(Flow::Json(path, _)) => {
                self.locals.insert(name, path);
            }
            _ => {
                self.locals.remove(&name);
            }
        }
    }

    fn is_root_name(&self, name: &str) -> bool {
        match &self.root {
            FlowRoot::Param(p) => p == name,
            FlowRoot::Dio { var, .. } => var.as_deref() == Some(name),
        }
    }

    fn read(&mut self, path: Vec<Step>, terminal: Terminal, defaulted: bool) {
        self.reads.push(Read {
            path,
            terminal,
            defaulted,
            firm: false,
        });
    }

    /// `n` may not run when the body does: it sits in a branch of a
    /// conditional, an `if`, a collection `if`, a loop, a `switch` case, a
    /// `catch`, a closure, the right side of `&&` / `||` / `??`.
    fn guarded(&self, n: TsNode) -> bool {
        let first_holds = |p: TsNode, at: TsNode| named_kids(p).first().is_some_and(|c| c.id() == at.id());
        let mut at = n;
        while let Some(p) = at.parent() {
            if self.top.is_some_and(|t| t.id() == p.id()) {
                return false;
            }
            let guarded = match p.kind() {
                "conditional_expression" | "if_statement" | "if_element" | "logical_and_expression"
                | "logical_or_expression" => !first_holds(p, at),
                "if_null_expression" => {
                    let mut c = p.walk();
                    p.children(&mut c)
                        .take_while(|k| k.kind() != "??")
                        .all(|k| k.id() != at.id())
                }
                "function_expression" | "for_statement" | "while_statement" | "do_statement"
                | "switch_statement_case" | "switch_expression_case" | "catch_clause" => true,
                _ => false,
            };
            if guarded {
                return true;
            }
            at = p;
        }
        false
    }

    fn lookup(&self, name: &str) -> Option<Flow> {
        match &self.root {
            FlowRoot::Param(p) if p == name => return Some(Flow::Json(Vec::new(), false)),
            FlowRoot::Dio { var: Some(v), .. } if v == name => return Some(Flow::Response),
            _ => {}
        }
        self.locals.get(name).map(|p| Flow::Json(p.clone(), false))
    }

    /// `n` is the Dio call, or it awaited / parenthesised.
    fn is_call(&self, n: TsNode) -> bool {
        let FlowRoot::Dio { call, .. } = &self.root else {
            return false;
        };
        let mut at = n;
        loop {
            if at.id() == call.id() {
                return true;
            }
            if !matches!(at.kind(), "unary_expression" | "await_expression" | "parenthesized_expression") {
                return false;
            }
            match named_kids(at).as_slice() {
                [only] => at = *only,
                _ => return false,
            }
        }
    }

    /// The value of one whole expression node.
    fn eval_node(&mut self, n: TsNode<'t>) -> Option<Flow> {
        if self.is_call(n) {
            return Some(Flow::Response);
        }
        match n.kind() {
            "identifier" => self.lookup(text_of(n, self.src)),
            "parenthesized_expression" | "argument" => self.eval_parts(&named_kids(n)),
            "type_cast_expression" => {
                let kids = named_kids(n);
                let (cast, value) = kids.split_last()?;
                if cast.kind() != "type_cast" {
                    return None;
                }
                let Flow::Json(path, defaulted) = self.eval_parts(value)? else {
                    return None;
                };
                if let Some(ty) = cast_type(*cast, self.src) {
                    let firm = !dart_nullable(&ty) && !defaulted && !self.guarded(n);
                    self.reads.push(Read {
                        path: path.clone(),
                        terminal: Terminal::Cast(ty),
                        defaulted,
                        firm,
                    });
                }
                Some(Flow::Json(path, defaulted))
            }
            "if_null_expression" => {
                let mut c = n.walk();
                let lhs: Vec<TsNode> = n
                    .children(&mut c)
                    .take_while(|k| k.kind() != "??")
                    .filter(|k| k.is_named() && k.kind() != "comment")
                    .collect();
                let Flow::Json(path, _) = self.eval_parts(&lhs)? else {
                    return None;
                };
                self.read(path.clone(), Terminal::Plain, true);
                Some(Flow::Json(path, true))
            }
            "relational_expression" => self.eval_generic_misparse(n),
            _ => None,
        }
    }

    /// A generic call on a JSON value that tree-sitter-dart 0.1.0 misparses
    /// as a comparison (CH.5a's shape, [`dart_client_call`]):
    /// `raw.whereType<Map<String, dynamic>>().map(C.fromJson)` is
    /// `((raw.whereType < Map<String, dynamic>) > ()).map(..)`, the outer
    /// `relational_expression` holding `[inner, ">", (args), selectors..]`
    /// and the inner one the receiver chain before its `<`. The member before
    /// the `<` is applied with the parenthesised arguments as its call: a
    /// pass-through ([`PASS_THROUGH`]) with none, or `map<T>(cb)`; the outer
    /// selectors then apply as usual.
    fn eval_generic_misparse(&mut self, n: TsNode<'t>) -> Option<Flow> {
        let kids = named_kids(n);
        let [inner, close, args, rest @ ..] = kids.as_slice() else {
            return None;
        };
        if inner.kind() != "relational_expression"
            || text_of(*close, self.src) != ">"
            || !matches!(args.kind(), "record_literal" | "parenthesized_expression")
            || rest.iter().any(|k| k.kind() != "selector")
        {
            return None;
        }
        let chain = named_kids(*inner);
        let open = chain.iter().position(|k| k.kind() == "relational_operator")?;
        if text_of(chain[open], self.src) != "<" {
            return None;
        }
        let (member, receiver) = chain[..open].split_last()?;
        let SelectorPart::Member(m) = selector_part(*member, self.src) else {
            return None;
        };
        let Flow::Json(mut path, defaulted) = self.eval_parts(receiver)? else {
            return None;
        };
        let call_args = named_kids(*args);
        if m == "map" {
            path.push(Step::Each);
            self.map_callback(&call_args, path);
            return None;
        }
        if !PASS_THROUGH.contains(&m) || !call_args.is_empty() {
            return None;
        }
        let mut cur = Flow::Json(path, defaulted);
        let mut i = 0;
        while i < rest.len() {
            cur = self.step(cur, rest, &mut i)?;
        }
        Some(cur)
    }

    /// The value of a primary and its selectors. `C.fromJson(x)` (or
    /// `p.C.fromJson(x)`) records `x` as a `C` and is no JSON.
    fn eval_parts(&mut self, parts: &[TsNode<'t>]) -> Option<Flow> {
        let (&head, sels) = parts.split_first()?;
        if sels.is_empty() {
            return self.eval_node(head);
        }
        if head.kind() == "identifier"
            && let Some((model, call)) = from_json_call(head, sels, self.src)
        {
            if let Some(arg) = first_positional(call)
                && let Some(Flow::Json(path, defaulted)) = self.eval_node(arg)
            {
                self.read(path, Terminal::Model(model), defaulted);
            }
            return None;
        }
        let mut cur = self.eval_node(head)?;
        let mut i = 0;
        while i < sels.len() {
            cur = self.step(cur, sels, &mut i)?;
        }
        Some(cur)
    }

    /// Apply the selector at `sels[*i]` (and the call it takes) to `cur`.
    fn step(&mut self, cur: Flow, sels: &[TsNode<'t>], i: &mut usize) -> Option<Flow> {
        let sel = sels[*i];
        *i += 1;
        let (mut path, defaulted) = match cur {
            Flow::Response => {
                return matches!(selector_part(sel, self.src), SelectorPart::Member("data"))
                    .then(|| Flow::Json(Vec::new(), false));
            }
            Flow::Json(path, defaulted) => (path, defaulted),
        };
        let at_root = path.is_empty();
        if let Some(index) = subscript(sel, self.src) {
            match index {
                Index::Key(k) => {
                    if at_root {
                        self.key_sites.push((k.clone(), sel));
                    }
                    path.push(Step::Key(k));
                    let at = sel.start_byte();
                    let first = self.key_at.entry(path.clone()).or_insert(at);
                    *first = (*first).min(at);
                    self.read(path.clone(), Terminal::Plain, false);
                }
                Index::Int => path.push(Step::Each),
                Index::Other => {
                    self.escaped |= at_root;
                    return None;
                }
            }
            return Some(Flow::Json(path, defaulted));
        }
        match selector_part(sel, self.src) {
            SelectorPart::TypeArgs => Some(Flow::Json(path, defaulted)),
            SelectorPart::Opaque if text_of(sel, self.src).trim() == "!" => Some(Flow::Json(path, defaulted)),
            SelectorPart::Member("map") => {
                let call = sels
                    .get(*i)
                    .copied()
                    .filter(|c| matches!(selector_part(*c, self.src), SelectorPart::Call))?;
                *i += 1;
                path.push(Step::Each);
                let kids = first_positional(call).map(named_kids).unwrap_or_default();
                self.map_callback(&kids, path);
                None
            }
            SelectorPart::Member(m) if PASS_THROUGH.contains(&m) => {
                while let Some(next) = sels.get(*i) {
                    match selector_part(*next, self.src) {
                        SelectorPart::TypeArgs => *i += 1,
                        SelectorPart::Call => {
                            *i += 1;
                            break;
                        }
                        _ => break,
                    }
                }
                Some(Flow::Json(path, defaulted))
            }
            SelectorPart::Member("first" | "last" | "single" | "firstOrNull" | "lastOrNull") => {
                path.push(Step::Each);
                Some(Flow::Json(path, defaulted))
            }
            SelectorPart::Member("containsKey") => None,
            _ => {
                self.escaped |= at_root && matches!(self.root, FlowRoot::Param(_));
                None
            }
        }
    }

    /// `.map(<cb>)` over an array whose element is at `elem`, `kids` the
    /// callback argument's parts: a `C.fromJson` tear-off reads the element
    /// as a `C`; a one-parameter closure is followed with its parameter bound
    /// to the element.
    fn map_callback(&mut self, kids: &[TsNode<'t>], elem: Vec<Step>) {
        if let Some(model) = from_json_tear_off(kids, self.src) {
            self.read(elem, Terminal::Model(model), false);
            return;
        }
        let [cb] = kids else {
            return;
        };
        if cb.kind() != "function_expression" || self.depth >= FLOW_DEPTH {
            return;
        }
        let params: Vec<TsNode> = cb
            .child_by_field_name("parameters")
            .map(|l| named_kids(l).into_iter().filter(|p| p.kind() == "formal_parameter").collect())
            .unwrap_or_default();
        let [param] = params.as_slice() else {
            // A two-parameter callback is a Map's `.map((k, v) => ..)`: over
            // the factory's own parameter it reads every key.
            self.escaped |= params.len() == 2 && elem == [Step::Each] && matches!(self.root, FlowRoot::Param(_));
            return;
        };
        let Some(name) = param
            .child_by_field_name("name")
            .or_else(|| named_kids(*param).into_iter().rfind(|k| k.kind() == "identifier"))
            .map(|n| text_of(n, self.src).to_string())
        else {
            return;
        };
        let Some(body) = cb.child_by_field_name("body") else {
            return;
        };
        let saved = self.locals.insert(name.clone(), elem);
        self.depth += 1;
        match named_kids(body).as_slice() {
            [block] if block.kind() == "block" => self.walk(*block),
            parts => {
                self.eval_parts(parts);
            }
        }
        self.depth -= 1;
        match saved {
            Some(p) => self.locals.insert(name, p),
            None => self.locals.remove(&name),
        };
    }
}

/// `C.fromJson(..)` / `p.C.fromJson(..)` at `head` + `sels`: the model name
/// and the call selector.
fn from_json_call<'t>(head: TsNode<'t>, sels: &[TsNode<'t>], src: &[u8]) -> Option<(String, TsNode<'t>)> {
    let name = text_of(head, src);
    let (model, rest) = match sels {
        [m, rest @ ..] if is_dart_class_name(name) => (name.to_string(), (m, rest)),
        [c, m, rest @ ..] if !is_dart_class_name(name) => match selector_part(*c, src) {
            SelectorPart::Member(cls) if is_dart_class_name(cls) => (format!("{name}.{cls}"), (m, rest)),
            _ => return None,
        },
        _ => return None,
    };
    let (m, rest) = rest;
    let call = rest.first().copied()?;
    (matches!(selector_part(*m, src), SelectorPart::Member("fromJson"))
        && matches!(selector_part(call, src), SelectorPart::Call))
    .then_some((model, call))
}

/// `C.fromJson` / `p.C.fromJson` as a value (a tear-off): the model name.
fn from_json_tear_off(kids: &[TsNode], src: &[u8]) -> Option<String> {
    let (head, sels) = kids.split_first()?;
    if head.kind() != "identifier" {
        return None;
    }
    let name = text_of(*head, src);
    let parts: Vec<SelectorPart> = sels.iter().map(|s| selector_part(*s, src)).collect();
    match parts.as_slice() {
        [SelectorPart::Member("fromJson")] if is_dart_class_name(name) => Some(name.to_string()),
        [SelectorPart::Member(cls), SelectorPart::Member("fromJson")] if is_dart_class_name(cls) => {
            Some(format!("{name}.{cls}"))
        }
        _ => None,
    }
}

/// The first positional `argument` of a call selector.
fn first_positional(call: TsNode) -> Option<TsNode> {
    let args = first_descendant_of_kind(call, "arguments")?;
    named_kids(args)
        .into_iter()
        .filter(|a| a.kind() == "argument")
        .find(|a| a.named_child(0).is_some_and(|n| n.kind() != "named_argument"))
}

/// What a subscript selector indexes with.
enum Index {
    /// A string literal: a key.
    Key(String),
    /// An integer literal: an element.
    Int,
    /// Anything else.
    Other,
}

/// The index of a `[..]` / `?[..]` selector; `None` for any other selector.
fn subscript(sel: TsNode, src: &[u8]) -> Option<Index> {
    let inner = sel.named_child(0)?;
    if !matches!(
        inner.kind(),
        "unconditional_assignable_selector" | "conditional_assignable_selector"
    ) {
        return None;
    }
    if !inner.child(0).is_some_and(|t| matches!(t.kind(), "[" | "?[" | "?")) {
        return None;
    }
    let index = named_kids(inner).into_iter().next()?;
    Some(match index.kind() {
        "string_literal" => literal_text(index, src).map_or(Index::Other, Index::Key),
        "decimal_integer_literal" | "hex_integer_literal" => Index::Int,
        _ => Index::Other,
    })
}

/// DT.1l: the reads of one flow folded into a tree of the value's parts.
#[derive(Debug, Default)]
struct ReadTree {
    /// The models the part was read as (`C.fromJson`), first read first.
    models: Vec<String>,
    /// The `as T` casts on the part.
    casts: Vec<String>,
    /// A `??` default covers some read of it.
    defaulted: bool,
    /// Some read of it is firm ([`Read`]).
    firm: bool,
    /// Its keys, in source order: (key, first byte, part).
    keys: Vec<(String, usize, ReadTree)>,
    /// Its array element.
    each: Option<Box<ReadTree>>,
}

impl ReadTree {
    fn build(reads: &[Read], key_at: &HashMap<Vec<Step>, usize>) -> ReadTree {
        let mut root = ReadTree::default();
        for r in reads {
            let mut node = &mut root;
            for (i, step) in r.path.iter().enumerate() {
                node = match step {
                    Step::Key(k) => {
                        let ix = match node.keys.iter().position(|(n, _, _)| n == k) {
                            Some(ix) => ix,
                            None => {
                                let at = key_at.get(&r.path[..=i]).copied().unwrap_or(usize::MAX);
                                node.keys.push((k.clone(), at, ReadTree::default()));
                                node.keys.len() - 1
                            }
                        };
                        &mut node.keys[ix].2
                    }
                    Step::Each => node.each.get_or_insert_with(Box::default),
                };
            }
            match &r.terminal {
                Terminal::Model(m) if !node.models.contains(m) => node.models.push(m.clone()),
                Terminal::Cast(c) if !node.casts.contains(c) => node.casts.push(c.clone()),
                _ => {}
            }
            node.defaulted |= r.defaulted;
            node.firm |= r.firm;
        }
        root.sort_keys();
        root
    }

    fn sort_keys(&mut self) {
        self.keys.sort_by_key(|(_, at, _)| *at);
        for (_, _, t) in &mut self.keys {
            t.sort_keys();
        }
        if let Some(e) = &mut self.each {
            e.sort_keys();
        }
    }

    /// A key's part is always present: some firm read of it.
    fn required(&self) -> bool {
        self.firm
    }

    /// The part as a wire shape: an Object of its keys, each model it was
    /// read as, an array of each model its elements were; one of them alone,
    /// several a Union. `None` when it was read as none of these.
    fn shape(&self, depth: usize) -> Option<Shape> {
        if depth > SHAPE_DEPTH {
            return None;
        }
        let mut members: Vec<Shape> = Vec::new();
        if !self.keys.is_empty() {
            let fields = self
                .keys
                .iter()
                .map(|(k, _, t)| ShapeField::new(k, t.required(), t.value_shape(depth + 1)))
                .collect();
            members.push(Shape::Object { fields, open: false });
        }
        for m in &self.models {
            members.push(Shape::Named {
                ty: m.clone(),
                array: false,
                model: None,
            });
        }
        for m in self.each.iter().flat_map(|e| e.models.iter()) {
            members.push(Shape::Named {
                ty: m.clone(),
                array: true,
                model: None,
            });
        }
        let mut unique: Vec<Shape> = Vec::with_capacity(members.len());
        for m in members {
            if !unique.contains(&m) {
                unique.push(m);
            }
        }
        match unique.len() {
            0 => None,
            1 => unique.pop(),
            _ => Some(Shape::Union(unique)),
        }
    }

    /// A key's value: its [`ReadTree::shape`], else its cast type, else open.
    fn value_shape(&self, depth: usize) -> Shape {
        self.shape(depth)
            .or_else(|| self.casts.first().map(|c| Shape::named(c)))
            .unwrap_or_else(|| Shape::open("dynamic"))
    }
}

/// How many levels of envelope keys a response shape lists.
const SHAPE_DEPTH: usize = 4;

// ---- Dio shapes ---------------------------------------------------------------

/// DT.1l: splice the shapes the Dio call at `n` states onto the ENDPOINT_HIT
/// of the node at `acc.nodes[at]` (the one the call just wrote).
fn splice_dio_shapes(n: TsNode, call: &DioCall, at: usize, src: &[u8], acc: &mut Acc) {
    let request = call.args.and_then(|a| dio_request_shape(a, src));
    let response = match enclosing_callable(n) {
        Some(body) if dio_calls_in(body, src, acc) == 1 => dio_response_shape(body, n, src),
        Some(_) => {
            acc.wire.skipped_multi_call += 1;
            None
        }
        None => None,
    };
    if response.is_none() && request.is_none() {
        return;
    }
    let Some(cell) = acc.nodes[at]
        .cells
        .iter_mut()
        .find(|c| c.kind == cell_type::ENDPOINT_HIT)
    else {
        return;
    };
    let CellPayload::Json(payload) = &mut cell.payload else {
        return;
    };
    *payload = wire::append_hit_fields(payload, response.as_ref(), request.as_ref());
    acc.wire.responses += usize::from(response.is_some());
    acc.wire.requests += usize::from(request.is_some());
}

/// The callable body a node sits in: the nearest method / function body or
/// closure body.
fn enclosing_callable(n: TsNode) -> Option<TsNode> {
    let mut at = n;
    while let Some(p) = at.parent() {
        match p.kind() {
            "function_body" | "function_expression_body" => return Some(p),
            "class_body" | "source_file" => return None,
            _ => at = p,
        }
    }
    None
}

/// How many Dio calls ([`dio_call`]) the callable `body` holds, closures
/// included; cached per body.
fn dio_calls_in(body: TsNode, src: &[u8], acc: &mut Acc) -> usize {
    if let Some(n) = acc.dio_calls.get(&body.id()) {
        return *n;
    }
    let mut count = 0;
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        count += usize::from(dio_call(n, src, &acc.dio_receivers).is_some());
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
    }
    acc.dio_calls.insert(body.id(), count);
    count
}

/// The local a Dio call's result is bound to: `final res = await
/// dio.get(..)` (typed or not).
fn result_var(call: TsNode, src: &[u8]) -> Option<String> {
    let mut at = call;
    while let Some(p) = at.parent()
        && matches!(p.kind(), "unary_expression" | "await_expression")
    {
        at = p;
    }
    let def = at.parent().filter(|p| p.kind() == "initialized_variable_definition")?;
    let mut c = def.walk();
    let value: Vec<TsNode> = def
        .children_by_field_name("value", &mut c)
        .filter(|k| k.kind() != "comment")
        .collect();
    let [only] = value.as_slice() else {
        return None;
    };
    (only.id() == at.id())
        .then(|| def.child_by_field_name("name").map(|n| text_of(n, src).to_string()))
        .flatten()
}

/// DT.1l: the response the callable `body` reads off the Dio call `call`
/// (section head); `None` when it reads no key and no model.
fn dio_response_shape(body: TsNode, call: TsNode, src: &[u8]) -> Option<Shape> {
    let var = result_var(call, src);
    let mut flow = JsonFlow::new(src, FlowRoot::Dio { var, call });
    flow.walk(body);
    ReadTree::build(&flow.reads, &flow.key_at).shape(0)
}

/// DT.1l: the request a Dio call's `data:` argument sends (section head).
fn dio_request_shape(args: TsNode, src: &[u8]) -> Option<Shape> {
    let data = named_kids(args)
        .into_iter()
        .filter(|a| a.kind() == "argument")
        .filter_map(|a| a.named_child(0).filter(|n| n.kind() == "named_argument"))
        .find(|na| {
            na.named_child(0)
                .is_some_and(|l| l.kind() == "label" && text_of(l, src).trim_end_matches(':').trim() == "data")
        })?;
    let value: Vec<TsNode> = named_kids(data).into_iter().skip(1).collect();
    match value.as_slice() {
        [lit] if lit.kind() == "set_or_map_literal" => map_literal_shape(*lit, src, 2),
        parts => to_json_type(parts, data, src).map(|t| Shape::named(&t)),
    }
}

/// `x.toJson()`: the declared type of `x` ([`declared_type`]).
fn to_json_type(parts: &[TsNode], at: TsNode, src: &[u8]) -> Option<String> {
    let [recv, m, call] = parts else {
        return None;
    };
    let empty_call = matches!(selector_part(*call, src), SelectorPart::Call)
        && first_descendant_of_kind(*call, "argument").is_none();
    (recv.kind() == "identifier" && matches!(selector_part(*m, src), SelectorPart::Member("toJson")) && empty_call)
        .then(|| declared_type(text_of(*recv, src), at, src))
        .flatten()
}

/// A map literal as an Object of its string keys; `None` for a set literal.
/// A collection-`if` key is not required; a spread, a `for` or a computed key
/// leaves it open. `depth` bounds nested literals.
fn map_literal_shape(lit: TsNode, src: &[u8], depth: u8) -> Option<Shape> {
    fn entries(node: TsNode, required: bool, src: &[u8], depth: u8, fields: &mut Vec<ShapeField>, open: &mut bool) -> bool {
        match node.kind() {
            "pair" => {
                let key = node.child_by_field_name("key").and_then(|k| literal_text(k, src));
                let Some(key) = key else {
                    *open = true;
                    return true;
                };
                let mut c = node.walk();
                let value: Vec<TsNode> = node
                    .children_by_field_name("value", &mut c)
                    .filter(|k| k.kind() != "comment")
                    .collect();
                let shape = literal_value_shape(&value, node, src, depth);
                match fields.iter_mut().find(|f| f.name == key) {
                    Some(f) => f.required &= required,
                    None => fields.push(ShapeField::new(&key, required, shape)),
                }
                true
            }
            "if_element" => {
                for branch in ["consequence", "alternative"] {
                    let mut c = node.walk();
                    for b in node.children_by_field_name(branch, &mut c) {
                        if !entries(b, false, src, depth, fields, open) {
                            return false;
                        }
                    }
                }
                true
            }
            "spread_element" | "for_element" => {
                *open = true;
                true
            }
            _ => false,
        }
    }
    let mut fields = Vec::new();
    let mut open = false;
    for e in named_kids(lit) {
        if !entries(e, true, src, depth, &mut fields, &mut open) {
            return None;
        }
    }
    Some(Shape::Object { fields, open })
}

/// One map-literal value as a shape: a literal by its Dart type, a nested map
/// by its keys, a name (or its `.toJson()`) by its declared type, `x as T` by
/// `T`, anything else open.
fn literal_value_shape(value: &[TsNode], at: TsNode, src: &[u8], depth: u8) -> Shape {
    let named = |t: &str| Shape::named(t);
    match value {
        [v] => match v.kind() {
            "string_literal" => named("String"),
            "decimal_integer_literal" | "hex_integer_literal" => named("int"),
            "decimal_floating_point_literal" => named("double"),
            "true" | "false" => named("bool"),
            "null_literal" => named("Null"),
            "list_literal" => named("List"),
            "set_or_map_literal" if depth > 0 => {
                map_literal_shape(*v, src, depth - 1).unwrap_or_else(|| Shape::open(text_of(*v, src)))
            }
            "identifier" => {
                declared_type(text_of(*v, src), at, src).map_or_else(|| Shape::open(text_of(*v, src)), |t| named(&t))
            }
            "type_cast_expression" => named_kids(*v)
                .last()
                .filter(|c| c.kind() == "type_cast")
                .and_then(|c| cast_type(*c, src))
                .map_or_else(|| Shape::open(text_of(*v, src)), |t| named(&t)),
            _ => Shape::open(&collapse_ws(text_of(*v, src))),
        },
        parts => match to_json_type(parts, at, src) {
            Some(t) => named(&t),
            None => Shape::open(&collapse_ws(
                &parts.iter().map(|p| text_of(*p, src)).collect::<Vec<_>>().join(""),
            )),
        },
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The type `name` is declared with where `at` reads it: a local declared
/// before `at` in an enclosing block, a parameter of an enclosing function or
/// closure, then a field of the enclosing class.
fn declared_type(name: &str, at: TsNode, src: &[u8]) -> Option<String> {
    let mut cur = at;
    while let Some(p) = cur.parent() {
        match p.kind() {
            "block" => {
                for stmt in named_kids(p) {
                    if stmt.start_byte() >= at.start_byte() {
                        break;
                    }
                    if stmt.kind() != "local_variable_declaration" {
                        continue;
                    }
                    for def in named_kids(stmt) {
                        let Some(n) = def.child_by_field_name("name") else {
                            continue;
                        };
                        if def.kind() == "initialized_variable_definition" && text_of(n, src) == name {
                            return type_text(def, n.id(), src);
                        }
                    }
                }
            }
            "function_body" => {
                let list = p
                    .prev_named_sibling()
                    .and_then(|sig| first_descendant_of_kind(sig, "formal_parameter_list"));
                if let Some(t) = list.and_then(|l| param_type(l, name, src)) {
                    return Some(t);
                }
            }
            "function_expression" => {
                if let Some(t) = p.child_by_field_name("parameters").and_then(|l| param_type(l, name, src)) {
                    return Some(t);
                }
            }
            "class_body" => {
                return class_fields(p, src)
                    .into_iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, t)| t);
            }
            _ => {}
        }
        cur = p;
    }
    None
}

/// The declared type of the parameter `name` in a `formal_parameter_list`.
fn param_type(list: TsNode, name: &str, src: &[u8]) -> Option<String> {
    for p in named_kids(list) {
        match p.kind() {
            "optional_formal_parameters" => {
                if let Some(t) = param_type(p, name, src) {
                    return Some(t);
                }
            }
            "formal_parameter" => {
                if let Some(n) = p.child_by_field_name("name")
                    && text_of(n, src) == name
                {
                    return type_text(p, n.id(), src);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    #[test]
    fn class_and_enum() {
        let source = r#"
class User {
  String name;
  void greet() {
    print('Hello $name');
  }
}

enum Status { active, inactive }
"#;
        let fp = parse_file(source, "lib/user.dart", "lib::user", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::CLASS).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::ENUM).count(), 1);
    }

    #[test]
    fn imports() {
        let source = r#"
import 'package:flutter/material.dart';
import 'dart:async';
"#;
        let fp = parse_file(source, "lib/main.dart", "lib::main", repo()).unwrap();
        assert_eq!(fp.imports.len(), 2);
    }

    #[test]
    fn top_level_function() {
        let source = r#"
void main() {
  runApp(MyApp());
}
"#;
        let fp = parse_file(source, "lib/main.dart", "lib::main", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 1);
    }

    /// A6.5: the Bare heritage refs `class_id` carries, as (name, category),
    /// sorted. Every one must come from the file's MODULE.
    fn heritage_refs(fp: &FileParse, class_id: NodeId, module_id: NodeId) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = fp
            .refs
            .iter()
            .filter(|r| r.from == class_id)
            .map(|r| {
                assert_eq!(r.from_module, module_id, "heritage ref must come from the file MODULE");
                let CallQualifier::Bare(name) = &r.qualifier else {
                    panic!("heritage ref must be Bare, got {:?}", r.qualifier);
                };
                (name.clone(), r.category.0)
            })
            .collect();
        out.sort();
        out
    }

    fn is_heritage_edge(e: &Edge) -> bool {
        e.category == edge_category::INHERITS_FROM || e.category == edge_category::IMPLEMENTS
    }

    #[test]
    fn heritage_implements_and_extends() {
        let source = r#"
class IFoo {}
class Base {}
class Mix {}
class X extends Base with Mix implements IFoo {
  void run() {}
}
"#;
        let fp = parse_file(source, "lib/x.dart", "lib::x", repo()).unwrap();
        let x_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::x::X");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::x");
        // extends → INHERITS_FROM; with / implements → IMPLEMENTS. All refs the
        // graph crate binds, none a parser-minted edge.
        assert_eq!(
            heritage_refs(&fp, x_id, module_id),
            vec![
                ("Base".to_string(), edge_category::INHERITS_FROM.0),
                ("IFoo".to_string(), edge_category::IMPLEMENTS.0),
                ("Mix".to_string(), edge_category::IMPLEMENTS.0),
            ]
        );
        assert!(!fp.edges.iter().any(is_heritage_edge));
    }

    #[test]
    fn implements_edge_only() {
        let source = "class IFoo {}\nclass X implements IFoo {}\n";
        let fp = parse_file(source, "lib/x.dart", "lib::x", repo()).unwrap();
        let x_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::x::X");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::x");
        assert_eq!(
            heritage_refs(&fp, x_id, module_id),
            vec![("IFoo".to_string(), edge_category::IMPLEMENTS.0)]
        );
        assert!(!fp.edges.iter().any(is_heritage_edge));
    }

    /// A6.5: a single-file `class Dog extends Animal` names a ref the graph
    /// crate binds through the file's own symbols. `build` fills
    /// `module_symbols[module]` from the MODULE's nav children by simple name,
    /// so the ref's `from_module` must own a CLASS child of exactly that name:
    /// the node every Dart class is keyed by (`<module>::Animal`), not the bare
    /// `Animal` id the parser minted before.
    #[test]
    fn same_file_heritage_binds() {
        let source = "class Animal {}\nclass Dog extends Animal {}\n";
        let fp = parse_file(source, "lib/pets.dart", "lib::pets", repo()).unwrap();
        let dog = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::pets::Dog");
        let animal = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::pets::Animal");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::pets");
        let r = fp
            .refs
            .iter()
            .find(|r| r.from == dog && r.category == edge_category::INHERITS_FROM)
            .expect("Dog's extends ref");
        let CallQualifier::Bare(name) = &r.qualifier else {
            panic!("extends ref must be Bare, got {:?}", r.qualifier);
        };
        let bound: Vec<NodeId> = fp
            .nav
            .children_of
            .get(&r.from_module)
            .into_iter()
            .flatten()
            .copied()
            .filter(|c| fp.nav.name_by_id.get(c) == Some(name))
            .collect();
        assert_eq!(r.from_module, module_id);
        assert_eq!(bound, vec![animal]);
        assert_eq!(fp.nav.kind_by_id.get(&animal), Some(&node_kind::CLASS));
    }

    /// A6.5: a generic or library-prefixed type reduces to its simple name, so
    /// `extends p.Base<T>` binds like `extends Base`, in every clause.
    #[test]
    fn heritage_ref_strips_generics_and_prefix() {
        let source = "import 'm.dart' as p;\n\
                      class X extends p.Base<int> with p.Mix implements Comparable<X>, p.I {}\n";
        let fp = parse_file(source, "lib/x.dart", "lib::x", repo()).unwrap();
        let x_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::x::X");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::x");
        // The grammar splits `p.Base` into two sibling identifiers; the prefix
        // `p` is never a ref of its own.
        assert_eq!(
            heritage_refs(&fp, x_id, module_id),
            vec![
                ("Base".to_string(), edge_category::INHERITS_FROM.0),
                ("Comparable".to_string(), edge_category::IMPLEMENTS.0),
                ("I".to_string(), edge_category::IMPLEMENTS.0),
                ("Mix".to_string(), edge_category::IMPLEMENTS.0),
            ]
        );
    }

    #[test]
    fn library_const_with_doc_emits_state_var() {
        let source = "/// Fee.\nconst feeBps = 250;\n";
        let fp = parse_file(source, "lib/cfg.dart", "lib::cfg", repo()).unwrap();
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "lib::cfg::feeBps");
        // STATE_VAR node emitted (documented primitive survives the noise gate).
        let node = fp.nodes.iter().find(|n| n.id == id).expect("feeBps STATE_VAR node");
        assert_eq!(
            *fp.nav.kind_by_id.get(&id).unwrap(),
            node_kind::STATE_VAR
        );
        // Doc cell carried through.
        assert!(node.cells.iter().any(|c| c.kind == cell_type::DOC));
        // DEFINES edge module→const.
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::cfg");
        assert!(fp.edges.iter().any(|e| e.from == module_id
            && e.to == id
            && e.category == edge_category::DEFINES));
    }

    #[test]
    fn undocumented_primitive_const_is_gated() {
        let source = "const k = 1;\n";
        let fp = parse_file(source, "lib/cfg.dart", "lib::cfg", repo()).unwrap();
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "lib::cfg::k");
        assert!(!fp.nodes.iter().any(|n| n.id == id));
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
    fn go_router_routes_emit() {
        let source = r#"
final router = GoRouter(routes: [
  GoRoute(path: '/users', builder: (c, s) => UsersScreen()),
  GoRoute(path: '/users/:id', builder: (c, s) => UserDetail()),
]);
"#;
        let fp = parse_file(source, "lib/router.dart", "lib::router", repo()).unwrap();
        // LB.4c: go_router pages are `page:<path>`, display name the bare path,
        // never the `ANY <path>` shape a server route on the same path takes.
        for path in ["/users", "/users/:id"] {
            let id = page_id(path);
            let node = fp.nodes.iter().find(|n| n.id == id).expect("page node");
            assert_eq!(
                fp.nav.qname_by_id.get(&id).map(String::as_str),
                Some(format!("page:{path}").as_str())
            );
            assert_eq!(fp.nav.name_by_id.get(&id).map(String::as_str), Some(path));
            assert_eq!(fp.nav.kind_by_id.get(&id), Some(&node_kind::ROUTE));
            // ROUTE_METHOD and the A3.4 ORIGIN mark are unchanged.
            assert!(node.cells.iter().any(|c| c.kind == cell_type::ROUTE_METHOD
                && matches!(&c.payload, CellPayload::Text(m) if m == "ANY")));
            assert!(node.cells.iter().any(|c| c.kind == cell_type::ORIGIN
                && matches!(&c.payload, CellPayload::Json(j) if j.contains("\"provenance\":\"nav_route\""))));
            assert!(!fp.nodes.iter().any(|n| n.id == route_id("ANY", path)));
        }
    }

    fn page_id(path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            &format!("page:{path}"),
        )
    }

    #[test]
    fn go_router_page_and_shelf_route_on_one_path_are_two_nodes() {
        // LB.4c: the page and the server route serving `/users` in one file
        // keep distinct identities; only the server route keeps `<METHOD> <path>`
        // and only the page carries the nav_route ORIGIN mark.
        let source = r#"
final router = GoRouter(routes: [
  GoRoute(path: '/users', builder: (c, s) => UsersScreen()),
]);
final app = Router()
  ..get('/users', handleList);
"#;
        let fp = parse_file(source, "lib/app.dart", "lib::app", repo()).unwrap();
        let page = page_id("/users");
        let server = route_id("GET", "/users");
        assert_ne!(page, server);
        let page_node = fp.nodes.iter().find(|n| n.id == page).expect("page node");
        let server_node = fp
            .nodes
            .iter()
            .find(|n| n.id == server)
            .expect("server route");
        assert!(page_node.cells.iter().any(|c| c.kind == cell_type::ORIGIN));
        assert!(
            !server_node
                .cells
                .iter()
                .any(|c| c.kind == cell_type::ORIGIN)
        );
        assert_eq!(
            fp.nav.name_by_id.get(&server).map(String::as_str),
            Some("GET /users")
        );
        assert_eq!(
            fp.nav.qname_by_id.get(&server).map(String::as_str),
            Some("GET /users")
        );
    }

    #[test]
    fn shelf_routes_emit() {
        let source = r#"
import 'package:shelf_router/shelf_router.dart';

final app = Router()
  ..get('/users', handleList)
  ..post('/users', handleCreate);
"#;
        let fp = parse_file(source, "bin/server.dart", "bin::server", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
    }

    #[test]
    fn dio_client_call_emits_endpoint_not_route() {
        // Pattern A: `dio.get('/users/$id')` / `dio.post('/users')` in a class
        // method → ENDPOINT nodes (not phantom ROUTEs), with a CALLS edge from
        // the enclosing method. Path interpolation → `${…}`.
        let source = r#"class ApiClient {
  final Dio dio;
  Future<void> fetchUser(String id) async {
    final res = await dio.get('/users/$id');
    await dio.post('/users', data: body);
  }
}
"#;
        let fp = parse_file(source, "lib/api.dart", "lib::api", repo()).unwrap();
        let ep_get = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/users/${…}");
        let ep_post = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:POST:/users");
        assert!(
            fp.nodes.iter().any(|n| n.id == ep_get),
            "expected GET /users/${{…}} ENDPOINT node"
        );
        assert!(
            fp.nodes.iter().any(|n| n.id == ep_post),
            "expected POST /users ENDPOINT node"
        );
        // Must NOT emit phantom ROUTE nodes for the client calls.
        assert!(
            !fp.nodes.iter().any(|n| n.id == route_id("GET", "/users/$id")),
            "client dio.get must not become a server ROUTE"
        );
        // CALLS edge from the enclosing method to each endpoint.
        assert!(
            fp.edges
                .iter()
                .any(|e| e.to == ep_get && e.category == edge_category::CALLS),
            "expected CALLS edge into the GET endpoint"
        );
    }

    #[test]
    fn shelf_router_still_emits_route_not_endpoint() {
        // Server routes must be unaffected by the client-endpoint change.
        let source = r#"
final app = Router()
  ..get('/things', handleList)
  ..post('/things', handleCreate);
"#;
        let fp = parse_file(source, "bin/server.dart", "bin::server", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/things")));
        assert!(fp.nodes.iter().any(|n| n.id == route_id("POST", "/things")));
    }

    fn endpoint_hit(fp: &FileParse, id: NodeId) -> Option<String> {
        fp.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .find(|c| c.kind == cell_type::ENDPOINT_HIT)
            .and_then(|c| match &c.payload {
                CellPayload::Json(j) => Some(j.clone()),
                _ => None,
            })
    }

    /// A3.3 — an absolute-URL dio call used to be DROPPED (the path did not
    /// start with `/`). It now normalises to its request path, keeps the
    /// original literal as `raw`, and gets its CALLS edge; an interpolated base
    /// survives for BaseFold, while a whole-URL variable is still rejected.
    #[test]
    fn absolute_url_dio_call_emits_endpoint() {
        let source = r#"import 'package:dio/dio.dart';
class ApiClient {
  final Dio dio = Dio();
  Future<void> createUser(Map body) async {
    await dio.post('https://api.example.com/users', data: body);
    await dio.get('$baseUrl/users/$id?expand=1');
    await dio.get('$url');
    await dio.delete('/users');
  }
}
"#;
        let fp = parse_file(source, "lib/api_client.dart", "lib::api_client", repo()).unwrap();
        let ep = |m: &str, p: &str| {
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, &format!("endpoint:{m}:{p}"))
        };

        let post = ep("POST", "/users");
        let hit = endpoint_hit(&fp, post).expect("POST /users ENDPOINT from an absolute URL");
        let v: serde_json::Value = serde_json::from_str(&hit).unwrap();
        assert_eq!(v["path"], "/users");
        assert_eq!(v["raw"], "https://api.example.com/users");
        assert_eq!(v["host"], "api.example.com");
        let method = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "lib::api_client::ApiClient::createUser",
        );
        assert!(
            fp.edges
                .iter()
                .any(|e| e.from == method && e.to == post && e.category == edge_category::CALLS),
            "expected CALLS createUser -> POST /users"
        );
        assert!(
            !fp.nodes
                .iter()
                .any(|n| n.id == ep("POST", "https://api.example.com/users")),
            "the full URL must not become the endpoint qname"
        );

        // Interpolated base + query: base kept, query dropped, raw recorded.
        let based = endpoint_hit(&fp, ep("GET", "${…}/users/${…}"))
            .expect("interpolated-base endpoint must survive for BaseFold");
        let v: serde_json::Value = serde_json::from_str(&based).unwrap();
        assert_eq!(v["raw"], "${…}/users/${…}?expand=1");

        // A whole-URL variable is not a request path.
        assert!(!fp.nodes.iter().any(|n| n.id == ep("GET", "${…}")));

        // An unchanged path carries no `raw`.
        let plain = endpoint_hit(&fp, ep("DELETE", "/users")).expect("DELETE /users");
        assert!(!plain.contains("\"raw\""), "unchanged path must not carry raw: {plain}");
        // A11.5: nor a `host`; the interpolated base has none either.
        assert!(
            !plain.contains("\"host\""),
            "relative path must not carry host: {plain}"
        );
        assert!(
            !based.contains("\"host\""),
            "interpolated base has no host: {based}"
        );

        // And no phantom server ROUTE for any of these client calls.
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("DELETE", "/users")));
    }

    /// A11.5 — the dio literal's authority lands on ENDPOINT_HIT as `host`,
    /// AFTER `raw` (where the engine fold used to append it, so the folded
    /// bytes do not move); an interpolated authority (`https://$host/x`)
    /// names no service, so it writes `raw` but no `host`.
    #[test]
    fn dio_endpoint_carries_the_url_authority_as_host() {
        let source = r#"import 'package:dio/dio.dart';
class ApiClient {
  final Dio dio = Dio();
  Future<void> run() async {
    await dio.get('http://svc:8080/users?page=2');
    await dio.get('https://$host/orders');
  }
}
"#;
        let fp = parse_file(source, "lib/api_client.dart", "lib::api_client", repo()).unwrap();
        let ep = |m: &str, p: &str| {
            NodeId::from_parts(
                GRAPH_TYPE,
                repo(),
                node_kind::ENDPOINT,
                &format!("endpoint:{m}:{p}"),
            )
        };
        let users = endpoint_hit(&fp, ep("GET", "/users")).expect("GET /users");
        assert!(
            users.ends_with(r#","raw":"http://svc:8080/users?page=2","host":"svc:8080"}"#),
            "{users}"
        );
        let orders = endpoint_hit(&fp, ep("GET", "/orders")).expect("GET /orders");
        assert!(
            orders.contains(r#""raw":"https://${…}/orders""#),
            "{orders}"
        );
        assert!(!orders.contains("\"host\""), "{orders}");
    }

    // ---- LA.23e: selector-chain call sites + field types --------------------

    fn method_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, qname)
    }

    /// The call qualifiers emitted from one method, sorted: `collect_calls_in`
    /// walks a body with a stack (last statement first), which is
    /// deterministic but not source order.
    fn calls_from(fp: &FileParse, qname: &str) -> Vec<CallQualifier> {
        let id = method_id(qname);
        let mut out: Vec<CallQualifier> = fp
            .calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| c.qualifier.clone())
            .collect();
        out.sort_by_key(|q| format!("{q:?}"));
        out
    }

    /// DT.5h: [`calls_from`] for a CLASS scope (a field initialiser's calls).
    fn calls_from_class(fp: &FileParse, qname: &str) -> Vec<CallQualifier> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, qname);
        let mut out: Vec<CallQualifier> = fp
            .calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| c.qualifier.clone())
            .collect();
        out.sort_by_key(|q| format!("{q:?}"));
        out
    }

    fn sorted(mut v: Vec<CallQualifier>) -> Vec<CallQualifier> {
        v.sort_by_key(|q| format!("{q:?}"));
        v
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    fn complex(receiver: &str, name: &str) -> CallQualifier {
        CallQualifier::ComplexReceiver {
            receiver: receiver.to_string(),
            name: name.to_string(),
        }
    }

    fn field_types(fp: &FileParse, class_qname: &str) -> std::collections::HashMap<String, String> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, class_qname);
        fp.nav.field_types.get(&id).cloned().unwrap_or_default()
    }

    /// The grammar shape the scanner is written against: tree-sitter-dart
    /// 0.1.0 has no call node; a call is a primary followed by sibling
    /// `selector`s, and `this` is an anonymous token.
    #[test]
    fn grammar_pins_the_sibling_selector_shape() {
        let source = "class A { void m() { repo.find(id); this.go(); } }\n";
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let src = source.as_bytes();
        let mut stmts = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "expression_statement" {
                stmts.push(n);
            }
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
        }
        stmts.sort_by_key(|n| n.start_byte());
        let kinds = |n: TsNode| -> Vec<(String, bool)> {
            let mut c = n.walk();
            n.children(&mut c)
                .map(|k| (k.kind().to_string(), k.is_named()))
                .collect()
        };
        assert_eq!(text_of(stmts[0], src), "repo.find(id);");
        assert_eq!(
            kinds(stmts[0]),
            vec![
                ("identifier".to_string(), true),
                ("selector".to_string(), true),
                ("selector".to_string(), true),
                (";".to_string(), false),
            ]
        );
        assert_eq!(kinds(stmts[1])[0], ("this".to_string(), false));
        assert!(!tree_sitter_dart::NODE_TYPES.contains("\"selector_expression\""));
    }

    #[test]
    fn selector_chains_emit_call_sites() {
        let source = r#"class A {
  void run() {
    x.m();
    this.m();
    this.f.m();
    f();
    a.b().c();
  }
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(
            calls_from(&fp, "lib::a::A::run"),
            sorted(vec![
                attr("x", "m"),
                CallQualifier::SelfMethod("m".to_string()),
                complex("this.f", "m"),
                CallQualifier::Bare("f".to_string()),
                attr("a", "b"),
                complex("a.b()", "c"),
            ])
        );
        // Within one chain the sites come out in call order.
        let id = method_id("lib::a::A::run");
        let chain: Vec<&CallQualifier> = fp
            .calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| &c.qualifier)
            .filter(|q| matches!(q, CallQualifier::Attribute { base, .. } if base == "a")
                || matches!(q, CallQualifier::ComplexReceiver { receiver, .. } if receiver == "a.b()"))
            .collect();
        assert_eq!(chain, vec![&attr("a", "b"), &complex("a.b()", "c")]);
    }

    #[test]
    fn selector_chains_in_other_positions_and_shapes() {
        let source = r#"class A {
  String get(int id) => repo.find(id);
  Future<void> run() async {
    final r = await _other.find(1);
    x?.m();
    y!.m();
    list[i].m();
    g(1);
    use(repo.find(2));
    f()();
    super.m();
    repo..find(3);
    (a).m();
    return this
        .repo
        .find(4);
  }
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(
            calls_from(&fp, "lib::a::A::get"),
            vec![attr("repo", "find")]
        );
        // `super.m()`, the cascade, `(a).m()` and the call on `f()`'s result
        // emit nothing. (A generic call `g<int>()` is not probed: this grammar
        // parses it as a relational expression, `g < int > ()`.)
        assert_eq!(
            calls_from(&fp, "lib::a::A::run"),
            sorted(vec![
                attr("_other", "find"),
                attr("x", "m"),
                complex("y!", "m"),
                complex("list[i]", "m"),
                CallQualifier::Bare("g".to_string()),
                CallQualifier::Bare("use".to_string()),
                attr("repo", "find"),
                CallQualifier::Bare("f".to_string()),
                complex("this.repo", "find"),
            ])
        );
    }

    /// LA.37b / CB.9: a constructor body is credited to the constructor's
    /// own METHOD and a getter body to the getter's - never lent to the
    /// member before it (LA.23e emitted no call site for either), and never
    /// to the class.
    #[test]
    fn body_is_credited_to_its_owner_not_the_previous_member() {
        let source = r#"class A {
  void first() {}
  A(this.repo) {
    repo.init();
  }
  String get name => repo.name();
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        let class = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "lib::a::A");
        assert_eq!(calls_from(&fp, "lib::a::A::A"), vec![attr("repo", "init")]);
        assert_eq!(calls_from(&fp, "lib::a::A::name"), vec![attr("repo", "name")]);
        assert_eq!(calls_from(&fp, "lib::a::A::first"), vec![]);
        assert!(!fp.calls.iter().any(|c| c.from == class), "{:?}", fp.calls);
        assert_eq!(fp.calls.len(), 2, "{:?}", fp.calls);
    }

    #[test]
    fn typed_and_late_and_nullable_fields_record_their_type() {
        let source = r#"class S {
  final UserRepo repo;
  late UserRepo lateRepo;
  late final Cache cache;
  UserRepo? maybe;
  AuthApi a, b;
  final p.Remote remote;
  final Box<UserRepo> box;
  Mailer mailer = Mailer();
  static final Clock clock = Clock();
  S(this.repo);
}
"#;
        let fp = parse_file(source, "lib/s.dart", "lib::s", repo()).unwrap();
        let f = field_types(&fp, "lib::s::S");
        let want = [
            ("repo", "UserRepo"),
            ("lateRepo", "UserRepo"),
            ("cache", "Cache"),
            ("maybe", "UserRepo"),
            ("a", "AuthApi"),
            ("b", "AuthApi"),
            ("remote", "Remote"),
            ("box", "Box"),
            ("mailer", "Mailer"),
            ("clock", "Clock"),
        ];
        for (field, ty) in want {
            assert_eq!(f.get(field).map(String::as_str), Some(ty), "{field}: {f:?}");
        }
        assert_eq!(f.len(), want.len(), "{f:?}");
    }

    #[test]
    fn untyped_fields_take_the_constructed_class() {
        let source = r#"class S {
  final _other = UserRepo();
  var named = UserRepo.named(1);
  final generic = Holder<int>();
  final prefixed = p.Remote();
  final konst = const Settings();
  final fresh = new Pool();
  final cascaded = Bus()..start();
  final _impl = _RepoImpl();
  static const k = Registry();
  final fromCall = makeRepo();
  final chained = Factory().build();
  final lower = repo.create();
  final literal = 3;
}
"#;
        let fp = parse_file(source, "lib/s.dart", "lib::s", repo()).unwrap();
        let f = field_types(&fp, "lib::s::S");
        let want = [
            ("_other", "UserRepo"),
            ("named", "UserRepo"),
            ("generic", "Holder"),
            ("prefixed", "Remote"),
            ("konst", "Settings"),
            ("fresh", "Pool"),
            ("cascaded", "Bus"),
            ("_impl", "_RepoImpl"),
            ("k", "Registry"),
        ];
        for (field, ty) in want {
            assert_eq!(f.get(field).map(String::as_str), Some(ty), "{field}: {f:?}");
        }
        for untyped in ["fromCall", "chained", "lower", "literal"] {
            assert!(!f.contains_key(untyped), "{untyped}: {f:?}");
        }
    }

    #[test]
    fn builtin_function_and_record_types_are_rejected() {
        let source = r#"class S {
  final int count;
  String name = 'x';
  dynamic d;
  Object o = UserRepo();
  final List<UserRepo> repos = [];
  final Future<UserRepo> pending;
  final void Function(int) cb;
  final (int, String) rec;
  final list = List<int>.filled(3, 0);
  final done = Future.value(1);
}
"#;
        let fp = parse_file(source, "lib/s.dart", "lib::s", repo()).unwrap();
        assert!(
            field_types(&fp, "lib::s::S").is_empty(),
            "{:?}",
            fp.nav.field_types
        );
    }

    /// The LA.23e fixture's own file: both fields typed, all three call shapes
    /// emitted from the three methods.
    #[test]
    fn field_dispatch_fixture_shape() {
        let source = r#"import 'user_repo.dart';

class UserService {
  final UserRepo repo;
  final _other = UserRepo();

  UserService(this.repo);

  String get(int id) {
    return repo.find(id);
  }

  String other(int id) {
    return _other.find(id);
  }

  String viaThis(int id) {
    return this.repo.find(id);
  }
}
"#;
        let fp = parse_file(source, "lib/user_service.dart", "lib::user_service", repo()).unwrap();
        let f = field_types(&fp, "lib::user_service::UserService");
        assert_eq!(f.get("repo").map(String::as_str), Some("UserRepo"));
        assert_eq!(f.get("_other").map(String::as_str), Some("UserRepo"));
        assert_eq!(f.len(), 2);
        let q = "lib::user_service::UserService";
        assert_eq!(
            calls_from(&fp, &format!("{q}::get")),
            vec![attr("repo", "find")]
        );
        assert_eq!(
            calls_from(&fp, &format!("{q}::other")),
            vec![attr("_other", "find")]
        );
        assert_eq!(
            calls_from(&fp, &format!("{q}::viaThis")),
            vec![complex("this.repo", "find")]
        );
        // DT.5h: the untyped field's initialiser `UserRepo()` is the class's
        // code: a Bare(UserRepo) call site from the CLASS.
        assert_eq!(calls_from_class(&fp, q), vec![bare("UserRepo")]);
        assert_eq!(fp.calls.len(), 4, "{:?}", fp.calls);
    }

    // ---- LA.34: Dart lexical scope for unqualified calls ---------------------

    fn self_m(name: &str) -> CallQualifier {
        CallQualifier::SelfMethod(name.to_string())
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.to_string())
    }

    /// The `dart-bare-calls` fixture's own file.
    const BARE_CALLS_FIXTURE: &str = r#"int add(int a, int b) => a - b;

class Calc {
  int add(int a, int b) => a + b;

  int total(List<int> xs) {
    var acc = 0;
    for (final x in xs) {
      acc = add(acc, x);
    }
    return acc;
  }

  int viaTop(int a) => helper(a);

  int shadow(int a) {
    final twice = (int v) => v * 2;
    return twice(a);
  }

  int twice(int a) => a + a;

  int param(int Function(int) add) => add(1);

  static int make() => 1;

  int useStatic() => make();
}

int helper(int x) => x + 1;
"#;

    fn bare_calls_fixture() -> FileParse {
        parse_file(BARE_CALLS_FIXTURE, "lib/calc.dart", "lib::calc", repo()).unwrap()
    }

    #[test]
    fn bare_member_call_is_self_method() {
        let fp = bare_calls_fixture();
        assert_eq!(calls_from(&fp, "lib::calc::Calc::total"), vec![self_m("add")]);
    }

    /// The top-level `add` shares the member's name: inside the class the
    /// member wins, so the call never reaches library scope as a Bare.
    #[test]
    fn member_wins_over_top_level_function() {
        let fp = bare_calls_fixture();
        assert!(
            !fp.calls.iter().any(|c| c.qualifier == bare("add")),
            "{:?}",
            fp.calls
        );
    }

    #[test]
    fn static_member_bare_call_is_self_method() {
        let fp = bare_calls_fixture();
        assert_eq!(calls_from(&fp, "lib::calc::Calc::useStatic"), vec![self_m("make")]);
    }

    #[test]
    fn bare_call_to_a_non_member_stays_bare() {
        let fp = bare_calls_fixture();
        assert_eq!(calls_from(&fp, "lib::calc::Calc::viaTop"), vec![bare("helper")]);
    }

    /// `final twice = (v) => ..; twice(a)`: the local closure shadows the
    /// member `twice`, so the call is neither a SelfMethod nor a Bare.
    #[test]
    fn local_closure_shadows_member() {
        let fp = bare_calls_fixture();
        assert_eq!(calls_from(&fp, "lib::calc::Calc::shadow"), vec![]);
    }

    /// `param(int Function(int) add) => add(1)`: the parameter shadows both
    /// the member `add` and the top-level `add`.
    #[test]
    fn parameter_shadows_member_and_top_level() {
        let fp = bare_calls_fixture();
        assert_eq!(calls_from(&fp, "lib::calc::Calc::param"), vec![]);
        // Every call site of the file: total, useStatic, viaTop.
        assert_eq!(fp.calls.len(), 3, "{:?}", fp.calls);
    }

    /// A bare `Calc(..)` / `Calc.named(..)` inside `Calc` constructs: the
    /// class name and a named constructor are not member names.
    #[test]
    fn constructor_call_stays_bare() {
        let source = r#"class Calc {
  Calc();
  Calc.named();
  Calc copy() => Calc();
  Calc other() => Calc.named();
}
"#;
        let fp = parse_file(source, "lib/c.dart", "lib::c", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::c::Calc::copy"), vec![bare("Calc")]);
        assert_eq!(calls_from(&fp, "lib::c::Calc::other"), vec![attr("Calc", "named")]);
    }

    /// `local_names` of every member of the first class in `source`, keyed by
    /// the member's name.
    fn member_locals(source: &str) -> std::collections::HashMap<String, HashSet<String>> {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let src = source.as_bytes();
        let class = tree.root_node().named_child(0).unwrap();
        let body = class.child_by_field_name("body").unwrap();
        let mut out = std::collections::HashMap::new();
        let mut c = body.walk();
        for member in body.named_children(&mut c) {
            let mut k = member.walk();
            let kids: Vec<TsNode> = member.named_children(&mut k).collect();
            let sig = kids.iter().copied().find(|n| n.kind() == "method_signature");
            let Some(fbody) = kids.iter().copied().find(|n| n.kind() == "function_body") else {
                continue;
            };
            let name = sig
                .and_then(|s| s.named_child(0))
                .and_then(|n| n.child_by_field_name("name"))
                .map(|n| text_of(n, src).to_string())
                .unwrap();
            out.insert(name, local_names(sig, fbody, src));
        }
        out
    }

    /// Every way a member binds a name: named and optional parameters, the
    /// old function-typed parameter form, a setter parameter, `var a, b`
    /// declarators, a local function, a for-in variable, catch parameters,
    /// typed and untyped pattern variables, and closure parameters (typed and
    /// untyped).
    #[test]
    fn local_names_covers_every_binding_form() {
        let source = r#"class A {
  void named({required int Function() pn}) { pn(); }
  void optional([int Function()? po]) { po(); }
  void oldStyle(int of(int inner)) { of(1); }
  set value(void Function() sv) { sv(); }
  void decls() {
    var d1 = f, d2 = f;
    d1();
  }
  void localFn() {
    int lf(int lp) => lp;
    lf(1);
  }
  void loops(List<void Function()> xs) {
    for (final fx in xs) { fx(); }
  }
  void caught() {
    try {} catch (ce, cs) { ce(); }
  }
  void patterns(r) {
    var (pa, pb) = r;
    final (void Function() pc, _) = r;
    for (final (pk, pv) in r) {}
    pa();
  }
  void closure() {
    run((cp) => cp());
    run((int ct) => ct);
  }
}
"#;
        let locals = member_locals(source);
        let want: &[(&str, &[&str])] = &[
            ("named", &["pn"]),
            ("optional", &["po"]),
            ("oldStyle", &["of"]),
            ("value", &["sv"]),
            ("decls", &["d1", "d2"]),
            ("localFn", &["lf", "lp"]),
            ("loops", &["xs", "fx"]),
            ("caught", &["ce", "cs"]),
            ("patterns", &["r", "pa", "pb", "pc", "pk", "pv"]),
            ("closure", &["cp", "ct"]),
        ];
        for (member, names) in want {
            let got = &locals[*member];
            let want: HashSet<String> = names.iter().map(|n| n.to_string()).collect();
            assert_eq!(got, &want, "{member}");
        }
    }

    /// Through `parse_file`: every locally bound name shadows the class member
    /// of the same name, so no call below is a SelfMethod; only `run`, which
    /// nothing binds and the class does not declare, stays Bare. The closure
    /// parameter `cp` suppresses the `cp()` after the closure too - the
    /// conservative per-body rule.
    #[test]
    fn every_local_binding_form_shadows_a_member() {
        let source = r#"class A {
  void named({required int Function() pn}) { pn(); }
  void optional([int Function()? po]) { po(); }
  void oldStyle(int of(int inner)) { of(1); }
  void decls() {
    var d1 = f, d2 = f;
    d1();
    d2();
  }
  void localFn() {
    int lf() => 1;
    lf();
  }
  void loops(List<void Function()> xs) {
    for (final fx in xs) { fx(); }
  }
  void caught() {
    try {} catch (ce, cs) { ce(); cs(); }
  }
  void patterns(r) {
    var (pa, pb) = r;
    pa();
    pb();
    final (void Function() pc, _) = r;
    pc();
  }
  void closure() {
    run((cp) => cp());
    cp();
  }
  void pn() {}
  void po() {}
  void of() {}
  void d1() {}
  void d2() {}
  void lf() {}
  void fx() {}
  void ce() {}
  void cs() {}
  void pa() {}
  void pb() {}
  void pc() {}
  void cp() {}
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        let calls: Vec<CallQualifier> = fp.calls.iter().map(|c| c.qualifier.clone()).collect();
        assert_eq!(calls, vec![bare("run")], "{:?}", fp.calls);
    }

    /// A name bound in the member shadows only inside that member: a sibling
    /// member calling the same name still reaches the class member.
    #[test]
    fn a_local_shadows_only_its_own_member() {
        let source = r#"class A {
  void a(void Function() go) { go(); }
  void b() { go(); }
  void go() {}
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::a::A::a"), vec![]);
        assert_eq!(calls_from(&fp, "lib::a::A::b"), vec![self_m("go")]);
    }

    /// Fields, getters, setters and abstract members are class members too:
    /// Dart binds `cb()` to the field `cb` (a function-typed field), never to
    /// a same-named top-level function.
    #[test]
    fn fields_getters_and_abstract_members_are_members() {
        let source = r#"void cb() {}
void builder() {}
void hook() {}
void later() {}
void ext() {}
abstract class A {
  final void Function() cb;
  static final Function builder = () {};
  void Function() get hook => () {};
  late void Function() later, other;
  void ext();
  A(this.cb);
  void run() {
    cb();
    builder();
    hook();
    later();
    ext();
  }
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(
            calls_from(&fp, "lib::a::A::run"),
            sorted(vec![
                self_m("cb"),
                self_m("builder"),
                self_m("hook"),
                self_m("later"),
                self_m("ext"),
            ])
        );
    }

    // ---- LA.37a: top-level functions own their sibling function_body ------

    fn function_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname)
    }

    /// The call qualifiers emitted from one top-level FUNCTION, sorted.
    fn fn_calls(fp: &FileParse, qname: &str) -> Vec<CallQualifier> {
        let id = function_id(qname);
        let mut out: Vec<CallQualifier> = fp
            .calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| c.qualifier.clone())
            .collect();
        out.sort_by_key(|q| format!("{q:?}"));
        out
    }

    fn cell_text(fp: &FileParse, id: NodeId, kind: glia_core::CellTypeId) -> String {
        fp.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .find(|c| c.kind == kind)
            .map(|c| match &c.payload {
                CellPayload::Text(t) | CellPayload::Json(t) => t.clone(),
                other => format!("{other:?}"),
            })
            .unwrap_or_default()
    }

    #[test]
    fn top_level_body_is_walked() {
        let fp = parse_file("void f() { g(); }\nvoid g() {}\n", "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fn_calls(&fp, "lib::m::f"), vec![bare("g")]);
        assert_eq!(fn_calls(&fp, "lib::m::g"), vec![]);
    }

    #[test]
    fn top_level_endpoint_has_the_function_as_caller() {
        let source = "Future<void> load() async {\n  await dio.get('/users');\n}\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        let ep = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/users");
        assert!(fp.nodes.iter().any(|n| n.id == ep), "{:?}", fp.nodes);
        assert!(
            fp.edges.iter().any(|e| e.from == function_id("lib::m::load")
                && e.to == ep
                && e.category == edge_category::CALLS),
            "{:?}",
            fp.edges
        );
    }

    #[test]
    fn top_level_getter_is_a_function_and_owns_its_body() {
        let fp = parse_file("String get banner => describe();\n", "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.get(&function_id("lib::m::banner")), Some(&node_kind::FUNCTION));
        assert_eq!(fn_calls(&fp, "lib::m::banner"), vec![bare("describe")]);
    }

    /// `void set x(int v)` parses as a `setter_signature` (an untyped
    /// `set x(..)` is a `function_signature` returning `set`); both are the
    /// FUNCTION `x` owning the body.
    #[test]
    fn void_setter_signature_is_a_function_and_owns_its_body() {
        let fp = parse_file("void set x(int v) { a(); }\n", "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.get(&function_id("lib::m::x")), Some(&node_kind::FUNCTION));
        assert_eq!(fn_calls(&fp, "lib::m::x"), vec![bare("a")]);
        let fp = parse_file("set y(int v) { b(); }\n", "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fn_calls(&fp, "lib::m::y"), vec![bare("b")]);
    }

    #[test]
    fn getter_setter_pair_is_one_function() {
        let source = "int get level => h();\nvoid set level(int v) => s(v);\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        let id = function_id("lib::m::level");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == id).count(), 1, "{:?}", fp.nodes);
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::m");
        assert_eq!(
            fp.edges
                .iter()
                .filter(|e| e.from == module && e.to == id && e.category == edge_category::DEFINES)
                .count(),
            1
        );
        assert_eq!(fn_calls(&fp, "lib::m::level"), vec![bare("h"), bare("s")]);
        // The pair's cells are the first declaration's (the getter's) span.
        assert_eq!(cell_text(&fp, id, cell_type::CODE), "int get level => h();");
    }

    #[test]
    fn comment_between_signature_and_body() {
        let source = "int commented() // the body follows a comment\n    => helper(3);\nint helper(int x) => x;\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fn_calls(&fp, "lib::m::commented"), vec![bare("helper")]);
        let pos = cell_text(&fp, function_id("lib::m::commented"), cell_type::POSITION);
        assert!(pos.contains(r#""start_line":0,"end_line":1"#), "{pos}");
    }

    #[test]
    fn external_function_has_no_body_and_does_not_steal_the_next() {
        let source = "external void n();\nint m() => h();\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fn_calls(&fp, "lib::m::n"), vec![]);
        assert_eq!(fn_calls(&fp, "lib::m::m"), vec![bare("h")]);
        // Both are FUNCTIONs; the external one keeps its signature-only CODE.
        assert_eq!(cell_text(&fp, function_id("lib::m::n"), cell_type::CODE), "void n()");
        // A trailing comment after the external signature changes nothing.
        let source = "external int e(); // trailing\nint after() => d();\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fn_calls(&fp, "lib::m::e"), vec![]);
        assert_eq!(fn_calls(&fp, "lib::m::after"), vec![bare("d")]);
    }

    /// LA.34's local scope applies to top-level bodies: a parameter or local
    /// named like a top-level function binds nothing.
    #[test]
    fn top_level_parameter_shadows_a_function() {
        let source = r#"int helper(int x) => x;
int apply(int Function(int) helper) => helper(2);
int local() {
  final helper = (int v) => v;
  return helper(3);
}
int named({required int Function() helper}) => helper();
"#;
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fn_calls(&fp, "lib::m::apply"), vec![]);
        assert_eq!(fn_calls(&fp, "lib::m::local"), vec![]);
        assert_eq!(fn_calls(&fp, "lib::m::named"), vec![]);
        assert!(fp.calls.is_empty(), "{:?}", fp.calls);
    }

    #[test]
    fn top_level_code_and_position_span_the_body() {
        let source = r#"/// Loads users.
Future<void> loadUsers() async {
  await client.get('/users');
  helper(1);
}
"#;
        let fp = parse_file(source, "lib/app.dart", "lib::app", repo()).unwrap();
        let id = function_id("lib::app::loadUsers");
        let code = cell_text(&fp, id, cell_type::CODE);
        assert!(code.starts_with("Future<void> loadUsers() async {"), "{code}");
        assert!(code.contains("client.get('/users')"), "{code}");
        assert!(code.ends_with('}'), "{code}");
        assert_eq!(
            cell_text(&fp, id, cell_type::POSITION),
            r#"{"file":"lib/app.dart","start_line":1,"end_line":4}"#
        );
        assert_eq!(cell_text(&fp, id, cell_type::DOC), "Loads users.");
    }

    /// The committed `matrix/dart/calls` probe: `add(acc, x)` in `total`.
    #[test]
    fn matrix_dart_calls_probe_shape() {
        let source = r#"class Calc {
  int add(int a, int b) => a + b;

  int total(List<int> xs) {
    var acc = 0;
    for (final x in xs) {
      acc = add(acc, x);
    }
    return acc;
  }
}
"#;
        let fp = parse_file(source, "calc.dart", "calc", repo()).unwrap();
        assert_eq!(calls_from(&fp, "calc::Calc::total"), vec![self_m("add")]);
    }

    // ---- LA.37b: member containers and member-body owners ----------------

    fn class_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, qname)
    }

    /// The nav parent recorded for `id`.
    fn parent(fp: &FileParse, id: NodeId) -> Option<NodeId> {
        fp.nav.parent_of.get(&id).copied()
    }

    fn defines(fp: &FileParse, from: NodeId, to: NodeId) -> usize {
        fp.edges
            .iter()
            .filter(|e| e.from == from && e.to == to && e.category == edge_category::DEFINES)
            .count()
    }

    /// The committed `dart-body-owners` fixture's file.
    const BODY_OWNERS_FIXTURE: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/dart-body-owners/lib/app.dart");

    fn body_owners() -> FileParse {
        parse_file(BODY_OWNERS_FIXTURE, "lib/app.dart", "lib::app", repo()).unwrap()
    }

    #[test]
    fn mixin_members_are_emitted() {
        let fp = body_owners();
        let greets = class_id("lib::app::Greets");
        assert_eq!(fp.nav.kind_by_id.get(&greets), Some(&node_kind::CLASS));
        for m in ["lib::app::Greets::greet", "lib::app::Greets::hello"] {
            assert_eq!(fp.nav.kind_by_id.get(&method_id(m)), Some(&node_kind::METHOD), "{m}");
            assert_eq!(parent(&fp, method_id(m)), Some(greets), "{m}");
        }
        assert_eq!(defines(&fp, greets, method_id("lib::app::Greets::greet")), 1);
        assert_eq!(calls_from(&fp, "lib::app::Greets::greet"), vec![self_m("hello")]);
    }

    #[test]
    fn named_extension_is_a_class() {
        let fp = body_owners();
        let shout = class_id("lib::app::Shout");
        assert_eq!(fp.nav.kind_by_id.get(&shout), Some(&node_kind::CLASS));
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::app");
        assert_eq!(defines(&fp, module, shout), 1);
        assert_eq!(parent(&fp, method_id("lib::app::Shout::shout")), Some(shout));
        assert_eq!(calls_from(&fp, "lib::app::Shout::shout"), vec![self_m("twice")]);
    }

    #[test]
    fn unnamed_extension_on_a_same_file_type_hangs_members_on_it() {
        let fp = body_owners();
        let api = class_id("lib::app::Api");
        let doubled = method_id("lib::app::Api::doubled");
        assert_eq!(fp.nav.kind_by_id.get(&doubled), Some(&node_kind::METHOD));
        assert_eq!(parent(&fp, doubled), Some(api));
        assert_eq!(defines(&fp, api, doubled), 1);
        // Declared before its on-type or after, the extension adds no second
        // Api node.
        let source = "extension on Api { int d() => compute(); }\nclass Api { int compute() => 1; }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        let api = class_id("lib::m::Api");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == api).count(), 1);
        assert_eq!(parent(&fp, method_id("lib::m::Api::d")), Some(api));
        assert_eq!(calls_from(&fp, "lib::m::Api::d"), vec![self_m("compute")]);
    }

    /// Inside an extension, library scope comes before the on-type's members
    /// (an implicit `this` reaches those last): a same-file top-level
    /// `compute` wins over the on-type's `compute`, and the extension's own
    /// members win over both.
    #[test]
    fn unnamed_extension_scope_puts_library_before_the_on_type() {
        let source = r#"int compute() => 2;
class Api { int compute() => 1; int area() => 3; }
extension on Api {
  int a() => compute();
  int b() => area();
  int c() => own();
  int own() => 4;
}
"#;
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::m::Api::a"), vec![bare("compute")]);
        assert_eq!(calls_from(&fp, "lib::m::Api::b"), vec![self_m("area")]);
        assert_eq!(calls_from(&fp, "lib::m::Api::c"), vec![self_m("own")]);
    }

    /// CB.17 (was LA.37b's unnamed_extension_on_a_foreign_type_is_skipped):
    /// an unnamed extension on a type this file does not declare is its own
    /// CLASS `<module>::extension<T>` owning its members - never a node for
    /// the on-type itself.
    #[test]
    fn unnamed_extension_on_a_foreign_type_is_a_container() {
        let fp = body_owners();
        for (kind, q) in [
            (node_kind::CLASS, "lib::app::String"),
            (node_kind::METHOD, "lib::app::String::whisper"),
        ] {
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, q);
            assert!(!fp.nodes.iter().any(|n| n.id == id), "{q}");
        }
        assert!(!fp.nav.name_by_id.values().any(|n| n == "String"));
        let container = class_id("lib::app::extension<String>");
        let whisper = method_id("lib::app::extension<String>::whisper");
        assert_eq!(fp.nav.kind_by_id.get(&container), Some(&node_kind::CLASS));
        assert_eq!(fp.nav.name_by_id.get(&container).map(String::as_str), Some("extension<String>"));
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::app");
        assert_eq!(defines(&fp, module, container), 1);
        assert_eq!(defines(&fp, container, whisper), 1);
        assert_eq!(parent(&fp, whisper), Some(container));
        // `toLowerCase` is the on-type's member, unknown here: library scope.
        assert_eq!(calls_from(&fp, "lib::app::extension<String>::whisper"), vec![bare("toLowerCase")]);
        // An import-prefixed on-type is never this file's, even when this
        // file declares a type of the same simple name; a type argument of a
        // foreign type does not make it this file's either.
        let source = "class Api {}\nextension on p.Api { void q() {} }\nextension on List<Api> { void r() {} }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert!(!fp.nodes.iter().any(|n| n.id == method_id("lib::m::Api::q")), "{:?}", fp.nav.qname_by_id);
        assert_eq!(parent(&fp, method_id("lib::m::extension<Api>::q")), Some(class_id("lib::m::extension<Api>")));
        assert_eq!(parent(&fp, method_id("lib::m::extension<List>::r")), Some(class_id("lib::m::extension<List>")));
        // A generic / nullable spelling of a same-file type still hangs on it.
        let source = "class Box<T> {}\nextension on Box<int>? { void s() {} }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(parent(&fp, method_id("lib::m::Box::s")), Some(class_id("lib::m::Box")));
    }

    #[test]
    fn extension_type_members() {
        let fp = body_owners();
        let meters = class_id("lib::app::Meters");
        assert_eq!(fp.nav.kind_by_id.get(&meters), Some(&node_kind::CLASS));
        assert_eq!(fp.nav.name_by_id.get(&meters).map(String::as_str), Some("Meters"));
        assert_eq!(parent(&fp, method_id("lib::app::Meters::plus")), Some(meters));
        assert_eq!(
            calls_from(&fp, "lib::app::Meters::twicePlus"),
            vec![self_m("plus"), self_m("plus")]
        );
        // The representation field is a member: `value()` binds to it, not
        // to a same-named top-level function.
        let source = "int value() => 0;\nextension type V(int Function() value) { int f() => value(); }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::m::V::f"), vec![self_m("value")]);
    }

    #[test]
    fn enum_members_hang_on_the_enum() {
        let fp = body_owners();
        let color = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENUM, "lib::app::Color");
        for m in ["lib::app::Color::label", "lib::app::Color::describe"] {
            assert_eq!(fp.nav.kind_by_id.get(&method_id(m)), Some(&node_kind::METHOD), "{m}");
            assert_eq!(parent(&fp, method_id(m)), Some(color), "{m}");
        }
        assert_eq!(defines(&fp, color, method_id("lib::app::Color::label")), 1);
        assert_eq!(calls_from(&fp, "lib::app::Color::label"), vec![self_m("describe")]);
        // Enum constants are members; a plain enum still has no METHOD.
        let source = "void red() {}\nenum C { red, blue; void f() => red(); }\nenum S { a, b }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::m::C::f"), vec![self_m("red")]);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 1);
    }

    #[test]
    fn getter_is_a_method_and_owns_its_body() {
        let fp = body_owners();
        let status = method_id("lib::app::Api::status");
        assert_eq!(fp.nav.kind_by_id.get(&status), Some(&node_kind::METHOD));
        assert_eq!(parent(&fp, status), Some(class_id("lib::app::Api")));
        let ep = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/status");
        assert!(
            fp.edges.iter().any(|e| e.from == status && e.to == ep && e.category == edge_category::CALLS),
            "{:?}",
            fp.edges
        );
        assert_eq!(calls_from(&fp, "lib::app::Api::area"), vec![self_m("compute"), self_m("store")]);
        assert_eq!(calls_from(&fp, "lib::app::Api::compute"), vec![]);
    }

    #[test]
    fn getter_and_setter_share_one_method() {
        let fp = body_owners();
        let area = method_id("lib::app::Api::area");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == area).count(), 1);
        assert_eq!(defines(&fp, class_id("lib::app::Api"), area), 1);
        // The pair's cells are the first declaration's (the getter's).
        assert_eq!(cell_text(&fp, area, cell_type::CODE), "int get area => compute();");
        // The setter parameter is a local of the setter body.
        let source = "class A {\n  set v(void Function() cb) => cb();\n  void cb() {}\n}\n";
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::a::A::v"), vec![]);
    }

    /// CB.9 (was LA.37b's constructor_body_is_credited_to_the_type): a
    /// constructor, factory and operator body is its own METHOD's, never the
    /// class's; their parameters are locals.
    #[test]
    fn constructor_bodies_are_their_own_methods() {
        let fp = body_owners();
        let api_ctor = method_id("lib::app::Api::Api");
        let warm = method_id("lib::app::Api::warm");
        let boot = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENDPOINT, "endpoint:GET:/boot");
        let calls_to_boot: Vec<NodeId> = fp
            .edges
            .iter()
            .filter(|e| e.to == boot && e.category == edge_category::CALLS)
            .map(|e| e.from)
            .collect();
        assert_eq!(calls_to_boot, vec![api_ctor]);
        assert_eq!(parent(&fp, api_ctor), Some(class_id("lib::app::Api")));
        assert!(!fp.edges.iter().any(|e| e.from == warm), "{:?}", fp.edges);
        // Factory and operator bodies too; their parameters are locals.
        let source = r#"void helper() {}
class Box {
  void first() {}
  Box(Function cb) { cb(); helper(); }
  factory Box.make(void Function() f) { f(); return Box(helper); }
  Box operator +(Box o) { helper(); return o; }
}
"#;
        let fp = parse_file(source, "lib/b.dart", "lib::b", repo()).unwrap();
        // `cb()` / `f()` call parameters and emit nothing; `Box(helper)`
        // passes `helper` and constructs the class, a call of its own.
        assert_eq!(calls_from(&fp, "lib::b::Box::Box"), vec![bare("helper")]);
        assert_eq!(calls_from(&fp, "lib::b::Box::make"), vec![bare("Box")]);
        assert_eq!(calls_from(&fp, "lib::b::Box::operator+"), vec![bare("helper")]);
        assert_eq!(calls_from(&fp, "lib::b::Box::first"), vec![]);
        assert!(!fp.calls.iter().any(|c| c.from == class_id("lib::b::Box")), "{:?}", fp.calls);
    }

    #[test]
    fn getter_body_is_never_credited_to_an_endpoint() {
        let fp = body_owners();
        let endpoints: HashSet<NodeId> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ENDPOINT)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(endpoints.len(), 2, "{:?}", fp.nav.qname_by_id);
        assert!(!fp.edges.iter().any(|e| endpoints.contains(&e.from)), "{:?}", fp.edges);
        assert!(!fp.calls.iter().any(|c| endpoints.contains(&c.from)), "{:?}", fp.calls);
    }

    /// Every member body in the fixture has an owner: the nine member nodes
    /// the key expects, the constructor `Api()` (CB.9), the unnamed
    /// extension on `String`'s `whisper` under its container (CB.17), and
    /// nothing credited to `acc.nodes.last()`.
    #[test]
    fn body_owners_fixture_members() {
        let fp = body_owners();
        let mut methods: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::METHOD)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).map(String::as_str))
            .collect();
        methods.sort();
        assert_eq!(
            methods,
            vec![
                "lib::app::Api::Api",
                "lib::app::Api::area",
                "lib::app::Api::compute",
                "lib::app::Api::doubled",
                "lib::app::Api::status",
                "lib::app::Api::store",
                "lib::app::Api::warm",
                "lib::app::Color::describe",
                "lib::app::Color::label",
                "lib::app::Greets::greet",
                "lib::app::Greets::hello",
                "lib::app::Meters::plus",
                "lib::app::Meters::twicePlus",
                "lib::app::Shout::shout",
                "lib::app::Shout::twice",
                "lib::app::extension<String>::whisper",
            ]
        );
    }

    // ---- CB.9: constructors, operators, bodiless members, enum constants --

    /// The committed `dart-members` fixture's file.
    const MEMBERS_FIXTURE: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/dart-members/lib/src/money.dart");

    fn members() -> FileParse {
        parse_file(MEMBERS_FIXTURE, "lib/src/money.dart", "lib::src::money", repo()).unwrap()
    }

    fn attribute_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, qname)
    }

    fn has_edge(fp: &FileParse, from: NodeId, to: NodeId, category: glia_core::EdgeCategoryId) -> bool {
        fp.edges.iter().any(|e| e.from == from && e.to == to && e.category == category)
    }

    /// The unnamed constructor is `<T>::<T>`, a named constructor or factory
    /// `C.name` is `<T>::name`; each a METHOD named for its last segment, once
    /// under its class.
    #[test]
    fn constructors_are_methods() {
        let fp = members();
        let money = class_id("lib::src::money::Money");
        for (q, name) in [
            ("lib::src::money::Money::Money", "Money"),
            ("lib::src::money::Money::zero", "zero"),
            ("lib::src::money::Money::parse", "parse"),
        ] {
            let id = method_id(q);
            assert_eq!(fp.nav.kind_by_id.get(&id), Some(&node_kind::METHOD), "{q}");
            assert_eq!(fp.nav.name_by_id.get(&id).map(String::as_str), Some(name), "{q}");
            assert_eq!(parent(&fp, id), Some(money), "{q}");
            assert_eq!(defines(&fp, money, id), 1, "{q}");
            assert_eq!(fp.nodes.iter().filter(|n| n.id == id).count(), 1, "{q}");
        }
        assert_eq!(calls_from(&fp, "lib::src::money::Money::Money"), vec![self_m("validate")]);
        // `Money(int.parse(s))` constructs the class; `int.parse` is a call too.
        assert_eq!(
            calls_from(&fp, "lib::src::money::Money::parse"),
            sorted(vec![bare("Money"), attr("int", "parse")])
        );
        // `C.new(..)` is the unnamed constructor; an external factory is a
        // constructor with no body.
        let source = "class C {\n  C.new();\n  external factory C.ext();\n}\n";
        let fp = parse_file(source, "lib/c.dart", "lib::c", repo()).unwrap();
        let mut methods: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::METHOD)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).map(String::as_str))
            .collect();
        methods.sort();
        assert_eq!(methods, vec!["lib::c::C::C", "lib::c::C::ext"]);
    }

    /// A constructor's initializer list, `super(..)` / `this(..)` arguments
    /// and body are all the constructor's; its parameters are locals there.
    #[test]
    fn initializer_list_calls_come_from_the_constructor() {
        let fp = members();
        assert_eq!(calls_from(&fp, "lib::src::money::Money::zero"), vec![bare("round2")]);
        let money = class_id("lib::src::money::Money");
        assert!(!fp.calls.iter().any(|c| c.from == money), "{:?}", fp.calls);
        let source = r#"class A extends B {
  final int x;
  A.s(int v) : x = f(v), super(g(v)) { h(); }
  A.p(int Function() k) : x = k();
  A.r() : this.s(mk());
  A.q(int v) : assert(ok(v)), x = v;
}
"#;
        let fp = parse_file(source, "lib/a.dart", "lib::a", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::a::A::s"), sorted(vec![bare("f"), bare("g"), bare("h")]));
        assert_eq!(calls_from(&fp, "lib::a::A::p"), vec![]);
        assert_eq!(calls_from(&fp, "lib::a::A::r"), vec![bare("mk")]);
        assert_eq!(calls_from(&fp, "lib::a::A::q"), vec![bare("ok")]);
        assert!(!fp.calls.iter().any(|c| c.from == class_id("lib::a::A")), "{:?}", fp.calls);
    }

    /// An operator is `<T>::operator<op>`, the token verbatim with no space.
    /// Unary and binary `-` are one METHOD whose calls stack.
    #[test]
    fn operators_are_methods() {
        let fp = members();
        let plus = method_id("lib::src::money::Money::operator+");
        assert_eq!(fp.nav.kind_by_id.get(&plus), Some(&node_kind::METHOD));
        assert_eq!(fp.nav.name_by_id.get(&plus).map(String::as_str), Some("operator+"));
        assert_eq!(
            calls_from(&fp, "lib::src::money::Money::operator+"),
            sorted(vec![bare("Money"), bare("round2")])
        );
        let source = r#"class V {
  V operator [](int i) => at(i);
  void operator []=(int i, V v) { put(i, v); }
  bool operator ==(Object o) => same(o);
  V operator ~/(V o) => div(o);
  V operator ~() => inv();
  V operator -() => neg();
  V operator -(V o) => sub(o);
}
"#;
        let fp = parse_file(source, "lib/v.dart", "lib::v", repo()).unwrap();
        let v = class_id("lib::v::V");
        for (op, calls) in [
            ("operator[]", vec![bare("at")]),
            ("operator[]=", vec![bare("put")]),
            ("operator==", vec![bare("same")]),
            ("operator~/", vec![bare("div")]),
            ("operator~", vec![bare("inv")]),
            ("operator-", sorted(vec![bare("neg"), bare("sub")])),
        ] {
            let q = format!("lib::v::V::{op}");
            assert_eq!(fp.nav.kind_by_id.get(&method_id(&q)), Some(&node_kind::METHOD), "{q}");
            assert_eq!(parent(&fp, method_id(&q)), Some(v), "{q}");
            assert_eq!(calls_from(&fp, &q), calls, "{q}");
        }
        let minus = method_id("lib::v::V::operator-");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == minus).count(), 1);
        assert_eq!(defines(&fp, v, minus), 1);
    }

    /// A bodiless member (abstract or `external`: method, getter, setter,
    /// operator) is a METHOD under its owner with no calls.
    #[test]
    fn abstract_members_are_methods() {
        let fp = members();
        let repo_class = class_id("lib::src::money::Repo");
        for q in ["lib::src::money::Repo::load", "lib::src::money::Repo::save"] {
            assert_eq!(fp.nav.kind_by_id.get(&method_id(q)), Some(&node_kind::METHOD), "{q}");
            assert_eq!(parent(&fp, method_id(q)), Some(repo_class), "{q}");
            assert_eq!(defines(&fp, repo_class, method_id(q)), 1, "{q}");
            assert_eq!(calls_from(&fp, q), vec![], "{q}");
        }
        assert_eq!(
            cell_text(&fp, method_id("lib::src::money::Repo::load"), cell_type::CODE),
            "Money load();"
        );
        let source = r#"abstract mixin class M {
  int get level;
  set level(int v);
  M operator +(M o);
  external void ext();
  static final int field = 1;
}
"#;
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        let mut methods: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::METHOD)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).map(String::as_str))
            .collect();
        methods.sort();
        // The getter + setter pair is one METHOD; the field is no METHOD.
        assert_eq!(methods, vec!["lib::m::M::ext", "lib::m::M::level", "lib::m::M::operator+"]);
        assert!(fp.calls.is_empty(), "{:?}", fp.calls);
    }

    /// `const` constructors and redirecting factories end in `;`: a METHOD
    /// with no body, spanning the member.
    #[test]
    fn const_constructor_without_body() {
        let source = r#"class Point {
  final int x, y;
  const Point(this.x, this.y);
  const Point.origin() : x = 0, y = 0;
  factory Point.polar(int r) = PolarPoint;
}
"#;
        let fp = parse_file(source, "lib/p.dart", "lib::p", repo()).unwrap();
        let point = class_id("lib::p::Point");
        for q in ["lib::p::Point::Point", "lib::p::Point::origin", "lib::p::Point::polar"] {
            assert_eq!(fp.nav.kind_by_id.get(&method_id(q)), Some(&node_kind::METHOD), "{q}");
            assert_eq!(defines(&fp, point, method_id(q)), 1, "{q}");
        }
        assert_eq!(
            cell_text(&fp, method_id("lib::p::Point::Point"), cell_type::CODE),
            "const Point(this.x, this.y);"
        );
        assert_eq!(
            cell_text(&fp, method_id("lib::p::Point::origin"), cell_type::POSITION),
            r#"{"file":"lib/p.dart","start_line":3,"end_line":3}"#
        );
        assert!(fp.calls.is_empty(), "{:?}", fp.calls);
        // The field list stays a field: field types still record, no METHOD.
        assert!(!fp.nav.qname_by_id.values().any(|q| q == "lib::p::Point::x"));
    }

    /// Each enum constant is an ATTRIBUTE `<Enum>::<constant>` under its ENUM
    /// (HAS_ATTRIBUTE, the Rust / TypeScript / Java enum-member edge - no
    /// DEFINES); an enhanced-enum constant's arguments are its calls.
    #[test]
    fn enum_constants_are_attributes() {
        let fp = members();
        let currency = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENUM, "lib::src::money::Currency");
        for (q, name) in [("lib::src::money::Currency::aud", "aud"), ("lib::src::money::Currency::usd", "usd")] {
            let id = attribute_id(q);
            assert_eq!(fp.nav.kind_by_id.get(&id), Some(&node_kind::ATTRIBUTE), "{q}");
            assert_eq!(fp.nav.name_by_id.get(&id).map(String::as_str), Some(name), "{q}");
            assert_eq!(parent(&fp, id), Some(currency), "{q}");
            assert!(has_edge(&fp, currency, id, edge_category::HAS_ATTRIBUTE), "{q}");
            assert!(!has_edge(&fp, currency, id, edge_category::DEFINES), "{q}");
            assert!(cell_text(&fp, id, cell_type::POSITION).contains(r#""start_line":20"#), "{q}");
        }
        let source = r#"int round2(int v) => v;
enum E {
  aud(round2(1)),
  usd.named(2);
  const E(this.v);
  const E.named(this.v);
  final int v;
}
"#;
        let fp = parse_file(source, "lib/e.dart", "lib::e", repo()).unwrap();
        let e = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ENUM, "lib::e::E");
        let aud = attribute_id("lib::e::E::aud");
        let aud_calls: Vec<&CallQualifier> =
            fp.calls.iter().filter(|c| c.from == aud).map(|c| &c.qualifier).collect();
        assert_eq!(aud_calls, vec![&bare("round2")]);
        assert!(has_edge(&fp, e, attribute_id("lib::e::E::usd"), edge_category::HAS_ATTRIBUTE));
        // The enum's constructors are METHODs; its field is not.
        for q in ["lib::e::E::E", "lib::e::E::named"] {
            assert_eq!(parent(&fp, method_id(q)), Some(e), "{q}");
        }
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::METHOD).count(), 2);
        assert_eq!(fp.calls.len(), 1, "{:?}", fp.calls);
    }

    /// `Money.zero()` / `Money.parse('1')` are Attribute calls on the class;
    /// the METHODs they name are the class's `zero` / `parse` children, the
    /// `class_methods` keys the graph binds them through (end-to-end in
    /// engine/tests/dart_body_owners.rs and the `dart-members` fixture).
    #[test]
    fn named_constructor_call_binds() {
        let fp = members();
        let total = function_id("lib::src::money::total");
        let mut calls: Vec<CallQualifier> =
            fp.calls.iter().filter(|c| c.from == total).map(|c| c.qualifier.clone()).collect();
        calls.sort_by_key(|q| format!("{q:?}"));
        assert_eq!(calls, sorted(vec![attr("Money", "zero"), attr("Money", "parse")]));
        let money = class_id("lib::src::money::Money");
        let children: Vec<&str> = fp.nav.children_of[&money]
            .iter()
            .filter(|c| fp.nav.kind_by_id.get(c) == Some(&node_kind::METHOD))
            .filter_map(|c| fp.nav.name_by_id.get(c).map(String::as_str))
            .collect();
        for name in ["zero", "parse"] {
            assert!(children.contains(&name), "{name}: {children:?}");
        }
    }

    /// CB.9 stopgap ([`record_type_receiver`]): an Attribute call on a type
    /// this file declares records the name as that type in the caller's
    /// scope, unless a parameter, local or member shadows it; a type from
    /// elsewhere or a plain receiver records nothing.
    #[test]
    fn same_file_type_member_calls_record_the_type() {
        let fp = members();
        let total = function_id("lib::src::money::total");
        assert_eq!(
            fp.nav.local_types.get(&total).and_then(|l| l.get("Money")).map(String::as_str),
            Some("Money")
        );
        let source = r#"class Money { static Money make() => Money(); }
enum Color { red; static Color pick() => red; }
Money a() => Money.make();
Color b() => Color.pick();
Money c(Money Money) => Money.make();
Other d() => Other.make();
Money e(Money m) => m.make();
class Wallet {
  int Money = 0;
  void f() { Money.make(); }
  void g() { Money2.make(); }
}
"#;
        let fp = parse_file(source, "lib/w.dart", "lib::w", repo()).unwrap();
        let typed = |scope: NodeId, name: &str| {
            fp.nav.local_types.get(&scope).and_then(|l| l.get(name)).cloned()
        };
        assert_eq!(typed(function_id("lib::w::a"), "Money").as_deref(), Some("Money"));
        assert_eq!(typed(function_id("lib::w::b"), "Color").as_deref(), Some("Color"));
        // A parameter named `Money` shadows the class.
        assert_eq!(typed(function_id("lib::w::c"), "Money"), None);
        // `Other` is declared elsewhere; `m` is a plain receiver.
        assert_eq!(typed(function_id("lib::w::d"), "Other"), None);
        assert_eq!(typed(function_id("lib::w::e"), "m"), None);
        // A member field named `Money` shadows the class inside `Wallet`.
        assert_eq!(typed(method_id("lib::w::Wallet::f"), "Money"), None);
        assert!(!fp.nav.local_types.contains_key(&method_id("lib::w::Wallet::g")));
    }

    // ---- CB.17: unnamed-extension containers, top-level initialisers, import combinators

    /// The committed `dart-extensions-imports` fixture's two files.
    const EXT_FIXTURE: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/dart-extensions-imports/lib/src/ext.dart");
    const MONEY_FIXTURE: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/dart-extensions-imports/lib/src/money.dart");

    fn state_var_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, qname)
    }

    /// The call qualifiers emitted from the node `id`, sorted.
    fn calls_of(fp: &FileParse, id: NodeId) -> Vec<CallQualifier> {
        let mut out: Vec<CallQualifier> =
            fp.calls.iter().filter(|c| c.from == id).map(|c| c.qualifier.clone()).collect();
        out.sort_by_key(|q| format!("{q:?}"));
        out
    }

    fn module(path: &str, alias: Option<&str>) -> ImportTarget {
        ImportTarget::Module {
            path: path.to_string(),
            alias: alias.map(str::to_string),
        }
    }

    fn symbol(module: &str, name: &str) -> ImportTarget {
        ImportTarget::Symbol {
            module: module.to_string(),
            name: name.to_string(),
            alias: None,
            level: 0,
        }
    }

    fn import_targets(source: &str) -> Vec<ImportTarget> {
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        fp.imports.into_iter().map(|i| i.target).collect()
    }

    /// D4: `extension on m.Money` (the fixture's ext.dart) is the CLASS
    /// `lib::src::ext::extension<Money>`, its member a METHOD under it whose
    /// body's calls are its own; the import prefix and the on-type's generic
    /// arguments / `?` never reach the name.
    #[test]
    fn unnamed_extension_on_imported_type_is_a_container() {
        let fp = parse_file(EXT_FIXTURE, "lib/src/ext.dart", "lib::src::ext", repo()).unwrap();
        let container = class_id("lib::src::ext::extension<Money>");
        let doubled = method_id("lib::src::ext::extension<Money>::doubled");
        assert_eq!(fp.nav.kind_by_id.get(&container), Some(&node_kind::CLASS));
        assert_eq!(fp.nav.kind_by_id.get(&doubled), Some(&node_kind::METHOD));
        assert_eq!(parent(&fp, doubled), Some(container));
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::src::ext");
        assert_eq!(defines(&fp, module, container), 1);
        assert_eq!(defines(&fp, container, doubled), 1);
        assert_eq!(
            calls_from(&fp, "lib::src::ext::extension<Money>::doubled"),
            vec![attr("m", "Money"), attr("m", "round2")]
        );
        // Never a node for the prefix or the on-type itself.
        for q in ["lib::src::ext::Money", "lib::src::ext::m", "lib::src::ext::m::Money"] {
            assert!(!fp.nodes.iter().any(|n| n.id == class_id(q)), "{q}");
        }
        assert_eq!(fp.nodes.iter().filter(|n| n.id == container).count(), 1);
        // Generic and nullable spellings, function and record on-types.
        let source = "extension on p.Api<int>? { void a() {} }\nextension on void Function(int) { void b() {} }\n\
                      extension on (int, String) { void c() {} }\nextension <T> on T { void d() {} }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        for (owner, m) in [
            ("lib::m::extension<Api>", "a"),
            ("lib::m::extension<Function>", "b"),
            ("lib::m::extension<Record>", "c"),
            ("lib::m::extension<T>", "d"),
        ] {
            assert_eq!(parent(&fp, method_id(&format!("{owner}::{m}"))), Some(class_id(owner)), "{owner}");
        }
    }

    /// D4: an unnamed extension on a core type is a container too; its own
    /// members are its member scope, the on-type's (`length`) are not known.
    /// Two unnamed extensions on one type in one file share the container.
    #[test]
    fn unnamed_extension_on_core_type() {
        let fp = parse_file(EXT_FIXTURE, "lib/src/ext.dart", "lib::src::ext", repo()).unwrap();
        let container = class_id("lib::src::ext::extension<String>");
        let to_cents = method_id("lib::src::ext::extension<String>::toCents");
        assert_eq!(fp.nav.name_by_id.get(&container).map(String::as_str), Some("extension<String>"));
        assert_eq!(parent(&fp, to_cents), Some(container));
        assert_eq!(calls_from(&fp, "lib::src::ext::extension<String>::toCents"), vec![attr("m", "round2")]);
        let source = "int twice() => 2;\nextension on String { int a() => twice(); int twice() => 1; }\n\
                      extension on String { int b() => a(); }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        let container = class_id("lib::m::extension<String>");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == container).count(), 1);
        assert_eq!(defines(&fp, NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::m"), container), 1);
        for m in ["a", "twice", "b"] {
            assert_eq!(parent(&fp, method_id(&format!("lib::m::extension<String>::{m}"))), Some(container), "{m}");
        }
        // The extension's own `twice` wins over the top-level one; each
        // extension's member scope is its own body's.
        assert_eq!(calls_from(&fp, "lib::m::extension<String>::a"), vec![self_m("twice")]);
        assert_eq!(calls_from(&fp, "lib::m::extension<String>::b"), vec![bare("a")]);
    }

    /// LA.37b control: an unnamed extension on a type this file declares
    /// still hangs its members on that type - no container.
    #[test]
    fn same_file_unnamed_extension_unchanged() {
        let source = "class Money { int cents = 0; int half() => cents ~/ 2; }\n\
                      extension on Money { int quarter() => half() ~/ 2; }\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        let money = class_id("lib::m::Money");
        assert_eq!(parent(&fp, method_id("lib::m::Money::quarter")), Some(money));
        assert_eq!(calls_from(&fp, "lib::m::Money::quarter"), vec![self_m("half")]);
        assert!(!fp.nav.qname_by_id.values().any(|q| q.contains("extension<")), "{:?}", fp.nav.qname_by_id);
        let fp = body_owners();
        assert_eq!(parent(&fp, method_id("lib::app::Api::doubled")), Some(class_id("lib::app::Api")));
        assert!(!fp.nav.qname_by_id.values().any(|q| q.contains("extension<Api>")));
    }

    /// D5: a `final` / `const` top-level initialiser's calls come FROM its
    /// STATE_VAR (G19 minted the node and never walked the value).
    #[test]
    fn final_initialiser_calls_from_its_state_var() {
        let fp = parse_file(MONEY_FIXTURE, "lib/src/money.dart", "lib::src::money", repo()).unwrap();
        let tax = state_var_id("lib::src::money::defaultTax");
        assert_eq!(fp.nav.kind_by_id.get(&tax), Some(&node_kind::STATE_VAR));
        assert_eq!(calls_of(&fp, tax), vec![bare("seed")]);
        let source = "const k = h();\nfinal client = Dio();\nfinal f = () => g();\nfinal r = a.b().c(d());\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(calls_of(&fp, state_var_id("lib::m::k")), vec![bare("h")]);
        assert_eq!(calls_of(&fp, state_var_id("lib::m::client")), vec![bare("Dio")]);
        // DT.5e: a closure's call is the initialiser's, as a member's
        // closure's call is the member's.
        assert_eq!(calls_of(&fp, state_var_id("lib::m::f")), vec![bare("g")]);
        assert_eq!(
            calls_of(&fp, state_var_id("lib::m::r")),
            sorted(vec![attr("a", "b"), complex("a.b()", "c"), bare("d")])
        );
        // The declared name is never a call; the noise gate still holds.
        assert!(!fp.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Bare(n) if n == "k" || n == "r")));
        let fp = parse_file("const k = 1;\n", "lib/m.dart", "lib::m", repo()).unwrap();
        assert!(fp.nodes.iter().all(|n| n.id != state_var_id("lib::m::k")));
    }

    /// D5: a top-level `var` list (and a typed / `late` one) is one STATE_VAR
    /// per declarator, its initialiser walked the same way; the doc above
    /// the declaration's first token is carried, and an undocumented
    /// primitive literal is gated as G19 gates it.
    #[test]
    fn var_list_entries_are_state_vars() {
        let fp = parse_file(MONEY_FIXTURE, "lib/src/money.dart", "lib::src::money", repo()).unwrap();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::src::money");
        for q in ["lib::src::money::cache", "lib::src::money::hits"] {
            let id = state_var_id(q);
            assert_eq!(fp.nav.kind_by_id.get(&id), Some(&node_kind::STATE_VAR), "{q}");
            assert_eq!(defines(&fp, module, id), 1, "{q}");
        }
        assert_eq!(calls_of(&fp, state_var_id("lib::src::money::hits")), vec![bare("round2")]);
        assert_eq!(calls_of(&fp, state_var_id("lib::src::money::cache")), vec![]);
        assert_eq!(
            cell_text(&fp, state_var_id("lib::src::money::hits"), cell_type::POSITION),
            r#"{"file":"lib/src/money.dart","start_line":4,"end_line":4}"#
        );
        let source = "late final String z = f();\nint counter;\nString a = 'x', b = g();\n\
                      /// Documented.\nvar documented = 3;\nvar plain = 4;\n";
        let fp = parse_file(source, "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(calls_of(&fp, state_var_id("lib::m::z")), vec![bare("f")]);
        assert_eq!(calls_of(&fp, state_var_id("lib::m::b")), vec![bare("g")]);
        assert!(fp.nodes.iter().any(|n| n.id == state_var_id("lib::m::counter")));
        assert_eq!(cell_text(&fp, state_var_id("lib::m::documented"), cell_type::DOC), "Documented.");
        for gated in ["lib::m::a", "lib::m::plain"] {
            assert!(fp.nodes.iter().all(|n| n.id != state_var_id(gated)), "{gated}");
        }
        // The doc of one declaration never reaches the next.
        assert_eq!(cell_text(&fp, state_var_id("lib::m::counter"), cell_type::DOC), "");
    }

    /// D6: `as` binds the prefix (whatever it shows), `show` without a prefix
    /// binds each shown name, `hide` binds nothing extra; the uri is the
    /// string without quotes; `export` is not an import.
    #[test]
    fn import_as_show_hide() {
        assert_eq!(import_targets("import 'a.dart' as m show X;\n"), vec![module("a.dart", Some("m"))]);
        assert_eq!(
            import_targets("import 'a.dart' show X, y;\n"),
            vec![symbol("a.dart", "X"), symbol("a.dart", "y")]
        );
        assert_eq!(import_targets("import 'a.dart' hide Z;\n"), vec![module("a.dart", None)]);
        assert_eq!(import_targets("import 'b.dart' deferred as d;\n"), vec![module("b.dart", Some("d"))]);
        // `show A, B hide B` / `show A, B show B` narrow the shown names.
        assert_eq!(import_targets("import 'a.dart' show A, B hide B;\n"), vec![symbol("a.dart", "A")]);
        assert_eq!(import_targets("import 'a.dart' show A, B show B;\n"), vec![symbol("a.dart", "B")]);
        // A conditional import is its default uri.
        assert_eq!(
            import_targets("import 'c.dart' if (dart.library.io) 'c_io.dart' as c;\n"),
            vec![module("c.dart", Some("c"))]
        );
        assert_eq!(import_targets("export 'e.dart' show E;\n"), vec![]);
        // The fixture's `import 'money.dart' as m show Money, round2;`.
        let fp = parse_file(EXT_FIXTURE, "lib/src/ext.dart", "lib::src::ext", repo()).unwrap();
        assert_eq!(fp.imports.len(), 1);
        assert_eq!(fp.imports[0].target, module("money.dart", Some("m")));
        assert_eq!(fp.imports[0].from_module, "lib::src::ext");
        assert_eq!(fp.imports[0].line, 0);
    }

    /// D6 control: a plain import keeps today's shape, Module { uri, None },
    /// single- or double-quoted.
    #[test]
    fn plain_import_unchanged() {
        assert_eq!(
            import_targets("import 'package:flutter/material.dart';\nimport \"dart:async\";\nimport 'models.dart';\n"),
            vec![
                module("package:flutter/material.dart", None),
                module("dart:async", None),
                module("models.dart", None),
            ]
        );
        let fp = parse_file("\n\nimport 'models.dart';\n", "lib/m.dart", "lib::m", repo()).unwrap();
        assert_eq!(fp.imports[0].line, 2);
    }

    /// CH.5a: the ENDPOINT node id of `<method> <path>`.
    fn endpoint_id(method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::ENDPOINT,
            &format!("endpoint:{method}:{path}"),
        )
    }

    /// CH.5a: the facts recorded on MODULE `module_qname`, in record order.
    fn module_facts(fp: &FileParse, module_qname: &str) -> Vec<NavFact> {
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module_qname);
        fp.nav.nav_facts.get(&module).cloned().unwrap_or_default()
    }

    /// CH.5a: a generic client call `dio.get<T>('/x')` is an ENDPOINT with its
    /// CALLS edge in both shapes tree-sitter-dart gives it (the relational
    /// misparse of a one-argument call, the selector chain of a call with a
    /// named argument), and the plain call's payload is HEAD's.
    #[test]
    fn generic_dio_call_is_an_endpoint() {
        let source = include_str!(
            "../../../../bench/substrate-gap/fixtures/dart-dio-generic-calls/lib/friend_service.dart"
        );
        let fp = parse_file(
            source,
            "lib/friend_service.dart",
            "lib::friend_service",
            repo(),
        )
        .unwrap();
        for (method, path, caller) in [
            ("GET", "/protected/friends", "listAll"),
            ("GET", "/protected/user/profile", "profile"),
            ("POST", "/protected/swipe", "swipe"),
            ("POST", "/protected/friends/accept/${…}", "accept"),
        ] {
            let ep = endpoint_id(method, path);
            assert!(
                fp.nodes.iter().any(|n| n.id == ep),
                "expected ENDPOINT {method} {path}"
            );
            let from = NodeId::from_parts(
                GRAPH_TYPE,
                repo(),
                node_kind::METHOD,
                &format!("lib::friend_service::FriendService::{caller}"),
            );
            assert!(
                fp.edges
                    .iter()
                    .any(|e| e.from == from && e.to == ep && e.category == edge_category::CALLS),
                "expected CALLS {caller} -> {method} {path}"
            );
        }
        let endpoints = fp
            .nodes
            .iter()
            .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
            .count();
        assert_eq!(endpoints, 4, "one ENDPOINT per call");
        assert!(
            !fp.nodes
                .iter()
                .any(|n| n.id == route_id("GET", "/protected/friends")),
            "a client call is never a server ROUTE"
        );
        // The plain call's ENDPOINT_HIT is byte-for-byte HEAD's.
        assert_eq!(
            endpoint_hit(&fp, endpoint_id("POST", "/protected/friends/accept/${…}")).as_deref(),
            Some(PLAIN_ACCEPT_HIT)
        );
        // Every generic shape in quokka_android: a one-argument call with a
        // trailing comma (a one-field record literal after the misparse), a
        // nested `>>>` type argument, `<void>` misparsed after `await`, an
        // un-awaited call held as an argument, a nullable type argument with a
        // named argument, and a `return`.
        let more = r#"class S {
  Future<void> a() async {
    final r0 = await dio.get<Map<String, dynamic>>(
      '/l0/${Uri.encodeComponent(id)}',
    );
    final r1 = await dio.get<List<Map<String, dynamic>>>('/l1');
    final r2 = await dio.get<void>('/l2');
    unawaited(dio.delete<dynamic>('/l3'));
    final r4 = await dio.get<String?>('/l4', queryParameters: {'q': q});
    return dio.put<dynamic>('/l5');
  }
  bool b(int x, int y, int z) => x.y < z > ('/not-a-call');
}
"#;
        let fp = parse_file(more, "lib/s.dart", "lib::s", repo()).unwrap();
        for (method, path) in [
            ("GET", "/l0/${…}"),
            ("GET", "/l1"),
            ("GET", "/l2"),
            ("DELETE", "/l3"),
            ("GET", "/l4"),
            ("PUT", "/l5"),
        ] {
            assert!(
                fp.nodes.iter().any(|n| n.id == endpoint_id(method, path)),
                "expected ENDPOINT {method} {path}"
            );
        }
        assert_eq!(
            fp.nodes
                .iter()
                .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
                .count(),
            6,
            "a relational expression on a non-client receiver mints nothing"
        );
    }

    /// CH.5a: HEAD's (cbaf4ca) ENDPOINT_HIT for the fixture's plain
    /// `await dio.post('/protected/friends/accept/$id')` (line 20, col 5).
    const PLAIN_ACCEPT_HIT: &str = r#"{"method":"POST","path":"/protected/friends/accept/${…}","file":"lib/friend_service.dart","line":20,"col":5,"confidence":"strong"}"#;

    /// CH.5a: skipping the type-argument selector touches no server router: a
    /// plain `router.get('/x', h)` is still a ROUTE, and a generic
    /// `router.get<Response>(..)`, which the route text scan never read, mints
    /// neither a ROUTE nor (its receiver is no HTTP client) an ENDPOINT.
    #[test]
    fn generic_server_router_still_a_route() {
        let source = r#"
final router = Router();
void routes() {
  router.get('/plain', handler);
  router.get<Response>('/generic', handler);
  router.post<Response>('/one');
}
"#;
        let fp = parse_file(source, "bin/server.dart", "bin::server", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/plain")));
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("GET", "/generic")));
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("POST", "/one")));
        assert!(
            !fp.nav
                .kind_by_id
                .values()
                .any(|k| *k == node_kind::ENDPOINT),
            "a server router is no HTTP client"
        );
    }

    /// CH.5a: every `BaseOptions(baseUrl: X)` - inside a top-level provider
    /// closure, read by the whole-tree scan - and every
    /// `<recv>.options.baseUrl = X` records a ClientBase on the MODULE.
    #[test]
    fn base_options_records_client_base() {
        let source = r#"final p = FutureProvider((ref) async {
  final dio = Dio(BaseOptions(baseUrl: Env.apiBaseUrl, connectTimeout: const Duration(seconds: 15)));
  final retryDio = Dio(BaseOptions(baseUrl: retry.baseUrl));
  dio.options.baseUrl = 'https://x/api';
  const fixed = BaseOptions(baseUrl: '/v1');
  x.baseUrl = 'not-a-client';
  return dio;
});
"#;
        let fp = parse_file(source, "lib/client.dart", "lib::client", repo()).unwrap();
        let base = |expr: &str, line: u32| NavFact::ClientBase {
            via: "dio".into(),
            expr: expr.into(),
            line,
        };
        assert_eq!(
            module_facts(&fp, "lib::client"),
            [
                base("Env.apiBaseUrl", 1),
                base("retry.baseUrl", 2),
                base("'https://x/api'", 3),
                base("'/v1'", 4),
            ]
        );
        let tree = dart_tree(source);
        let mut acc = Acc::default();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::client");
        assert_eq!(
            collect_client_bases(tree.root_node(), source.as_bytes(), module, &mut acc),
            (4, 1)
        );
    }

    /// CH.5a: a getter returning one string literal, and a static / library
    /// const initialised to one, record a ValueLiteral when the literal is
    /// URL-shaped; a host, a name, or a getter with two returns record nothing.
    #[test]
    fn getter_and_const_value_literals() {
        let source = r#"class Env {
  static const String prodApiHost = 'api.example.net';
  static String get apiBaseUrl {
    final s = 'https';
    return '$s://$prodApiHost/api';
  }
  static String get name => 'x';
}
"#;
        let fp = parse_file(source, "lib/env.dart", "lib::env", repo()).unwrap();
        let lit = |name: &str, value: &str| NavFact::ValueLiteral {
            name: name.into(),
            value: value.into(),
        };
        assert_eq!(
            module_facts(&fp, "lib::env"),
            [lit("Env.apiBaseUrl", "${…}://${…}/api")]
        );

        let source = r#"const apiRoot = 'https://top/api';
String get health => '/health';
class Two {
  static String get base {
    if (local) {
      return '/a';
    }
    return '/b';
  }
  static String get closure {
    final f = () {
      return '/inner';
    };
    return '/outer';
  }
  static final legacy = '/v0';
  final String instance = '/instance';
}
mixin Paths {
  static String get root => 'https://m/api';
}
"#;
        let fp = parse_file(source, "lib/two.dart", "lib::two", repo()).unwrap();
        assert_eq!(
            module_facts(&fp, "lib::two"),
            [
                lit("apiRoot", "https://top/api"),
                lit("health", "/health"),
                lit("Two.closure", "/outer"),
                lit("Two.legacy", "/v0"),
                lit("Paths.root", "https://m/api"),
            ]
        );
    }

    /// CH.5a: the `[dart-dio-base]` marker's counts, and no line for a file
    /// that recorded nothing.
    #[test]
    fn dio_base_marker_counts_bases_copies_and_literals() {
        let source = r#"class Env {
  static String get apiBaseUrl => 'https://api.example.net/api';
}
final p = FutureProvider((ref) async {
  final dio = Dio(BaseOptions(baseUrl: Env.apiBaseUrl));
  final retryDio = Dio(BaseOptions(baseUrl: retry.baseUrl));
});
"#;
        let tree = dart_tree(source);
        let mut acc = Acc::default();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::x");
        let stats = collect_dio_base_facts(source, tree.root_node(), module, &mut acc);
        assert_eq!(
            stats,
            DioBaseStats {
                bases: 2,
                copies: 1,
                literals: 1
            }
        );
        assert_eq!(
            stats.marker("lib/x.dart").as_deref(),
            Some("[dart-dio-base] bases=2 (copy=1) literals=1 file=lib/x.dart")
        );
        assert_eq!(DioBaseStats::default().marker("lib/x.dart"), None);
    }

    fn dart_tree(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
        parser.set_language(&lang).expect("dart grammar");
        parser.parse(source, None).expect("tree")
    }

    /// CH.5c: quokka_android's token refresh. A receiver the file binds to a
    /// Dio - a cascade `Dio(..)..x` local, a typed local, a field and a getter
    /// declared `Dio` - is a client whatever its name: its verb calls are
    /// ENDPOINTs with their CALLS edge, never server ROUTEs.
    #[test]
    fn dio_bound_receiver_is_a_client_not_a_route() {
        let source = r#"import 'package:dio/dio.dart';
class AuthApi {
  AuthApi(this.conn);
  final Dio conn;
  Dio get session => conn;
  Future<void> refresh() async {
    final fresh = Dio(BaseOptions(baseUrl: Env.apiBaseUrl))
      ..interceptors.add(CookieManager(jar));
    final res = await fresh.post(
      '/auth/refresh',
      options: Options(extra: {'skipAuth': true}),
    );
  }
  Future<void> me() async {
    Dio transport = Dio();
    var made = new Dio();
    await transport.get('/auth/me');
    await made.head('/auth/ping');
    await conn.put('/auth/password', data: {});
    await session.delete('/auth/session');
  }
}
"#;
        let fp = parse_file(source, "lib/api_client.dart", "lib::api_client", repo()).unwrap();
        for (method, path) in [
            ("POST", "/auth/refresh"),
            ("GET", "/auth/me"),
            ("HEAD", "/auth/ping"),
            ("PUT", "/auth/password"),
            ("DELETE", "/auth/session"),
        ] {
            let ep = endpoint_id(method, path);
            assert!(
                fp.nodes.iter().any(|n| n.id == ep),
                "expected ENDPOINT {method} {path}"
            );
            assert!(
                fp.edges
                    .iter()
                    .any(|e| e.to == ep && e.category == edge_category::CALLS),
                "expected a CALLS edge into {method} {path}"
            );
            assert!(
                !fp.nodes.iter().any(|n| n.id == route_id(method, path)),
                "a Dio client call is never a ROUTE: {method} {path}"
            );
        }
        let tree = dart_tree(source);
        let bound = collect_dio_receivers(source, tree.root_node(), source.as_bytes());
        let mut names: Vec<&str> = bound.iter().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["conn", "fresh", "made", "session", "transport"]);
    }

    /// CH.5c: the binding set never turns a server router into a client. A
    /// name the file binds to a `Router()` (or any other type) is left to the
    /// name heuristic even when it is ALSO bound to a Dio, and a file that
    /// never spells `Dio` binds nothing.
    #[test]
    fn dio_binding_never_claims_a_router() {
        let source = r#"
final router = Router();
void routes() {
  router.get('/things', handler);
}
class Other {
  final Dio svc;
  void m(String svc) {
    svc.get('/both');
  }
}
"#;
        let fp = parse_file(source, "bin/server.dart", "bin::server", repo()).unwrap();
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/things")));
        assert!(!fp.nodes.iter().any(|n| n.id == endpoint_id("GET", "/things")));
        // `svc` is a Dio field and a String parameter: not in the set, so
        // today's name heuristic decides (no client name: a route).
        assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", "/both")));
        let tree = dart_tree(source);
        assert!(collect_dio_receivers(source, tree.root_node(), source.as_bytes()).is_empty());
        let plain = "final router = Router();\nvoid f() { router.get('/x', h); }\n";
        let tree = dart_tree(plain);
        assert!(collect_dio_receivers(plain, tree.root_node(), plain.as_bytes()).is_empty());
    }

    /// CH.5c fired_on marker: only a file the binding set changed prints.
    #[test]
    fn dio_bound_marker_reports_reclassified_calls() {
        let stats = DioBoundStats {
            endpoints: 2,
            route_skips: 2,
        };
        assert_eq!(
            stats.marker(3, "lib/a.dart").as_deref(),
            Some("[dart-dio-receivers] bound=3 endpoints=2 route_skips=2 file=lib/a.dart")
        );
        assert_eq!(DioBoundStats::default().marker(3, "lib/a.dart"), None);
    }

    // ---- DH.15: injectable constructor DI -> INJECTS refs ------------------

    /// The INJECTS refs of a parse: (from, injected type head, line).
    fn injects_refs(fp: &FileParse) -> Vec<(NodeId, String, u32)> {
        fp.refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => (r.from, n.clone(), r.line),
                q => (r.from, format!("{q:?}"), r.line),
            })
            .collect()
    }

    fn module_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, qname)
    }

    /// matrix/dart/injects home_controller.dart, verbatim.
    const HOME_CONTROLLER: &str = "import 'package:injectable/injectable.dart';

import 'api_service.dart';

@injectable
class HomeController {
  final ApiService api;

  HomeController(this.api);
}
";

    #[test]
    fn injectable_this_field_param() {
        // `HomeController(this.api)`: the field formal's type is the field's
        // declared type, read through collect_dart_field_types.
        let fp = parse_file(HOME_CONTROLLER, "home_controller.dart", "home_controller", repo()).unwrap();
        let ctl = class_id("home_controller::HomeController");
        assert_eq!(injects_refs(&fp), vec![(ctl, "ApiService".to_string(), 8)]);
        let r = fp.refs.iter().find(|r| r.category == edge_category::INJECTS).unwrap();
        assert_eq!(r.from_module, module_id("home_controller"));
    }

    #[test]
    fn injectable_typed_and_named_params() {
        let source = r#"import 'package:injectable/injectable.dart';
import 'package:http/http.dart' as p;

@LazySingleton(as: AuthRepo)
class AuthRepoImpl implements AuthRepo {
  final TokenStore store;
  AuthRepoImpl(this.store, p.HttpClient<Json>? client,
      {required this.cache, required Logger log, Clock? clock,
      @factoryParam Session? session, Audit this.audit});
  const AuthRepoImpl.named(TokenStore again);
  factory AuthRepoImpl.make(Metrics m) => AuthRepoImpl.named(TokenStore());
  final Cache cache;
  final Object audit;
}

@singleton
class Ticking {
  Ticking(final Ticker t, [Zone? zone]);
}

@Injectable()
class Plain {
  Plain(Foo foo);
}

@lazySingleton
class Lazy {
  factory Lazy(Bar bar) = LazyImpl;
}

@Singleton(dispose: close)
class Closing {
  Closing(super.x, Sink sink);
}
"#;
        let fp = parse_file(source, "lib/auth.dart", "lib::auth", repo()).unwrap();
        let of = |class: &str| -> Vec<String> {
            let id = class_id(&format!("lib::auth::{class}"));
            injects_refs(&fp)
                .into_iter()
                .filter(|(from, _, _)| *from == id)
                .map(|(_, t, _)| t)
                .collect()
        };
        // Field formals (`this.store`, `required this.cache`) take the field's
        // type; a typed field formal (`Audit this.audit`) its own; a typed
        // parameter its head without the prefix, generics or `?`. A type
        // two constructors inject is one ref; `@factoryParam` is the
        // caller's argument, never a container lookup.
        assert_eq!(
            of("AuthRepoImpl"),
            ["TokenStore", "HttpClient", "Cache", "Logger", "Clock", "Audit", "Metrics"]
        );
        assert_eq!(of("Ticking"), ["Ticker", "Zone"]);
        assert_eq!(of("Plain"), ["Foo"]);
        assert_eq!(of("Lazy"), ["Bar"]);
        // `super.x` names no type here; `Sink` is typed.
        assert_eq!(of("Closing"), ["Sink"]);
        // Each ref sits on its parameter's row and starts from the class.
        let line_of_clock = injects_refs(&fp)
            .into_iter()
            .find(|(_, t, _)| t == "Clock")
            .map(|(_, _, l)| l);
        assert_eq!(line_of_clock, Some(7));
        assert!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::INJECTS)
                .all(|r| r.from_module == module_id("lib::auth"))
        );
    }

    #[test]
    fn unannotated_class_emits_no_inject() {
        // Plain constructor parameters are not DI: only the annotation makes
        // get_it build the class.
        let source = r#"import 'package:injectable/injectable.dart';
import 'package:meta/meta.dart';

class HomeController {
  final ApiService api;
  HomeController(this.api);
}

@immutable
class View {
  View(ApiService api);
}
"#;
        let fp = parse_file(source, "lib/home.dart", "lib::home", repo()).unwrap();
        assert!(injects_refs(&fp).is_empty(), "{:?}", injects_refs(&fp));
    }

    #[test]
    fn injectable_skips_core_types() {
        let source = r#"import 'package:injectable/injectable.dart';

@injectable
class Settings {
  final String name;
  Settings(this.name, int n, double d, num x, bool b, dynamic y, Object o,
      List<Repo> l, Map<String, Repo> m, Set<Repo> s, Future<Repo> f,
      Stream<Repo> st, Duration dur, DateTime at, void Function() cb,
      int pick(int i), (int, Repo) pair, Repo repo);
}
"#;
        let fp = parse_file(source, "lib/settings.dart", "lib::settings", repo()).unwrap();
        let types: Vec<String> = injects_refs(&fp).into_iter().map(|(_, t, _)| t).collect();
        assert_eq!(types, ["Repo"]);
    }

    #[test]
    fn injectable_needs_the_import() {
        // A file that never imports `package:injectable/` has no injectable
        // annotation, whatever its annotations are called.
        let source = HOME_CONTROLLER.replace("import 'package:injectable/injectable.dart';\n", "");
        let fp = parse_file(&source, "home_controller.dart", "home_controller", repo()).unwrap();
        assert!(injects_refs(&fp).is_empty(), "{:?}", injects_refs(&fp));
    }

    // ---- DH.15: shelf_router route handlers -> HANDLED_BY refs ------------

    /// The HANDLED_BY refs of a parse: (from, qualifier, line).
    fn handler_refs(fp: &FileParse) -> Vec<(NodeId, CallQualifier, u32)> {
        fp.refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .map(|r| (r.from, r.qualifier.clone(), r.line))
            .collect()
    }

    #[test]
    fn shelf_router_handler_refs() {
        // matrix/dart/http_server server/server.dart, verbatim, plus a
        // `base.name` handler.
        let source = r#"import 'package:shelf/shelf.dart';
import 'package:shelf_router/shelf_router.dart';

Response getUser(Request request, String id) => Response.ok('user $id');

Response createUser(Request request) => Response(201);

final router = Router()
  ..get('/users/<id>', getUser)
  ..post('/users', createUser);

final admin = Router()
  ..put( '/users/<id>' ,  handlers.updateUser )
  ..delete("/users/<id>", h.remove,);
"#;
        let fp = parse_file(source, "server.dart", "server", repo()).unwrap();
        let bare = |n: &str| CallQualifier::Bare(n.to_string());
        let attr = |b: &str, n: &str| CallQualifier::Attribute {
            base: b.to_string(),
            name: n.to_string(),
        };
        let mut refs = handler_refs(&fp);
        refs.sort_by_key(|(_, _, line)| *line);
        assert_eq!(
            refs,
            vec![
                (route_id("GET", "/users/<id>"), bare("getUser"), 8),
                (route_id("POST", "/users"), bare("createUser"), 9),
                (route_id("PUT", "/users/<id>"), attr("handlers", "updateUser"), 12),
                (route_id("DELETE", "/users/<id>"), attr("h", "remove"), 13),
            ]
        );
        assert!(
            fp.refs
                .iter()
                .filter(|r| r.category == edge_category::HANDLED_BY)
                .all(|r| r.from_module == module_id("server"))
        );
    }

    #[test]
    fn shelf_inline_closure_has_no_handler() {
        let source = r#"import 'package:shelf_router/shelf_router.dart';
import 'package:go_router/go_router.dart';

final app = Router()
  ..get('/hi', (Request r) => Response.ok('hi'))
  ..get('/made', makeHandler())
  ..get('/opts', base, middleware: m)
  ..get('/bare');
final nav = GoRouter(routes: [GoRoute(path: '/page', builder: buildPage)]);
"#;
        let fp = parse_file(source, "bin/app.dart", "bin::app", repo()).unwrap();
        for path in ["/hi", "/made", "/bare"] {
            assert!(fp.nodes.iter().any(|n| n.id == route_id("GET", path)), "route {path}");
        }
        // `base` is argument #1 of `/opts`: a handler, its named argument
        // after it notwithstanding.
        assert_eq!(
            handler_refs(&fp),
            vec![(route_id("GET", "/opts"), CallQualifier::Bare("base".to_string()), 6)]
        );
    }

    #[test]
    fn dio_client_calls_still_skipped() {
        // CH.5c control: a Dio-bound receiver's call is a client ENDPOINT,
        // never a ROUTE, so its second argument is never a handler.
        let source = r#"import 'package:dio/dio.dart';

final api = Dio();

Future<void> load(Options options, Object data) async {
  await api.get('/users', options);
  await dio.post('/users', data);
}
"#;
        let fp = parse_file(source, "lib/api.dart", "lib::api", repo()).unwrap();
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("GET", "/users")));
        assert!(!fp.nodes.iter().any(|n| n.id == route_id("POST", "/users")));
        assert!(handler_refs(&fp).is_empty(), "{:?}", handler_refs(&fp));
    }

    #[test]
    fn injectable_factory_params_skip_in_both_placements() {
        // The annotation sits inside the first parameter's node and before a
        // later one's as a sibling; either way the parameter is the caller's.
        let source = r#"import 'package:injectable/injectable.dart';

@injectable
class Detail {
  Detail(@factoryParam Item first, Repo repo, @factoryParam Item second,
      {@ignoreParam Clock? clock, Cache? cache, @factoryParam Item? third});
}
"#;
        let fp = parse_file(source, "lib/detail.dart", "lib::detail", repo()).unwrap();
        let types: Vec<String> = injects_refs(&fp).into_iter().map(|(_, t, _)| t).collect();
        assert_eq!(types, ["Repo", "Cache"]);
    }

    #[test]
    fn dh15_markers() {
        let di = DiStats {
            classes: 1,
            injects: 1,
        };
        assert_eq!(
            di.marker("home_controller.dart").as_deref(),
            Some("[dart-di] injectable classes=1 injects=1 path=home_controller.dart")
        );
        assert_eq!(DiStats::default().marker("a.dart"), None);
        let shelf = ShelfRouteStats {
            routes: 2,
            handlers: 2,
        };
        assert_eq!(
            shelf.marker("server/server.dart").as_deref(),
            Some("[dart-routes] shelf routes=2 handlers=2 path=server/server.dart")
        );
        assert_eq!(ShelfRouteStats::default().marker("a.dart"), None);
    }

    // ---- DT.5e: closures, creation sites, names passed by value, Flutter ----

    /// The USES edges the parser emitted from `from`, as (to, evidence rule),
    /// sorted.
    fn uses_edges(fp: &FileParse, from: NodeId) -> Vec<(NodeId, Option<String>)> {
        let mut out: Vec<(NodeId, Option<String>)> = fp
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::USES)
            .map(|e| {
                let ev = glia_code_domain::evidence::Evidence::read(&e.cells);
                (e.to, ev.and_then(|ev| ev.rule))
            })
            .collect();
        out.sort_by_key(|(to, _)| format!("{to:?}"));
        out
    }

    /// The qnames of this file's COMPONENT overlay nodes, sorted.
    fn component_overlays(fp: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = fp
            .nodes
            .iter()
            .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::COMPONENT))
            .filter_map(|n| fp.nav.qname_by_id.get(&n.id).cloned())
            .collect();
        out.sort();
        out
    }

    const CLOSURES_FIXTURE: &str = r#"class Page {
  Widget build(BuildContext context) {
    return Builder(builder: (ctx) {
      return CardPage(title: label());
    });
  }

  void arrow() => list.forEach((x) => render(x));

  void nested() {
    run(() {
      later(() => deep());
    });
  }

  void local() {
    void inner() {
      target();
    }
    inner();
  }

  void shadow() {
    items.map((save) => save());
  }

  void save() {}
  String label() => 'x';
}
"#;

    /// DT.5e: a call inside a block closure, an arrow closure and a nested
    /// closure is the enclosing member's; at HEAD every one was lost.
    #[test]
    fn closure_calls_are_the_members() {
        let fp = parse_file(CLOSURES_FIXTURE, "lib/page.dart", "lib::page", repo()).unwrap();
        assert_eq!(
            calls_from(&fp, "lib::page::Page::build"),
            sorted(vec![bare("Builder"), bare("CardPage"), self_m("label")])
        );
        assert_eq!(
            calls_from(&fp, "lib::page::Page::arrow"),
            sorted(vec![attr("list", "forEach"), bare("render")])
        );
        assert_eq!(
            calls_from(&fp, "lib::page::Page::nested"),
            sorted(vec![bare("run"), bare("later"), bare("deep")])
        );
    }

    /// A local function's body was always walked for its member (HEAD
    /// behaviour, pinned), and the local function's own name is a local.
    #[test]
    fn local_function_calls_stay_the_members() {
        let fp = parse_file(CLOSURES_FIXTURE, "lib/page.dart", "lib::page", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::page::Page::local"), vec![bare("target")]);
    }

    /// `items.map((save) => save())`: the closure parameter shadows the
    /// member `save`, so `save()` is no call site at all.
    #[test]
    fn closure_parameter_shadows_a_member() {
        let fp = parse_file(CLOSURES_FIXTURE, "lib/page.dart", "lib::page", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::page::Page::shadow"), vec![attr("items", "map")]);
    }

    /// `const T(..)`, `new T(..)`, `const T.named(..)`, `new T.named(..)` and
    /// a prefixed `const p.T(..)` are calls of T, like `T(..)` / `T.named(..)`.
    #[test]
    fn const_and_new_are_creation_calls() {
        let source = r#"import 'package:ui/ui.dart' as p;

class Dot {
  const Dot();
  const Dot.small();
}

class Page {
  void make() {
    final a = const Dot();
    final b = new Label('x');
    final c = const Dot.small();
    final d = new Label.big(2);
    final e = const p.Chip();
    final f = const p.Chip.round();
    final g = const <int>[];
  }
}
"#;
        let fp = parse_file(source, "lib/page.dart", "lib::page", repo()).unwrap();
        assert_eq!(
            calls_from(&fp, "lib::page::Page::make"),
            sorted(vec![
                bare("Dot"),
                bare("Label"),
                attr("Dot", "small"),
                attr("Label", "big"),
                bare("Chip"),
                attr("Chip", "round"),
            ])
        );
        // CB.9's receiver-type record: `const Dot.small()` names a type this
        // file declares, so `Dot` is recorded as the caller's type `Dot`.
        let make = method_id("lib::page::Page::make");
        assert_eq!(
            fp.nav.local_types.get(&make).and_then(|t| t.get("Dot")).map(String::as_str),
            Some("Dot")
        );
    }

    const BY_NAME_FIXTURE: &str = r#"import 'helpers.dart';

void _noop() {}

String get title => 'x';

final kAction = _noop;

class Card {
  final VoidCallback onTap;
  Card(this.onTap);

  void _save() {}

  bool get ready => true;

  void wire() {
    final onLocal = _save;
    use(_save);
    Button(onPressed: _save, onLong: _noop);
    Button(onPressed: onLocal);
    Button(onPressed: onTap);
    Button(onPressed: ready);
    Button(onPressed: title);
    Button(onPressed: kAction);
    Button(onPressed: shared);
    Button(onPressed: Card);
    Button(onPressed: widget.onTap);
    Button(onPressed: () => _save());
    Button(onPressed: _save);
  }

  void _relisten() {
    removeListener(_relisten);
  }
}
"#;

    /// DT.5e: `onPressed: _save` / `use(_save)` name the member METHOD: a USES
    /// edge, emitter parser:dart rule method_ref, once per (from, to). A
    /// same-file top-level FUNCTION by name is a USES edge too.
    #[test]
    fn a_method_or_function_passed_by_name_is_used() {
        let fp = parse_file(BY_NAME_FIXTURE, "lib/card.dart", "lib::card", repo()).unwrap();
        let wire = method_id("lib::card::Card::wire");
        let rule = Some("method_ref".to_string());
        let mut want = vec![
            (method_id("lib::card::Card::_save"), rule.clone()),
            (function_id("lib::card::_noop"), rule),
        ];
        want.sort_by_key(|(to, _)| format!("{to:?}"));
        assert_eq!(uses_edges(&fp, wire), want);
        let save = fp
            .edges
            .iter()
            .find(|e| e.from == wire && e.to == method_id("lib::card::Card::_save"))
            .and_then(|e| glia_code_domain::evidence::Evidence::read(&e.cells))
            .expect("evidence");
        assert_eq!(save.emitter, "parser:dart");
        // DT.5h: `final onLocal = _save;` names `_save` too (an initialiser
        // is a value position), so its row is the earliest.
        assert_eq!(save.line, Some(17));
    }

    /// A library-scope name this file does not declare (an imported
    /// function, or a Riverpod provider variable) binds nothing: no USES edge
    /// and no ref the graph would bind to a STATE_VAR.
    #[test]
    fn an_imported_name_passed_by_name_binds_nothing() {
        let fp = parse_file(BY_NAME_FIXTURE, "lib/card.dart", "lib::card", repo()).unwrap();
        assert!(!fp.refs.iter().any(|r| r.category == edge_category::USES), "{:?}", fp.refs);
        let wire = method_id("lib::card::Card::wire");
        let named = |to: &NodeId| fp.nav.name_by_id.get(to).cloned().unwrap_or_default();
        assert!(uses_edges(&fp, wire).iter().all(|(to, _)| named(to) != "shared"));
    }

    /// A local (`onLocal`), a field (`onTap`), a getter (`ready`, `title`), a
    /// same-file variable (`kAction`), a class (`Card`), a member access
    /// (`widget.onTap`) and a self-reference bind nothing.
    #[test]
    fn locals_fields_accessors_and_self_bind_nothing() {
        let fp = parse_file(BY_NAME_FIXTURE, "lib/card.dart", "lib::card", repo()).unwrap();
        let card = "lib::card::Card";
        let wire = method_id(&format!("{card}::wire"));
        let targets: Vec<NodeId> = uses_edges(&fp, wire).into_iter().map(|(to, _)| to).collect();
        assert!(!targets.contains(&method_id(&format!("{card}::ready"))), "a getter");
        assert!(!targets.contains(&function_id("lib::card::title")), "a top-level getter");
        let relisten = method_id(&format!("{card}::_relisten"));
        assert_eq!(uses_edges(&fp, relisten), vec![], "a self-reference");
        assert!(!targets.contains(&state_var_id("lib::card::kAction")), "a variable");
        // The two edges from `wire` (`_save` and `_noop`), and DT.5h's
        // `final kAction = _noop;`: a library initialiser names `_noop`, a
        // USES from the STATE_VAR.
        let uses = fp.edges.iter().filter(|e| e.category == edge_category::USES).count();
        assert_eq!(uses, 3);
        assert_eq!(
            uses_edges(&fp, state_var_id("lib::card::kAction")),
            vec![(function_id("lib::card::_noop"), Some("method_ref".to_string()))]
        );
    }

    /// The by-name rule runs in a top-level function's body too: its own
    /// parameter shadows a same-file function.
    #[test]
    fn by_name_in_a_top_level_function() {
        let source = r#"void tick() {}
void other() {}
void start(void Function() other) {
  Timer.periodic(d, (_) => tick());
  schedule(tick);
  schedule(other);
}
"#;
        let fp = parse_file(source, "lib/t.dart", "lib::t", repo()).unwrap();
        let start = function_id("lib::t::start");
        assert_eq!(
            uses_edges(&fp, start),
            vec![(function_id("lib::t::tick"), Some("method_ref".to_string()))]
        );
        assert_eq!(
            fn_calls(&fp, "lib::t::start"),
            sorted(vec![
                attr("Timer", "periodic"),
                bare("tick"),
                bare("schedule"),
                bare("schedule"),
            ])
        );
    }

    const FLUTTER_IMPORT: &str = "import 'package:flutter/material.dart';\n";

    /// DT.5e: a subclass of each Flutter / Riverpod / hooks base, in a file
    /// importing a Flutter library, gets a COMPONENT overlay of its qname.
    #[test]
    fn every_flutter_base_tags_a_component() {
        for base in FLUTTER_COMPONENT_BASES {
            let source = format!("{FLUTTER_IMPORT}class W extends {base}<X> {{}}\n");
            let fp = parse_file(&source, "lib/w.dart", "lib::w", repo()).unwrap();
            assert_eq!(component_overlays(&fp), ["lib::w::W"], "{base}");
            let overlay = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::COMPONENT, "lib::w::W");
            let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::w");
            assert_eq!(fp.nav.parent_of.get(&overlay), Some(&module), "{base}");
        }
        for import in [
            "package:flutter/widgets.dart",
            "package:flutter_riverpod/flutter_riverpod.dart",
            "package:hooks_riverpod/hooks_riverpod.dart",
            "package:flutter_hooks/flutter_hooks.dart",
        ] {
            let source = format!("import '{import}';\nclass W extends p.StatelessWidget {{}}\n");
            let fp = parse_file(&source, "lib/w.dart", "lib::w", repo()).unwrap();
            assert_eq!(component_overlays(&fp), ["lib::w::W"], "{import}");
        }
    }

    /// No Flutter import, a ChangeNotifier, a mixin-only class, an
    /// `implements` of a widget base and a user-defined base widget: none is
    /// tagged.
    #[test]
    fn non_flutter_classes_are_no_components() {
        let plain = "import 'package:meta/meta.dart';\nclass W extends StatelessWidget {}\n";
        let fp = parse_file(plain, "lib/w.dart", "lib::w", repo()).unwrap();
        assert!(component_overlays(&fp).is_empty(), "no Flutter import");
        let source = format!(
            "{FLUTTER_IMPORT}class CartModel extends ChangeNotifier {{}}\n\
             class M with State {{}}\n\
             class I implements StatelessWidget {{}}\n\
             abstract class BaseScreen extends StatelessWidget {{}}\n\
             class Home extends BaseScreen {{}}\n"
        );
        let fp = parse_file(&source, "lib/w.dart", "lib::w", repo()).unwrap();
        assert_eq!(component_overlays(&fp), ["lib::w::BaseScreen"]);
    }

    #[test]
    fn dt5e_flutter_marker() {
        let stats = FlutterStats {
            widgets: 3,
            states: 1,
        };
        assert_eq!(
            stats.marker("lib/features/card.dart").as_deref(),
            Some("[dart-flutter] components=4 (widgets=3 states=1) file=lib/features/card.dart")
        );
        assert_eq!(FlutterStats::default().marker("a.dart"), None);
        let source = format!(
            "{FLUTTER_IMPORT}class A extends StatelessWidget {{}}\n\
             class B extends ConsumerStatefulWidget {{}}\n\
             class _BState extends ConsumerState<B> {{}}\n"
        );
        let mut acc = Acc::default();
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(&source, None).unwrap();
        let (root, src) = (tree.root_node(), source.as_bytes());
        acc.flutter_import = imports_any(root, src, FLUTTER_IMPORT_PREFIXES);
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::w");
        visit_top(root, src, "lib/w.dart", "lib::w", module, repo(), &mut acc);
        assert_eq!((acc.flutter.widgets, acc.flutter.states), (2, 1));
    }

    // ---- DT.1l: Dart wire models and Dio shapes -----------------------------

    /// DT.1l: the SCHEMA_FIELDS payload of the CLASS `qname`, if any.
    fn schema_of(fp: &FileParse, qname: &str) -> Option<ModelSchema> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, qname);
        fp.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .find_map(ModelSchema::from_cell)
    }

    /// DT.1l: the `response` / `request` of the ENDPOINT `<method> <path>`.
    fn shapes_of(fp: &FileParse, method: &str, path: &str) -> (Option<Shape>, Option<Shape>) {
        endpoint_hit(fp, endpoint_id(method, path)).map_or((None, None), |p| wire::hit_shapes(&p))
    }

    fn named(ty: &str, array: bool) -> Shape {
        Shape::Named {
            ty: ty.to_string(),
            array,
            model: None,
        }
    }

    /// The fixture's model: `models-dart-dio/app/lib/models/member.dart`.
    const MEMBER_DART: &str = r#"class Member {
  Member({this.memberId, this.handle, this.displayName, this.badges = const []});

  final String? memberId;
  final String? handle;
  final String? displayName;
  final List<String> badges;

  factory Member.fromJson(Map<String, dynamic> j) => Member(
        memberId: j['memberId'] as String?,
        handle: j['handle'] as String?,
        displayName: j['display_name'] as String?,
        badges: (j['badges'] as List?)?.cast<String>() ?? const [],
      );
}
"#;

    /// The fixture's service: `models-dart-dio/app/lib/services/member_service.dart`.
    const MEMBER_SERVICE_DART: &str = r#"import 'package:dio/dio.dart';
import '../models/member.dart';

class MemberService {
  MemberService(this.dio);

  final Dio dio;

  Future<Member> get(String id) async {
    final res = await dio.get<Map<String, dynamic>>('/api/members/$id');
    return Member.fromJson(res.data ?? const {});
  }

  Future<List<Member>> list() async {
    final res = await dio.get<dynamic>('/api/members');
    final raw = res.data as Map<String, dynamic>;
    return (raw['members'] as List)
        .whereType<Map<String, dynamic>>()
        .map(Member.fromJson)
        .toList();
  }

  Future<void> rename(String id, String handle) async {
    await dio.put('/api/members/$id', data: {'handle': handle, 'reason': 'user'});
  }
}
"#;

    /// DT.1l: a hand-written `fromJson`'s JSON names are its subscript keys,
    /// in source order: `display_name`, not the field `displayName` it fills
    /// (kept as `field`). Types come from the subscript's cast; `required`
    /// needs a non-nullable type and no `??` default.
    #[test]
    fn dt1l_from_json_keys_types_and_required() {
        let fp = parse_file(MEMBER_DART, "lib/models/member.dart", "lib::models::member", repo()).unwrap();
        let schema = schema_of(&fp, "lib::models::member::Member").expect("SCHEMA_FIELDS on Member");
        assert_eq!(schema.format, WireFormat::Dart);
        assert!(!schema.open);
        let rows: Vec<(&str, Option<&str>, &str, bool)> = schema
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.field.as_deref(), f.ty.as_str(), f.required))
            .collect();
        assert_eq!(
            rows,
            [
                ("memberId", None, "String?", false),
                ("handle", None, "String?", false),
                ("display_name", Some("displayName"), "String?", false),
                ("badges", None, "List?", false),
            ]
        );
        assert_eq!(
            schema.to_payload(),
            r#"{"fields":[{"name":"memberId","required":false,"type":"String?"},{"name":"handle","required":false,"type":"String?"},{"field":"displayName","name":"display_name","required":false,"type":"String?"},{"name":"badges","required":false,"type":"List?"}],"format":"dart"}"#
        );
        let stats = LAST_DART_WIRE.with(|w| w.borrow().clone());
        assert_eq!((stats.fromjson, stats.serializable), (1, 0));

        // A block body, a nested model through fromJson (its type), a list of
        // models, a non-nullable directly-assigned field (required), a value
        // converted on the way (no declared type), and a map parameter that
        // escapes (open).
        let source = r#"class Order {
  Order({required this.id, this.address, this.items = const [], this.state, this.total = 0});
  final String id;
  final Address? address;
  final List<Item> items;
  final OrderState? state;
  final int total;
  factory Order.fromJson(Map<String, dynamic> json) {
    final raw = json['items'] as List?;
    return Order(
      id: json['id'],
      address: json['address'] is Map<String, dynamic>
          ? Address.fromJson(json['address'] as Map<String, dynamic>)
          : null,
      items: raw?.map((e) => Item.fromJson(e as Map<String, dynamic>)).toList() ?? const [],
      state: stateFrom(json['state']),
      total: (json['total'] as num?)?.toInt() ?? 0,
    );
  }
}
class Loose {
  Loose(this.a);
  final String a;
  factory Loose.fromJson(Map<String, dynamic> j) => Loose(pick(j, 'a'));
}
class Child {
  Child(this.base, this.extra);
  final Base base;
  final String? extra;
  factory Child.fromJson(Map<String, dynamic> j) =>
      Child(Base.fromJson(j), j['extra'] as String?);
}
"#;
        let fp = parse_file(source, "lib/order.dart", "lib::order", repo()).unwrap();
        let schema = schema_of(&fp, "lib::order::Order").expect("Order schema");
        let rows: Vec<(&str, &str, bool, bool)> = schema
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.ty.as_str(), f.required, f.array))
            .collect();
        assert_eq!(
            rows,
            [
                ("items", "List<Item>", false, true),
                ("id", "String", true, false),
                ("address", "Address", false, false),
                ("state", "dynamic", false, false),
                ("total", "num?", false, false),
            ]
        );
        assert!(!schema.open);
        assert_eq!(schema_of(&fp, "lib::order::Loose"), None, "zero literal keys, no cell");
        let child = schema_of(&fp, "lib::order::Child").expect("Child schema");
        assert!(child.open, "the map passed whole carries keys the body does not list");
        assert_eq!(child.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["extra"]);
    }

    /// DT.1l: a `@JsonSerializable` class lists its instance fields: a
    /// `@JsonKey(name:)` rename, the class's `fieldRename`, skipped
    /// `includeFromJson: false` / `ignore: true` fields, and `required` from
    /// nullability, `defaultValue` and constructor defaults.
    #[test]
    fn dt1l_json_serializable_fields() {
        let source = r#"import 'package:json_annotation/json_annotation.dart';
part 'order.g.dart';

@JsonSerializable(fieldRename: FieldRename.snake)
class Order {
  Order(this.orderId, this.customerName, {this.notes = '', this.count = 0});
  @JsonKey(name: 'id')
  final String orderId;
  final String customerName;
  final String? notes;
  final int count;
  @JsonKey(includeFromJson: false)
  final String local;
  @JsonKey(ignore: true)
  final String cache;
  @JsonKey(defaultValue: 1)
  final int pageSize;
  static const kind = 'order';
  factory Order.fromJson(Map<String, dynamic> json) => _$OrderFromJson(json);
}

@JsonSerializable(fieldRename: FieldRename.screamingSnake)
class Shout {
  Shout(this.userId);
  final String userId;
}

@JsonSerializable(fieldRename: FieldRename.kebab)
class Kebab {
  Kebab(this.userId);
  final String userId;
}

@JsonSerializable(fieldRename: FieldRename.pascal)
class Pascal {
  Pascal(this.userId);
  final String userId;
}
"#;
        let fp = parse_file(source, "lib/order.dart", "lib::order", repo()).unwrap();
        let schema = schema_of(&fp, "lib::order::Order").expect("Order schema");
        let rows: Vec<(&str, Option<&str>, &str, bool)> = schema
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.field.as_deref(), f.ty.as_str(), f.required))
            .collect();
        assert_eq!(
            rows,
            [
                ("id", Some("orderId"), "String", true),
                ("customer_name", Some("customerName"), "String", true),
                ("notes", None, "String?", false),
                ("count", None, "int", false),
                ("page_size", Some("pageSize"), "int", false),
            ]
        );
        let first = |q: &str| schema_of(&fp, q).map(|s| s.fields[0].name.clone());
        assert_eq!(first("lib::order::Shout").as_deref(), Some("USER_ID"));
        assert_eq!(first("lib::order::Kebab").as_deref(), Some("user-id"));
        assert_eq!(first("lib::order::Pascal").as_deref(), Some("UserId"));
        let stats = LAST_DART_WIRE.with(|w| w.borrow().clone());
        assert_eq!((stats.fromjson, stats.serializable), (0, 4));
    }

    /// DT.1l: a protobuf `fromJson($core.String i, ..)` reads no map, and a
    /// generated file (by name or by banner) writes no schema at all: its
    /// factories are counted `skipped_generated`.
    #[test]
    fn dt1l_generated_and_protobuf_factories_write_nothing() {
        let pb = r#"//
//  Generated code. Do not modify.
//  source: quokka_chat.proto
//
import 'package:protobuf/protobuf.dart' as $pb;
import 'dart:core' as $core;

class ChatMessage extends $pb.GeneratedMessage {
  factory ChatMessage.fromJson($core.String i, [$pb.ExtensionRegistry r = $pb.ExtensionRegistry.EMPTY]) => create()..mergeFromJson(i, r);
  factory ChatMessage.fromMap(Map<String, dynamic> j) => ChatMessage(j['id']);
}
"#;
        let fp = parse_file(pb, "lib/core/generated/quokka_chat.pb.dart", "lib::core::generated::quokka_chat.pb", repo())
            .unwrap();
        assert_eq!(schema_of(&fp, "lib::core::generated::quokka_chat.pb::ChatMessage"), None);
        let stats = LAST_DART_WIRE.with(|w| w.borrow().clone());
        assert_eq!(stats.skipped_generated, 1);
        assert_eq!(
            stats.marker("lib/core/generated/quokka_chat.pb.dart").as_deref(),
            Some(
                "[dart-wire] models=0 (fromjson=0 serializable=0) skipped_generated=1 responses=0 \
                 requests=0 skipped_multi_call=0 file=lib/core/generated/quokka_chat.pb.dart"
            )
        );

        // The same protobuf factory in a hand-written file is still no map
        // reader: its first parameter is a String.
        let fp = parse_file(pb.replace("Generated code. Do not modify.", "chat").as_str(), "lib/chat.dart", "lib::chat", repo())
            .unwrap();
        assert_eq!(schema_of(&fp, "lib::chat::ChatMessage"), None);

        // A build_runner banner marks a file whatever its name.
        let banner = format!("// GENERATED CODE - DO NOT MODIFY BY HAND\n\n{MEMBER_DART}");
        let fp = parse_file(&banner, "lib/models/member.dart", "lib::models::member", repo()).unwrap();
        assert_eq!(schema_of(&fp, "lib::models::member::Member"), None);
        assert!(dart_generated("lib/a.freezed.dart", ""));
        assert!(dart_generated("lib/a.g.dart", ""));
        assert!(!dart_generated("lib/a.dart", MEMBER_DART));
        assert!(!dart_generated("lib/a.dart", "// Generated by hand, edit freely\nclass A {}\n"));
    }

    /// DT.1l: the fixture's three calls: a direct response (`Member.fromJson`
    /// of `res.data ?? ..`), an envelope (`raw['members']` mapped through
    /// `Member.fromJson`, a non-nullable cast so required) and a `data:` map
    /// literal request typed through the method's parameters.
    #[test]
    fn dt1l_dio_direct_envelope_and_request() {
        let fp = parse_file(
            MEMBER_SERVICE_DART,
            "lib/services/member_service.dart",
            "lib::services::member_service",
            repo(),
        )
        .unwrap();
        assert_eq!(shapes_of(&fp, "GET", "/api/members/${…}"), (Some(named("Member", false)), None));
        let envelope = Shape::Object {
            fields: vec![ShapeField::new("members", true, named("Member", true))],
            open: false,
        };
        assert_eq!(shapes_of(&fp, "GET", "/api/members"), (Some(envelope), None));
        let request = Shape::Object {
            fields: vec![
                ShapeField::new("handle", true, named("String", false)),
                ShapeField::new("reason", true, named("String", false)),
            ],
            open: false,
        };
        assert_eq!(shapes_of(&fp, "PUT", "/api/members/${…}"), (None, Some(request)));
        // The shapes follow every other key, so the old bytes are a prefix.
        assert_eq!(
            endpoint_hit(&fp, endpoint_id("GET", "/api/members/${…}")).as_deref(),
            Some(
                r#"{"method":"GET","path":"/api/members/${…}","file":"lib/services/member_service.dart","line":10,"col":17,"confidence":"strong","response":{"type":"Member"}}"#
            )
        );
        let stats = LAST_DART_WIRE.with(|w| w.borrow().clone());
        assert_eq!((stats.responses, stats.requests, stats.skipped_multi_call), (2, 1, 0));
        assert_eq!(
            stats.marker("lib/services/member_service.dart").as_deref(),
            Some(
                "[dart-wire] models=0 (fromjson=0 serializable=0) skipped_generated=0 responses=2 \
                 requests=1 skipped_multi_call=0 file=lib/services/member_service.dart"
            )
        );
    }

    /// DT.1l: arrays and alternatives. `.map(C.fromJson)` and `.map((e) =>
    /// C.fromJson(e))` over the data or a key are arrays of C; quokka's
    /// `ActivityService.get()` reads either `data['result']` or `data` as an
    /// ActivitySummary, a Union; a generic call misparsed as a comparison
    /// (`raw.whereType<Map<String, dynamic>>()`) is still followed; an inline
    /// `(await dio.get(..)).data` is the data too.
    #[test]
    fn dt1l_dio_arrays_unions_and_misparse() {
        let source = r#"class ActivityService {
  Future<ActivitySummary?> get(String id) async {
    final dio = await _dio();
    final res = await dio.get<Map<String, dynamic>>(
      '/protected/activity/${Uri.encodeComponent(id)}',
    );
    final data = res.data;
    if (data == null) return null;
    final inner = data['result'];
    if (inner is Map<String, dynamic>) return ActivitySummary.fromJson(inner);
    return ActivitySummary.fromJson(data);
  }

  Future<List<ActivitySummary>> mine() async {
    final dio = await _dio();
    final res = await dio.get<dynamic>('/protected/activities/mine');
    final raw = res.data;
    if (raw is List) {
      return raw
          .whereType<Map<String, dynamic>>()
          .map(ActivitySummary.fromJson)
          .toList();
    }
    return const [];
  }

  Future<List<Tag>> tags() async {
    final rows = (await dio.get<List<dynamic>>('/tags')).data!;
    return rows.map((e) => Tag.fromJson(e as Map<String, dynamic>)).toList();
  }

  Future<Tag> first() async {
    final res = await dio.get<List<dynamic>>('/tags/first');
    return Tag.fromJson(res.data!.first as Map<String, dynamic>);
  }
}
"#;
        let fp = parse_file(source, "lib/activity_service.dart", "lib::activity_service", repo()).unwrap();
        let result = Shape::Object {
            fields: vec![ShapeField::new("result", false, named("ActivitySummary", false))],
            open: false,
        };
        assert_eq!(
            shapes_of(&fp, "GET", "/protected/activity/${…}").0,
            Some(Shape::Union(vec![result, named("ActivitySummary", false)]))
        );
        assert_eq!(
            shapes_of(&fp, "GET", "/protected/activities/mine").0,
            Some(named("ActivitySummary", true))
        );
        assert_eq!(shapes_of(&fp, "GET", "/tags").0, Some(named("Tag", true)));
        assert_eq!(shapes_of(&fp, "GET", "/tags/first").0, Some(named("Tag", true)));
    }

    /// DT.1l: a callable holding two Dio calls reads no response for either
    /// (counted); its requests are still read. A call whose result is never
    /// read writes nothing, and its payload keeps its bytes.
    #[test]
    fn dt1l_two_dio_calls_read_no_response() {
        let source = r#"class S {
  Future<Member> both(String id) async {
    final a = await dio.get('/members/$id');
    final b = await dio.post('/audit', data: {'id': id});
    return Member.fromJson(a.data);
  }
  Future<void> fire() async {
    await dio.delete('/members');
  }
}
"#;
        let fp = parse_file(source, "lib/s.dart", "lib::s", repo()).unwrap();
        assert_eq!(shapes_of(&fp, "GET", "/members/${…}"), (None, None));
        let (response, request) = shapes_of(&fp, "POST", "/audit");
        assert_eq!(response, None);
        assert!(request.is_some(), "a request is read off the call itself");
        assert_eq!(
            endpoint_hit(&fp, endpoint_id("DELETE", "/members")).as_deref(),
            Some(r#"{"method":"DELETE","path":"/members","file":"lib/s.dart","line":8,"col":5,"confidence":"strong"}"#)
        );
        let stats = LAST_DART_WIRE.with(|w| w.borrow().clone());
        assert_eq!((stats.responses, stats.requests, stats.skipped_multi_call), (0, 1, 2));
    }

    /// DT.1l: requests. A map literal's keys (a collection-`if` key not
    /// required, a spread leaving it open, literal and typed values), and
    /// `x.toJson()` as x's declared type: a parameter, a local, a field.
    /// Any other `data:` (a Map variable, FormData) writes none.
    #[test]
    fn dt1l_dio_requests() {
        let source = r#"class AuthService {
  AuthService(this.draft);
  final ProfileDraft draft;

  Future<void> login(String identifier, String password, int tries) async {
    await dio.post('/auth/login', data: {
      if (identifier.contains('@')) 'email': identifier else 'username': identifier,
      'password': password,
      'tries': tries,
      'remember': true,
      'meta': {'client': 'android'},
      ...extra,
    });
  }

  Future<void> save(UpdateUser req) async {
    await dio.put('/user', data: req.toJson());
  }

  Future<void> local() async {
    final NewGroup g = build();
    await dio.post('/groups', data: g.toJson());
  }

  Future<void> field() async {
    await dio.put('/profile', data: draft.toJson());
  }

  Future<void> raw(Map<String, dynamic> body) async {
    await dio.post('/raw', data: body);
    await dio.post('/form', data: FormData.fromMap({'a': 1}));
  }
}
"#;
        let fp = parse_file(source, "lib/auth.dart", "lib::auth", repo()).unwrap();
        let meta = Shape::Object {
            fields: vec![ShapeField::new("client", true, named("String", false))],
            open: false,
        };
        let login = Shape::Object {
            fields: vec![
                ShapeField::new("email", false, named("String", false)),
                ShapeField::new("username", false, named("String", false)),
                ShapeField::new("password", true, named("String", false)),
                ShapeField::new("tries", true, named("int", false)),
                ShapeField::new("remember", true, named("bool", false)),
                ShapeField::new("meta", true, meta),
            ],
            open: true,
        };
        assert_eq!(shapes_of(&fp, "POST", "/auth/login").1, Some(login));
        assert_eq!(shapes_of(&fp, "PUT", "/user").1, Some(named("UpdateUser", false)));
        assert_eq!(shapes_of(&fp, "POST", "/groups").1, Some(named("NewGroup", false)));
        assert_eq!(shapes_of(&fp, "PUT", "/profile").1, Some(named("ProfileDraft", false)));
        assert_eq!(shapes_of(&fp, "POST", "/raw"), (None, None));
        assert_eq!(shapes_of(&fp, "POST", "/form"), (None, None));
    }

    // ---- DT.5h: names in value positions, member reads, tear-offs ---------

    /// The CALLS edges the parser emitted from `from`, as (to, evidence
    /// rule), sorted.
    fn parser_calls_edges(fp: &FileParse, from: NodeId) -> Vec<(NodeId, Option<String>)> {
        let mut out: Vec<(NodeId, Option<String>)> = fp
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::CALLS)
            .map(|e| {
                let ev = glia_code_domain::evidence::Evidence::read(&e.cells);
                (e.to, ev.and_then(|ev| ev.rule))
            })
            .collect();
        out.sort_by_key(|(to, rule)| format!("{to:?}{rule:?}"));
        out
    }

    /// The MemberRead facts recorded on one scope, as (receiver, name).
    fn member_reads_of(fp: &FileParse, scope: NodeId) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .nav
            .nav_facts
            .get(&scope)
            .into_iter()
            .flatten()
            .filter_map(|f| match f {
                NavFact::MemberRead { receiver, name, .. } => Some((receiver.clone(), name.clone())),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    fn local_type_of(fp: &FileParse, scope: NodeId, name: &str) -> Option<String> {
        fp.nav.local_types.get(&scope).and_then(|t| t.get(name)).cloned()
    }

    const VALUE_POSITIONS: &str = r#"import 'package:x/p.dart' as p;

class Page {
  bool _busy = false;
  bool get _ready => true;
  int _modeValue = 0;
  set _mode(int v) => _modeValue = v;
  String get _title => 'x';
  Holder get _obj => Holder();

  void _save() {}
  void _alt() {}
  void _cancel() {}
  void _make() {}
  void onPressed() {}
  void text() {}
  void addListener() {}
  void named() {}
  void outer() {}

  void wire(Ctrl ctrl, Widget widget) {
    final a = _busy ? null : _save;
    final b = _ready ? (_busy ? _cancel : _alt) : null;
    final c = widget.onTap ?? _make;
    final d = '$_title';
    _mode = 2;
    _obj.x = 1;
    Button(onPressed: null);
    ctrl..text = 'v'..addListener(f);
    const p.T.named();
    outer: for (;;) { break outer; }
  }

  void loops() {
    again: for (;;) { continue again; }
  }

  void calls() {
    _save();
    this._save();
  }

  void member(Holder x) {
    x._alt;
  }

  void shadow() {
    final _ready = false;
    if (_ready) {}
  }
}
"#;

    /// DT.5h: a name in any value position is a reference: a conditional's
    /// branches and condition, a parenthesised nested conditional, `??`, an
    /// initialiser, an interpolation; a getter read is CALLS accessor_read, a
    /// setter write CALLS accessor_write, a method a USES method_ref. A label,
    /// a cascade selector, a constructor name, a statement label, a callee, a
    /// member selector and a shadowing local name nothing.
    #[test]
    fn names_in_value_positions_are_references() {
        let fp = parse_file(VALUE_POSITIONS, "lib/page.dart", "lib::page", repo()).unwrap();
        let q = |m: &str| method_id(&format!("lib::page::Page::{m}"));
        let method_ref = Some("method_ref".to_string());
        let mut want = vec![
            (q("_save"), method_ref.clone()),
            (q("_cancel"), method_ref.clone()),
            (q("_alt"), method_ref.clone()),
            (q("_make"), method_ref),
        ];
        want.sort_by_key(|(to, _)| format!("{to:?}"));
        assert_eq!(uses_edges(&fp, q("wire")), want);
        let mut calls = vec![
            (q("_ready"), Some("accessor_read".to_string())),
            (q("_title"), Some("accessor_read".to_string())),
            (q("_mode"), Some("accessor_write".to_string())),
            // `_obj.x = 1` reads the getter `_obj`; it writes nothing of it.
            (q("_obj"), Some("accessor_read".to_string())),
        ];
        calls.sort_by_key(|(to, rule)| format!("{to:?}{rule:?}"));
        assert_eq!(parser_calls_edges(&fp, q("wire")), calls);
        for m in ["loops", "calls", "member", "shadow"] {
            assert_eq!(uses_edges(&fp, q(m)), vec![], "{m}");
            assert_eq!(parser_calls_edges(&fp, q(m)), vec![], "{m}");
        }
        // The getters and the setter are the property-style METHODs.
        let mut props: Vec<NodeId> = fp.properties.iter().copied().collect();
        props.sort_by_key(|id| format!("{id:?}"));
        let mut want_props = vec![q("_ready"), q("_mode"), q("_title"), q("_obj")];
        want_props.sort_by_key(|id| format!("{id:?}"));
        assert_eq!(props, want_props);
        assert!(!fp.properties.contains(&q("_save")));
    }

    const MEMBER_READS: &str = r#"import 'env.dart';
import 'models.dart';

class Svc {
  bool get ready => true;
}

class Item {
  Map<String, dynamic> toJson() => {};
}

class Box<T> {}

class Factory {}

class Page {
  final Svc _svc = Svc();
  final List<Item> _items = [];
  bool get _ok => true;

  void read(Object local, List<Map<String, dynamic>> raw) {
    final u = Env.baseUrl;
    final f = raw.map(Friend.fromJson);
    final r = _svc.ready;
    final l = local.ready;
    final t = widget.title;
    Env.port();
    _register(Factory.new);
    final make = Box<int>.new;
    final ok = this._ok;
    _items.map((i) => i.toJson());
    _items.map((Item j) => j.toJson());
  }
}
"#;

    /// DT.5h: a member read no call follows is recorded for the graph when
    /// its receiver can be typed (a type name, an owner field), never on an
    /// untyped local, an inherited lowercase name or a call; `T.new` builds
    /// a T; `this._ok` is the getter's CALLS; a one-parameter closure over a
    /// `List<Item>` field types its parameter Item, a typed one records
    /// nothing.
    #[test]
    fn member_reads_and_tear_offs() {
        let fp = parse_file(MEMBER_READS, "lib/page.dart", "lib::page", repo()).unwrap();
        let read = method_id("lib::page::Page::read");
        assert_eq!(
            member_reads_of(&fp, read),
            vec![
                ("Env".to_string(), "baseUrl".to_string()),
                ("Friend".to_string(), "fromJson".to_string()),
                ("this._svc".to_string(), "ready".to_string()),
            ]
        );
        let calls = calls_from(&fp, "lib::page::Page::read");
        assert!(calls.contains(&bare("Factory")), "{calls:?}");
        assert!(calls.contains(&bare("Box")), "{calls:?}");
        assert_eq!(
            parser_calls_edges(&fp, read),
            vec![(method_id("lib::page::Page::_ok"), Some("accessor_read".to_string()))]
        );
        assert_eq!(local_type_of(&fp, read, "i").as_deref(), Some("Item"));
        assert_eq!(local_type_of(&fp, read, "j"), None);
    }

    const ELEMENT_CLOSURES: &str = r#"class Tag {
  String label() => '';
}

class Item {}

class Page {
  final List<Item> _items = [];
  late final Set<Tag>? _tags;
  final List<int> _nums = [];
  final Map<String, Item> _byId = {};

  void loop() {
    for (final i in [1]) {
      if (i.isEven) {}
    }
    _items.map((i) => i.x).toList();
  }

  void tags() {
    _tags?.forEach((t) => t.label());
  }

  void ints() {
    _nums.map((n) => n.isEven);
  }

  void byId() {
    _byId.forEach((k) => k.x);
  }

  void shadowed() {
    final _items = <Item>[];
    _items.map((s) => s.x);
  }

  void nested() {
    _items.map((r) => [1].map((r) => r.isEven));
  }

  void viaThis() {
    this._items.where((w) => w.ok);
  }
}
"#;

    /// DT.5h: the element type reaches a closure parameter only when the
    /// closure owns the name and the field holds a non-builtin element type:
    /// a loop's `i.isEven` beside the closure, `List<int>`, a Map, a local
    /// shadowing the field and a nested rebinding closure type nothing.
    #[test]
    fn element_closures_need_an_exclusive_name() {
        let fp = parse_file(ELEMENT_CLOSURES, "lib/page.dart", "lib::page", repo()).unwrap();
        let q = |m: &str| method_id(&format!("lib::page::Page::{m}"));
        assert_eq!(local_type_of(&fp, q("loop"), "i"), None, "the loop's i");
        assert_eq!(local_type_of(&fp, q("tags"), "t").as_deref(), Some("Tag"));
        assert_eq!(local_type_of(&fp, q("ints"), "n"), None, "List<int>");
        assert_eq!(local_type_of(&fp, q("byId"), "k"), None, "a Map");
        assert_eq!(local_type_of(&fp, q("shadowed"), "s"), None, "a shadowing local");
        assert_eq!(local_type_of(&fp, q("nested"), "r"), None, "a nested rebinding");
        assert_eq!(local_type_of(&fp, q("viaThis"), "w").as_deref(), Some("Item"));
    }

    /// DT.5h: a class field's initialiser is the class's code: its calls
    /// come from the CLASS (a list element, a constructor call, a static
    /// factory), and a field with no initialiser walks nothing.
    #[test]
    fn field_initialisers_are_their_class_calls() {
        let source = r#"class Page {
  static const _tabs = [_Tab('a')];
  final _ctrl = Ctrl();
  late final _r = Repo.open();
  final int plain;

  Page(this.plain);
}
"#;
        let fp = parse_file(source, "lib/page.dart", "lib::page", repo()).unwrap();
        assert_eq!(
            calls_from_class(&fp, "lib::page::Page"),
            sorted(vec![bare("_Tab"), bare("Ctrl"), attr("Repo", "open")])
        );
    }
}
