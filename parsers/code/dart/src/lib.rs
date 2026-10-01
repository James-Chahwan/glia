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

    visit_top(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);
    scan_dart_routes(source, repo, &mut acc);

    // CH.5a: the client base URL inputs of the endpoint fold (CH.5c), as
    // build-time facts on this file's MODULE, and their fired_on marker.
    let base_stats = collect_dio_base_facts(source, root, module_id, &mut acc);
    if let Some(line) = base_stats.marker(file_rel_path) {
        eprintln!("{line}");
    }

    // LA.34 fired_on marker: this file had an unqualified call that Dart's
    // lexical scope decided (a class member or a local binding).
    let s = &acc.bare_calls;
    if s.self_calls + s.local_skip > 0 {
        eprintln!(
            "[dart-calls] self={} bare={} local_skip={} file={file_rel_path}",
            s.self_calls, s.bare, s.local_skip
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
        properties: Default::default(),
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
            let owner = Owner {
                qname: &qname,
                id,
                members: &members,
            };
            visit_members(child, src, file_rel, &owner, repo, acc);
            // LA.23e: declared / constructor-initialised field types, for
            // A6.2a's receiver-type pass.
            collect_dart_field_types(child, src, id, acc);
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
    let owner = Owner {
        qname: &qname,
        id,
        members: &members,
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
    let owner = Owner {
        qname: &ty.qname,
        id: ty.id,
        members: &members,
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
                    continue;
                };
                let id = declare_member(node, &name, owner, repo, file_rel, src, acc);
                acc.members.count(kind, child.kind() == "declaration");
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
                    members: owner.members,
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
            members: owner.members,
            locals: local_names(Some(sig), part, src),
        };
        collect_calls_in(part, src, from, &scope, repo, file_rel, acc);
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
        let owner = Owner {
            qname: &qname,
            id,
            members: &members,
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
        let scope = CallScope {
            members: owner.members,
            locals: HashSet::new(),
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
    let scope = CallScope {
        members: &no_members,
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
/// client ENDPOINTs FROM the STATE_VAR, under library scope (no members, no
/// locals; closures are not entered, as in every body walk).
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
    let scope = CallScope {
        members: &no_members,
        locals: HashSet::new(),
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
/// through here). Nested closures and local functions are not entered.
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
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(
                child.kind(),
                "function_expression" | "class_definition" | "function_definition"
            ) {
                stack.push(child);
            }
        }
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
    }
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
    /// Everything the enclosing class declares: methods, getters, setters and
    /// fields, instance or static ([`class_member_names`]).
    members: &'a HashSet<String>,
    /// The member's parameters and every name bound in its body
    /// ([`local_names`]).
    locals: HashSet<String>,
}

impl CallScope<'_> {
    /// Apply Dart's lexical order to one call site. Only `Bare` changes:
    /// `this.m()` and receiver calls pass through as LA.23e built them.
    fn classify(&self, q: CallQualifier, stats: &mut BareCallStats) -> Option<CallQualifier> {
        let CallQualifier::Bare(name) = q else {
            return Some(q);
        };
        if self.locals.contains(&name) {
            stats.local_skip += 1;
            None
        } else if self.members.contains(&name) {
            stats.self_calls += 1;
            Some(CallQualifier::SelfMethod(name))
        } else {
            stats.bare += 1;
            Some(CallQualifier::Bare(name))
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
    let Some((recv, verb_sel, sl)) = dart_client_call(n, src) else {
        return;
    };
    if !is_http_client_receiver(text_of(recv, src)) {
        return;
    }
    // Verb is the first identifier under the `.verb` selector.
    let Some(method) = first_identifier_text(verb_sel, src) else {
        return;
    };
    let method_l = method.to_ascii_lowercase();
    if !HTTP_VERBS.contains(&method_l.as_str()) {
        return;
    }
    // A3.3: normalise BEFORE the guard, so `dio.post('https://api/users')`
    // becomes `/users` instead of being dropped as "not a path".
    let raw = dart_string_path(sl, src);
    let (path, changed) = normalise_client_path(&raw);
    if !is_dart_request_path(&path) {
        return; // not a request path (full-URL var, non-path first arg, etc.)
    }
    // A11.5: the authority of the pre-normalisation literal, as `host`. Only
    // the host half is read: the path stays A3.3's normaliser's, so no qname
    // moves. `$base/users` has no authority; `https://$host/x` has a
    // placeholder one, which names no service.
    let (host, _) = client_url_split(&raw);
    let pos = n.start_position();
    let ep = ClientEndpoint {
        method: method_l.to_ascii_uppercase(),
        path,
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: Confidence::Strong,
    };
    let extras = HitExtras {
        raw: changed.then_some(raw.as_str()),
        host: host.as_deref(),
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
/// None for anything else: the caller then mints nothing, as before.
fn dart_client_call<'t>(n: TsNode<'t>, src: &[u8]) -> Option<(TsNode<'t>, TsNode<'t>, TsNode<'t>)> {
    let mut c = n.walk();
    let kids: Vec<TsNode> = n.named_children(&mut c).collect();
    if n.kind() == "relational_expression" {
        return dart_generic_misparse(&kids, src);
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
    Some((kids[0], kids[1], sl))
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
/// whole tree (not the body walks: quokka's Dio is built inside a top-level
/// provider closure, which they do not enter). Returns (bases, copies).
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

fn scan_dart_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    // Track emitted routes to dedup — a file may hit the same path twice
    // between the tree walk and the text scan.
    let mut seen = std::collections::HashSet::new();

    // go_router — look for `GoRoute(path:` token.
    let needle = "GoRoute(";
    let mut idx = 0;
    while let Some(pos) = source[idx..].find(needle) {
        let start = idx + pos + needle.len();
        if let Some(path) = extract_kwarg_string(&source[start..], "path") {
            // A3.4: go_router is BROWSER/app navigation, not a server
            // endpoint — mark it so the HTTP route index skips it.
            emit_dart_route("ANY", &path, repo, acc, &mut seen, true);
        }
        idx = start;
    }

    // shelf-style `.get('/...' / .post('/...' / etc. — SERVER routes only.
    // A client HTTP call (`dio.get('/x')`) has the same textual shape but is an
    // outbound ENDPOINT, handled by `try_detect_dart_endpoint`; skip it here (by
    // receiver name) so it is not mis-emitted as a phantom server ROUTE.
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let needle = format!(".{method}(");
        let mut idx = 0;
        while let Some(pos) = source[idx..].find(&needle) {
            let dot_at = idx + pos;
            let after = &source[dot_at + needle.len()..];
            if let Some(path) = first_string_literal_dart(after)
                && path.starts_with('/')
                && !is_http_client_receiver(ident_before(source, dot_at))
            {
                let verb = method.to_ascii_uppercase();
                emit_dart_route(&verb, &path, repo, acc, &mut seen, false);
            }
            idx = dot_at + needle.len();
        }
    }
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
    let trimmed = s.trim_start();
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
    Some(lit.to_string())
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
fn emit_dart_route(
    method: &str,
    path: &str,
    repo: RepoId,
    acc: &mut Acc,
    seen: &mut std::collections::HashSet<(String, String)>,
    nav: bool,
) {
    let route_qname = if nav {
        format!("page:{path}")
    } else {
        format!("{method} {path}")
    };
    let display_name = if nav { path } else { route_qname.as_str() };
    let key = (method.to_string(), route_qname.clone());
    if !seen.insert(key) {
        return;
    }
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_qname);
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
        // The untyped field's initialiser is a class-level expression, not a
        // method body: it emits no call site.
        assert_eq!(fp.calls.len(), 3, "{:?}", fp.calls);
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
        // A closure's body is not the initialiser's call.
        assert_eq!(calls_of(&fp, state_var_id("lib::m::f")), vec![]);
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
    /// closure, which the body walks never enter - and every
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
}
