//! glia-parser-go — tree-sitter Go → code-domain FileParse.
//!
//! Single-file scan. A Go package spans multiple files; `parse_file` emits a
//! Module node per file with the package's NodeId and one Code+Position cell.
//! The graph crate deduplicates the Module by NodeId at build time and the
//! cells from all files stack up on the single Module node (multicellular).
//!
//! Emits:
//! - Module (one per file, collapses on same NodeId at graph build)
//! - Struct / Interface (type declarations)
//! - Function (top-level `func` without receiver)
//! - Method (`func (r T) m()` — qname `pkg::T::m`, parent is the struct)
//!
//! Cross-file references recorded as `ImportStmt` and `CallSite` for the
//! resolver to wire up. All Go imports are `ImportTarget::Module` (Go has no
//! named symbol imports).

use std::collections::HashMap;

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_code_domain::data_entity;
use glia_code_domain::di_stats::{self, DiShape};
use glia_code_domain::endpoint::{
    ClientEndpoint, HitExtras, canonical_http_path, client_url_split, join_path,
    push_client_endpoint_with, route_qname,
};

// ============================================================================
// Public entry point
// ============================================================================

/// Parse one Go source file under a single `go.mod` at the repo root.
///
/// `package_qname` is the repo-local `::`-separated path for the package
/// (e.g. `svc::users` for `<repo>/svc/users/*.go`).
///
/// `module_import_prefix` is the `module` line from the root `go.mod` (e.g.
/// `github.com/foo/bar`) — used to map absolute Go import paths onto
/// repo-local qnames. Pass `""` for a packageless / single-file parse. A repo
/// with nested `go.mod`s goes through [`parse_file_with_modules`] (LA.13).
pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    package_qname: &str,
    module_import_prefix: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let go = GoModules::root_only(module_import_prefix);
    parse_file_with_modules(source, file_rel_path, package_qname, &go, repo)
}

/// [`parse_file`] under every `go.mod` of the repo (LA.13): an import path
/// under any of their module paths maps onto the repo-local qname of that
/// module's root directory ([`GoModules`]), everything else stays a raw
/// external path.
pub fn parse_file_with_modules(
    source: &str,
    file_rel_path: &str,
    package_qname: &str,
    go: &GoModules,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let mut acc = Acc::default();

    // Module node (one per file — collapses at graph build via NodeId dedup).
    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, package_qname);
    acc.module_id = Some(module_id);
    acc.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: file_cells(&root, src, file_rel_path),
    });
    let module_simple = package_qname
        .rsplit("::")
        .next()
        .unwrap_or(package_qname);
    acc.nav
        .record(module_id, module_simple, package_qname, node_kind::MODULE, None);

    // Struct/interface name → NodeId map for this file. Populated in a first
    // pass so method declarations can attach to their receiver struct.
    let mut type_ids: HashMap<String, NodeId> = HashMap::new();

    // First pass: types. Go allows methods to be declared before their
    // receiver struct lexically, so collecting types up front is required.
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "type_declaration" {
            acc.gorm.tagged |= has_gorm_tag(child, src);
            collect_types(
                child,
                src,
                file_rel_path,
                package_qname,
                module_id,
                repo,
                &mut acc,
                &mut type_ids,
            );
        }
    }

    // Second pass: everything else.
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "package_clause" => { /* already known; nothing to emit */ }
            "import_declaration" => {
                collect_imports(child, src, package_qname, go, &mut acc);
            }
            "function_declaration" => {
                visit_function(
                    child,
                    src,
                    file_rel_path,
                    package_qname,
                    module_id,
                    repo,
                    &mut acc,
                );
            }
            "method_declaration" => {
                visit_method(
                    child,
                    src,
                    file_rel_path,
                    package_qname,
                    repo,
                    &type_ids,
                    module_id,
                    &mut acc,
                );
            }
            // Types were collected in the first pass. LA.23c: their field
            // types are read here, where `type_ids` holds every type of this
            // file and `acc.external_pkgs` every import (Go requires imports
            // before every other declaration); both guard `collect_field_types`.
            "type_declaration" => collect_field_types(child, src, &mut acc, &type_ids),
            // glia v5 G19 — package-level state variables: `var X = …`,
            // `var X T = …`, `const X = …`, including grouped blocks. Only
            // top-level declarations (parent == source_file) reach here.
            "var_declaration" | "const_declaration" => {
                collect_state_vars(
                    child,
                    src,
                    file_rel_path,
                    package_qname,
                    module_id,
                    repo,
                    &mut acc,
                );
            }
            _ => {}
        }
    }

    if !acc.func_literal_handlers.is_empty() {
        eprintln!(
            "[go-routes] {} func-literal handlers -> {} HANDLED_BY refs in {file_rel_path}",
            acc.func_literal_handlers.len(),
            acc.func_literal_refs
        );
    }
    if acc.closure_bodies + acc.route_literals_skipped > 0 {
        eprintln!(
            "[go-calls] func-literal bodies={} calls={} route-literal bodies skipped={} in {file_rel_path}",
            acc.closure_bodies, acc.closure_calls, acc.route_literals_skipped
        );
    }
    let forms = &acc.route_forms;
    if forms.registrations > 0 {
        eprintln!(
            "[go-routes] registrations={} positioned={} forms(handle={} any={} match={} pattern={}) in {file_rel_path} nodes={} paths={}",
            forms.registrations,
            forms.positioned,
            forms.handle,
            forms.any,
            forms.matched,
            forms.pattern,
            acc.route_qnames.len(),
            acc.route_paths.len()
        );
    }
    let gorm = &acc.gorm;
    if !gorm.models.is_empty() || !gorm.tables.is_empty() {
        eprintln!(
            "[orm-gorm] models={} table_cells={} tables={} in {file_rel_path}",
            gorm.models.len(),
            gorm.table_cells,
            gorm.tables.len()
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

// ============================================================================
// Accumulator
// ============================================================================

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    calls: Vec<CallSite>,
    refs: Vec<UnresolvedRef>,
    nav: CodeNav,
    /// LB.11a: route ids already recorded in `nav` in this file. A route id is
    /// one (method, path), so a second registration of the same pair (two
    /// routers mounting one path, a repeated `.Methods` verb) records nav once;
    /// its node copy still stacks its POSITION / ROUTE_METHOD cells.
    route_nav_seen: std::collections::HashSet<NodeId>,
    /// Dedup for client-HTTP ENDPOINT nodes (Pattern A) — one node per
    /// (method, path) even if the same endpoint is called twice in a file.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Dedup for GORM ACCESSES_DATA edges — one edge per (enclosing fn,
    /// DATA_ENTITY) even if the same model is queried repeatedly inside the
    /// same function.
    data_access_seen: std::collections::HashSet<(NodeId, NodeId)>,
    /// This file's MODULE id, so the DI detector can stamp `from_module`
    /// without threading it through `collect_calls_in` (the TypeScript
    /// parser's `Acc.file_rel` precedent). Set at the top of `parse_file`.
    module_id: Option<NodeId>,
    /// Local package name → DI container, from this file's imports. An import
    /// alias is followed. Go requires imports before every other declaration,
    /// so this is filled before any function or var is visited.
    di_containers: HashMap<String, DiContainer>,
    /// One INJECTS ref per (registering node, provider) per file.
    di_seen: std::collections::HashSet<(NodeId, String)>,
    /// LA.18d: local names bound by this file's imports that lie OUTSIDE the
    /// go.mod module (stdlib + third-party). A func-literal route handler's
    /// `pkg.Fn(..)` through one of them is never an in-repo callee, and the
    /// graph's HANDLED_BY fallback (`unique_global_function` /
    /// `unique_global_method`) would otherwise bind `log.Println` to any
    /// uniquely named repo `Println`. Filled with `di_containers`, so it is
    /// complete before any route is visited. Only looked up, never iterated.
    external_pkgs: std::collections::HashSet<String>,
    /// LA.18d: start byte of every func-literal route handler seen in this
    /// file. `len()` is the marker's handler count.
    func_literal_handlers: std::collections::HashSet<usize>,
    /// LA.18d / LB.11a: (literal start byte, route id) pairs already expanded.
    /// A Gorilla `.Methods("GET", "POST")` chain re-enters
    /// `emit_route_from_call` once per verb with the same literal; each verb is
    /// its own route node (LB.11a), so each gets the literal's callee refs,
    /// and a repeat of one (literal, route) pair pushes nothing.
    func_literal_expanded: std::collections::HashSet<(usize, NodeId)>,
    /// LA.18d: HANDLED_BY refs pushed from func-literal handlers in this file.
    func_literal_refs: usize,
    /// CA.1: non-route func-literal bodies walked for their enclosing
    /// function's calls, the CallSites pushed from them, and the route-handler
    /// literals left to LA.18d — the `[go-calls] func-literal` marker's counters.
    closure_bodies: usize,
    closure_calls: usize,
    route_literals_skipped: usize,
    /// CA.2a: the type parameters of the callable being visited (its own
    /// `[T any]`, or a generic receiver's `Collection[T]`), so a receiver
    /// fact never types a local or result by one. Set and cleared by
    /// `visit_function` / `visit_method`.
    type_params: Vec<String>,
    /// LA.32a: route registrations emitted in this file, the POSITION cells
    /// pushed for them, and the method-bearing forms among them — the
    /// `[go-routes] registrations=` marker's counters.
    route_forms: RouteFormCounts,
    /// LB.11a: the distinct `<METHOD> <path>` route qnames and the distinct
    /// paths among them emitted in this file — the marker's `nodes=` /
    /// `paths=`. Only counted, never iterated into output.
    route_qnames: std::collections::BTreeSet<String>,
    route_paths: std::collections::BTreeSet<String>,
    /// A13.12: this file's GORM evidence and the `[orm-gorm]` marker counters.
    gorm: GormFile,
}

/// A13.12: what one Go file shows of GORM. The two evidence flags gate the
/// detectors, because `.Model(` / `.Table(` / `TableName()` are generic names
/// outside a GORM file; the sets and count feed the `[orm-gorm]` marker.
#[derive(Default)]
struct GormFile {
    /// The file imports `gorm.io/gorm` or `github.com/jinzhu/gorm`. Gates the
    /// query sites and the `TableName()` declaration. Set by `record_import`,
    /// which runs before any function body is walked (Go requires imports
    /// before every other declaration).
    import: bool,
    /// A top-level struct in this file carries a `gorm:"…"` field tag. A pure
    /// model file often imports nothing, so this also gates the `TableName()`
    /// declaration (never a query site). Set in the first (types) pass.
    tagged: bool,
    /// Distinct model names this file keys an entity on: query-site models
    /// plus `TableName()` receivers. Only counted, never iterated.
    models: std::collections::HashSet<String>,
    /// `TableName()` table cells emitted.
    table_cells: usize,
    /// Distinct `.Table("x")` literals. Only counted, never iterated.
    tables: std::collections::HashSet<String>,
}

/// LA.32a: per-file route registration counters. `registrations` counts
/// ROUTE_METHOD cells pushed (a Gorilla `.Methods("GET", "POST")` chain or a
/// `Match([]string{"GET", "POST"}, ..)` is two); `positioned` counts the
/// POSITION cells pushed with them — equal by construction, the token proves
/// the POSITION path ran. The form counters count registration CALLS.
#[derive(Default)]
struct RouteFormCounts {
    registrations: usize,
    positioned: usize,
    /// `Handle` / `Add` / `Method` / `MethodFunc` with a method literal at arg #0.
    handle: usize,
    /// `Any("/path", h)`.
    any: usize,
    /// `Match([]string{..}, "/path", h)`.
    matched: usize,
    /// Go 1.22 ServeMux `"<VERB> /path"` patterns.
    pattern: usize,
}

// ============================================================================
// Type declarations (struct + interface)
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn collect_types(
    type_decl: TsNode,
    src: &[u8],
    file_rel: &str,
    package_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
    type_ids: &mut HashMap<String, NodeId>,
) {
    let mut cursor = type_decl.walk();
    for spec in type_decl.named_children(&mut cursor) {
        if spec.kind() != "type_spec" {
            continue;
        }
        let Some(name_node) = spec.child_by_field_name("name") else {
            continue;
        };
        let name = text_of(name_node, src).to_string();
        let qname = format!("{package_qname}::{name}");

        let Some(type_node) = spec.child_by_field_name("type") else {
            continue;
        };

        let kind = match type_node.kind() {
            "struct_type" => node_kind::STRUCT,
            "interface_type" => node_kind::INTERFACE,
            // Type aliases (`type Foo = Bar`) and non-struct/non-interface
            // types skipped for v0.4.3b.
            _ => continue,
        };

        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: entity_cells(spec, src, file_rel),
        });
        acc.nav.record(id, &name, &qname, kind, Some(module_id));
        acc.edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        if kind == node_kind::INTERFACE {
            collect_interface_elems(type_node, src, file_rel, &qname, id, module_id, repo, acc);
        }
        type_ids.insert(name, id);
    }
}

/// LD.7b: the body of one `interface_type`. tree-sitter-go 0.25 names its
/// elements `method_elem` (fields `name` / `parameters` / `result`) and
/// `type_elem` (one child per union term).
///
/// * A `method_elem` becomes a METHOD node `<interface qname>::<name>` under
///   the interface: nav parent, an interface -> method DEFINES edge, and the
///   CODE / POSITION / DOC cells of [`entity_cells`]. The graph files it in
///   `interface_methods` (A6.6), never `class_methods`, so the Go HANDLED_BY
///   fallback (`unique_global_method`) never sees it.
/// * A `type_elem` of exactly one named type is an embedded interface: an
///   INHERITS_FROM ref out of the interface ([`embedded_iface_qualifier`]).
///   Unions and `~T` terms are constraint type sets, not embedded method
///   sets, and emit nothing.
///
/// The graph's implicit-IMPLEMENTS pass reads the METHOD children as the
/// interface's own method set and follows the INHERITS_FROM edges for the
/// embedded ones.
#[allow(clippy::too_many_arguments)]
fn collect_interface_elems(
    iface: TsNode,
    src: &[u8],
    file_rel: &str,
    iface_qname: &str,
    iface_id: NodeId,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let type_parameters = iface
        .parent()
        .filter(|spec| spec.kind() == "type_spec")
        .and_then(|spec| spec.child_by_field_name("type_parameters"));
    // CA.3a: an interface with type parameters records no signature.
    let generic = type_parameters.is_some();
    let iface_params = type_param_names(type_parameters, src);
    let mut cursor = iface.walk();
    for elem in iface.named_children(&mut cursor) {
        match elem.kind() {
            "method_elem" => {
                let Some(name_node) = elem.child_by_field_name("name") else {
                    continue;
                };
                let name = text_of(name_node, src);
                if name.is_empty() {
                    continue;
                }
                let qname = format!("{iface_qname}::{name}");
                let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);
                acc.nodes.push(Node {
                    id,
                    repo,
                    confidence: Confidence::Strong,
                    cells: entity_cells(elem, src, file_rel),
                });
                acc.nav.record(id, name, &qname, node_kind::METHOD, Some(iface_id));
                acc.edges.push(Edge {
                    from: iface_id,
                    to: id,
                    category: edge_category::DEFINES,
                    confidence: Confidence::Strong,
                    cells: Vec::new(),
                });
                // CA.2a: the method's result type; the interface's own type
                // parameters (`type Repo[T any] interface`) type nothing.
                if let Some(ty) =
                    result_type(elem.child_by_field_name("result"), src, &iface_params)
                {
                    acc.nav.record_return_type(id, &ty);
                }
                // CA.3a: the element's normalised signature, the same text
                // an implementation in another package records.
                let sig = (!generic)
                    .then(|| elem.child_by_field_name("parameters"))
                    .flatten()
                    .and_then(|p| go_signature(p, elem.child_by_field_name("result"), src));
                if let Some(sig) = sig {
                    acc.nav.record_method_sig(id, &sig);
                }
            }
            "type_elem" => {
                let mut tc = elem.walk();
                let mut terms = elem.named_children(&mut tc);
                let (Some(term), None) = (terms.next(), terms.next()) else {
                    continue;
                };
                if let Some(qualifier) = embedded_iface_qualifier(term, src) {
                    acc.refs.push(UnresolvedRef {
                        from: iface_id,
                        from_module: module_id,
                        qualifier,
                        category: edge_category::INHERITS_FROM,
                        line: line_at(term),
                    });
                }
            }
            _ => {}
        }
    }
}

/// How an embedded interface term is named: `R` and `R[T]` -> `Bare("R")`,
/// `pkg.R` and `pkg.R[T]` -> `Attribute { base: "pkg", name: "R" }`. `any`
/// embeds no method and gives `None`, as does every other term shape (`~T`,
/// pointer / slice / map / func literals), none of which is an interface.
///
/// An embed the graph cannot bind (another module's interface, the
/// predeclared `error` and `comparable`) stays in `unresolved_refs`, which is
/// how the graph's implicit-IMPLEMENTS pass knows the interface's method set
/// is not fully known.
fn embedded_iface_qualifier(term: TsNode, src: &[u8]) -> Option<CallQualifier> {
    let term = if term.kind() == "generic_type" { term.child_by_field_name("type")? } else { term };
    match term.kind() {
        "type_identifier" => {
            let name = text_of(term, src);
            (!name.is_empty() && name != "any").then(|| CallQualifier::Bare(name.to_string()))
        }
        "qualified_type" => {
            let base = text_of(term.child_by_field_name("package")?, src);
            let name = text_of(term.child_by_field_name("name")?, src);
            (!base.is_empty() && !name.is_empty()).then(|| CallQualifier::Attribute {
                base: base.to_string(),
                name: name.to_string(),
            })
        }
        _ => None,
    }
}

/// LA.23c: a struct's field declarations into `CodeNav::field_types` (A6.2a's
/// carrier), so the graph's receiver-type pass binds `s.repo.Find()` (which
/// `classify_call` normalises to receiver `self.repo`) to `UserRepo::Find`.
///
/// Runs after the types pass, so `type_ids` holds every type this file
/// declares. `repo *UserRepo` records `repo -> UserRepo`; `a, b *T` records
/// both names; an embedded field records under its type's own name
/// (`*UserRepo` / `pkg.UserRepo` -> field `UserRepo`). Interface-typed fields
/// are recorded too: until an interface fallback exists the lookup finds no
/// method table on an INTERFACE and stays unresolved. Types
/// [`go_type_name`] cannot name (slices, maps, channels, funcs, generics)
/// record nothing.
///
/// A qualified `pkg.T` types nothing when the graph's bare-name lookup would
/// land on the wrong `T`:
/// * `pkg` is an import outside the repo's go.mod modules (`c net.Conn`): no
///   repo type is `net.Conn`, but the unique-name fallback would bind a repo's
///   own `Conn` (LA.18d's `external_pkgs`; with no go.mod every import is
///   external, so every qualified field is skipped);
/// * `T` is also a type of this file (the wrapper shape `type Logger struct
///   { l *zap.Logger }`): the module lookup would turn every delegating call
///   into a self-call — the same guard as the Python parser's (LA.23a).
fn collect_field_types(
    type_decl: TsNode,
    src: &[u8],
    acc: &mut Acc,
    type_ids: &HashMap<String, NodeId>,
) {
    let mut cursor = type_decl.walk();
    for spec in type_decl.named_children(&mut cursor) {
        if spec.kind() != "type_spec" {
            continue;
        }
        let (Some(name_node), Some(type_node)) =
            (spec.child_by_field_name("name"), spec.child_by_field_name("type"))
        else {
            continue;
        };
        if type_node.kind() != "struct_type" {
            continue;
        }
        let Some(&struct_id) = type_ids.get(text_of(name_node, src)) else {
            continue;
        };
        let mut sc = type_node.walk();
        let Some(list) = type_node
            .named_children(&mut sc)
            .find(|c| c.kind() == "field_declaration_list")
        else {
            continue;
        };
        let mut lc = list.walk();
        for decl in list.named_children(&mut lc) {
            if decl.kind() != "field_declaration" {
                continue;
            }
            for (field, ty) in field_decl_types(decl, src, type_ids, &acc.external_pkgs) {
                acc.nav.record_field_type(struct_id, &field, &ty);
            }
        }
    }
}

/// `(field name, declared type name)` pairs of one `field_declaration`, with
/// the qualified-type guards of [`collect_field_types`] applied.
fn field_decl_types(
    decl: TsNode,
    src: &[u8],
    type_ids: &HashMap<String, NodeId>,
    external_pkgs: &std::collections::HashSet<String>,
) -> Vec<(String, String)> {
    let Some(type_node) = decl.child_by_field_name("type") else {
        return Vec::new();
    };
    let Some(ty) = go_type_name(type_node, src) else {
        return Vec::new();
    };
    // CA.2a: a generic instantiation's guards read its base, so an external
    // `atomic.Pointer[T]` records nothing, as `atomic.Value` never did.
    let Some(inner) = generic_base(unwrap_pointer(type_node)) else {
        return Vec::new();
    };
    if inner.kind() == "qualified_type" {
        let external = inner
            .child_by_field_name("package")
            .is_some_and(|p| external_pkgs.contains(text_of(p, src)));
        if external || type_ids.contains_key(&ty) {
            return Vec::new();
        }
    }
    let mut nc = decl.walk();
    let names: Vec<String> = decl
        .children_by_field_name("name", &mut nc)
        .map(|n| text_of(n, src).to_string())
        .collect();
    if names.is_empty() {
        // Embedded field: its name is the type's own name.
        return vec![(ty.clone(), ty)];
    }
    names.into_iter().map(|n| (n, ty.clone())).collect()
}

/// The bare type name a field's declared type binds by: `*T` -> `T`,
/// `pkg.T` -> `T`, `T` -> `T`, and a generic instantiation by its base
/// (`*Collection[User]` -> `Collection`, `pkg.Page[T]` -> `Page`, CA.2a).
/// Slices, arrays, maps, channels, func types and anonymous struct /
/// interface types -> `None`: a call through such a field has no single
/// method table to bind against.
fn go_type_name(type_node: TsNode, src: &[u8]) -> Option<String> {
    let inner = generic_base(unwrap_pointer(type_node))?;
    let name = match inner.kind() {
        "type_identifier" => text_of(inner, src),
        "qualified_type" => text_of(inner.child_by_field_name("name")?, src),
        _ => return None,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// A generic instantiation's base type (`Collection[T]` -> `Collection`,
/// `pkg.Page[T]` -> `pkg.Page`); any other node is its own base. `None` for
/// a `generic_type` without a base (a parse error).
fn generic_base(node: TsNode) -> Option<TsNode> {
    if node.kind() == "generic_type" {
        node.child_by_field_name("type")
    } else {
        Some(node)
    }
}

/// Strip every `*` off a pointer type: `**T` -> `T`.
fn unwrap_pointer(mut node: TsNode) -> TsNode {
    while node.kind() == "pointer_type" {
        let mut c = node.walk();
        let Some(inner) = node.named_children(&mut c).next() else {
            break;
        };
        node = inner;
    }
    node
}

// ============================================================================
// State variables (glia v5 G19)
// ============================================================================
//
// Package-level `var`/`const` declarations. tree-sitter-go wraps each in a
// `var_declaration` / `const_declaration` containing one or more `var_spec` /
// `const_spec` children (grouped `var ( … )` blocks yield several specs). Each
// spec may declare multiple names (`var a, b = 1, 2`); we emit one STATE_VAR
// per name. Qname is `<module>::<Name>`. The module DEFINES each var.
//
// Noise gate: a spec with no leading doc whose initialiser is a single literal
// primitive (number / string / bool / nil / iota) is skipped — those add bulk
// without conveying structure. Documented vars and non-trivial initialisers
// (calls, composites, multiple names) are kept.

#[allow(clippy::too_many_arguments)]
fn collect_state_vars(
    decl: TsNode,
    src: &[u8],
    file_rel: &str,
    package_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = decl.walk();
    for child in decl.named_children(&mut cursor) {
        match child.kind() {
            "var_spec" | "const_spec" => {
                emit_state_var_spec(child, src, file_rel, package_qname, module_id, repo, acc);
            }
            // Grouped `var ( … )` blocks wrap their specs in a var_spec_list.
            // (Grouped `const ( … )` puts const_spec directly under the decl.)
            "var_spec_list" => {
                let mut sc = child.walk();
                for spec in child.named_children(&mut sc) {
                    if spec.kind() == "var_spec" {
                        emit_state_var_spec(
                            spec, src, file_rel, package_qname, module_id, repo, acc,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_state_var_spec(
    spec: TsNode,
    src: &[u8],
    file_rel: &str,
    package_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // Names: the `name` field is multiple (`var a, b = …`). They precede the
    // optional type and the value, so collect leading identifier children and
    // stop at the first non-identifier (the type or `=` value).
    let mut names: Vec<String> = Vec::new();
    let mut nc = spec.walk();
    for child in spec.named_children(&mut nc) {
        if child.kind() == "identifier" {
            names.push(text_of(child, src).to_string());
        } else {
            break;
        }
    }
    if names.is_empty() {
        return;
    }

    // CA.2a: a package-level var's type, on the file MODULE scope, before the
    // noise gate (`var repo *repositories.X` has no initialiser).
    if spec.kind() == "var_spec" {
        record_package_vars(spec, &names, src, module_id, acc);
    }

    if state_var_is_noise(spec, src) {
        return;
    }

    // Initialisers, paired with names by position (`var a, b = x, y`).
    let values: Vec<TsNode> = match spec.child_by_field_name("value") {
        Some(v) => {
            let mut vc = v.walk();
            v.named_children(&mut vc).collect()
        }
        None => Vec::new(),
    };

    for (i, name) in names.into_iter().enumerate() {
        let qname = format!("{package_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STATE_VAR, &qname);
        acc.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: entity_cells(spec, src, file_rel),
        });
        acc.nav
            .record(id, &name, &qname, node_kind::STATE_VAR, Some(module_id));
        acc.edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        // A7.6: `var ProviderSet = wire.NewSet(NewA, NewB)` registers its
        // providers from the var, which a `wire.Build(ProviderSet)` then names.
        if let Some(value) = values.get(i) {
            collect_provider_sets_in(*value, src, id, acc);
        }
    }
}

/// Noise gate: keep documented specs and non-trivial initialisers; skip a spec
/// whose only value is a single literal primitive and which carries no doc.
fn state_var_is_noise(spec: TsNode, src: &[u8]) -> bool {
    if glia_doc::leading_doc(&spec, src).is_some() {
        return false;
    }
    // Values live under the `value` field — an `expression_list`. A trivial
    // spec has exactly one literal-primitive value (or none, e.g. iota const).
    let Some(values) = spec.child_by_field_name("value") else {
        // No initialiser (`var x int`, or a const carrying only iota) — trivial.
        return true;
    };
    let mut vc = values.walk();
    let value_nodes: Vec<TsNode> = values.named_children(&mut vc).collect();
    if value_nodes.len() != 1 {
        // Multiple initialisers or composite — non-trivial, keep.
        return false;
    }
    is_literal_primitive(value_nodes[0])
}

/// True for a single literal primitive: number, string, bool, nil, iota.
fn is_literal_primitive(node: TsNode) -> bool {
    matches!(
        node.kind(),
        "int_literal"
            | "float_literal"
            | "imaginary_literal"
            | "rune_literal"
            | "interpreted_string_literal"
            | "raw_string_literal"
            | "true"
            | "false"
            | "nil"
            | "iota"
    )
}

// ============================================================================
// Function + method visitors
// ============================================================================

fn visit_function(
    decl: TsNode,
    src: &[u8],
    file_rel: &str,
    package_qname: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name_node) = decl.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src).to_string();
    let qname = format!("{package_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(decl, src, file_rel),
    });
    acc.nav
        .record(id, &name, &qname, node_kind::FUNCTION, Some(module_id));
    acc.edges.push(Edge {
        from: module_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });

    // CA.2a: result type and parameters.
    acc.type_params = type_param_names(decl.child_by_field_name("type_parameters"), src);
    if let Some(ty) = result_type(decl.child_by_field_name("result"), src, &acc.type_params) {
        acc.nav.record_return_type(id, &ty);
    }
    record_params(decl.child_by_field_name("parameters"), src, id, acc);

    if let Some(body) = decl.child_by_field_name("body") {
        let mut closures = Vec::new();
        collect_calls_in(body, src, id, None, repo, file_rel, acc, &mut closures);
        collect_routes_in(body, src, file_rel, module_id, repo, acc);
        collect_closure_calls(closures, src, id, None, repo, file_rel, acc);
    }
    acc.type_params.clear();
}

#[allow(clippy::too_many_arguments)]
fn visit_method(
    decl: TsNode,
    src: &[u8],
    file_rel: &str,
    package_qname: &str,
    repo: RepoId,
    type_ids: &HashMap<String, NodeId>,
    module_id: NodeId,
    acc: &mut Acc,
) {
    let Some(name_node) = decl.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, src).to_string();

    // Receiver: `(r *User)` — we want the receiver type name (User) and the
    // bound variable name (r). The type can be a pointer or bare identifier.
    let Some(receiver) = decl.child_by_field_name("receiver") else {
        return;
    };
    let (receiver_var, receiver_type) = parse_receiver(receiver, src);
    let Some(receiver_type) = receiver_type else {
        return;
    };

    // Parent: the struct this method belongs to. If we haven't seen it (could
    // be declared in another file of the same package), we still attach to the
    // module — the graph crate will rewire under the struct at build time via
    // class_methods lookup by qname.
    let parent_id = type_ids.get(&receiver_type).copied().unwrap_or(module_id);

    let qname = format!("{package_qname}::{receiver_type}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::METHOD, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(decl, src, file_rel),
    });
    acc.nav
        .record(id, &name, &qname, node_kind::METHOD, Some(parent_id));
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });

    if name == "TableName" {
        try_emit_gorm_table_name(decl, &receiver_type, src, repo, acc);
    }

    // CA.2a: result type and parameters (the receiver is not a local:
    // SelfMethod and `self.` cover it).
    acc.type_params = receiver_type_params(receiver, src);
    if let Some(ty) = result_type(decl.child_by_field_name("result"), src, &acc.type_params) {
        acc.nav.record_return_type(id, &ty);
    }
    // CA.3a: the normalised signature, unless the receiver is generic (its
    // type parameters stand for whatever an instantiation picks).
    if !receiver_is_generic(receiver) {
        let sig = decl
            .child_by_field_name("parameters")
            .and_then(|p| go_signature(p, decl.child_by_field_name("result"), src));
        if let Some(sig) = sig {
            acc.nav.record_method_sig(id, &sig);
        }
    }
    record_params(decl.child_by_field_name("parameters"), src, id, acc);

    if let Some(body) = decl.child_by_field_name("body") {
        let receiver_var = receiver_var.as_deref();
        let mut closures = Vec::new();
        collect_calls_in(
            body,
            src,
            id,
            receiver_var,
            repo,
            file_rel,
            acc,
            &mut closures,
        );
        collect_routes_in(body, src, file_rel, module_id, repo, acc);
        collect_closure_calls(closures, src, id, receiver_var, repo, file_rel, acc);
    }
    acc.type_params.clear();
}

/// Pull the receiver variable name and type name out of a `parameter_list`
/// like `(r *User)`. Returns `(Some("r"), Some("User"))` — either may be None
/// for unusual receiver forms (e.g. bare `_` receiver).
fn parse_receiver(receiver: TsNode, src: &[u8]) -> (Option<String>, Option<String>) {
    // Receiver is a parameter_list with one parameter_declaration.
    let mut cursor = receiver.walk();
    for param in receiver.named_children(&mut cursor) {
        if param.kind() != "parameter_declaration" {
            continue;
        }
        let name = param
            .child_by_field_name("name")
            .map(|n| text_of(n, src).to_string());
        let type_node = param.child_by_field_name("type");
        let type_name = type_node.map(|t| extract_type_name(t, src));
        return (name, type_name);
    }
    (None, None)
}

/// Extract the bare type name from a type expression. Strips pointer (`*T`),
/// generic args (`T[U]`), package qualifier (`pkg.T`) down to just `T`.
fn extract_type_name(type_node: TsNode, src: &[u8]) -> String {
    match type_node.kind() {
        "pointer_type" => {
            let mut cursor = type_node.walk();
            if let Some(c) = type_node.named_children(&mut cursor).next() {
                return extract_type_name(c, src);
            }
            text_of(type_node, src).trim_start_matches('*').to_string()
        }
        "generic_type" => {
            if let Some(inner) = type_node.child_by_field_name("type") {
                extract_type_name(inner, src)
            } else {
                text_of(type_node, src).split('[').next().unwrap_or("").to_string()
            }
        }
        "qualified_type" => {
            // pkg.Name → take just the name side.
            if let Some(name) = type_node.child_by_field_name("name") {
                text_of(name, src).to_string()
            } else {
                text_of(type_node, src).rsplit('.').next().unwrap_or("").to_string()
            }
        }
        _ => text_of(type_node, src).to_string(),
    }
}

// ============================================================================
// Go module map (LA.13)
// ============================================================================

/// Every `go.mod` of a repo, as the Go parser maps import paths through them
/// (LA.13). A repo can hold several modules (a monorepo of services, a
/// `go.work` / `replace` layout, a nested module under `tools/`), and none
/// need sit at the repo root: an import is in-repo when it lies under ANY of
/// their module paths, and its repo-local qname is that module's root
/// directory joined with the path below the module
/// (`example.com/svc/internal/store` under `svc/go.mod` ->
/// `svc::internal::store`).
///
/// Built from `(root dir, module path)` pairs ([`GoModules::from_entries`]) and
/// kept sorted by root dir, so a lookup and [`GoModules::context_key`] never
/// depend on discovery order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoModules {
    mods: Vec<GoModule>,
}

/// One `go.mod`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GoModule {
    /// Repo-relative dir holding the go.mod, `/`-separated, `""` at the root.
    root_dir: String,
    /// The same dir as a qname prefix (`services::api`), `""` at the root.
    root_qname: String,
    /// Its `module` line (`example.com/svc`).
    module_path: String,
}

impl GoModules {
    /// One `go.mod` at the repo root with module path `prefix` (the pre-LA.13
    /// model); `""` is no module at all.
    pub fn root_only(prefix: &str) -> Self {
        Self::from_entries(vec![(String::new(), prefix.to_string())])
    }

    /// From `(root dir, module path)` pairs: the root dir repo-relative with
    /// `/` separators (`""` or `.` for the repo root). Entries with an empty
    /// module path are dropped; the rest are sorted by root dir, then module
    /// path, and exact duplicates removed.
    pub fn from_entries(entries: Vec<(String, String)>) -> Self {
        let mut mods: Vec<GoModule> = entries
            .into_iter()
            .filter_map(|(dir, module)| {
                let module = module.trim();
                if module.is_empty() {
                    return None;
                }
                let dir = dir
                    .split(['/', '\\'])
                    .filter(|s| !s.is_empty() && *s != ".")
                    .collect::<Vec<_>>()
                    .join("/");
                Some(GoModule {
                    root_qname: dir.replace('/', "::"),
                    root_dir: dir,
                    module_path: module.to_string(),
                })
            })
            .collect();
        mods.sort_by(|a, b| {
            a.root_dir
                .cmp(&b.root_dir)
                .then_with(|| a.module_path.cmp(&b.module_path))
        });
        mods.dedup();
        Self { mods }
    }

    /// The module set as one string, `<root dir>=<module path>` joined by `;`
    /// in root order (`""` when empty). A parse cache keys on it: any go.mod
    /// added, removed, moved or re-pathed changes how unchanged `.go` files
    /// map their imports.
    pub fn context_key(&self) -> String {
        self.mods
            .iter()
            .map(|m| format!("{}={}", m.root_dir, m.module_path))
            .collect::<Vec<_>>()
            .join(";")
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    pub fn len(&self) -> usize {
        self.mods.len()
    }

    /// `(root dir, module path)` per module, in root order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.mods
            .iter()
            .map(|m| (m.root_dir.as_str(), m.module_path.as_str()))
    }

    /// The repo-local qname an import of `import_path` from the file whose
    /// package qname is `package_qname` names, or `None` for an import no
    /// module of the repo holds (stdlib, third-party).
    ///
    /// A module holds a path equal to its module path or below it at a `/`
    /// boundary, so module `example.com/svc` never claims
    /// `example.com/svc-b/client`. Of the modules holding the import, the
    /// longest module path wins: Go excludes a nested module's directory from
    /// the module around it, so `example.com/root/api/v2/users` is package
    /// `users` of the module `example.com/root/api/v2` rooted at `api/`, never
    /// the dir `api/v2/users` of `example.com/root`. Two go.mods declaring the
    /// same module path (a vendored copy, an example dir) are decided by the
    /// importing file's OWN module, the nearest go.mod enclosing it, else by
    /// the first root dir: never by discovery order.
    ///
    /// `Some("")` is the repo-root module's own path, which names no package
    /// directory.
    fn map_import(&self, package_qname: &str, import_path: &str) -> Option<String> {
        let rest_of = |m: &GoModule| -> Option<String> {
            let rest = if import_path == m.module_path {
                ""
            } else {
                import_path
                    .strip_prefix(m.module_path.as_str())?
                    .strip_prefix('/')?
            };
            Some(
                rest.split('/')
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("::"),
            )
        };
        // (module, path below it, does it enclose the importer)
        let mut best: Option<(&GoModule, String, bool)> = None;
        for m in &self.mods {
            let Some(rel) = rest_of(m) else { continue };
            let own = encloses(&m.root_qname, package_qname);
            let better = match &best {
                None => true,
                Some((b, _, b_own)) => {
                    m.module_path.len() > b.module_path.len()
                        || (m.module_path.len() == b.module_path.len()
                            && own
                            && (!b_own || m.root_qname.len() > b.root_qname.len()))
                }
            };
            if better {
                best = Some((m, rel, own));
            }
        }
        let (m, rel, _) = best?;
        Some(match (m.root_qname.is_empty(), rel.is_empty()) {
            (true, _) => rel,
            (false, true) => m.root_qname.clone(),
            (false, false) => format!("{}::{rel}", m.root_qname),
        })
    }
}

/// Is the dir qname `root` (`""` = the repo root) `qname` itself or one of its
/// `::`-segment ancestors?
fn encloses(root: &str, qname: &str) -> bool {
    root.is_empty()
        || qname
            .strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

// ============================================================================
// Import collection
// ============================================================================

fn collect_imports(
    decl: TsNode,
    src: &[u8],
    package_qname: &str,
    go: &GoModules,
    acc: &mut Acc,
) {
    // import_declaration may wrap an import_spec_list or a single import_spec.
    let mut cursor = decl.walk();
    for child in decl.named_children(&mut cursor) {
        match child.kind() {
            "import_spec" => {
                record_import(child, src, package_qname, go, acc);
            }
            "import_spec_list" => {
                let mut inner = child.walk();
                for spec in child.named_children(&mut inner) {
                    if spec.kind() == "import_spec" {
                        record_import(spec, src, package_qname, go, acc);
                    }
                }
            }
            _ => {}
        }
    }
}

fn record_import(
    spec: TsNode,
    src: &[u8],
    package_qname: &str,
    go: &GoModules,
    acc: &mut Acc,
) {
    // import_spec children: optional name (alias) + path (interpreted_string_literal).
    let alias = spec
        .child_by_field_name("name")
        .map(|n| text_of(n, src).to_string());
    let Some(path_node) = spec.child_by_field_name("path") else {
        return;
    };
    // Strip the surrounding quotes.
    let raw = text_of(path_node, src);
    let path_str = raw.trim_matches('"').to_string();

    // A13.12: a GORM import (not a blank one, which binds no `*gorm.DB`) turns
    // on the GORM query-site and `TableName()` detectors for this file.
    if matches!(path_str.as_str(), "gorm.io/gorm" | "github.com/jinzhu/gorm")
        && alias.as_deref() != Some("_")
    {
        acc.gorm.import = true;
    }

    // A7.6: remember which local name binds a DI container package. Blank and
    // dot imports bind no selector base, so they are skipped.
    if let Some((container, pkg_name)) = DiContainer::from_import_path(&path_str) {
        let local = alias.as_deref().unwrap_or(pkg_name);
        if local != "_" && local != "." {
            acc.di_containers.insert(local.to_string(), container);
        }
    }

    // LA.13: the repo-local qname of an import under any of the repo's go.mod
    // modules, `None` for a stdlib / third-party path.
    let local = go.map_import(package_qname, &path_str);

    // LA.18d: remember the local name of every import outside the repo's
    // go.mod modules, so a func-literal handler's `pkg.Fn(..)` through it is
    // not mistaken for an in-repo callee. With no go.mod every import is
    // external. Blank and dot imports bind no selector base.
    if local.is_none() {
        match alias.as_deref() {
            Some("_") | Some(".") => {}
            Some(local) => {
                acc.external_pkgs.insert(local.to_string());
            }
            None => {
                for local in import_local_names(&path_str) {
                    acc.external_pkgs.insert(local.to_string());
                }
            }
        }
    }

    let qname = match local {
        // The repo-root module's own path (`import "github.com/foo/bar"` with
        // module == "github.com/foo/bar" at the root): no package dir to
        // name; ignore, as before LA.13. A nested module's path names its
        // root dir, which is a package.
        Some(q) if q.is_empty() => return,
        Some(q) => q,
        // External import (stdlib or third-party). Keep the raw path for now;
        // cross-repo resolution is a v0.4.4 concern.
        None => path_str.replace('/', "::"),
    };

    acc.imports.push(ImportStmt {
        from_module: package_qname.to_string(),
        target: ImportTarget::Module {
            path: qname,
            alias,
        },
        line: line_at(spec),
    });
}

/// LA.18d: the name(s) an un-aliased Go import can bind. Go binds the imported
/// package's declared name, which the path only suggests: normally its last
/// segment, but a major-version suffix (`github.com/go-chi/chi/v5`) names the
/// segment before it, gopkg.in drops a `.vN` (`gopkg.in/yaml.v3` → `yaml`),
/// and a `go-` prefix / `-go` suffix is conventionally not part of the name
/// (`go-sqlite3` → `sqlite3`, `stripe-go` → `stripe`). Every candidate is
/// returned: the set is only used to SKIP calls, and none of the extras is a
/// name an in-repo identifier could plausibly shadow.
fn import_local_names(path: &str) -> Vec<&str> {
    fn is_major_version(s: &str) -> bool {
        s.len() >= 2 && s.starts_with('v') && s[1..].bytes().all(|b| b.is_ascii_digit())
    }
    let mut segments = path.rsplit('/');
    let Some(mut last) = segments.next() else {
        return Vec::new();
    };
    if is_major_version(last)
        && let Some(prev) = segments.next()
    {
        last = prev;
    }
    let mut out = vec![last];
    if let Some((stem, version)) = last.rsplit_once('.')
        && is_major_version(version)
    {
        out.push(stem);
    }
    if let Some(stem) = last.strip_prefix("go-") {
        out.push(stem);
    }
    if let Some(stem) = last.strip_suffix("-go") {
        out.push(stem);
    }
    out
}

// ============================================================================
// Call collection
// ============================================================================

/// The calls under `node`, as CallSites `from` the enclosing function. A
/// `func_literal` child is not entered: it is queued on `closures` (tree order)
/// for [`collect_closure_calls`], which runs after `collect_routes_in` has
/// marked the route-handler literals (CA.1).
#[allow(clippy::too_many_arguments)]
fn collect_calls_in<'t>(
    node: TsNode<'t>,
    src: &[u8],
    from: NodeId,
    receiver_var: Option<&str>,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
    closures: &mut Vec<TsNode<'t>>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        // CA.2a: the locals this statement binds, on the enclosing fn.
        record_body_locals(child, src, from, receiver_var, acc);
        if child.kind() == "call_expression" {
            if let Some(q) = classify_call(child, src, receiver_var) {
                acc.calls.push(CallSite {
                    from,
                    qualifier: q,
                    line: line_at(child),
                });
            }
            // Pattern A: outbound client HTTP call (`http.Get('http://…/x')`) →
            // ENDPOINT node so HttpStackResolver can pair it with a server ROUTE.
            try_detect_go_endpoint(child, src, from, repo, file_rel, acc);
            // Raw SQL (`db.Query("SELECT … FROM users")`) is not read here:
            // the cross-cutting data-entities extractor reads every SQL
            // literal with LG.3b's rejects and the engine re-homes its edge
            // to this function (LE.4a, `anchor::rehome_to_owner`).
            // GORM: `db.Model(&User{})` → the model-keyed entity,
            // `db.Table("x")` → the table-keyed one, from the same `from` (A13.12).
            try_detect_gorm_access(child, src, from, repo, acc);
            // DI container registration: `wire.Build(NewA, NewB)` → INJECTS
            // from the injector (`from`) to each provider (A7.6).
            try_detect_go_provider_set(child, src, from, acc);
        }
        if child.kind() == "func_literal" {
            closures.push(child);
        } else {
            collect_calls_in(
                child,
                src,
                from,
                receiver_var,
                repo,
                file_rel,
                acc,
                closures,
            );
        }
    }
}

/// CA.1: the calls inside a function's func literals (`once.Do(func() {..})`,
/// `go func() {..}()`, `defer func() {..}()`, `g.Go(func() error {..})`, a
/// returned middleware closure) are CallSites of the enclosing function, at
/// each call's own row, with the same receiver variable. A route-handler
/// literal (LA.18d: its start byte is in `func_literal_handlers`, filled by
/// `collect_routes_in`, which runs first) is skipped whole, nested literals
/// included: its callees are its route's HANDLED_BY, not the registrar's
/// calls. Drained FIFO (breadth-first, each level in source order), after
/// every CallSite of the body proper, so a function without a closure parses
/// to the same FileParse as before.
#[allow(clippy::too_many_arguments)]
fn collect_closure_calls(
    closures: Vec<TsNode>,
    src: &[u8],
    from: NodeId,
    receiver_var: Option<&str>,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let calls_before = acc.calls.len();
    let mut queue: std::collections::VecDeque<TsNode> = closures.into();
    while let Some(lit) = queue.pop_front() {
        if acc.func_literal_handlers.contains(&lit.start_byte()) {
            acc.route_literals_skipped += 1;
            continue;
        }
        acc.closure_bodies += 1;
        // CA.2a: a drained literal's parameters are locals of `from`, as its
        // calls are `from`'s CallSites.
        record_params(lit.child_by_field_name("parameters"), src, from, acc);
        let Some(body) = lit.child_by_field_name("body") else {
            continue;
        };
        let mut nested = Vec::new();
        collect_calls_in(
            body,
            src,
            from,
            receiver_var,
            repo,
            file_rel,
            acc,
            &mut nested,
        );
        queue.extend(nested);
    }
    acc.closure_calls += acc.calls.len() - calls_before;
}

fn classify_call(call: TsNode, src: &[u8], receiver_var: Option<&str>) -> Option<CallQualifier> {
    let func = call.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => Some(CallQualifier::Bare(text_of(func, src).to_string())),
        "selector_expression" => {
            let operand = func.child_by_field_name("operand")?;
            let field = func.child_by_field_name("field")?;
            let name = text_of(field, src).to_string();
            match operand.kind() {
                "identifier" => {
                    let base = text_of(operand, src).to_string();
                    if Some(base.as_str()) == receiver_var {
                        Some(CallQualifier::SelfMethod(name))
                    } else {
                        Some(CallQualifier::Attribute { base, name })
                    }
                }
                // CA.2a: the receiver is its normalised chain ([`chain_text`]):
                // `s.repo` in a method of receiver `s` -> `self.repo` (LA.23c,
                // which A6.2a binds through the struct's field type),
                // `svc.Repo()`, `repositories.NewX()` (arguments elided);
                // a chain through an index or assertion stays raw.
                _ => Some(CallQualifier::ComplexReceiver {
                    receiver: chain_text(operand, src, receiver_var)
                        .unwrap_or_else(|| text_of(operand, src).to_string()),
                    name,
                }),
            }
        }
        _ => None,
    }
}

// ============================================================================
// Receiver-type facts (CA.2a)
// ============================================================================
//
// What a call's receiver is typed by, recorded for the graph: every callable's
// first result type (`CodeNav::return_types`), and every parameter, local and
// package-level var (`CodeNav::local_types`, a package var under the file's
// MODULE scope). A type keeps its package qualifier (`repositories.X`), and a
// value bound from a call records the call's normalised chain (`svc.Repo()`),
// whose type is that call's result. This parser only records; the graph's
// generic receiver pass reads a bare same-package type, the Go call hook the
// rest (CA.2b).

/// The text a receiver fact names a type by: every pointer unwrapped,
/// `T` -> `T`, `pkg.T` -> `pkg.T` (the file's own import name kept, so the
/// graph resolves it through this file's imports), a generic instantiation
/// by its base (`Collection[T]` -> `Collection`, `repositories.Page[User]`
/// -> `repositories.Page`), a parenthesised type by its inner type. A
/// predeclared type (it owns no in-repo method) and every other shape
/// (slice, array, map, chan, func, `interface{}`, `struct{}`) -> `None`.
fn go_type_ref(type_node: TsNode, src: &[u8]) -> Option<String> {
    let mut node = unwrap_pointer(type_node);
    while node.kind() == "parenthesized_type" {
        let mut c = node.walk();
        node = unwrap_pointer(node.named_children(&mut c).next()?);
    }
    let node = generic_base(node)?;
    match node.kind() {
        "type_identifier" => {
            let name = text_of(node, src);
            (!name.is_empty() && !is_predeclared_type(name)).then(|| name.to_string())
        }
        "qualified_type" => {
            let pkg = text_of(node.child_by_field_name("package")?, src);
            let name = text_of(node.child_by_field_name("name")?, src);
            (!pkg.is_empty() && !name.is_empty()).then(|| format!("{pkg}.{name}"))
        }
        _ => None,
    }
}

/// Go's predeclared type names (and the `comparable` constraint): none owns
/// an in-repo method.
fn is_predeclared_type(name: &str) -> bool {
    matches!(
        name,
        "any"
            | "bool"
            | "byte"
            | "comparable"
            | "complex64"
            | "complex128"
            | "error"
            | "float32"
            | "float64"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "rune"
            | "string"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
    )
}

/// [`go_type_ref`], except that a type naming one of the enclosing
/// callable's type parameters (`T` in `func Get[T any]() T`, or in a method
/// of `Collection[T]`) is unknown: it stands for whatever the caller picks.
fn fact_type(type_node: TsNode, src: &[u8], type_params: &[String]) -> Option<String> {
    go_type_ref(type_node, src).filter(|t| !type_params.iter().any(|p| p == t))
}

/// The names a `type_parameter_list` declares (`[K comparable, V any]` ->
/// `K`, `V`); empty for `None`.
fn type_param_names(list: Option<TsNode>, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(list) = list else {
        return out;
    };
    let mut lc = list.walk();
    for decl in list.named_children(&mut lc) {
        if decl.kind() != "type_parameter_declaration" {
            continue;
        }
        let mut nc = decl.walk();
        out.extend(
            decl.children_by_field_name("name", &mut nc)
                .map(|n| text_of(n, src).to_string()),
        );
    }
    out
}

/// The type parameters a generic receiver binds (`(c *Collection[T])` ->
/// `T`); empty for a plain receiver.
fn receiver_type_params(receiver: TsNode, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut rc = receiver.walk();
    for param in receiver.named_children(&mut rc) {
        let Some(ty) = param.child_by_field_name("type") else {
            continue;
        };
        let ty = unwrap_pointer(ty);
        let Some(args) = (ty.kind() == "generic_type")
            .then(|| ty.child_by_field_name("type_arguments"))
            .flatten()
        else {
            continue;
        };
        let mut ac = args.walk();
        for elem in args.named_children(&mut ac) {
            let mut ec = elem.walk();
            let arg = if elem.kind() == "type_elem" {
                elem.named_children(&mut ec).next()
            } else {
                Some(elem)
            };
            if let Some(arg) = arg.filter(|a| a.kind() == "type_identifier") {
                out.push(text_of(arg, src).to_string());
            }
        }
    }
    out
}

/// The first result type of a function, method or interface method whose
/// `result` field is `result`: a bare type, or the first declaration of a
/// result list (`(*T, error)`, `(u *T, err error)`), as [`fact_type`] names it.
fn result_type(result: Option<TsNode>, src: &[u8], type_params: &[String]) -> Option<String> {
    let result = result?;
    let ty = if result.kind() == "parameter_list" {
        let mut c = result.walk();
        let first = result
            .named_children(&mut c)
            .find(|p| p.kind() == "parameter_declaration");
        first?.child_by_field_name("type")?
    } else {
        result
    };
    fact_type(ty, src, type_params)
}

/// Record `name` as a local of `scope` with type text `ty` (`""` = unknown).
/// `_` binds nothing.
fn record_local(acc: &mut Acc, scope: NodeId, name: &str, ty: &str) {
    if name != "_" {
        acc.nav.record_local_type(scope, name, ty);
    }
}

/// Every parameter of a `parameter_list` as a local of `scope`: grouped names
/// (`a, b *T`) share the type, a variadic `...T` is a slice (`""`), an
/// unnamed parameter binds nothing.
fn record_params(params: Option<TsNode>, src: &[u8], scope: NodeId, acc: &mut Acc) {
    let Some(params) = params else {
        return;
    };
    let mut pc = params.walk();
    for param in params.named_children(&mut pc) {
        let ty = match param.kind() {
            "parameter_declaration" => param
                .child_by_field_name("type")
                .and_then(|t| fact_type(t, src, &acc.type_params))
                .unwrap_or_default(),
            "variadic_parameter_declaration" => String::new(),
            _ => continue,
        };
        let mut nc = param.walk();
        let names: Vec<TsNode> = param.children_by_field_name("name", &mut nc).collect();
        for name in names {
            record_local(acc, scope, text_of(name, src), &ty);
        }
    }
}

// ============================================================================
// Method signatures (CA.3a)
// ============================================================================

/// The normalised signature of a method whose `parameters` / `result` fields
/// are `params` / `result` (CA.3a): `(<p1>,<p2>,..)(<r1>,..)`, every entry a
/// [`type_shape`]. A parameter declaration contributes its type once per name
/// it declares (`a, b int` -> `int,int`) and once when unnamed; a variadic
/// `...T` contributes `...<T>`. The result is a parameter list (the same
/// rule), one type, or absent (`()`). Names never appear, so the
/// implementation `Get(key string) string` and the interface element
/// `Get(string) string` both give `(string)(string)`.
///
/// `None` when either side holds a parse error: the signature is then
/// unknown, never wrong.
fn go_signature(params: TsNode, result: Option<TsNode>, src: &[u8]) -> Option<String> {
    if params.has_error() || result.is_some_and(|r| r.has_error()) {
        return None;
    }
    let (params, results) = (type_list(params, src), result_list(result, src));
    Some(format!("({})({})", params.join(","), results.join(",")))
}

/// The result entries of a callable whose `result` field is `result`: a
/// parameter list's entries ([`type_list`]), one type, or none.
fn result_list(result: Option<TsNode>, src: &[u8]) -> Vec<String> {
    match result {
        Some(r) if r.kind() == "parameter_list" => type_list(r, src),
        Some(r) => vec![type_shape(r, src)],
        None => Vec::new(),
    }
}

/// The entry types of a `parameter_list`, in order, as [`go_signature`]
/// counts them. A comment in the list contributes nothing.
fn type_list(list: TsNode, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut c = list.walk();
    for param in list.named_children(&mut c) {
        let Some(ty) = param.child_by_field_name("type") else {
            continue;
        };
        match param.kind() {
            "parameter_declaration" => {
                let shape = type_shape(ty, src);
                let mut nc = param.walk();
                let names = param.children_by_field_name("name", &mut nc).count();
                out.extend(std::iter::repeat_n(shape, names.max(1)));
            }
            "variadic_parameter_declaration" => out.push(format!("...{}", type_shape(ty, src))),
            _ => {}
        }
    }
    out
}

/// A Go type as a signature entry (CA.3a): every package qualifier dropped
/// (`*mongo.Collection` -> `*Collection`), no whitespace, the empty interface
/// written `any`, a parenthesised type unwrapped, and the pointer / slice /
/// array / map / chan / func / generic structure kept (`map[string][]*pb.X`
/// -> `map[string][]*X`, `func(ctx context.Context) error` ->
/// `func(Context)error`). A func type's single result is written bare and a
/// longer list in parentheses, so `func() (err error)` and `func() error`
/// agree. A channel's element is parenthesised (`<-chan struct{}` ->
/// `<-chan(struct{})`) and a named struct field's type too
/// (`struct{a, b int}` -> `struct{a,b(int)}`), both of which Go reads the
/// same, so no space is needed. An inline interface lists its elements
/// sorted. The two sides of an implicit implementation are written in
/// different packages; qualifiers dropped, two same-named types of two
/// packages compare equal (a missed rejection, never a false one).
///
/// Every identifier comes from its node's text: UTF-8 safe.
fn type_shape(node: TsNode, src: &[u8]) -> String {
    let mut out = String::new();
    push_type_shape(node, src, &mut out);
    out
}

fn push_type_shape(node: TsNode, src: &[u8], out: &mut String) {
    let field = |name: &str| node.child_by_field_name(name);
    let push_field = |name: &str, out: &mut String| match field(name) {
        Some(n) => push_type_shape(n, src, out),
        None => push_compact(node, src, out),
    };
    match node.kind() {
        "qualified_type" => match field("name") {
            Some(n) => out.push_str(text_of(n, src)),
            None => push_compact(node, src, out),
        },
        "pointer_type" | "parenthesized_type" | "negated_type" => {
            match node.kind() {
                "pointer_type" => out.push('*'),
                "negated_type" => out.push('~'),
                _ => {}
            }
            match first_type_child(node) {
                Some(inner) => push_type_shape(inner, src, out),
                None => push_compact(node, src, out),
            }
        }
        "slice_type" => {
            out.push_str("[]");
            push_field("element", out);
        }
        "array_type" => {
            out.push('[');
            if let Some(len) = field("length") {
                // `[sha256.Size]byte`: a qualified constant, like a type.
                let len = match len.kind() {
                    "selector_expression" => len.child_by_field_name("field").unwrap_or(len),
                    _ => len,
                };
                push_compact(len, src, out);
            }
            out.push(']');
            push_field("element", out);
        }
        "implicit_length_array_type" => {
            out.push_str("[...]");
            push_field("element", out);
        }
        "map_type" => {
            out.push_str("map[");
            push_field("key", out);
            out.push(']');
            push_field("value", out);
        }
        "channel_type" => {
            out.push_str(channel_direction(node));
            out.push('(');
            push_field("value", out);
            out.push(')');
        }
        "function_type" => {
            out.push_str("func");
            push_func_tail(field("parameters"), field("result"), src, out);
        }
        "generic_type" => {
            push_field("type", out);
            out.push('[');
            if let Some(args) = field("type_arguments") {
                let mut c = args.walk();
                let shapes: Vec<String> = args
                    .named_children(&mut c)
                    .filter(|a| a.kind() != "comment")
                    .map(|a| type_elem_shape(a, src))
                    .collect();
                out.push_str(&shapes.join(","));
            }
            out.push(']');
        }
        "interface_type" => {
            let mut c = node.walk();
            let mut elems: Vec<String> = Vec::new();
            for elem in node.named_children(&mut c) {
                match elem.kind() {
                    "method_elem" => {
                        let mut s = elem
                            .child_by_field_name("name")
                            .map(|n| text_of(n, src).to_string())
                            .unwrap_or_default();
                        let params = elem.child_by_field_name("parameters");
                        push_func_tail(params, elem.child_by_field_name("result"), src, &mut s);
                        elems.push(s);
                    }
                    "type_elem" => elems.push(type_elem_shape(elem, src)),
                    _ => {}
                }
            }
            if elems.is_empty() {
                out.push_str("any");
            } else {
                elems.sort();
                out.push_str("interface{");
                out.push_str(&elems.join(";"));
                out.push('}');
            }
        }
        "struct_type" => {
            let mut fields: Vec<String> = Vec::new();
            let mut c = node.walk();
            for list in node.named_children(&mut c) {
                let mut lc = list.walk();
                for decl in list.named_children(&mut lc) {
                    let Some(ty) = decl.child_by_field_name("type") else {
                        continue;
                    };
                    let mut nc = decl.walk();
                    let names: Vec<&str> = decl
                        .children_by_field_name("name", &mut nc)
                        .map(|n| text_of(n, src))
                        .collect();
                    let shape = type_shape(ty, src);
                    fields.push(if names.is_empty() {
                        shape
                    } else {
                        format!("{}({shape})", names.join(","))
                    });
                }
            }
            out.push_str("struct{");
            out.push_str(&fields.join(";"));
            out.push('}');
        }
        _ => push_compact(node, src, out),
    }
}

/// `(<params>)<result>` of a func type or an inline interface's method: a
/// single result bare, none empty, more in parentheses.
fn push_func_tail(params: Option<TsNode>, result: Option<TsNode>, src: &[u8], out: &mut String) {
    let params = params.map(|p| type_list(p, src)).unwrap_or_default();
    out.push('(');
    out.push_str(&params.join(","));
    out.push(')');
    match result_list(result, src).as_slice() {
        [] => {}
        [one] => out.push_str(one),
        more => {
            out.push('(');
            out.push_str(&more.join(","));
            out.push(')');
        }
    }
}

/// A `type_elem` (a type argument, or an inline interface's embedded /
/// union term): its terms joined by `|`. Any other node is one type.
fn type_elem_shape(elem: TsNode, src: &[u8]) -> String {
    if elem.kind() != "type_elem" {
        return type_shape(elem, src);
    }
    let mut c = elem.walk();
    let terms: Vec<String> = elem
        .named_children(&mut c)
        .filter(|t| t.kind() != "comment")
        .map(|t| type_shape(t, src))
        .collect();
    terms.join("|")
}

/// The one type inside a pointer / parenthesised / negated type.
fn first_type_child(node: TsNode) -> Option<TsNode> {
    let mut c = node.walk();
    node.named_children(&mut c).find(|n| n.kind() != "comment")
}

/// `chan`, `<-chan` (receive-only) or `chan<-` (send-only), from where the
/// arrow token sits relative to the `chan` keyword.
fn channel_direction(node: TsNode) -> &'static str {
    let mut c = node.walk();
    let tokens: Vec<&str> = node
        .children(&mut c)
        .filter(|t| !t.is_named())
        .map(|t| t.kind())
        .collect();
    let arrow = tokens.iter().position(|t| *t == "<-");
    let chan = tokens.iter().position(|t| *t == "chan");
    match (arrow, chan) {
        (Some(a), Some(k)) if a < k => "<-chan",
        (Some(_), Some(_)) => "chan<-",
        _ => "chan",
    }
}

/// `node`'s source text with every whitespace character dropped.
fn push_compact(node: TsNode, src: &[u8], out: &mut String) {
    out.extend(text_of(node, src).chars().filter(|c| !c.is_whitespace()));
}

/// Whether a method's receiver names a generic type (`(c *Collection[T])`):
/// its signature mentions type parameters, whose meaning depends on the
/// instantiation, so CA.3a records none.
fn receiver_is_generic(receiver: TsNode) -> bool {
    let mut rc = receiver.walk();
    receiver.named_children(&mut rc).any(|param| {
        let Some(mut ty) = param.child_by_field_name("type") else {
            return false;
        };
        loop {
            ty = unwrap_pointer(ty);
            match (ty.kind() == "parenthesized_type").then(|| first_type_child(ty)).flatten() {
                Some(inner) => ty = inner,
                None => break,
            }
        }
        ty.kind() == "generic_type"
    })
}

/// Go's builtin functions whose result owns no in-repo method (a `new(T)` is
/// read for its `T` instead).
fn is_untyped_builtin(name: &str) -> bool {
    matches!(
        name,
        "append"
            | "cap"
            | "clear"
            | "close"
            | "complex"
            | "copy"
            | "delete"
            | "imag"
            | "len"
            | "make"
            | "max"
            | "min"
            | "panic"
            | "print"
            | "println"
            | "real"
            | "recover"
    )
}

/// The type text a local takes from its initialiser `value`:
/// * a call -> its [`chain_text`] (`repo.Find()`: the call's result), `new(T)`
///   -> `T`, and a builtin whose result owns no method (`make`, `len`) -> `""`;
/// * `T{..}` / `pkg.T{..}` / `&T{..}` -> the composite's type, `x.(T)` -> `T`;
/// * `s.repo` inside a method of receiver `s` -> `self.repo`, the alias of
///   the enclosing struct's field the receiver pass already follows;
/// * anything else -> `""`.
fn go_value_type(value: TsNode, src: &[u8], receiver_var: Option<&str>, acc: &Acc) -> String {
    let tp = acc.type_params.as_slice();
    match value.kind() {
        "call_expression" => {
            let func = value.child_by_field_name("function");
            if let Some(f) = func.filter(|f| f.kind() == "identifier") {
                let name = text_of(f, src);
                if name == "new" {
                    return value
                        .child_by_field_name("arguments")
                        .and_then(|a| a.named_child(0))
                        .and_then(|a| type_expr_ref(a, src, tp))
                        .unwrap_or_default();
                }
                if is_untyped_builtin(name) {
                    return String::new();
                }
            }
            chain_text(value, src, receiver_var).unwrap_or_default()
        }
        "composite_literal" | "type_assertion_expression" => value
            .child_by_field_name("type")
            .and_then(|t| fact_type(t, src, tp))
            .unwrap_or_default(),
        "unary_expression" => {
            let is_ref = value
                .child_by_field_name("operator")
                .is_some_and(|o| o.kind() == "&");
            match value.child_by_field_name("operand") {
                Some(inner) if is_ref && inner.kind() == "composite_literal" => {
                    go_value_type(inner, src, receiver_var, acc)
                }
                _ => String::new(),
            }
        }
        "selector_expression" => chain_text(value, src, receiver_var)
            .filter(|t| {
                t.strip_prefix("self.")
                    .is_some_and(|f| !f.is_empty() && !f.contains('.'))
            })
            .unwrap_or_default(),
        "parenthesized_expression" => {
            let mut c = value.walk();
            let inner = value.named_children(&mut c).find(|n| n.kind() != "comment");
            inner.map_or_else(String::new, |i| go_value_type(i, src, receiver_var, acc))
        }
        _ => String::new(),
    }
}

/// A type written where an expression may stand (`new(T)`'s argument):
/// `T` and `pkg.T` parse as an identifier / selector there, any other type
/// shape as a type node ([`fact_type`]).
fn type_expr_ref(node: TsNode, src: &[u8], type_params: &[String]) -> Option<String> {
    match node.kind() {
        "identifier" => {
            let name = text_of(node, src);
            (!name.is_empty()
                && !is_predeclared_type(name)
                && !type_params.iter().any(|p| p == name))
            .then(|| name.to_string())
        }
        "selector_expression" => {
            let pkg = node.child_by_field_name("operand")?;
            let name = node.child_by_field_name("field")?;
            (pkg.kind() == "identifier")
                .then(|| format!("{}.{}", text_of(pkg, src), text_of(name, src)))
        }
        _ => fact_type(node, src, type_params),
    }
}

/// The normalised text of a call receiver chain, or `None` when a link is
/// not a name, a field or a call: an identifier is its name (`self` when it
/// is the method's receiver `receiver_var`), `x.f` joins with `.`, a call
/// appends `()` with its arguments and type arguments elided (`NewX(a, b)`
/// -> `NewX()`), parentheses are dropped. An index, slice, type assertion or
/// literal anywhere in the chain -> `None`.
fn chain_text(node: TsNode, src: &[u8], receiver_var: Option<&str>) -> Option<String> {
    match node.kind() {
        "identifier" => {
            let name = text_of(node, src);
            if name.is_empty() {
                None
            } else if Some(name) == receiver_var {
                Some("self".to_string())
            } else {
                Some(name.to_string())
            }
        }
        "selector_expression" => {
            let operand = chain_text(node.child_by_field_name("operand")?, src, receiver_var)?;
            let field = text_of(node.child_by_field_name("field")?, src);
            (!field.is_empty()).then(|| format!("{operand}.{field}"))
        }
        "call_expression" => {
            let func = chain_text(node.child_by_field_name("function")?, src, receiver_var)?;
            Some(format!("{func}()"))
        }
        "parenthesized_expression" => {
            let mut c = node.walk();
            let inner = node
                .named_children(&mut c)
                .find(|n| n.kind() != "comment")?;
            chain_text(inner, src, receiver_var)
        }
        _ => None,
    }
}

/// The named children of `node` (an `expression_list`), none for `None`.
fn named_kids(node: Option<TsNode>) -> Vec<TsNode> {
    node.map(|n| {
        let mut c = n.walk();
        n.named_children(&mut c).collect()
    })
    .unwrap_or_default()
}

/// True when a `range_clause` / `receive_statement` declares its left-hand
/// names (`:=`), rather than assigning existing ones (`=`).
fn declares(node: TsNode) -> bool {
    let mut c = node.walk();
    node.children(&mut c).any(|ch| ch.kind() == ":=")
}

/// The initialiser of the `i`-th of `n` declared names: by position, or, for
/// one initialiser and several names (`x, err := f()`), that initialiser for
/// the first name and none for the others.
fn paired<'t>(values: &[TsNode<'t>], n: usize, i: usize) -> Option<TsNode<'t>> {
    if values.len() == n {
        values.get(i).copied()
    } else if i == 0 && values.len() == 1 {
        values.first().copied()
    } else {
        None
    }
}

/// Declared `names` as locals of `scope`, each typed by its [`paired`]
/// initialiser ([`go_value_type`]; none -> `""`).
fn pair_values(
    names: &[TsNode],
    values: &[TsNode],
    src: &[u8],
    scope: NodeId,
    receiver_var: Option<&str>,
    acc: &mut Acc,
) {
    for (i, name) in names.iter().enumerate() {
        if name.kind() != "identifier" {
            continue;
        }
        let ty = paired(values, names.len(), i)
            .map_or_else(String::new, |v| go_value_type(v, src, receiver_var, acc));
        record_local(acc, scope, text_of(*name, src), &ty);
    }
}

/// The locals one statement inside a callable body binds, recorded on
/// `scope` (the enclosing function, which a func literal's body shares):
/// `a, b := x, y`, `var x T` / `var x = v`, a `range` / `select` receive with
/// `:=` and a type switch's bound name (both of unknown type).
fn record_body_locals(
    node: TsNode,
    src: &[u8],
    scope: NodeId,
    receiver_var: Option<&str>,
    acc: &mut Acc,
) {
    match node.kind() {
        "short_var_declaration" => {
            let names = named_kids(node.child_by_field_name("left"));
            let values = named_kids(node.child_by_field_name("right"));
            pair_values(&names, &values, src, scope, receiver_var, acc);
        }
        "var_spec" => {
            let mut nc = node.walk();
            let names: Vec<TsNode> = node.children_by_field_name("name", &mut nc).collect();
            if let Some(ty) = node.child_by_field_name("type") {
                let ty = fact_type(ty, src, &acc.type_params).unwrap_or_default();
                for name in names {
                    record_local(acc, scope, text_of(name, src), &ty);
                }
            } else {
                let values = named_kids(node.child_by_field_name("value"));
                pair_values(&names, &values, src, scope, receiver_var, acc);
            }
        }
        "range_clause" | "receive_statement" if declares(node) => {
            for name in named_kids(node.child_by_field_name("left")) {
                if name.kind() == "identifier" {
                    record_local(acc, scope, text_of(name, src), "");
                }
            }
        }
        "type_switch_statement" => {
            for name in named_kids(node.child_by_field_name("alias")) {
                if name.kind() == "identifier" {
                    record_local(acc, scope, text_of(name, src), "");
                }
            }
        }
        _ => {}
    }
}

/// A package-level `var` spec's names as locals of the file's MODULE scope:
/// the declared type, else each name's initialiser ([`go_value_type`]).
fn record_package_vars(
    spec: TsNode,
    names: &[String],
    src: &[u8],
    module_id: NodeId,
    acc: &mut Acc,
) {
    if let Some(ty) = spec.child_by_field_name("type") {
        let ty = go_type_ref(ty, src).unwrap_or_default();
        for name in names {
            record_local(acc, module_id, name, &ty);
        }
        return;
    }
    let values = named_kids(spec.child_by_field_name("value"));
    for (i, name) in names.iter().enumerate() {
        let ty = paired(&values, names.len(), i)
            .map_or_else(String::new, |v| go_value_type(v, src, None, acc));
        record_local(acc, module_id, name, &ty);
    }
}

// ============================================================================
// DI container registration (A7.6) — google/wire, uber-go/fx, uber-go/dig
// ============================================================================

/// A dependency-injection container package a Go file imports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiContainer {
    Wire,
    Fx,
    Dig,
}

impl DiContainer {
    /// The container behind an import path, plus the package's own name (the
    /// local binding when the import carries no alias).
    fn from_import_path(path: &str) -> Option<(Self, &'static str)> {
        match path {
            "github.com/google/wire" => Some((Self::Wire, "wire")),
            "go.uber.org/fx" => Some((Self::Fx, "fx")),
            "go.uber.org/dig" => Some((Self::Dig, "dig")),
            _ => None,
        }
    }
}

/// Explicit DI-container registration, the only AST-visible dependency-
/// injection signal in Go. Providers are named as function identifiers, which
/// are call ARGUMENTS, so `classify_call` never sees them. Emits one INJECTS
/// ref from `from` (the injector function, or a package-level provider-set
/// var) to each provider. Gated on the file importing the container; an import
/// alias is followed. Recognised:
///
/// - `wire.Build(NewA, pkg.NewB)` / `wire.NewSet(...)`.
/// - `fx.Provide(...)` / `fx.Invoke(...)` / `fx.Decorate(...)`, including the
///   provider wrapped by `fx.Annotate(NewA, ...)`.
/// - `c.Provide(...)` / `c.Invoke(...)` / `c.Decorate(...)` in a file that
///   imports dig. dig has no package-level `Provide`; it registers through
///   `*dig.Container` methods.
///
/// Skipped: `wire.Bind(new(I), new(*T))`, `wire.Struct`, `wire.Value`, and
/// `fx.Supply` / `fx.Populate`, whose arguments are types, values or pointers,
/// not providers. Any argument that is not `Name` or `pkg.Name` is skipped.
///
/// Honest scope: these edges read "registers a provider", not "consumer
/// receives a service". Go's idiomatic `func NewX(dep *Dep) *X` constructor
/// cannot be recognised without return-type inference (area A6).
fn try_detect_go_provider_set(call: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let Some(from_module) = acc.module_id else {
        return;
    };
    if acc.di_containers.is_empty() {
        return;
    }
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    if func.kind() != "selector_expression" {
        return;
    }
    let (Some(operand), Some(field)) = (
        func.child_by_field_name("operand"),
        func.child_by_field_name("field"),
    ) else {
        return;
    };
    if operand.kind() != "identifier" {
        return;
    }
    let method = text_of(field, src);
    let registers = match acc.di_containers.get(text_of(operand, src)) {
        Some(DiContainer::Wire) => matches!(method, "Build" | "NewSet"),
        Some(DiContainer::Fx) => matches!(method, "Provide" | "Invoke" | "Decorate"),
        // `dig.New()` / `dig.Name(...)` register nothing themselves.
        Some(DiContainer::Dig) => false,
        None => {
            acc.di_containers.values().any(|c| *c == DiContainer::Dig)
                && matches!(method, "Provide" | "Invoke" | "Decorate")
        }
    };
    if !registers {
        return;
    }
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        let Some(qualifier) = provider_qualifier(arg, src, &acc.di_containers) else {
            continue;
        };
        if !acc.di_seen.insert((from, format!("{qualifier:?}"))) {
            continue;
        }
        acc.refs.push(UnresolvedRef {
            from,
            from_module,
            qualifier,
            category: edge_category::INJECTS,
            line: line_at(arg),
        });
        di_stats::record(DiShape::GoProvider);
    }
}

/// The provider one registration argument names. `NewA` gives `Bare`;
/// `pkg.NewA` gives `Attribute`, which binds through the file's import table
/// the way a `pkg.NewA()` call does, so an external package's `NewClient`
/// never binds to an unrelated local `NewClient`. `fx.Annotate(NewA, ...)`
/// gives its first argument.
fn provider_qualifier(
    arg: TsNode,
    src: &[u8],
    containers: &HashMap<String, DiContainer>,
) -> Option<CallQualifier> {
    match arg.kind() {
        "identifier" => Some(CallQualifier::Bare(text_of(arg, src).to_string())),
        "selector_expression" => {
            let operand = arg.child_by_field_name("operand")?;
            let field = arg.child_by_field_name("field")?;
            if operand.kind() != "identifier" {
                return None;
            }
            Some(CallQualifier::Attribute {
                base: text_of(operand, src).to_string(),
                name: text_of(field, src).to_string(),
            })
        }
        "call_expression" => {
            let func = arg.child_by_field_name("function")?;
            if func.kind() != "selector_expression" {
                return None;
            }
            let base = text_of(func.child_by_field_name("operand")?, src);
            let name = text_of(func.child_by_field_name("field")?, src);
            if containers.get(base) != Some(&DiContainer::Fx) || name != "Annotate" {
                return None;
            }
            let inner = arg.child_by_field_name("arguments")?;
            let mut cursor = inner.walk();
            let first = inner.named_children(&mut cursor).next()?;
            // One level only: `fx.Annotate(fx.Annotate(...))` is not a shape.
            if first.kind() == "call_expression" {
                return None;
            }
            provider_qualifier(first, src, containers)
        }
        _ => None,
    }
}

/// Walk a package-level initialiser for registrations, e.g.
/// `var Set = wire.NewSet(...)` or `var Module = fx.Module("m", fx.Provide(...))`.
/// Function bodies do not come through here; `collect_calls_in` covers them.
fn collect_provider_sets_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    if acc.di_containers.is_empty() {
        return;
    }
    if node.kind() == "call_expression" {
        try_detect_go_provider_set(node, src, from, acc);
    }
    if node.kind() == "func_literal" {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_provider_sets_in(child, src, from, acc);
    }
}

// ============================================================================
// Client HTTP calls (Pattern A) — outbound net/http calls become ENDPOINT
// nodes so the HttpStackResolver can pair them with a server ROUTE, giving a
// cross-stack HTTP_CALLS edge. Mirrors the Dart parser's `try_detect_dart_endpoint`.
// ============================================================================
//
// Recognised shapes:
//   `http.Get(url)` / `http.Post(url, …)` / `http.Head(url)` — stdlib package
//        funcs; verb from the method name, receiver is the `http` package.
//   `client.Get(url)` / `c.Post(…)` / `httpClient.Get(url)` — *http.Client
//        methods; receiver is an http-client variable (never a server router).
//   `http.NewRequest("GET", url, body)` /
//   `http.NewRequestWithContext(ctx, "POST", url, body)` — verb is the string
//        method arg, url is the following string arg.
//
// The URL literal is usually absolute (`http://host/users`); `client_url_split`
// splits it into the path `/users` (the ENDPOINT's identity) and the host
// (recorded as `"host"` on ENDPOINT_HIT, A11.5). A non-literal / non-path URL
// is skipped.

/// Map a Go client method name (verb form) to its canonical upper-case verb.
fn client_http_verb(name: &str) -> Option<&'static str> {
    match name {
        "Get" | "GET" => Some("GET"),
        "Post" | "POST" => Some("POST"),
        "Put" | "PUT" => Some("PUT"),
        "Patch" | "PATCH" => Some("PATCH"),
        "Delete" | "DELETE" => Some("DELETE"),
        "Head" | "HEAD" => Some("HEAD"),
        "Options" | "OPTIONS" => Some("OPTIONS"),
        _ => None,
    }
}

/// True for an http-client receiver variable in the verb form (`client.Get`,
/// `httpClient.Post`, `c.Get`). The stdlib `http` package is handled separately;
/// server routers (`r`, `app`, group vars) are deliberately excluded so route
/// registration (`r.Get("/x", h)`) is never mistaken for a client call.
fn is_http_client_receiver(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "client" || n == "httpclient" || n == "c" || n.ends_with("client")
}

/// Detect an outbound client HTTP call and emit a shared ENDPOINT node + CALLS
/// edge from the enclosing `from` node. No-op for anything that isn't a client
/// HTTP idiom.
fn try_detect_go_endpoint(
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
    if func.kind() != "selector_expression" {
        return;
    }
    let Some(operand) = func.child_by_field_name("operand") else {
        return;
    };
    let Some(field) = func.child_by_field_name("field") else {
        return;
    };
    if operand.kind() != "identifier" {
        return;
    }
    let recv = text_of(operand, src);
    let method = text_of(field, src);
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };

    // Form 3: http.NewRequest("GET", url, …) / NewRequestWithContext(ctx, "POST", url, …)
    if recv == "http" && (method == "NewRequest" || method == "NewRequestWithContext") {
        detect_new_request(call, args, src, from, repo, file_rel, acc);
        return;
    }

    // Forms 1 & 2: verb methods. Receiver must be the `http` package or a
    // recognised http-client variable — never a server router.
    let Some(verb) = client_http_verb(method) else {
        return;
    };
    if recv != "http" && !is_http_client_receiver(recv) {
        return;
    }
    let Some(first) = args.named_child(0) else {
        return;
    };
    let Some(raw) = string_literal_text(first, src) else {
        return; // non-literal URL (variable / fmt.Sprintf) — can't resolve a path
    };
    emit_go_endpoint(verb, &raw, call, from, repo, file_rel, acc);
}

/// `http.NewRequest`/`NewRequestWithContext`: the verb and url are string-literal
/// args in order (a leading `ctx` in the WithContext form is not a string, so
/// filtering to string literals lands verb first, url second).
fn detect_new_request(
    call: TsNode,
    args: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut strings: Vec<String> = Vec::new();
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if let Some(s) = string_literal_text(arg, src) {
            strings.push(s);
        }
    }
    if strings.len() < 2 {
        return;
    }
    let verb = strings[0].to_ascii_uppercase();
    let Some(canonical) = client_http_verb(&verb) else {
        return;
    };
    emit_go_endpoint(canonical, &strings[1], call, from, repo, file_rel, acc);
}

/// Build a `ClientEndpoint` from a URL literal and push it via the shared helper,
/// with the literal's authority as its `host`. Skips the call when the literal
/// yields no request path (bare host / non-path).
fn emit_go_endpoint(
    verb: &str,
    raw_url: &str,
    call: TsNode,
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let (host, path) = client_url_split(raw_url);
    let Some(path) = path else {
        return;
    };
    let pos = call.start_position();
    let ep = ClientEndpoint {
        method: verb.to_string(),
        path,
        file: file_rel.to_string(),
        line: pos.row + 1,
        col: pos.column + 1,
        confidence: Confidence::Strong,
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

// ============================================================================
// Data access (ACCESSES_DATA) — the GORM query sites below.
// ============================================================================
//
// Raw SQL has no scanner here (LE.4a). The cross-cutting `data_entities`
// extractor reads every SQL-shaped string literal of the file (LG.3b: joined
// across `+`, CTE names, SQL keywords, function FROMs and Go fmt messages
// rejected) and the engine re-homes each ACCESSES_DATA edge from the module to
// the innermost FUNCTION / METHOD whose span holds the statement, with the
// ACCESS_MODE edge cell (`anchor::rehome_to_owner`). This parser's earlier
// per-call-argument scan duplicated that without the rejects and minted
// `sql:spaces` from `fmt.Errorf("delete from spaces key=%q: %w")`, CTE aliases
// and `SET` from `DO UPDATE SET`.
//
// The DATA_ENTITY NodeId is built with the exact same qname shape the extractor
// uses (`data_entity:sql:<table>`) so the two collapse onto one node at graph
// build. When a function holds both a GORM site and a raw statement on one
// table, the engine puts the mode on this parser's edge rather than adding a
// second one.

/// Emit (once per enclosing-fn × table) a DATA_ENTITY node + ACCESSES_DATA edge.
/// The node mirrors the data-entities extractor so ids collapse at graph build.
///
/// `table` is the entity's key: a SQL / `.Table("x")` table name, or a GORM
/// model name on the model-keyed path (A13.1's identity rule). Either way the
/// nav name is the key itself, so a model-keyed entity is named after its model.
fn emit_data_access(table: &str, from: NodeId, repo: RepoId, acc: &mut Acc) {
    let (qname, entity_id) = sql_entity(table, repo);
    if !acc.data_access_seen.insert((from, entity_id)) {
        return;
    }
    acc.nodes.push(Node {
        id: entity_id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![],
    });
    acc.nav
        .record(entity_id, table, &qname, node_kind::DATA_ENTITY, None);
    acc.edges.push(Edge {
        from,
        to: entity_id,
        category: edge_category::ACCESSES_DATA,
        confidence: Confidence::Medium,
        cells: Vec::new(),
    });
}

/// The `data_entity:sql:<key>` qname and id — the one construction site shared
/// by the raw-SQL path, the GORM query sites and the `TableName()` declaration,
/// so every one of them lands on the same node as the data-entities extractor.
fn sql_entity(key: &str, repo: RepoId) -> (String, NodeId) {
    let qname = format!("data_entity:sql:{key}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
    (qname, id)
}

// ============================================================================
// GORM (A13.12)
// ============================================================================
//
// A GORM service issues no SQL strings, so the raw-SQL scan above sees none of
// its data access. Per A13.1's ORM identity rule a model is keyed on its MODEL
// name, `data_entity:sql:User` — the one token a query site in any file can
// name. `db.Model(&User{})` and the CRUD finishers given a `User` literal
// target that id; `db.Table("x")` names a table directly and stays table-keyed
// like raw SQL. A `func (User) TableName() string { return "app_users" }`
// override emits the model's entity at the declaration site with a table cell,
// which stacks onto the query sites' node at graph build, and DbResolver joins
// it to `app_users`. Without an override its fold joins `User` to `users`.

/// GORM chain methods whose argument #0, when it is a composite literal
/// (`&User{}`, `User{Name: n}`, `&[]User{}`) or `new(User)`, names the model
/// they read or write. A variable argument (`db.Create(&u)`) carries no type
/// the parser can see and is skipped.
const GORM_MODEL_ARG_METHODS: &[&str] = &[
    "Model",
    "Create",
    "Save",
    "Delete",
    "First",
    "Last",
    "Take",
    "Find",
    "FirstOrCreate",
    "FirstOrInit",
    "Updates",
];

/// Detect a GORM query call in a file that imports GORM and emit an
/// ACCESSES_DATA edge from `from` (the enclosing fn) through `emit_data_access`,
/// so the node id, dedupe and edge match the raw-SQL path. `AutoMigrate`
/// names a model in every argument; the other model methods in argument #0.
fn try_detect_gorm_access(call: TsNode, src: &[u8], from: NodeId, repo: RepoId, acc: &mut Acc) {
    if !acc.gorm.import {
        return;
    }
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    if func.kind() != "selector_expression" {
        return;
    }
    let (Some(field), Some(args)) = (
        func.child_by_field_name("field"),
        call.child_by_field_name("arguments"),
    ) else {
        return;
    };
    let method = text_of(field, src);
    let mut cursor = args.walk();
    let mut named = args.named_children(&mut cursor).filter(|a| a.kind() != "comment");
    if method == "Table" {
        // `db.Table("users u")` / `db.Table("public.users")`: the table is the
        // first word, normalised like a raw-SQL table so the two collapse. A
        // subquery (`"(?) as u"`) is no identifier and is dropped.
        let Some(table) = named
            .next()
            .and_then(|a| string_literal_text(a, src))
            .and_then(|lit| lit.split_whitespace().next().and_then(canonical_sql_table))
        else {
            return;
        };
        emit_data_access(&table, from, repo, acc);
        acc.gorm.tables.insert(table);
        return;
    }
    let models: Vec<String> = if method == "AutoMigrate" {
        named.filter_map(|a| gorm_model_arg(a, src)).collect()
    } else if GORM_MODEL_ARG_METHODS.contains(&method) {
        named.next().and_then(|a| gorm_model_arg(a, src)).into_iter().collect()
    } else {
        return;
    };
    for model in models {
        emit_data_access(&model, from, repo, acc);
        acc.gorm.models.insert(model);
    }
}

/// The model a GORM argument names: `&User{}`, `User{..}`, `&[]User{}`,
/// `models.User{}` or `new(User)` → `User`. Anything else (a variable, a map
/// literal, an anonymous struct) names no model.
fn gorm_model_arg(arg: TsNode, src: &[u8]) -> Option<String> {
    let literal = match arg.kind() {
        "unary_expression" => {
            let op = arg.child_by_field_name("operator")?;
            if text_of(op, src) != "&" {
                return None;
            }
            arg.child_by_field_name("operand")?
        }
        "call_expression" => {
            // `new(User)` — the argument parses as an expression or a type.
            let func = arg.child_by_field_name("function")?;
            if func.kind() != "identifier" || text_of(func, src) != "new" {
                return None;
            }
            let args = arg.child_by_field_name("arguments")?;
            let ty = args.named_child(0)?;
            return match ty.kind() {
                "identifier" => Some(text_of(ty, src).to_string()),
                "selector_expression" => {
                    Some(text_of(ty.child_by_field_name("field")?, src).to_string())
                }
                _ => gorm_model_type_name(ty, src),
            };
        }
        _ => arg,
    };
    if literal.kind() != "composite_literal" {
        return None;
    }
    gorm_model_type_name(literal.child_by_field_name("type")?, src)
}

/// The bare model name of a composite literal's type: `User`, `pkg.User`,
/// `*User`, `[]User` / `[]*User` → `User`. Maps, arrays, generics and
/// anonymous structs name no model.
fn gorm_model_type_name(ty: TsNode, src: &[u8]) -> Option<String> {
    let name = match ty.kind() {
        "type_identifier" => text_of(ty, src),
        "qualified_type" => text_of(ty.child_by_field_name("name")?, src),
        "pointer_type" => return gorm_model_type_name(ty.named_child(0)?, src),
        "slice_type" => return gorm_model_type_name(ty.child_by_field_name("element")?, src),
        _ => return None,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// `func (User) TableName() string { return "app_users" }` in a file with GORM
/// evidence: emit the model-keyed `data_entity:sql:User` carrying the declared
/// table as a `data_entity::table_cell`. The node stacks onto the query sites'
/// node (same id) at graph build, in whichever file they live. A body that is
/// anything but one string-literal return (a computed or tenant-prefixed name)
/// declares no fixed table and emits nothing.
fn try_emit_gorm_table_name(decl: TsNode, model: &str, src: &[u8], repo: RepoId, acc: &mut Acc) {
    if !(acc.gorm.import || acc.gorm.tagged) || model.is_empty() {
        return;
    }
    let (Some(params), Some(result), Some(body)) = (
        decl.child_by_field_name("parameters"),
        decl.child_by_field_name("result"),
        decl.child_by_field_name("body"),
    ) else {
        return;
    };
    if params.named_child_count() != 0 || text_of(result, src) != "string" {
        return;
    }
    let Some(table) = single_string_return(body, src) else {
        return;
    };
    let table = table.trim();
    if table.is_empty() {
        return;
    }
    let (qname, entity_id) = sql_entity(model, repo);
    acc.nodes.push(Node {
        id: entity_id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![data_entity::table_cell(table, data_entity::orm::GORM)],
    });
    acc.nav
        .record(entity_id, model, &qname, node_kind::DATA_ENTITY, None);
    acc.gorm.models.insert(model.to_string());
    acc.gorm.table_cells += 1;
}

/// The string literal of a block whose only statement is `return "<lit>"`.
fn single_string_return(body: TsNode, src: &[u8]) -> Option<String> {
    let mut cursor = body.walk();
    let list = body
        .named_children(&mut cursor)
        .find(|n| n.kind() == "statement_list")?;
    let mut cursor = list.walk();
    let mut stmts = list.named_children(&mut cursor).filter(|n| n.kind() != "comment");
    let ret = stmts.next()?;
    if stmts.next().is_some() || ret.kind() != "return_statement" {
        return None;
    }
    let exprs = ret.named_child(0)?;
    if exprs.kind() != "expression_list" || exprs.named_child_count() != 1 {
        return None;
    }
    string_literal_text(exprs.named_child(0)?, src)
}

/// True when `node` (a top-level `type_declaration`) holds a struct field whose
/// tag carries a `gorm:"…"` key — the GORM evidence of a model file that
/// imports nothing.
fn has_gorm_tag(node: TsNode, src: &[u8]) -> bool {
    if node.kind() == "field_declaration"
        && node
            .child_by_field_name("tag")
            .is_some_and(|tag| text_of(tag, src).contains("gorm:\""))
    {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(|c| has_gorm_tag(c, src))
}

/// Normalise a captured SQL identifier: drop the schema prefix, require an
/// identifier shape, and reject SQL keywords that can follow FROM/JOIN/etc.
fn canonical_sql_table(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 128 {
        return None;
    }
    let last = raw.rsplit('.').next().unwrap_or(raw);
    if last.is_empty() || !last.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if matches!(
        last.to_ascii_uppercase().as_str(),
        "SELECT" | "WHERE" | "AND" | "OR" | "IF" | "EXISTS" | "NULL" | "TRUE" | "FALSE"
    ) {
        return None;
    }
    Some(last.to_string())
}

// ============================================================================
// Route extraction — covers Gin / Echo (all-caps verbs), Chi / Fiber
// (Title-case verbs), stdlib `http.HandleFunc`, and Gorilla Mux
// (`HandleFunc(...).Methods("GET", ...)`).
// ============================================================================
//
// Walks the enclosing fn body once. For each statement of the form
// `x := y.Group("/prefix")`, records `x` → concatenated prefix in `prefix_map`.
// For each recognised registration call, builds the full path by prepending
// `prefix_map[recv]` and emits a Route node with one ROUTE_METHOD cell plus an
// `UnresolvedRef` (category=HANDLED_BY) for the handler.
//
// Recognised shapes:
//   `<recv>.GET("/path", h)` / `<recv>.Get("/path", h)`  → method = GET
//   `http.HandleFunc("/path", h)` / `<recv>.HandleFunc(...)` standalone
//                                                         → method = ANY
//   `<recv>.HandleFunc("/path", h).Methods("GET", "POST")`
//                                                         → one route per method
//   LA.32a — the method-bearing forms:
//   `<recv>.Handle("PATCH", "/path", h)` (gin), `<recv>.Add("GET", ..)` (echo),
//   `<recv>.Method("PUT", ..)` / `.MethodFunc(..)` (chi)  → method = arg #0
//   `<recv>.Any("/path", h)` (gin / echo)                 → method = ANY
//   `<recv>.Match([]string{"GET", "POST"}, "/path", h)`   → one route per method
//   `mux.HandleFunc("GET /items/{id}", h)` (Go 1.22)      → method = GET,
//                                                           path = `/items/{id}`
//
// Every registration pushes a POSITION cell (the call's 0-based rows) before
// its ROUTE_METHOD cell, so first-POSITION readers place the route at its
// registration.
//
// Routes use path-only NodeIds so that registrations across files in a package
// (or across methods on the same path) collapse at graph-build time and their
// cells stack onto one multicellular Route node.

/// Map a Go HTTP method receiver-method name to its canonical upper-case form.
/// Returns `None` for non-HTTP-method names (`Group`, `HandleFunc`, etc.).
fn normalize_http_method(s: &str) -> Option<&'static str> {
    match s {
        "GET" | "Get" => Some("GET"),
        "POST" | "Post" => Some("POST"),
        "PUT" | "Put" => Some("PUT"),
        "DELETE" | "Delete" => Some("DELETE"),
        "PATCH" | "Patch" => Some("PATCH"),
        "HEAD" | "Head" => Some("HEAD"),
        "OPTIONS" | "Options" => Some("OPTIONS"),
        // Fiber: `app.All("/", h)` — register on every method.
        "All" => Some("ANY"),
        // LA.32a — gin / echo: `r.Any("/", h)`. Title-case, so the
        // `first_arg_is_url_path` gate in `try_emit_route` keeps `lo.Any(xs, f)`
        // out.
        "Any" => Some("ANY"),
        _ => None,
    }
}

/// LA.32a: the HTTP verbs a method-ARGUMENT registration may name. Upper-case
/// only, exactly as `net/http`'s `Method*` constants spell them.
const HTTP_METHOD_LITERALS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];

/// LA.32a: `Some(verb)` when `node` is a string literal whose content is
/// exactly one of [`HTTP_METHOD_LITERALS`]. Case-sensitive, so `"get"`, `"k"`
/// and a non-literal (`wg.Add(1)`) are all `None`.
fn method_literal(node: TsNode, src: &[u8]) -> Option<&'static str> {
    let text = string_literal_text(node, src)?;
    HTTP_METHOD_LITERALS.iter().copied().find(|m| *m == text)
}

/// LA.32a: a Go 1.22 ServeMux pattern `"<VERB> /path"` split into its verb and
/// path. `net/http` cuts the method at the first space or tab and trims the
/// blanks after it; the rest must be a `/path` here, so a host pattern
/// (`"GET example.com/x"`) and a bare `/path` are both `None`.
fn method_pattern(node: TsNode, src: &[u8]) -> Option<(&'static str, String)> {
    let text = string_literal_text(node, src)?;
    let (verb, rest) = text.split_once([' ', '\t'])?;
    let verb = HTTP_METHOD_LITERALS.iter().copied().find(|m| *m == verb)?;
    let path = rest.trim_start_matches([' ', '\t']);
    path.starts_with('/').then(|| (verb, path.to_string()))
}

/// True when positional argument `i` of a call's `args` list is a string
/// literal starting with `/`.
fn arg_is_url_path(args: TsNode, i: u32, src: &[u8]) -> bool {
    args.named_child(i)
        .and_then(|a| string_literal_text(a, src))
        .is_some_and(|p| p.starts_with('/'))
}

/// LA.32a: the verbs of `Match([]string{"GET", "POST"}, ..)`'s arg #0, in
/// source order. `None` unless it is a composite literal whose every element
/// is a method literal — a variable list or a non-verb element is skipped,
/// never guessed at.
fn method_list_literal(node: TsNode, src: &[u8]) -> Option<Vec<&'static str>> {
    if node.kind() != "composite_literal" {
        return None;
    }
    let body = node.child_by_field_name("body")?;
    let mut verbs = Vec::new();
    let mut cursor = body.walk();
    for el in body.named_children(&mut cursor) {
        match el.kind() {
            "comment" => continue,
            "literal_element" => verbs.push(method_literal(el.named_child(0)?, src)?),
            _ => return None,
        }
    }
    (!verbs.is_empty()).then_some(verbs)
}

fn collect_routes_in(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut prefix_map: HashMap<String, String> = HashMap::new();
    walk_routes(body, src, file_rel, module_id, repo, &mut prefix_map, acc);
}

fn walk_routes(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    prefix_map: &mut HashMap<String, String>,
    acc: &mut Acc,
) {
    // Closure bodies run as handlers at request time; anything registered inside
    // them is unreachable from the surrounding group map. Skip.
    if matches!(n.kind(), "func_literal") {
        return;
    }
    if n.kind() == "short_var_declaration" {
        record_group_assignment(n, src, prefix_map);
    }
    if n.kind() == "call_expression" {
        try_emit_route(n, src, file_rel, module_id, repo, prefix_map, acc);
    }
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        walk_routes(child, src, file_rel, module_id, repo, prefix_map, acc);
    }
}

fn record_group_assignment(
    decl: TsNode,
    src: &[u8],
    prefix_map: &mut HashMap<String, String>,
) {
    let Some(left) = decl.child_by_field_name("left") else {
        return;
    };
    let Some(right) = decl.child_by_field_name("right") else {
        return;
    };
    if left.named_child_count() != 1 || right.named_child_count() != 1 {
        return;
    }
    let Some(lhs) = left.named_child(0) else {
        return;
    };
    if lhs.kind() != "identifier" {
        return;
    }
    let Some(rhs) = right.named_child(0) else {
        return;
    };
    if rhs.kind() != "call_expression" {
        return;
    }
    let Some(func) = rhs.child_by_field_name("function") else {
        return;
    };
    if func.kind() != "selector_expression" {
        return;
    }
    let Some(field) = func.child_by_field_name("field") else {
        return;
    };
    if text_of(field, src) != "Group" {
        return;
    }
    let Some(operand) = func.child_by_field_name("operand") else {
        return;
    };
    let parent_prefix = if operand.kind() == "identifier" {
        prefix_map
            .get(text_of(operand, src))
            .cloned()
            .unwrap_or_default()
    } else {
        String::new()
    };
    let Some(args) = rhs.child_by_field_name("arguments") else {
        return;
    };
    let Some(first) = args.named_child(0) else {
        return;
    };
    let Some(path_literal) = string_literal_text(first, src) else {
        return;
    };
    let full_prefix = join_path(&parent_prefix, &path_literal);
    prefix_map.insert(text_of(lhs, src).to_string(), full_prefix);
}

fn try_emit_route(
    call: TsNode,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    prefix_map: &HashMap<String, String>,
    acc: &mut Acc,
) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    if func.kind() != "selector_expression" {
        return;
    }
    let Some(field) = func.child_by_field_name("field") else {
        return;
    };
    let method_name = text_of(field, src);

    // Gorilla Mux: `r.HandleFunc("/u", h).Methods("GET", "POST")` — promote
    // the inner registration to one route per method.
    if method_name == "Methods" {
        try_emit_gorilla_methods_chain(call, src, file_rel, module_id, repo, prefix_map, acc);
        return;
    }

    // A registration wrapped in `.Methods(...)` — the wrapping call took the
    // route already.
    let registration = matches!(
        method_name,
        "HandleFunc" | "Handle" | "Add" | "Method" | "MethodFunc"
    );
    if registration && is_inner_of_methods_chain(call, src) {
        return;
    }
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };

    // LA.32a — method at arg #0: gin `Handle("PATCH", "/p", h)`, echo
    // `Add(..)`, chi `Method(..)` / `MethodFunc(..)`. The upper-case verb
    // literal, a `/path` at arg #1 and a handler at arg #2 are all required, so
    // `wg.Add(1)`, `h.Add("k", "/x")` and `q.Add("GET", "/x")` never mint one.
    if matches!(method_name, "Handle" | "Add" | "Method" | "MethodFunc")
        && args.named_child_count() >= 3
        && let Some(verb) = args.named_child(0).and_then(|a| method_literal(a, src))
        && arg_is_url_path(args, 1, src)
    {
        if emit_route_from_call(
            call, verb, 1, 2, None, src, file_rel, module_id, repo, prefix_map, acc,
        ) {
            acc.route_forms.handle += 1;
        }
        return;
    }

    // LA.32a — gin `Match([]string{"GET", "POST"}, "/p", h)`: one registration
    // per listed verb, in source order, stacking on the path-keyed node (the
    // Gorilla `.Methods(..)` precedent).
    if method_name == "Match" {
        if args.named_child_count() >= 3
            && let Some(verbs) = args.named_child(0).and_then(|a| method_list_literal(a, src))
            && arg_is_url_path(args, 1, src)
        {
            let mut emitted = false;
            for verb in verbs {
                emitted |= emit_route_from_call(
                    call, verb, 1, 2, None, src, file_rel, module_id, repo, prefix_map, acc,
                );
            }
            if emitted {
                acc.route_forms.matched += 1;
            }
        }
        return;
    }

    // stdlib + Gorilla Mux: bare `HandleFunc` / `Handle`. Require the path to
    // begin with `/` to avoid colliding with stdlib map/method names.
    if method_name == "HandleFunc" || method_name == "Handle" {
        // LA.32a — Go 1.22 ServeMux: `"GET /items/{id}"` is a method plus a
        // path, never a path. A host pattern matches neither arm below.
        if let Some((verb, path)) = args.named_child(0).and_then(|a| method_pattern(a, src)) {
            if emit_route_from_call(
                call, verb, 0, 1, Some(&path), src, file_rel, module_id, repo, prefix_map, acc,
            ) {
                acc.route_forms.pattern += 1;
            }
            return;
        }
        if !first_arg_is_url_path(call, src) {
            return;
        }
        emit_route_from_call(
            call, "ANY", 0, 1, None, src, file_rel, module_id, repo, prefix_map, acc,
        );
        return;
    }

    // Idiomatic verb form: Gin/Echo (all-caps) and Chi/Fiber (Title-case).
    // Title-case `Get` / `Post` collide with common getters (`Header.Get(...)`,
    // `pool.Get()`); require a URL-shaped path. All-caps `GET` is unambiguous
    // and stays permissive for back-compat with the original Gin scanner.
    let Some(canonical) = normalize_http_method(method_name) else {
        return;
    };
    // A client HTTP call (`client.Get("/x")`) has the same verb shape but is an
    // outbound ENDPOINT (handled by try_detect_go_endpoint); skip its receiver
    // here so it isn't mis-emitted as a phantom server ROUTE.
    if let Some(operand) = func.child_by_field_name("operand")
        && operand.kind() == "identifier"
        && is_http_client_receiver(text_of(operand, src))
    {
        return;
    }
    let is_title_case = method_name
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false)
        && method_name.chars().skip(1).any(|c| c.is_ascii_lowercase());
    if is_title_case && !first_arg_is_url_path(call, src) {
        return;
    }
    if emit_route_from_call(
        call, canonical, 0, 1, None, src, file_rel, module_id, repo, prefix_map, acc,
    ) && method_name == "Any"
    {
        acc.route_forms.any += 1;
    }
}

/// True if the call's first positional argument is a string literal beginning
/// with `/` — the conventional URL-path shape. Used to discriminate route
/// registrations from same-shape getters (`Header.Get("X-Foo")`).
fn first_arg_is_url_path(call: TsNode, src: &[u8]) -> bool {
    let Some(args) = call.child_by_field_name("arguments") else {
        return false;
    };
    let Some(first) = args.named_child(0) else {
        return false;
    };
    let Some(path) = string_literal_text(first, src) else {
        return false;
    };
    path.starts_with('/')
}

/// True when `call` is the operand of a `<call>.Methods(...)` selector — i.e.
/// the inner `HandleFunc` of a Gorilla Mux chain. Used to suppress duplicate
/// emission while the walker descends past both calls.
fn is_inner_of_methods_chain(call: TsNode, src: &[u8]) -> bool {
    let Some(parent) = call.parent() else {
        return false;
    };
    if parent.kind() != "selector_expression" {
        return false;
    }
    let Some(field) = parent.child_by_field_name("field") else {
        return false;
    };
    text_of(field, src) == "Methods"
}

/// Handle `<inner>.Methods("GET", "POST", ...)` where `<inner>` is itself a
/// `HandleFunc` / `Handle` registration. Emits one Route node per listed
/// method, all sharing the same path NodeId so cells stack.
fn try_emit_gorilla_methods_chain(
    outer: TsNode,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    prefix_map: &HashMap<String, String>,
    acc: &mut Acc,
) {
    let Some(func) = outer.child_by_field_name("function") else {
        return;
    };
    let Some(inner_call) = func.child_by_field_name("operand") else {
        return;
    };
    if inner_call.kind() != "call_expression" {
        return;
    }
    let Some(inner_func) = inner_call.child_by_field_name("function") else {
        return;
    };
    if inner_func.kind() != "selector_expression" {
        return;
    }
    let Some(inner_field) = inner_func.child_by_field_name("field") else {
        return;
    };
    let inner_method = text_of(inner_field, src);
    if inner_method != "HandleFunc" && inner_method != "Handle" {
        return;
    }

    let Some(method_args) = outer.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = method_args.walk();
    for arg in method_args.named_children(&mut cursor) {
        let Some(method_str) = string_literal_text(arg, src) else {
            continue;
        };
        let method_upper = method_str.to_ascii_uppercase();
        emit_route_from_call(
            inner_call,
            &method_upper,
            0,
            1,
            None,
            src,
            file_rel,
            module_id,
            repo,
            prefix_map,
            acc,
        );
    }
}

/// Emit a Route node + POSITION cell + ROUTE_METHOD cell + HANDLED_BY ref for
/// a registration call shaped like `<recv>.<METHOD>("/path", handler)`.
/// `method` is the canonical upper-case verb (or `"ANY"` for unrouted
/// HandleFunc). The path is `path_override` when given (a Go 1.22 pattern's
/// path, already split off its verb), else the string literal at positional
/// argument `path_arg`; the handler is argument `handler_arg`. Returns whether
/// a route was emitted.
#[allow(clippy::too_many_arguments)]
fn emit_route_from_call(
    call: TsNode,
    method: &str,
    path_arg: u32,
    handler_arg: u32,
    path_override: Option<&str>,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    prefix_map: &HashMap<String, String>,
    acc: &mut Acc,
) -> bool {
    let Some(func) = call.child_by_field_name("function") else {
        return false;
    };
    let Some(operand) = func.child_by_field_name("operand") else {
        return false;
    };
    if operand.kind() != "identifier" {
        return false;
    }
    let receiver = text_of(operand, src);
    let Some(args) = call.child_by_field_name("arguments") else {
        return false;
    };
    let path_literal = match path_override {
        Some(p) => p.to_string(),
        None => {
            let Some(path_node) = args.named_child(path_arg) else {
                return false;
            };
            let Some(p) = string_literal_text(path_node, src) else {
                return false;
            };
            p
        }
    };

    let prefix = prefix_map.get(receiver).cloned().unwrap_or_default();
    // LB.5: `join_path` deliberately keeps an unprefixed relative literal
    // relative; the qname builder adds the one canonical leading `/`.
    let full_path = canonical_http_path(&join_path(&prefix, &path_literal)).into_owned();

    // LB.11a: one node per (method, path), the `<METHOD> <path>` shape every
    // other server parser emits, so a route is HANDLED_BY only its own
    // handler and a client call pairs only with the route of its method.
    let qname = route_qname(method, &full_path);
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname);

    // The handler argument. Identifier → Bare; selector `pkg.Name` → Attribute.
    let handler_arg = args.named_child(handler_arg);
    let (handler_display, handler_qualifier): (Option<String>, Option<CallQualifier>) =
        match handler_arg {
            Some(h) if h.kind() == "identifier" => {
                let name = text_of(h, src).to_string();
                (Some(name.clone()), Some(CallQualifier::Bare(name)))
            }
            Some(h) if h.kind() == "selector_expression" => {
                match (
                    h.child_by_field_name("operand"),
                    h.child_by_field_name("field"),
                ) {
                    (Some(o), Some(f)) if o.kind() == "identifier" => {
                        let base = text_of(o, src).to_string();
                        let name = text_of(f, src).to_string();
                        let display = format!("{base}.{name}");
                        (Some(display), Some(CallQualifier::Attribute { base, name }))
                    }
                    _ => (None, None),
                }
            }
            _ => (None, None),
        };

    let start = call.start_position();
    let cell = route_method_cell(
        method,
        handler_display.as_deref(),
        file_rel,
        start.row + 1,
        start.column + 1,
    );

    // LA.32a: POSITION first, so a first-POSITION reader places the route at
    // this registration; one per registration, so a path registered twice
    // carries both spans.
    let cells = vec![position_cell(call, file_rel), cell];
    acc.route_forms.registrations += 1;
    acc.route_forms.positioned += cells
        .iter()
        .filter(|c| c.kind == cell_type::POSITION)
        .count();
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Strong,
        cells,
    });

    // Only record nav once per route id per file, else children_of would
    // duplicate entries. The display name is the qname, as the legacy-shape
    // parsers record it.
    if acc.route_nav_seen.insert(route_id) {
        acc.nav
            .record(route_id, &qname, &qname, node_kind::ROUTE, None);
    }
    acc.route_paths.insert(full_path);
    acc.route_qnames.insert(qname);

    if let Some(q) = handler_qualifier {
        acc.refs.push(UnresolvedRef {
            from: route_id,
            from_module: module_id,
            qualifier: q,
            category: edge_category::HANDLED_BY,
            line: line_at(call),
        });
    }

    // LA.18d: a func-literal handler (`http.HandleFunc("/ws", func(w, r) {
    // serveWs(hub, w, r) })`) names no single target, so it has no display
    // name; what runs for the route is the literal's own direct in-repo
    // callees. Not the enclosing function (it registers every route), not the
    // module (it carries no CALLS). Expanded once per (literal, route node)
    // per file: every verb of a `.Methods(..)` chain is its own node.
    let literal = handler_arg.filter(|h| h.kind() == "func_literal");
    if let Some(h) = literal {
        acc.func_literal_handlers.insert(h.start_byte());
    }
    if let Some(h) = literal
        && acc.func_literal_expanded.insert((h.start_byte(), route_id))
    {
        for (qualifier, line) in func_literal_callees(h, src, &acc.external_pkgs) {
            acc.refs.push(UnresolvedRef {
                from: route_id,
                from_module: module_id,
                qualifier,
                category: edge_category::HANDLED_BY,
                line,
            });
            acc.func_literal_refs += 1;
        }
    }
    true
}

/// LA.18d: at most this many HANDLED_BY refs per func-literal handler, so a
/// closure that calls a pile of helpers cannot fan one route out without bound.
const MAX_FUNC_LITERAL_CALLEES: usize = 8;

/// Go builtins and predeclared conversions. A bare call to one of these is
/// never an in-repo function, and the HANDLED_BY `unique_global_function`
/// fallback would otherwise bind `len(x)` to a repo function named `len`.
const GO_PREDECLARED_CALLEES: &[&str] = &[
    "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len", "make", "max",
    "min", "new", "panic", "print", "println", "real", "recover", "bool", "byte", "complex64",
    "complex128", "error", "float32", "float64", "int", "int8", "int16", "int32", "int64", "rune",
    "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any",
];

/// LA.18d: the in-repo-shaped direct callees of a func-literal route handler,
/// deduped, in source order, capped at [`MAX_FUNC_LITERAL_CALLEES`]. Nested
/// func literals are not entered — they run later, if at all. Kept shapes are
/// the ones the identifier / selector handler arms already resolve: a bare
/// `name(..)` and `base.Name(..)` with an identifier `base`. Dropped: Go
/// builtins, calls through the literal's own parameters (`c.JSON(..)`,
/// `w.Write(..)` — framework receivers), and calls through an external
/// import (`log.Println`, `json.NewEncoder`). A repo-local package
/// (`handlers.ListUsers(c)`) and a captured variable (`hub.register(..)`) stay.
fn func_literal_callees(
    lit: TsNode,
    src: &[u8],
    external_pkgs: &std::collections::HashSet<String>,
) -> Vec<(CallQualifier, u32)> {
    let mut params: Vec<&str> = Vec::new();
    if let Some(list) = lit.child_by_field_name("parameters") {
        let mut cursor = list.walk();
        for decl in list.named_children(&mut cursor) {
            let mut names = decl.walk();
            for name in decl.children_by_field_name("name", &mut names) {
                params.push(text_of(name, src));
            }
        }
    }
    let mut out = Vec::new();
    if let Some(body) = lit.child_by_field_name("body") {
        collect_literal_callees(body, src, &params, external_pkgs, &mut out);
    }
    out
}

fn collect_literal_callees(
    n: TsNode,
    src: &[u8],
    params: &[&str],
    external_pkgs: &std::collections::HashSet<String>,
    out: &mut Vec<(CallQualifier, u32)>,
) {
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        if out.len() >= MAX_FUNC_LITERAL_CALLEES {
            return;
        }
        if child.kind() == "func_literal" {
            continue;
        }
        if child.kind() == "call_expression"
            && let Some(q) = literal_callee(child, src, params, external_pkgs)
            && !out.iter().any(|(seen, _)| *seen == q)
        {
            out.push((q, line_at(child)));
        }
        collect_literal_callees(child, src, params, external_pkgs, out);
    }
}

fn literal_callee(
    call: TsNode,
    src: &[u8],
    params: &[&str],
    external_pkgs: &std::collections::HashSet<String>,
) -> Option<CallQualifier> {
    let func = call.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => {
            let name = text_of(func, src);
            // A parameter called as a function is a func value, never a repo
            // declaration.
            if GO_PREDECLARED_CALLEES.contains(&name) || params.contains(&name) {
                return None;
            }
            Some(CallQualifier::Bare(name.to_string()))
        }
        "selector_expression" => {
            let operand = func.child_by_field_name("operand")?;
            if operand.kind() != "identifier" {
                return None;
            }
            let base = text_of(operand, src);
            if params.contains(&base) || external_pkgs.contains(base) {
                return None;
            }
            let field = func.child_by_field_name("field")?;
            Some(CallQualifier::Attribute {
                base: base.to_string(),
                name: text_of(field, src).to_string(),
            })
        }
        _ => None,
    }
}

fn string_literal_text(n: TsNode, src: &[u8]) -> Option<String> {
    match n.kind() {
        "interpreted_string_literal" => {
            let full = text_of(n, src);
            if full.len() >= 2 && full.starts_with('"') && full.ends_with('"') {
                Some(full[1..full.len() - 1].to_string())
            } else {
                None
            }
        }
        "raw_string_literal" => {
            let full = text_of(n, src);
            if full.len() >= 2 && full.starts_with('`') && full.ends_with('`') {
                Some(full[1..full.len() - 1].to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn route_method_cell(
    method: &str,
    handler: Option<&str>,
    file_rel: &str,
    line: usize,
    col: usize,
) -> Cell {
    #[derive(serde::Serialize)]
    struct Payload<'a> {
        method: &'a str,
        handler: Option<&'a str>,
        file: &'a str,
        line: usize,
        col: usize,
    }
    let json = serde_json::to_string(&Payload {
        method,
        handler,
        file: file_rel,
        line,
        col,
    })
    .unwrap_or_else(|_| String::from("{}"));
    Cell {
        kind: cell_type::ROUTE_METHOD,
        payload: CellPayload::Json(json),
    }
}

// ============================================================================
// Cell helpers
// ============================================================================

fn file_cells(root: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*root, src).to_string()),
        },
        position_cell(*root, file_rel),
    ]
}

fn entity_cells(node: TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    let mut cells = vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(node, src).to_string()),
        },
        position_cell(node, file_rel),
    ];
    if let Some(doc) = glia_doc::leading_doc(&node, src) {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    cells
}

fn position_cell(node: TsNode, file_rel: &str) -> Cell {
    Cell {
        kind: cell_type::POSITION,
        payload: CellPayload::Json(glia_doc::position_json(&node, file_rel)),
    }
}

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
}

fn text_of<'a>(node: TsNode, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::EdgeCategoryId;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://go_smoke")
    }

    fn has_edge(parse: &FileParse, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> bool {
        parse
            .edges
            .iter()
            .any(|e| e.from == from && e.to == to && e.category == cat)
    }

    const HELPERS: &str = r#"package helpers

func HashPassword(p string) string {
    return inner(p)
}

func inner(p string) string {
    return p
}
"#;

    #[test]
    fn parses_package_and_two_functions() {
        let parse =
            parse_file(HELPERS, "svc/helpers/helpers.go", "svc::helpers", "", repo()).unwrap();

        let mod_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "svc::helpers");
        let hash_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "svc::helpers::HashPassword",
        );
        let inner_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "svc::helpers::inner",
        );

        assert!(parse.nodes.iter().any(|n| n.id == mod_id));
        assert!(parse.nodes.iter().any(|n| n.id == hash_id));
        assert!(parse.nodes.iter().any(|n| n.id == inner_id));
        assert!(has_edge(&parse, mod_id, hash_id, edge_category::DEFINES));
        assert!(has_edge(&parse, mod_id, inner_id, edge_category::DEFINES));

        // intra-file bare call: HashPassword → inner
        assert!(parse.calls.iter().any(|c| {
            c.from == hash_id && matches!(&c.qualifier, CallQualifier::Bare(n) if n == "inner")
        }));
    }

    const USERS: &str = r#"package users

type User struct {
    name string
}

type Greeter interface {
    Greet() string
}

func (u *User) Login(password string) error {
    u.save()
    return nil
}

func (u *User) save() error {
    return nil
}
"#;

    #[test]
    fn parses_struct_interface_and_methods_with_self_call() {
        let parse = parse_file(USERS, "svc/users/users.go", "svc::users", "", repo()).unwrap();

        let struct_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STRUCT, "svc::users::User");
        let iface_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::INTERFACE,
            "svc::users::Greeter",
        );
        let login_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "svc::users::User::Login",
        );
        let save_id = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::METHOD,
            "svc::users::User::save",
        );

        assert!(parse.nodes.iter().any(|n| n.id == struct_id));
        assert!(parse.nodes.iter().any(|n| n.id == iface_id));
        assert!(parse.nodes.iter().any(|n| n.id == login_id));
        assert!(parse.nodes.iter().any(|n| n.id == save_id));

        // Methods are children of the struct.
        assert!(has_edge(&parse, struct_id, login_id, edge_category::DEFINES));
        assert!(has_edge(&parse, struct_id, save_id, edge_category::DEFINES));

        // Self-call `u.save()` inside Login's body maps to SelfMethod (because
        // `u` is the receiver variable).
        assert!(parse.calls.iter().any(|c| {
            c.from == login_id
                && matches!(&c.qualifier, CallQualifier::SelfMethod(n) if n == "save")
        }));
    }

    const IFACES: &str = r#"package shop

// Store is the storage port.
type Store interface {
    // Get reads one value.
    Get(id string) (string, error)
    Put(id string, v string) error
}

type ReadStore interface {
    Reader
    io.Closer
    Paged[string]
    any
    Len() int
}

type Number interface {
    ~int | ~float64
}

type Ordered interface {
    int | string
}

type Small interface {
    ~int
}
"#;

    fn iface_refs(parse: &FileParse, iface: NodeId) -> Vec<(CallQualifier, EdgeCategoryId)> {
        parse
            .refs
            .iter()
            .filter(|r| r.from == iface)
            .map(|r| (r.qualifier.clone(), r.category))
            .collect()
    }

    /// LD.7b: every `method_elem` of an interface is a METHOD node
    /// `<iface>::<name>`, parented to the interface with a DEFINES edge and
    /// carrying CODE / POSITION (and its leading DOC) like any other entity.
    #[test]
    fn interface_method_elems_are_method_nodes() {
        let parse = parse_file(IFACES, "shop/store.go", "shop::store", "", repo()).unwrap();
        let store = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, "shop::store::Store");
        for name in ["Get", "Put"] {
            let q = format!("shop::store::Store::{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, &q);
            let node = parse.nodes.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("{q}"));
            assert!(has_edge(&parse, store, id, edge_category::DEFINES), "{q}");
            assert_eq!(parse.nav.parent_of.get(&id), Some(&store), "{q}");
            assert_eq!(parse.nav.name_by_id.get(&id).map(String::as_str), Some(name));
            let kinds: Vec<_> = node.cells.iter().map(|c| c.kind).collect();
            assert!(kinds.contains(&cell_type::CODE) && kinds.contains(&cell_type::POSITION), "{q}");
        }
        let get = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "shop::store::Store::Get");
        let get_node = parse.nodes.iter().find(|n| n.id == get).unwrap();
        assert!(get_node.cells.iter().any(|c| c.kind == cell_type::DOC));
        // Type-term-only interfaces declare no method.
        let methods = parse
            .nodes
            .iter()
            .filter(|n| parse.nav.kind_by_id.get(&n.id) == Some(&node_kind::METHOD))
            .count();
        assert_eq!(methods, 3, "Store::Get, Store::Put, ReadStore::Len");
    }

    /// LD.7b: an embedded interface is an INHERITS_FROM ref out of the
    /// embedding interface: `R` and `R[T]` Bare, `pkg.R` Attribute. `any`,
    /// unions and `~T` terms emit nothing.
    #[test]
    fn embedded_interface_emits_inherits_from_ref() {
        let parse = parse_file(IFACES, "shop/store.go", "shop::store", "", repo()).unwrap();
        let iface = |name: &str| {
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, &format!("shop::store::{name}"))
        };
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "shop::store");
        assert_eq!(
            iface_refs(&parse, iface("ReadStore")),
            vec![
                (CallQualifier::Bare("Reader".to_string()), edge_category::INHERITS_FROM),
                (
                    CallQualifier::Attribute { base: "io".to_string(), name: "Closer".to_string() },
                    edge_category::INHERITS_FROM
                ),
                (CallQualifier::Bare("Paged".to_string()), edge_category::INHERITS_FROM),
            ]
        );
        assert!(parse.refs.iter().filter(|r| r.from == iface("ReadStore")).all(|r| r.from_module == module));
        for name in ["Store", "Number", "Ordered", "Small"] {
            assert!(iface_refs(&parse, iface(name)).is_empty(), "{name}");
        }
    }

    const AUTH: &str = r#"package auth

import (
    "context"
    users "github.com/foo/bar/svc/users"
    "github.com/foo/bar/svc/helpers"
)

func Login(ctx context.Context) error {
    u := users.User{}
    _ = u
    return helpers.HashPassword("x")
}
"#;

    #[test]
    fn collects_imports_and_attribute_calls() {
        let parse = parse_file(
            AUTH,
            "svc/auth/auth.go",
            "svc::auth",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        // Three imports, two within-module.
        assert_eq!(parse.imports.len(), 3);
        assert!(parse.imports.iter().any(|i| {
            matches!(&i.target, ImportTarget::Module { path, alias }
                if path == "svc::users" && alias.as_deref() == Some("users"))
        }));
        assert!(parse.imports.iter().any(|i| {
            matches!(&i.target, ImportTarget::Module { path, alias: None }
                if path == "svc::helpers")
        }));
        assert!(parse.imports.iter().any(|i| {
            matches!(&i.target, ImportTarget::Module { path, .. } if path == "context")
        }));

        // helpers.HashPassword → Attribute call
        let login_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "svc::auth::Login");
        assert!(parse.calls.iter().any(|c| {
            c.from == login_id
                && matches!(&c.qualifier, CallQualifier::Attribute { base, name }
                    if base == "helpers" && name == "HashPassword")
        }));
    }

    // ========================================================================
    // State variables (glia v5 G19)
    // ========================================================================

    const STATE_VARS: &str = r#"package config

// MaxRetries is the cap on connection attempts before giving up.
const MaxRetries = 3

const internalSeed = 42

var Registry = newRegistry()
"#;

    #[test]
    fn documented_const_emits_state_var_but_bare_literal_does_not() {
        let parse =
            parse_file(STATE_VARS, "config/config.go", "config", "", repo()).unwrap();

        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "config");

        // Documented literal const → kept.
        let max_retries =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "config::MaxRetries");
        assert!(parse.nodes.iter().any(|n| n.id == max_retries));
        assert!(has_edge(&parse, module_id, max_retries, edge_category::DEFINES));

        // Undocumented bare-literal const → noise-gated out.
        let internal_seed =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "config::internalSeed");
        assert!(!parse.nodes.iter().any(|n| n.id == internal_seed));

        // Undocumented var with a call initialiser → non-trivial, kept.
        let registry =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "config::Registry");
        assert!(parse.nodes.iter().any(|n| n.id == registry));
    }

    // ========================================================================
    // Data access (ACCESSES_DATA) — raw SQL belongs to the extractor (LE.4a)
    // ========================================================================

    #[test]
    fn raw_sql_is_left_to_the_data_entities_extractor() {
        // LE.4a: the parser no longer scans call-argument SQL. The
        // cross-cutting extractor reads it (with LG.3b's rejects) and the
        // engine re-homes its edge to getUsers; engine/tests/access_mode.rs
        // covers the whole path.
        const SRC: &str = r#"package main

import (
    "database/sql"
    "fmt"
)

func getUsers(db *sql.DB) error {
    rows, err := db.Query("SELECT id, name FROM users WHERE active = true")
    if err != nil {
        return fmt.Errorf("delete from spaces key=%q: %w", "k", err)
    }
    defer rows.Close()
    return nil
}
"#;
        let parse = parse_file(SRC, "main.go", "main", "", repo()).unwrap();
        assert!(
            !parse
                .edges
                .iter()
                .any(|e| e.category == edge_category::ACCESSES_DATA),
            "raw SQL mints no parser-level ACCESSES_DATA"
        );
        assert!(
            !parse
                .nav
                .kind_by_id
                .values()
                .any(|k| *k == node_kind::DATA_ENTITY),
            "raw SQL mints no parser-level DATA_ENTITY"
        );
    }

    // ========================================================================
    // GORM (A13.12) — model-keyed entities, TableName() table cells
    // ========================================================================

    fn sql_entity_id(key: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            &format!("data_entity:sql:{key}"),
        )
    }

    fn access_targets(parse: &FileParse, from: NodeId) -> Vec<NodeId> {
        parse
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::ACCESSES_DATA)
            .map(|e| e.to)
            .collect()
    }

    fn table_cells_of(parse: &FileParse, id: NodeId) -> Vec<String> {
        parse
            .nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| data_entity::table_of(std::slice::from_ref(c)))
            .collect()
    }

    #[test]
    fn gorm_model_arg_targets_model_entity() {
        const SRC: &str = r#"package store

import "gorm.io/gorm"

func ListUsers(db *gorm.DB) ([]User, error) {
    var us []User
    err := db.Model(&User{}).Find(&us).Error
    return us, err
}

func Purge(db *gorm.DB) {
    db.Where("stale = ?", true).Delete(models.Order{})
    db.First(new(Invoice))
    db.AutoMigrate(&Account{}, &[]Ledger{})
}
"#;
        let parse = parse_file(SRC, "store.go", "store", "", repo()).unwrap();
        let list = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "store::ListUsers");
        let user = sql_entity_id("User");
        assert_eq!(access_targets(&parse, list), vec![user], "model-keyed, from the enclosing fn");
        assert_eq!(parse.nav.name_by_id.get(&user).map(String::as_str), Some("User"));
        assert!(table_cells_of(&parse, user).is_empty(), "a query site never knows the table");

        let purge = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "store::Purge");
        let got: std::collections::HashSet<NodeId> =
            access_targets(&parse, purge).into_iter().collect();
        let want: std::collections::HashSet<NodeId> = ["Order", "Invoice", "Account", "Ledger"]
            .iter()
            .map(|m| sql_entity_id(m))
            .collect();
        assert_eq!(got, want, "value literal, qualified type, new(T), AutoMigrate's every arg");
    }

    #[test]
    fn tablename_method_emits_table_cell() {
        const SRC: &str = r#"package store

import "gorm.io/gorm"

type User struct {
    gorm.Model
    Email string
}

func (User) TableName() string { return "app_users" }

func (*Order) TableName() string {
    // the legacy name
    return `legacy_orders`
}

func (t Tenant) TableName() string { return t.prefix + "_tenants" }
"#;
        let parse = parse_file(SRC, "model.go", "store", "", repo()).unwrap();
        assert_eq!(table_cells_of(&parse, sql_entity_id("User")), vec!["app_users"]);
        assert_eq!(table_cells_of(&parse, sql_entity_id("Order")), vec!["legacy_orders"]);
        let cell = parse
            .nodes
            .iter()
            .find(|n| n.id == sql_entity_id("User"))
            .and_then(|n| n.cells.first())
            .unwrap();
        let CellPayload::Json(raw) = &cell.payload else {
            panic!("table cell must be JSON");
        };
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v["orm"], data_entity::orm::GORM);
        assert!(
            !parse.nodes.iter().any(|n| n.id == sql_entity_id("Tenant")),
            "a computed TableName() declares no fixed table"
        );
        assert!(
            !parse.edges.iter().any(|e| e.category == edge_category::ACCESSES_DATA),
            "a declaration is not an access"
        );
    }

    #[test]
    fn tablename_in_tag_only_model_file_emits_table_cell() {
        // A pure model file imports nothing; its `gorm:"…"` tags are the evidence.
        const TAGGED: &str = "package store\n\ntype User struct {\n    ID uint `gorm:\"primaryKey\"`\n}\n\nfunc (User) TableName() string { return \"app_users\" }\n";
        let parse = parse_file(TAGGED, "model.go", "store", "", repo()).unwrap();
        assert_eq!(table_cells_of(&parse, sql_entity_id("User")), vec!["app_users"]);

        // No import, no tag: `TableName()` is just a method name.
        const PLAIN: &str = "package report\n\ntype Sheet struct{ Title string }\n\nfunc (Sheet) TableName() string { return \"sheet\" }\n";
        let parse = parse_file(PLAIN, "sheet.go", "report", "", repo()).unwrap();
        assert!(!parse.nodes.iter().any(|n| n.id == sql_entity_id("Sheet")));
    }

    #[test]
    fn gorm_table_literal_is_table_keyed() {
        const SRC: &str = r#"package store

import "github.com/jinzhu/gorm"

func Count(db *gorm.DB) {
    db.Table("public.deleted_users u").Count(&n)
    db.Table("(?) as sub", q).Scan(&rows)
}
"#;
        let parse = parse_file(SRC, "count.go", "store", "", repo()).unwrap();
        let count = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "store::Count");
        assert_eq!(
            access_targets(&parse, count),
            vec![sql_entity_id("deleted_users")],
            "schema and alias stripped like raw SQL; a subquery names no table"
        );
    }

    #[test]
    fn non_composite_model_arg_emits_nothing() {
        const GORM: &str = r#"package store

import "gorm.io/gorm"

func Update(db *gorm.DB, u *User, cfg Config) {
    db.Model(u).Update("name", "x")
    db.Create(&u)
    x.Model(cfg)
    db.Updates(map[string]any{"a": 1})
}
"#;
        let parse = parse_file(GORM, "update.go", "store", "", repo()).unwrap();
        assert!(
            !parse.edges.iter().any(|e| e.category == edge_category::ACCESSES_DATA),
            "a variable argument carries no model type"
        );

        // Without a GORM import, `.Model(&T{})` is somebody else's method.
        const NO_IMPORT: &str = r#"package view

func Render(t *Template) {
    t.Model(&Page{})
    t.Table("rows")
}
"#;
        let parse = parse_file(NO_IMPORT, "view.go", "view", "", repo()).unwrap();
        assert!(!parse.edges.iter().any(|e| e.category == edge_category::ACCESSES_DATA));
    }

    #[test]
    fn syntax_error_produces_partial_graph() {
        // Missing closing brace; tree-sitter still recovers.
        let broken = "package x\n\nfunc Foo() {\n    bar(\n";
        let parse = parse_file(broken, "x.go", "x", "", repo()).unwrap();
        // At minimum we got the module node.
        let mod_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "x");
        assert!(parse.nodes.iter().any(|n| n.id == mod_id));
    }

    // ========================================================================
    // Route extraction (v0.4.4)
    // ========================================================================

    /// LB.11a: a Go ROUTE is one node per (method, path), `<METHOD> <path>`.
    fn route_id(repo: RepoId, method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::ROUTE,
            &format!("{method} {path}"),
        )
    }

    fn route_methods(parse: &FileParse, route: NodeId) -> Vec<String> {
        parse
            .nodes
            .iter()
            .filter(|n| n.id == route)
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::ROUTE_METHOD)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => serde_json::from_str::<serde_json::Value>(s).ok(),
                _ => None,
            })
            .filter_map(|v| v.get("method").and_then(|m| m.as_str()).map(String::from))
            .collect()
    }

    const GIN_SIMPLE: &str = r#"package server

func setupRoutes(r *gin.Engine) {
    r.GET("/health", Health)
    r.POST("/login", controllers.AuthHandler)
}
"#;

    #[test]
    fn emits_route_node_per_method_and_path_with_method_cells() {
        let parse = parse_file(
            GIN_SIMPLE,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let health = route_id(repo(), "GET", "/health");
        let login = route_id(repo(), "POST", "/login");

        // Route nodes exist, one per (method, path).
        assert!(parse.nodes.iter().any(|n| n.id == health));
        assert!(parse.nodes.iter().any(|n| n.id == login));

        // Each route has exactly one ROUTE_METHOD cell in this fixture.
        assert_eq!(route_methods(&parse, health), vec!["GET".to_string()]);
        assert_eq!(route_methods(&parse, login), vec!["POST".to_string()]);
    }

    /// LB.5 — an unprefixed relative literal gets the one canonical leading
    /// `/`: `r.GET("items", h)` and `e.GET("/parts", h)` alike are
    /// `GET /<path>` (LB.11a), the nav name is that qname, and the handler
    /// ref hangs off that same node. A relative group prefix is canonical too.
    #[test]
    fn relative_route_literal_gets_one_leading_slash() {
        const SRC: &str = r#"package server

func setup(r *gin.Engine) {
    r.GET("items", listItems)
    r.GET("/parts", listParts)
    g := r.Group("api")
    g.GET("users", listUsers)
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        let qnames: Vec<&str> = parse
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| parse.nav.qname_by_id.get(id).map(String::as_str))
            .collect();
        for q in ["GET /items", "GET /parts", "GET /api/users"] {
            assert!(qnames.contains(&q), "missing {q}: {qnames:?}");
        }
        assert!(!qnames.contains(&"GET items"), "{qnames:?}");
        let items = route_id(repo(), "GET", "/items");
        assert_eq!(
            parse.nav.name_by_id.get(&items).map(String::as_str),
            Some("GET /items")
        );
        assert!(parse.refs.iter().any(|r| r.from == items
            && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "listItems")));
    }

    #[test]
    fn emits_handled_by_refs_for_identifier_and_selector_handlers() {
        let parse = parse_file(
            GIN_SIMPLE,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let health = route_id(repo(), "GET", "/health");
        let login = route_id(repo(), "POST", "/login");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "server");

        // Identifier handler → Bare
        assert!(parse.refs.iter().any(|r| {
            r.from == health
                && r.from_module == module_id
                && r.category == edge_category::HANDLED_BY
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Health")
        }));

        // Selector handler → Attribute
        assert!(parse.refs.iter().any(|r| {
            r.from == login
                && r.from_module == module_id
                && r.category == edge_category::HANDLED_BY
                && matches!(&r.qualifier, CallQualifier::Attribute { base, name }
                    if base == "controllers" && name == "AuthHandler")
        }));
    }

    const GIN_GROUP_CHAIN: &str = r#"package server

func setupRoutes(r *gin.Engine) {
    public := r.Group("/api")
    public.GET("/health", Health)
    protected := public.Group("/protected")
    protected.POST("/login", Login)
}
"#;

    #[test]
    fn group_prefix_chain_propagates_through_nested_groups() {
        let parse = parse_file(
            GIN_GROUP_CHAIN,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let health = route_id(repo(), "GET", "/api/health");
        let login = route_id(repo(), "POST", "/api/protected/login");

        assert!(
            parse.nodes.iter().any(|n| n.id == health),
            "expected /api/health route from public group"
        );
        assert!(
            parse.nodes.iter().any(|n| n.id == login),
            "expected /api/protected/login from nested group chain"
        );
    }

    const GIN_SAME_PATH_TWO_METHODS: &str = r#"package server

func setupRoutes(r *gin.Engine) {
    r.GET("/users", List)
    r.POST("/users", Create)
}
"#;

    /// LB.11a: GET and POST of one path are two ROUTE nodes, each with one
    /// ROUTE_METHOD cell and a HANDLED_BY ref to its own handler only — never
    /// one path node handled by both.
    #[test]
    fn same_path_two_methods_are_two_route_nodes() {
        let parse = parse_file(
            GIN_SAME_PATH_TWO_METHODS,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let get = route_id(repo(), "GET", "/users");
        let post = route_id(repo(), "POST", "/users");
        assert_ne!(get, post);
        assert_eq!(parse.nodes.iter().filter(|n| n.id == get).count(), 1);
        assert_eq!(parse.nodes.iter().filter(|n| n.id == post).count(), 1);
        assert_eq!(route_methods(&parse, get), vec!["GET".to_string()]);
        assert_eq!(route_methods(&parse, post), vec!["POST".to_string()]);
        assert_eq!(handled_by(&parse, get), vec![bare("List")]);
        assert_eq!(handled_by(&parse, post), vec![bare("Create")]);
        assert_eq!(
            route_qnames(&parse),
            vec!["GET /users".to_string(), "POST /users".to_string()]
        );
        assert!(
            !parse.nodes.iter().any(|n| n.id
                == NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "route:/users")),
            "the path-only node is gone"
        );
    }

    /// LB.11a: the nav name of a Go ROUTE is its qname, `GET /users`, as the
    /// legacy-shape parsers (java, python, rust, ...) record it.
    #[test]
    fn route_name_is_the_qname() {
        let parse = parse_file(
            GIN_SAME_PATH_TWO_METHODS,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();
        for (method, q) in [("GET", "GET /users"), ("POST", "POST /users")] {
            let id = route_id(repo(), method, "/users");
            assert_eq!(parse.nav.qname_by_id.get(&id).map(String::as_str), Some(q));
            assert_eq!(parse.nav.name_by_id.get(&id).map(String::as_str), Some(q));
            assert_eq!(parse.nav.kind_by_id.get(&id), Some(&node_kind::ROUTE));
        }
    }

    const GIN_TEMPLATED_PATH: &str = r#"package server

func setupRoutes(r *gin.Engine) {
    r.GET("/users/:id", Show)
}
"#;

    #[test]
    fn templated_path_retained_verbatim() {
        let parse = parse_file(
            GIN_TEMPLATED_PATH,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        // Normalisation happens in HttpStackResolver, not in the parser — the
        // parser stores the literal as written.
        let show = route_id(repo(), "GET", "/users/:id");
        assert!(parse.nodes.iter().any(|n| n.id == show));
    }

    // ------------------------------------------------------------------------
    // Chi / Fiber: Title-case verb form `r.Get("/path", h)`.
    // ------------------------------------------------------------------------

    const CHI_TITLE_CASE: &str = r#"package server

func setupRoutes(r *chi.Mux) {
    r.Get("/health", Health)
    r.Post("/login", Login)
    r.Delete("/users/:id", DeleteUser)
}
"#;

    #[test]
    fn chi_title_case_verbs_normalize_to_uppercase() {
        let parse = parse_file(
            CHI_TITLE_CASE,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        assert_eq!(
            route_methods(&parse, route_id(repo(), "GET", "/health")),
            vec!["GET".to_string()],
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "POST", "/login")),
            vec!["POST".to_string()],
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "DELETE", "/users/:id")),
            vec!["DELETE".to_string()],
        );
    }

    // ------------------------------------------------------------------------
    // Fiber: `app.All("/", h)` — register on every method, recorded as ANY.
    // ------------------------------------------------------------------------

    const FIBER_ALL: &str = r#"package server

func setupRoutes(app *fiber.App) {
    app.All("/wildcard", AnyHandler)
}
"#;

    #[test]
    fn fiber_all_verb_emits_any_method() {
        let parse = parse_file(
            FIBER_ALL,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        assert_eq!(
            route_methods(&parse, route_id(repo(), "ANY", "/wildcard")),
            vec!["ANY".to_string()],
        );
    }

    // ------------------------------------------------------------------------
    // stdlib: `http.HandleFunc("/path", h)` — bare handler, method = ANY.
    // ------------------------------------------------------------------------

    const STDLIB_HANDLEFUNC: &str = r#"package server

func main() {
    http.HandleFunc("/health", Health)
    http.HandleFunc("/users", controllers.ListUsers)
}
"#;

    #[test]
    fn stdlib_handlefunc_emits_any_route_with_handled_by() {
        let parse = parse_file(
            STDLIB_HANDLEFUNC,
            "server/main.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let health = route_id(repo(), "ANY", "/health");
        let users = route_id(repo(), "ANY", "/users");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "server");

        assert_eq!(route_methods(&parse, health), vec!["ANY".to_string()]);
        assert_eq!(route_methods(&parse, users), vec!["ANY".to_string()]);

        // Identifier handler retained.
        assert!(parse.refs.iter().any(|r| {
            r.from == health
                && r.from_module == module_id
                && r.category == edge_category::HANDLED_BY
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "Health")
        }));
        // Selector handler retained.
        assert!(parse.refs.iter().any(|r| {
            r.from == users
                && r.category == edge_category::HANDLED_BY
                && matches!(&r.qualifier, CallQualifier::Attribute { base, name }
                    if base == "controllers" && name == "ListUsers")
        }));
    }

    // ------------------------------------------------------------------------
    // Gorilla Mux: `r.HandleFunc("/u", h).Methods("GET", "POST")` — one route
    // per method, each its own `<METHOD> <path>` node (LB.11a). The inner
    // HandleFunc must NOT also emit an "ANY" route.
    // ------------------------------------------------------------------------

    const GORILLA_METHODS_CHAIN: &str = r#"package server

func setupRoutes(r *mux.Router) {
    r.HandleFunc("/users", UsersHandler).Methods("GET", "POST")
}
"#;

    #[test]
    fn gorilla_methods_chain_emits_one_route_per_method_no_any() {
        let parse = parse_file(
            GORILLA_METHODS_CHAIN,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let get = route_id(repo(), "GET", "/users");
        let post = route_id(repo(), "POST", "/users");
        assert_eq!(route_methods(&parse, get), vec!["GET".to_string()]);
        assert_eq!(route_methods(&parse, post), vec!["POST".to_string()]);
        assert_eq!(handled_by(&parse, get), vec![bare("UsersHandler")]);
        assert_eq!(handled_by(&parse, post), vec![bare("UsersHandler")]);
        assert_eq!(
            route_qnames(&parse),
            vec!["GET /users".to_string(), "POST /users".to_string()],
            "expected exactly 2 method routes, no ANY leak"
        );
    }

    // ------------------------------------------------------------------------
    // Gorilla Mux: standalone `r.HandleFunc(...)` (no `.Methods` chain) still
    // emits a Route with method ANY.
    // ------------------------------------------------------------------------

    const GORILLA_STANDALONE: &str = r#"package server

func setupRoutes(r *mux.Router) {
    r.HandleFunc("/legacy", LegacyHandler)
}
"#;

    // ------------------------------------------------------------------------
    // Real-repo eval — ignored by default. Run with:
    //   cargo test -p glia-parser-go -- --ignored eval --nocapture
    // Walks several real Go repos in ~/Code, parses every .go file, and
    // tabulates routes by method (and by inferred shape: HandleFunc / verb).
    // No assertions — diagnostic only, used to sanity-check the v0.4.x
    // route-shape additions against real-world code.
    // ------------------------------------------------------------------------

    fn collect_go_files(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if path.is_dir() {
                if name == "vendor" || name == ".git" || name == "node_modules" {
                    continue;
                }
                collect_go_files(&path, out);
            } else if name.ends_with(".go") && !name.ends_with("_test.go") {
                out.push(path);
            }
        }
    }

    #[test]
    #[ignore]
    fn eval_route_extraction_against_real_go_repos() {
        let repos: &[&str] = &[
            "/home/ivy/Code/lapse",
            "/home/ivy/Code/turps",
            "/home/ivy/Code/Kina/backend",
            "/home/ivy/Code/websocket",
        ];

        for repo_root in repos {
            let path = std::path::Path::new(repo_root);
            if !path.exists() {
                println!("SKIP {repo_root} (not found)");
                continue;
            }
            let mut files = Vec::new();
            collect_go_files(path, &mut files);

            let mut total_routes = 0usize;
            let mut by_method: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            let mut files_with_routes = 0usize;
            let mut parse_errors = 0usize;

            for file in &files {
                let Ok(source) = std::fs::read_to_string(file) else { continue };
                let rel = file.strip_prefix(path).unwrap_or(file).to_string_lossy();
                let parse = match parse_file(&source, &rel, "pkg", "github.com/x/y", repo()) {
                    Ok(p) => p,
                    Err(_) => {
                        parse_errors += 1;
                        continue;
                    }
                };
                let mut had_route = false;
                for node in &parse.nodes {
                    for cell in &node.cells {
                        if cell.kind != cell_type::ROUTE_METHOD {
                            continue;
                        }
                        had_route = true;
                        total_routes += 1;
                        if let CellPayload::Json(s) = &cell.payload {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                                if let Some(m) = v.get("method").and_then(|m| m.as_str()) {
                                    *by_method.entry(m.to_string()).or_insert(0) += 1;
                                }
                            }
                        }
                    }
                }
                if had_route {
                    files_with_routes += 1;
                }
            }

            println!("\n=== {repo_root} ===");
            println!(
                "  files={}  files_with_routes={}  parse_errors={}",
                files.len(),
                files_with_routes,
                parse_errors,
            );
            println!("  total route cells: {total_routes}");
            for (method, count) in &by_method {
                println!("    {method:<8} {count}");
            }
        }
    }

    // ------------------------------------------------------------------------
    // Negative test: same-shape getters (`Header.Get("X-Foo")`,
    // `pool.Get("key")`) must NOT emit Route nodes. Path-must-start-with-`/`
    // is the discriminator. Found in the wild in `/home/ivy/Code/websocket`.
    // ------------------------------------------------------------------------

    const GETTER_LOOKALIKES: &str = r#"package server

func handle(r *http.Request) string {
    accept := r.Header.Get("Sec-Websocket-Accept")
    other := pool.Get("some-key")
    return accept + other
}
"#;

    #[test]
    fn title_case_getters_with_non_path_strings_do_not_emit_routes() {
        let parse = parse_file(
            GETTER_LOOKALIKES,
            "server/handle.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let any_route = parse
            .nodes
            .iter()
            .any(|n| n.cells.iter().any(|c| c.kind == cell_type::ROUTE_METHOD));
        assert!(!any_route, "getters with non-`/` strings must not be routes");
    }

    // ========================================================================
    // Client HTTP calls (Pattern A) — net/http outbound calls → ENDPOINT nodes.
    // ========================================================================

    fn endpoint_id(repo: RepoId, method: &str, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::ENDPOINT,
            &format!("endpoint:{method}:{path}"),
        )
    }

    const HTTP_CLIENT_CALLS: &str = r#"package client

import (
    "context"
    "net/http"
)

func FetchUsers() ([]byte, error) {
    resp, _ := http.Get("http://api.example.com/users")
    return nil, nil
}

func CreateUser(httpClient *http.Client) {
    httpClient.Post("http://api.example.com/users", "application/json", nil)
}

func GetOne(ctx context.Context) {
    http.NewRequestWithContext(ctx, "DELETE", "http://api.example.com/things", nil)
}
"#;

    #[test]
    fn client_http_calls_emit_endpoints_with_calls_edges() {
        let parse = parse_file(
            HTTP_CLIENT_CALLS,
            "client/client.go",
            "client",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let get_users = endpoint_id(repo(), "GET", "/users");
        let post_users = endpoint_id(repo(), "POST", "/users");
        let del_things = endpoint_id(repo(), "DELETE", "/things");

        // http.Get(absolute URL) → ENDPOINT GET /users (host stripped).
        assert!(
            parse.nodes.iter().any(|n| n.id == get_users),
            "expected GET /users ENDPOINT from http.Get"
        );
        // client-method form `hc.Post(...)` → ENDPOINT POST /users.
        assert!(
            parse.nodes.iter().any(|n| n.id == post_users),
            "expected POST /users ENDPOINT from hc.Post"
        );
        // NewRequestWithContext(ctx, "DELETE", url, …) → ENDPOINT DELETE /things.
        assert!(
            parse.nodes.iter().any(|n| n.id == del_things),
            "expected DELETE /things ENDPOINT from NewRequestWithContext"
        );

        // CALLS edge from the enclosing function into each endpoint.
        let fetch_users =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "client::FetchUsers");
        assert!(
            has_edge(&parse, fetch_users, get_users, edge_category::CALLS),
            "expected CALLS edge FetchUsers -> GET /users endpoint"
        );

        // Client calls must NOT be mis-emitted as server ROUTE nodes.
        assert!(
            !parse
                .nodes
                .iter()
                .any(|n| n.cells.iter().any(|c| c.kind == cell_type::ROUTE_METHOD)),
            "client HTTP calls must not become server ROUTE nodes"
        );
    }

    /// A11.5 — the absolute URL's authority lands on the ENDPOINT_HIT cell as
    /// `host`; a relative path writes no `host` at all.
    #[test]
    fn client_endpoint_carries_the_url_authority_as_host() {
        let source = r#"package client

func FetchUsers() {
    http.Get("http://api.example.com/users")
}

func Local(client *http.Client) {
    client.Get("/orders")
}
"#;
        let parse = parse_file(source, "client/client.go", "client", "", repo()).unwrap();
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
        let users = hit(endpoint_id(repo(), "GET", "/users"));
        assert!(
            users.ends_with(r#","confidence":"strong","host":"api.example.com"}"#),
            "{users}"
        );
        let orders = hit(endpoint_id(repo(), "GET", "/orders"));
        assert!(!orders.contains("host"), "{orders}");
    }

    #[test]
    fn relative_path_client_call_is_endpoint_not_route() {
        // A client with a relative path shares the `verb("/path")` shape with a
        // chi route registration; the client-receiver guard keeps it an ENDPOINT.
        let source = r#"package client

func hit(client *http.Client) {
    client.Get("/users")
}
"#;
        let parse = parse_file(source, "client/c.go", "client", "", repo()).unwrap();
        let ep = endpoint_id(repo(), "GET", "/users");
        assert!(parse.nodes.iter().any(|n| n.id == ep), "expected ENDPOINT");
        assert!(
            !parse
                .nodes
                .iter()
                .any(|n| n.cells.iter().any(|c| c.kind == cell_type::ROUTE_METHOD)),
            "client.Get('/users') must not emit a phantom ROUTE"
        );
    }

    #[test]
    fn gorilla_standalone_handlefunc_emits_any() {
        let parse = parse_file(
            GORILLA_STANDALONE,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        assert_eq!(
            route_methods(&parse, route_id(repo(), "ANY", "/legacy")),
            vec!["ANY".to_string()],
        );
    }

    // ------------------------------------------------------------------------
    // LA.32a: method-bearing registration forms + a POSITION per registration.
    // ------------------------------------------------------------------------

    /// Every ROUTE qname the parse recorded, sorted.
    fn route_qnames(parse: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = parse
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| parse.nav.qname_by_id.get(id).cloned())
            .collect();
        out.sort();
        out
    }

    /// The POSITION payloads on every emitted copy of `route`, in emit order.
    fn route_positions(parse: &FileParse, route: NodeId) -> Vec<String> {
        parse
            .nodes
            .iter()
            .filter(|n| n.id == route)
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::POSITION)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn handle_with_method_arg_emits_method_route() {
        const SRC: &str = r#"package server

func setup(r *gin.Engine) {
    r.Handle("PATCH", "/users/:id", patchUser)
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        let route = route_id(repo(), "PATCH", "/users/:id");
        assert_eq!(route_qnames(&parse), vec!["PATCH /users/:id".to_string()]);
        assert_eq!(route_methods(&parse, route), vec!["PATCH".to_string()]);
        assert_eq!(handled_by(&parse, route), vec![bare("patchUser")]);
    }

    #[test]
    fn echo_add_and_chi_method_forms() {
        const SRC: &str = r#"package server

func setup(e *echo.Echo, r chi.Router) {
    e.Add("DELETE", "/items/:id", deleteItem)
    r.Method("PUT", "/items/{id}", handlers.PutItem)
    r.MethodFunc("GET", "/health", health)
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        assert_eq!(
            route_qnames(&parse),
            vec![
                "DELETE /items/:id".to_string(),
                "GET /health".to_string(),
                "PUT /items/{id}".to_string(),
            ]
        );
        let del = route_id(repo(), "DELETE", "/items/:id");
        assert_eq!(route_methods(&parse, del), vec!["DELETE".to_string()]);
        assert_eq!(handled_by(&parse, del), vec![bare("deleteItem")]);
        let put = route_id(repo(), "PUT", "/items/{id}");
        assert_eq!(route_methods(&parse, put), vec!["PUT".to_string()]);
        assert_eq!(handled_by(&parse, put), vec![attr("handlers", "PutItem")]);
        let health = route_id(repo(), "GET", "/health");
        assert_eq!(route_methods(&parse, health), vec!["GET".to_string()]);
        assert_eq!(handled_by(&parse, health), vec![bare("health")]);
    }

    #[test]
    fn any_emits_any_method() {
        const SRC: &str = r#"package server

func setup(r *gin.Engine) {
    r.Any("/ping", anyPing)
    found := lo.Any(xs, isAdmin)
    _ = found
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        let ping = route_id(repo(), "ANY", "/ping");
        assert_eq!(route_qnames(&parse), vec!["ANY /ping".to_string()]);
        assert_eq!(route_methods(&parse, ping), vec!["ANY".to_string()]);
        assert_eq!(handled_by(&parse, ping), vec![bare("anyPing")]);
    }

    #[test]
    fn match_emits_one_route_per_listed_method() {
        const SRC: &str = r#"package server

func setup(r *gin.Engine, verbs []string) {
    r.Match([]string{"GET", "POST"}, "/orders", matchOrders)
    r.Match(verbs, "/dynamic", dyn)
    r.Match([]string{"GET", "fetch"}, "/mixed", mixed)
    ok := cache.Match("/orders")
    _ = ok
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        // A variable method list and a list with a non-verb are skipped, not
        // guessed at; a two-argument `Match` is never a registration.
        assert_eq!(
            route_qnames(&parse),
            vec!["GET /orders".to_string(), "POST /orders".to_string()]
        );
        for method in ["GET", "POST"] {
            let orders = route_id(repo(), method, "/orders");
            assert_eq!(route_methods(&parse, orders), vec![method.to_string()]);
            assert_eq!(handled_by(&parse, orders), vec![bare("matchOrders")]);
        }
    }

    #[test]
    fn go122_method_pattern_splits_method_and_path() {
        const SRC: &str = r#"package main

func main() {
    mux := http.NewServeMux()
    mux.HandleFunc("GET /items/{id}", getItem)
    mux.Handle("POST  /items", createItem)
    mux.HandleFunc("GET example.com/x", hostScoped)
    mux.HandleFunc("FETCH /y", notAVerb)
}
"#;
        let parse = parse_file(SRC, "main.go", "main", "", repo()).unwrap();
        assert_eq!(
            route_qnames(&parse),
            vec!["GET /items/{id}".to_string(), "POST /items".to_string()]
        );
        let item = route_id(repo(), "GET", "/items/{id}");
        assert_eq!(route_methods(&parse, item), vec!["GET".to_string()]);
        assert_eq!(handled_by(&parse, item), vec![bare("getItem")]);
        let items = route_id(repo(), "POST", "/items");
        assert_eq!(route_methods(&parse, items), vec!["POST".to_string()]);
        assert_eq!(handled_by(&parse, items), vec![bare("createItem")]);
    }

    #[test]
    fn method_literal_rejects_non_verbs() {
        const SRC: &str = r#"package server

func setup(h http.Header, wg *sync.WaitGroup, q url.Values) {
    wg.Add(1)
    h.Add("k", "/x")
    h.Add("get", "/x", f)
    q.Add("GET", "/x")
    r.Handle("PATCH", "users", patchUser)
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        assert!(route_qnames(&parse).is_empty(), "{:?}", route_qnames(&parse));
        assert!(parse.refs.iter().all(|r| r.category != edge_category::HANDLED_BY));
    }

    #[test]
    fn every_registration_has_a_position_cell() {
        const SRC: &str = r#"package server

func setup(r *gin.Engine) {
    r.GET("/users", List)
    r.POST("/users", Create)
    r.HandleFunc("/legacy", Legacy).Methods("GET", "PUT")
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        // LB.11a: GET and POST of one path are two nodes with one POSITION
        // each, at their own registration.
        let get_users = route_id(repo(), "GET", "/users");
        let post_users = route_id(repo(), "POST", "/users");
        assert_eq!(
            route_positions(&parse, get_users),
            vec![r#"{"file":"server.go","start_line":3,"end_line":3}"#.to_string()]
        );
        assert_eq!(
            route_positions(&parse, post_users),
            vec![r#"{"file":"server.go","start_line":4,"end_line":4}"#.to_string()]
        );
        // The Gorilla chain places both of its registrations at the inner
        // `HandleFunc` call, one per verb node.
        let get_legacy = route_id(repo(), "GET", "/legacy");
        let put_legacy = route_id(repo(), "PUT", "/legacy");
        for legacy in [get_legacy, put_legacy] {
            assert_eq!(
                route_positions(&parse, legacy),
                vec![r#"{"file":"server.go","start_line":5,"end_line":5}"#.to_string()]
            );
        }
        // Each emitted copy is exactly [POSITION, ROUTE_METHOD]: POSITION
        // first, so a first-POSITION reader sees the registration.
        let ids = [get_users, post_users, get_legacy, put_legacy];
        for n in parse.nodes.iter().filter(|n| ids.contains(&n.id)) {
            let kinds: Vec<_> = n.cells.iter().map(|c| c.kind).collect();
            assert_eq!(kinds, vec![cell_type::POSITION, cell_type::ROUTE_METHOD]);
        }
        // The ROUTE_METHOD payload keeps its 1-based line.
        let first = parse.nodes.iter().find(|n| n.id == get_users).unwrap();
        match &first.cells[1].payload {
            CellPayload::Json(j) => assert!(j.contains(r#""line":4,"#), "{j}"),
            other => panic!("ROUTE_METHOD is not JSON: {other:?}"),
        }
    }

    /// LB.11a: two registrations of one (method, path) — two routers mounting
    /// the same path — stay one node carrying both POSITION cells, and nav
    /// records it once.
    #[test]
    fn same_method_and_path_twice_is_one_node_with_two_positions() {
        const SRC: &str = r#"package server

func setup(r *gin.Engine, admin *gin.Engine) {
    r.GET("/users", List)
    admin.GET("/users", ListAll)
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        let users = route_id(repo(), "GET", "/users");
        assert_eq!(route_qnames(&parse), vec!["GET /users".to_string()]);
        assert_eq!(
            route_positions(&parse, users),
            vec![
                r#"{"file":"server.go","start_line":3,"end_line":3}"#.to_string(),
                r#"{"file":"server.go","start_line":4,"end_line":4}"#.to_string(),
            ]
        );
        assert_eq!(handled_by(&parse, users), vec![bare("List"), bare("ListAll")]);
    }

    #[test]
    fn handle_with_a_path_first_is_still_any() {
        const SRC: &str = r#"package server

func setup(r *mux.Router) {
    r.Handle("/static", fileServer)
    http.Handle("/metrics", promhttp.Handler())
}
"#;
        let parse = parse_file(SRC, "server.go", "server", "", repo()).unwrap();
        assert_eq!(
            route_qnames(&parse),
            vec!["ANY /metrics".to_string(), "ANY /static".to_string()]
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "ANY", "/static")),
            vec!["ANY".to_string()]
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "ANY", "/metrics")),
            vec!["ANY".to_string()]
        );
    }

    // ---- A7.6: DI container registration → INJECTS ----

    /// INJECTS refs as (from, qualifier) pairs, in emission order.
    fn injects_refs(parse: &FileParse) -> Vec<(NodeId, CallQualifier)> {
        parse
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INJECTS)
            .map(|r| (r.from, r.qualifier.clone()))
            .collect()
    }

    fn func_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname)
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.to_string())
    }

    #[test]
    fn wire_build_emits_injects_refs_for_each_provider() {
        let source = r#"package app

import (
    "github.com/google/wire"
    "github.com/foo/bar/repo"
)

func InitializeUserService() *UserService {
    wire.Build(NewUserService, repo.NewUserRepo, NewUserService, wire.Bind(new(Store), new(*Repo)))
    return nil
}
"#;
        let parse = parse_file(source, "app/wire.go", "app", "github.com/foo/bar", repo()).unwrap();
        let injector = func_id("app::InitializeUserService");
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "app");

        // Duplicate provider collapses; wire.Bind's `new(...)` type args emit nothing.
        assert_eq!(
            injects_refs(&parse),
            vec![
                (injector, bare("NewUserService")),
                (
                    injector,
                    CallQualifier::Attribute {
                        base: "repo".to_string(),
                        name: "NewUserRepo".to_string(),
                    }
                ),
            ]
        );
        assert!(
            parse
                .refs
                .iter()
                .filter(|r| r.category == edge_category::INJECTS)
                .all(|r| r.from_module == module_id)
        );
    }

    #[test]
    fn non_container_selector_call_emits_no_injects() {
        // `log.Printf(NewThing)` is not a container. `wire.Build` without the
        // wire import is not one either: the gate follows imports, not names.
        let source = r#"package app

import "log"

func Run() {
    log.Printf(NewThing)
    wire.Build(NewThing)
    fx.Provide(NewThing)
    c.Provide(NewThing)
}
"#;
        let parse = parse_file(source, "app/run.go", "app", "", repo()).unwrap();
        assert!(injects_refs(&parse).is_empty(), "{:?}", injects_refs(&parse));
    }

    #[test]
    fn fx_alias_annotate_and_dig_container_methods_emit_injects() {
        let source = r#"package main

import (
    uberfx "go.uber.org/fx"
    "go.uber.org/dig"
)

func main() {
    uberfx.New(uberfx.Provide(NewA, uberfx.Annotate(NewB, uberfx.As(new(I)))), uberfx.Invoke(Run), uberfx.Supply(cfg))
    c := dig.New()
    c.Provide(NewC)
}
"#;
        let parse = parse_file(source, "cmd/main.go", "main", "", repo()).unwrap();
        let main_fn = func_id("main::main");
        // `uberfx.New` registers nothing; `Supply` takes values, not providers.
        assert_eq!(
            injects_refs(&parse),
            vec![
                (main_fn, bare("NewA")),
                (main_fn, bare("NewB")),
                (main_fn, bare("Run")),
                (main_fn, bare("NewC")),
            ]
        );
    }

    #[test]
    fn wire_newset_var_emits_injects_from_state_var() {
        let source = r#"package app

import "github.com/google/wire"

var ProviderSet = wire.NewSet(NewA, NewB)
"#;
        let parse = parse_file(source, "app/set.go", "app", "", repo()).unwrap();
        let set = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, "app::ProviderSet");
        assert!(parse.nodes.iter().any(|n| n.id == set), "expected STATE_VAR");
        assert_eq!(
            injects_refs(&parse),
            vec![(set, bare("NewA")), (set, bare("NewB"))]
        );
    }

    // ---- LA.18d: func-literal route handler → HANDLED_BY its callees ----

    /// HANDLED_BY qualifiers from `route`, in emission order.
    fn handled_by(parse: &FileParse, route: NodeId) -> Vec<CallQualifier> {
        parse
            .refs
            .iter()
            .filter(|r| r.from == route && r.category == edge_category::HANDLED_BY)
            .map(|r| r.qualifier.clone())
            .collect()
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn func_literal_handler_refs_its_callees() {
        let source = r#"package main

import "net/http"

func main() {
    hub := newHub()
    http.HandleFunc("/ws", func(w http.ResponseWriter, r *http.Request) {
        serveWs(hub, w, r)
    })
}
"#;
        let parse = parse_file(source, "main.go", "main", "example.com/chat", repo()).unwrap();
        let ws = route_id(repo(), "ANY", "/ws");
        assert_eq!(route_methods(&parse, ws), vec!["ANY".to_string()]);
        assert_eq!(handled_by(&parse, ws), vec![bare("serveWs")]);
        // Same module stamp as the identifier arm.
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "main");
        assert!(parse.refs.iter().all(|r| r.from != ws || r.from_module == module_id));
    }

    #[test]
    fn gin_closure_skips_param_receiver_calls() {
        let source = r#"package server

import "github.com/gin-gonic/gin"

func setup(r *gin.Engine) {
    r.GET("/x", func(c *gin.Context) {
        c.JSON(200, build())
    })
}
"#;
        let parse = parse_file(source, "server/server.go", "server", "example.com/app", repo())
            .unwrap();
        assert_eq!(
            handled_by(&parse, route_id(repo(), "GET", "/x")),
            vec![bare("build")]
        );
    }

    #[test]
    fn external_package_calls_are_not_refs() {
        let source = r#"package main

import (
    "encoding/json"
    "log"
    "net/http"
)

func main() {
    http.HandleFunc("/health", func(w http.ResponseWriter, r *http.Request) {
        v := []string{"ok"}
        log.Println("health", len(v))
        json.NewEncoder(w).Encode(v)
        writeHealth(w)
    })
}
"#;
        let parse = parse_file(source, "main.go", "main", "example.com/health", repo()).unwrap();
        assert_eq!(
            handled_by(&parse, route_id(repo(), "ANY", "/health")),
            vec![bare("writeHealth")]
        );
    }

    #[test]
    fn versioned_and_aliased_external_imports_are_not_refs() {
        let source = r#"package main

import (
    "net/http"

    "github.com/go-chi/chi/v5"
    jsoniter "github.com/json-iterator/go"
    "gopkg.in/yaml.v3"
)

func main() {
    r := chi.NewRouter()
    r.Get("/items/{id}", func(w http.ResponseWriter, req *http.Request) {
        id := chi.URLParam(req, "id")
        out, _ := yaml.Marshal(id)
        jsoniter.Marshal(out)
        showItem(w, id)
    })
}
"#;
        let parse = parse_file(source, "main.go", "main", "example.com/shop", repo()).unwrap();
        assert_eq!(
            handled_by(&parse, route_id(repo(), "GET", "/items/{id}")),
            vec![bare("showItem")]
        );
    }

    #[test]
    fn repo_local_package_call_is_a_ref() {
        let source = r#"package server

import (
    "example.com/app/handlers"
    "github.com/gin-gonic/gin"
)

func setup(r *gin.Engine, h *Hub) {
    r.GET("/users", func(c *gin.Context) {
        handlers.ListUsers(c)
        h.ServeWS(c.Writer, c.Request)
    })
}
"#;
        let parse = parse_file(source, "server/server.go", "server", "example.com/app", repo())
            .unwrap();
        // A repo-local package and a captured variable both stay.
        assert_eq!(
            handled_by(&parse, route_id(repo(), "GET", "/users")),
            vec![attr("handlers", "ListUsers"), attr("h", "ServeWS")]
        );
    }

    #[test]
    fn nested_func_literal_calls_are_not_refs() {
        let source = r#"package main

import "net/http"

func main() {
    http.HandleFunc("/n", func(w http.ResponseWriter, r *http.Request) {
        defer func() { cleanup() }()
        go func() { background() }()
        serve(w, r)
        serve(w, r)
    })
}
"#;
        let parse = parse_file(source, "main.go", "main", "example.com/app", repo()).unwrap();
        assert_eq!(
            handled_by(&parse, route_id(repo(), "ANY", "/n")),
            vec![bare("serve")]
        );
        // CA.1: nothing inside the route literal, nested closures included,
        // is a call of the function that registers it.
        let from_main = calls_from(&parse, func_id("main::main"));
        for name in ["cleanup", "background", "serve"] {
            assert!(
                !from_main.iter().any(|(q, _)| *q == bare(name)),
                "main::main must not call {name}: {from_main:?}"
            );
        }
    }

    // ---- CA.1: calls inside func literals belong to the enclosing function ----

    /// The CallSites from `from`, in emission order, with their 0-based rows.
    fn calls_from(parse: &FileParse, from: NodeId) -> Vec<(CallQualifier, u32)> {
        parse
            .calls
            .iter()
            .filter(|c| c.from == from)
            .map(|c| (c.qualifier.clone(), c.line))
            .collect()
    }

    /// The bench fixture's source (`go-closure-calls`), so the unit tests and
    /// the graded fixture describe one program.
    const CLOSURE_APP: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/go-closure-calls/app.go");

    fn closure_app() -> FileParse {
        parse_file(CLOSURE_APP, "app.go", "app", "example.com/closures", repo()).unwrap()
    }

    fn row_of(source: &str, needle: &str) -> u32 {
        let row = source.lines().position(|l| l.contains(needle));
        u32::try_from(row.expect("needle in source")).unwrap()
    }

    #[test]
    fn closure_calls_belong_to_the_enclosing_function() {
        let parse = closure_app();
        let provider = calls_from(&parse, func_id("app::Provider"));
        assert!(
            provider.contains(&(bare("NewRepo"), row_of(CLOSURE_APP, "repo = NewRepo()"))),
            "once.Do closure: {provider:?}"
        );
        let start = calls_from(&parse, func_id("app::Start"));
        for (name, site) in [
            ("worker", "\t\tworker()"),
            ("cleanup", "\t\tcleanup()"),
            ("flush", "return flush()"),
        ] {
            assert!(
                start.contains(&(bare(name), row_of(CLOSURE_APP, site))),
                "go / defer / g.Go closure must call {name}: {start:?}"
            );
        }
        // The returned middleware closure (Kina JWTAuthMiddleware shape).
        let auth = calls_from(&parse, func_id("app::Auth"));
        assert!(
            auth.contains(&(bare("parseToken"), row_of(CLOSURE_APP, "!parseToken(r)"))),
            "returned closure: {auth:?}"
        );
    }

    #[test]
    fn route_literal_callees_are_not_calls_of_the_registrar() {
        let parse = closure_app();
        let routes = calls_from(&parse, func_id("app::Routes"));
        assert!(
            !routes.iter().any(|(q, _)| *q == bare("writeHealth")),
            "{routes:?}"
        );
        assert_eq!(
            handled_by(&parse, route_id(repo(), "ANY", "/health")),
            vec![bare("writeHealth")]
        );
    }

    #[test]
    fn nested_closures_are_walked() {
        let source = r#"package app

func flush() {}

func Run() {
    go func() {
        defer func() {
            flush()
        }()
    }()
}
"#;
        let parse = parse_file(source, "app/run.go", "app", "", repo()).unwrap();
        assert_eq!(
            calls_from(&parse, func_id("app::Run")),
            vec![(bare("flush"), row_of(source, "        flush()"))]
        );
    }

    #[test]
    fn method_closure_keeps_the_receiver() {
        let source = r#"package svc

import "sync"

type Svc struct{ once sync.Once }

func (s *Svc) init() {}

func (s *Svc) Boot() {
    s.once.Do(func() {
        s.init()
    })
}
"#;
        let parse = parse_file(source, "svc/svc.go", "svc", "", repo()).unwrap();
        let boot = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "svc::Svc::Boot");
        let calls = calls_from(&parse, boot);
        assert!(
            calls.contains(&(
                CallQualifier::SelfMethod("init".to_string()),
                row_of(source, "s.init()")
            )),
            "{calls:?}"
        );
    }

    /// LA.18d + LB.11a: a `.Methods("GET", "POST")` chain over one literal is
    /// two route nodes; each carries the literal's capped callee refs once.
    #[test]
    fn func_literal_callees_are_capped_and_expanded_once_per_method_route() {
        let source = r#"package main

import "github.com/gorilla/mux"

func main() {
    r := mux.NewRouter()
    r.HandleFunc("/many", func(w http.ResponseWriter, req *http.Request) {
        a1(); a2(); a3(); a4(); a5(); a6(); a7(); a8(); a9(); a10()
    }).Methods("GET", "POST")
}
"#;
        let parse = parse_file(source, "main.go", "main", "example.com/app", repo()).unwrap();
        let expected: Vec<CallQualifier> = (1..=8).map(|i| bare(&format!("a{i}"))).collect();
        for method in ["GET", "POST"] {
            let many = route_id(repo(), method, "/many");
            assert_eq!(route_methods(&parse, many), vec![method.to_string()]);
            assert_eq!(handled_by(&parse, many), expected);
        }
    }

    #[test]
    fn import_local_names_follow_go_package_naming() {
        assert_eq!(import_local_names("log"), vec!["log"]);
        assert_eq!(import_local_names("encoding/json"), vec!["json"]);
        assert_eq!(import_local_names("github.com/go-chi/chi/v5"), vec!["chi"]);
        assert_eq!(import_local_names("gopkg.in/yaml.v3"), vec!["yaml.v3", "yaml"]);
        assert_eq!(
            import_local_names("github.com/mattn/go-sqlite3"),
            vec!["go-sqlite3", "sqlite3"]
        );
        assert_eq!(
            import_local_names("github.com/stripe/stripe-go/v76"),
            vec!["stripe-go", "stripe"]
        );
    }

    // ------------------------------------------------------------------------
    // LA.23c: receiver-field calls and struct field types
    // ------------------------------------------------------------------------

    fn method_calls(parse: &FileParse, qname: &str) -> Vec<CallQualifier> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, qname);
        parse
            .calls
            .iter()
            .filter(|c| c.from == id)
            .map(|c| c.qualifier.clone())
            .collect()
    }

    fn complex(receiver: &str, name: &str) -> CallQualifier {
        CallQualifier::ComplexReceiver {
            receiver: receiver.to_string(),
            name: name.to_string(),
        }
    }

    fn struct_fields(parse: &FileParse, qname: &str) -> Vec<(String, String)> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STRUCT, qname);
        let mut out: Vec<(String, String)> = parse
            .nav
            .field_types
            .get(&id)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    const FIELD_CALLS: &str = r#"package shop

type UserService struct {
    repo *UserRepo
}

func (s *UserService) Get(id int) string {
    x := other()
    x.repo.Find(id)
    s.a.b.Find(id)
    return s.repo.Find(id)
}

func (svc UserService) Put(id int) {
    svc.repo.Save(id)
}
"#;

    #[test]
    fn receiver_field_calls_normalise_to_self_field() {
        let parse = parse_file(FIELD_CALLS, "svc.go", "shop", "", repo()).unwrap();
        let get = method_calls(&parse, "shop::UserService::Get");
        // Receiver `s`: one hop off the receiver is normalised.
        assert!(get.contains(&complex("self.repo", "Find")), "{get:?}");
        // A local variable that happens to hold a struct is untouched.
        assert!(get.contains(&complex("x.repo", "Find")), "{get:?}");
        // CA.2a: a longer chain off the receiver is normalised whole (A6.2a's
        // `receiver_field` still rejects it: only one hop has a field type).
        assert!(get.contains(&complex("self.a.b", "Find")), "{get:?}");
        assert!(!get.contains(&complex("s.a.b", "Find")), "{get:?}");
        // Receiver `svc`, value receiver.
        let put = method_calls(&parse, "shop::UserService::Put");
        assert_eq!(put, vec![complex("self.repo", "Save")]);
    }

    #[test]
    fn free_function_selector_chain_stays_raw() {
        let source = r#"package shop

func run(s *UserService) {
    s.repo.Find(1)
}
"#;
        let parse = parse_file(source, "run.go", "shop", "", repo()).unwrap();
        let run = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "shop::run");
        let calls: Vec<&CallQualifier> =
            parse.calls.iter().filter(|c| c.from == run).map(|c| &c.qualifier).collect();
        assert_eq!(calls, vec![&complex("s.repo", "Find")]);
    }

    const FIELD_DECLS: &str = r#"package shop

import (
    "net"

    "example.com/shop/store"
    "go.uber.org/zap"
)

type Cache interface {
    Get(key string) string
}

type Logger struct {
    *store.Logger
    sink store.Sink
}

type UserService struct {
    repo    *UserRepo
    db      store.DB
    pdb     *store.DB
    a, b    *Audit
    cache   Cache
    pp      **UserRepo
    UserRepo
    *Audit
    store.Tx
    conn    net.Conn
    log     *zap.Logger
    *zap.SugaredLogger
    tags    []string
    byID    map[int]*UserRepo
    ch      chan int
    fn      func() error
    gen     Box[int]
    anon    struct{ n int }
    Base[int]
}
"#;

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(f, t)| (f.to_string(), t.to_string())).collect()
    }

    #[test]
    fn struct_field_declarations_record_their_type() {
        let parse = parse_file(FIELD_DECLS, "svc.go", "shop", "example.com/shop", repo()).unwrap();
        let expect = pairs(&[
            // Embedded fields record under the type's own name; `*Audit`
            // embedded and `a, b *Audit` agree on the type. CA.2a: an
            // embedded generic instantiation `Base[int]` records its base.
            ("Audit", "Audit"),
            ("Base", "Base"),
            ("Tx", "Tx"),
            ("UserRepo", "UserRepo"),
            // Multi-name: both names.
            ("a", "Audit"),
            ("b", "Audit"),
            // Interface-typed: recorded (A6.6 adds the interface fallback).
            ("cache", "Cache"),
            // Qualified through an in-module import: the type name side.
            ("db", "DB"),
            // CA.2a: a generic instantiation binds by its base type.
            ("gen", "Box"),
            ("pdb", "DB"),
            ("pp", "UserRepo"),
            // Pointer: the pointee.
            ("repo", "UserRepo"),
        ]);
        // Slices, maps, channels, funcs, anonymous structs and types of
        // packages outside the module (`net.Conn`, `*zap.Logger`, embedded
        // `*zap.SugaredLogger`): none.
        assert_eq!(struct_fields(&parse, "shop::UserService"), expect);
    }

    #[test]
    fn qualified_field_named_like_a_local_type_records_nothing() {
        let parse = parse_file(FIELD_DECLS, "svc.go", "shop", "example.com/shop", repo()).unwrap();
        // Embedded `*store.Logger` inside `type Logger`: the graph binds types
        // by bare name, so recording `Logger` would bind every
        // `l.Logger.Info()` back onto `Logger` itself. `sink` is unaffected.
        assert_eq!(struct_fields(&parse, "shop::Logger"), pairs(&[("sink", "Sink")]));
    }

    #[test]
    fn without_a_module_prefix_qualified_fields_record_nothing() {
        // No go.mod: every import is external, so only local types remain.
        let parse = parse_file(FIELD_DECLS, "svc.go", "shop", "", repo()).unwrap();
        let expect = pairs(&[
            ("Audit", "Audit"),
            ("Base", "Base"),
            ("UserRepo", "UserRepo"),
            ("a", "Audit"),
            ("b", "Audit"),
            ("cache", "Cache"),
            ("gen", "Box"),
            ("pp", "UserRepo"),
            ("repo", "UserRepo"),
        ]);
        assert_eq!(struct_fields(&parse, "shop::UserService"), expect);
        assert!(struct_fields(&parse, "shop::Logger").is_empty());
    }

    #[test]
    fn go_type_name_shapes() {
        let src = "package p\nvar a *T\nvar b pkg.T\nvar c T\nvar d []T\nvar e map[K]T\nvar f chan T\nvar g func()\nvar h G[T]\nvar i **pkg.T\nvar j *pkg.G[T, U]\n";
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(src, None).unwrap();
        let root = tree.root_node();
        let mut got = Vec::new();
        let mut cursor = root.walk();
        for decl in root.named_children(&mut cursor) {
            if decl.kind() != "var_declaration" {
                continue;
            }
            let mut dc = decl.walk();
            for spec in decl.named_children(&mut dc) {
                let ty = spec.child_by_field_name("type").unwrap();
                got.push(go_type_name(ty, src.as_bytes()));
            }
        }
        let t = || Some("T".to_string());
        // CA.2a: a generic instantiation names its base type.
        let g = || Some("G".to_string());
        assert_eq!(
            got,
            vec![t(), t(), t(), None, None, None, None, g(), t(), g()]
        );
    }

    // ---- CA.2a: receiver-type facts (return types, params, locals, vars) ----

    /// A name -> type map as sorted pairs (`None` = no entry at all).
    fn sorted(m: Option<&HashMap<String, String>>) -> Option<Vec<(String, String)>> {
        m.map(|m| {
            let mut v: Vec<(String, String)> =
                m.iter().map(|(k, t)| (k.clone(), t.clone())).collect();
            v.sort();
            v
        })
    }

    fn locals_of(parse: &FileParse, id: NodeId) -> Option<Vec<(String, String)>> {
        sorted(parse.nav.local_types.get(&id))
    }

    fn method_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, qname)
    }

    #[test]
    fn return_types_keep_the_package_qualifier() {
        let source = r#"package app

import (
    "example.com/app/models"
    "example.com/app/repositories"
)

type Bundle struct{}

type Repo struct{}

type Collection[T any] struct{}

type Page[T any] struct{}

type Builder interface {
    Build() Bundle
    Close() error
}

func UserRepository() *repositories.UserRepository {
    return nil
}

func (r *Repo) Find() (*models.User, error) {
    return nil, nil
}

func Get[T any]() T {
    var zero T
    return zero
}

func N() error {
    return nil
}

func (c *Collection[T]) One() (*T, error) {
    return nil, nil
}

func (c *Collection[T]) Paged() Page[T] {
    return Page[T]{}
}

func Names() []string {
    return nil
}
"#;
        let parse = parse_file(source, "app/app.go", "app", "example.com/app", repo()).unwrap();
        let mut got: Vec<(String, String)> = parse
            .nav
            .return_types
            .iter()
            .map(|(id, ty)| (parse.nav.qname_by_id[id].clone(), ty.clone()))
            .collect();
        got.sort();
        // `Get[T]() T` and the generic receiver's `*T` name a type parameter;
        // `error` and `[]string` own no in-repo method.
        assert_eq!(
            got,
            pairs(&[
                ("app::Builder::Build", "Bundle"),
                ("app::Collection::Paged", "Page"),
                ("app::Repo::Find", "models.User"),
                ("app::UserRepository", "repositories.UserRepository"),
            ])
        );
    }

    #[test]
    fn params_and_locals_are_recorded() {
        let source = r#"package app

func H(repo *repositories.UserRepository, n int) {
    a := services.UserRepository()
    b, err := repo.Find()
    c := &models.User{}
    d := x.y
    for _, u := range us {
        _ = u
    }
    _ = err
}
"#;
        let parse = parse_file(source, "app/h.go", "app", "", repo()).unwrap();
        assert_eq!(
            locals_of(&parse, func_id("app::H")),
            Some(pairs(&[
                ("a", "services.UserRepository()"),
                ("b", "repo.Find()"),
                ("c", "models.User"),
                ("d", ""),
                ("err", ""),
                ("n", ""),
                ("repo", "repositories.UserRepository"),
                ("u", ""),
            ]))
        );
    }

    #[test]
    fn every_binding_form_records_a_local() {
        let source = r#"package app

type Svc struct{ repo *Repo }

func (s *Svc) M(a, b *Repo, opts ...Option) {
    var x Repo
    var y = NewRepo(ctx, 1)
    var p, q = &Repo{}, Other{}
    z := new(Repo)
    w := new(store.Repo)
    r := s.repo
    k, v := Repo{}, store.Page[User]{}
    m := make(map[string]int)
    switch t := val.(type) {
    case int:
        _ = t
    }
    select {
    case msg := <-ch:
        _ = msg
    }
    if f, ok := s.repo.Get(); ok {
        _ = f
    }
    for i := 0; i < 3; i++ {
    }
    _, _ = k, m
}

func Gen[T any](x T, y *T, h Handler[T]) {}
"#;
        let parse = parse_file(source, "app/m.go", "app", "", repo()).unwrap();
        // The receiver `s` is not a local: SelfMethod / `self.` cover it.
        assert_eq!(
            locals_of(&parse, method_id("app::Svc::M")),
            Some(pairs(&[
                ("a", "Repo"),
                ("b", "Repo"),
                ("f", "self.repo.Get()"),
                ("i", ""),
                ("k", "Repo"),
                ("m", ""),
                ("msg", ""),
                ("ok", ""),
                ("opts", ""),
                ("p", "Repo"),
                ("q", "Other"),
                ("r", "self.repo"),
                ("t", ""),
                ("v", "store.Page"),
                ("w", "store.Repo"),
                ("x", "Repo"),
                ("y", "NewRepo()"),
                ("z", "Repo"),
            ]))
        );
        // A parameter typed by a type parameter has no known type.
        assert_eq!(
            locals_of(&parse, func_id("app::Gen")),
            Some(pairs(&[("h", "Handler"), ("x", ""), ("y", "")]))
        );
    }

    #[test]
    fn package_vars_use_the_module_scope() {
        let source = r#"package app

import "sync"

var defaultRepo = repositories.NewUserRepository()
var svc = New()
var once sync.Once
"#;
        let parse = parse_file(source, "app/vars.go", "app", "", repo()).unwrap();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "app");
        assert_eq!(
            locals_of(&parse, module),
            Some(pairs(&[
                ("defaultRepo", "repositories.NewUserRepository()"),
                ("once", "sync.Once"),
                ("svc", "New()"),
            ]))
        );
        // The module scope holds package vars only: no fn scope appeared.
        assert_eq!(parse.nav.local_types.len(), 1);
    }

    #[test]
    fn chained_receivers_are_normalised() {
        let source = r#"package app

func H(id int) {
    Services.UserRepository().FindByID(id)
    repositories.NewX(client, db).Find()
}
"#;
        let parse = parse_file(source, "app/h.go", "app", "", repo()).unwrap();
        let calls: Vec<CallQualifier> = calls_from(&parse, func_id("app::H"))
            .into_iter()
            .map(|(q, _)| q)
            .collect();
        assert!(
            calls.contains(&complex("Services.UserRepository()", "FindByID")),
            "{calls:?}"
        );
        assert!(
            calls.contains(&complex("repositories.NewX()", "Find")),
            "{calls:?}"
        );
    }

    #[test]
    fn generic_field_type_binds_by_base() {
        let source = r#"package app

import (
    "sync/atomic"

    "example.com/app/store"
)

type R struct {
    c *Collection[User]
    p atomic.Pointer[Cfg]
    s store.Page[User]
}
"#;
        let parse = parse_file(source, "app/r.go", "app", "example.com/app", repo()).unwrap();
        // `atomic` is outside the module: its `Pointer` records nothing.
        assert_eq!(
            struct_fields(&parse, "app::R"),
            pairs(&[("c", "Collection"), ("s", "Page")])
        );
    }

    #[test]
    fn closure_params_are_locals_of_the_enclosing_fn() {
        let source = r#"package app

import "github.com/gin-gonic/gin"

func Run() {
    go func(repo *Repo) {
        repo.Save()
    }(r)
}

func Routes(r *gin.Engine) {
    r.GET("/x", func(c *gin.Context) {
        c.JSON(200, nil)
    })
}
"#;
        let parse = parse_file(source, "app/run.go", "app", "example.com/app", repo()).unwrap();
        assert_eq!(
            locals_of(&parse, func_id("app::Run")),
            Some(pairs(&[("repo", "Repo")]))
        );
        // A route-handler literal is never drained: its `c` records nothing.
        assert_eq!(
            locals_of(&parse, func_id("app::Routes")),
            Some(pairs(&[("r", "gin.Engine")]))
        );
    }

    // ---- LA.13: per-go.mod module map ----------------------------------------

    fn nested() -> GoModules {
        GoModules::from_entries(vec![
            ("svc-b".into(), "example.com/svc-b".into()),
            ("svc".into(), "example.com/svc".into()),
        ])
    }

    #[test]
    fn go_modules_map_at_a_slash_boundary() {
        let go = nested();
        // Own module: svc/go.mod maps under svc/.
        assert_eq!(
            go.map_import("svc::cmd::main", "example.com/svc/internal/store"),
            Some("svc::internal::store".to_string())
        );
        // Module `example.com/svc` never claims `example.com/svc-b/...`.
        assert_eq!(
            go.map_import("svc::cmd::main", "example.com/svc-b/client"),
            Some("svc-b::client".to_string())
        );
        assert_eq!(go.map_import("svc::cmd::main", "example.com/svcx/y"), None);
        assert_eq!(go.map_import("svc::cmd::main", "github.com/google/uuid"), None);
        // A nested module's own path names its root dir.
        assert_eq!(go.map_import("svc::cmd::main", "example.com/svc"), Some("svc".to_string()));
        // Root-only: HEAD semantics, the root module's own path is degenerate.
        let root = GoModules::root_only("example.com/app");
        assert_eq!(root.map_import("x::y", "example.com/app/x"), Some("x".to_string()));
        assert_eq!(root.map_import("x::y", "example.com/app"), Some(String::new()));
        assert_eq!(root.map_import("x::y", "example.com/apple/x"), None);
        assert!(GoModules::root_only("").is_empty());
    }

    #[test]
    fn go_modules_pick_the_longest_module_then_the_own_one() {
        // Two go.mods declaring one module path: the importer's own wins.
        let go = GoModules::from_entries(vec![
            ("a".into(), "example.com/m".into()),
            ("b".into(), "example.com/m".into()),
        ]);
        assert_eq!(go.map_import("b::main", "example.com/m/p"), Some("b::p".to_string()));
        assert_eq!(go.map_import("a::main", "example.com/m/p"), Some("a::p".to_string()));
        // An importer in neither: the first root by dir, never HashMap order.
        assert_eq!(go.map_import("c::main", "example.com/m/p"), Some("a::p".to_string()));
        // A file of a nested module still reaches the module around it.
        let go = GoModules::from_entries(vec![
            (String::new(), "example.com/root".into()),
            ("tools".into(), "example.com/tools".into()),
        ]);
        assert_eq!(
            go.map_import("tools::gen::main", "example.com/root/pkg/util"),
            Some("pkg::util".to_string())
        );
        assert_eq!(
            go.map_import("cmd::main", "example.com/tools/gen"),
            Some("tools::gen".to_string())
        );
        // A longer module path beats a shorter one that also holds the import.
        let go = GoModules::from_entries(vec![
            (String::new(), "example.com/root".into()),
            ("api".into(), "example.com/root/api/v2".into()),
        ]);
        assert_eq!(
            go.map_import("web::main", "example.com/root/api/v2/users"),
            Some("api::users".to_string())
        );
    }

    #[test]
    fn go_modules_context_key_is_order_free() {
        let a = nested();
        let b = GoModules::from_entries(vec![
            ("svc".into(), "example.com/svc".into()),
            ("./svc-b/".into(), "example.com/svc-b".into()),
            ("svc".into(), "example.com/svc".into()),
            ("empty".into(), String::new()),
        ]);
        assert_eq!(a, b);
        assert_eq!(a.context_key(), "svc=example.com/svc;svc-b=example.com/svc-b");
        assert_eq!(GoModules::root_only("example.com/app").context_key(), "=example.com/app");
        assert_eq!(GoModules::default().context_key(), "");
        let pairs: Vec<(&str, &str)> = a.iter().collect();
        assert_eq!(pairs, vec![("svc", "example.com/svc"), ("svc-b", "example.com/svc-b")]);
    }

    #[test]
    fn nested_modules_turn_intra_repo_imports_local() {
        const MAIN: &str = "package main\n\nimport (\n\t\"github.com/google/uuid\"\n\t\"example.com/svc/internal/store\"\n\t\"example.com/svc-b/client\"\n)\n\nfunc main() {\n\t_ = uuid.New()\n\tstore.Save()\n\tclient.Get()\n}\n";
        let parse =
            parse_file_with_modules(MAIN, "svc/cmd/main.go", "svc::cmd::main", &nested(), repo())
                .unwrap();
        let paths: Vec<&str> = parse
            .imports
            .iter()
            .filter_map(|i| match &i.target {
                ImportTarget::Module { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            paths,
            vec!["github.com::google::uuid", "svc::internal::store", "svc-b::client"]
        );
        // Root-only parse of the same file: both intra-repo imports stay raw.
        let raw = parse_file(MAIN, "svc/cmd/main.go", "svc::cmd::main", "", repo()).unwrap();
        assert!(raw.imports.iter().any(|i| matches!(&i.target,
            ImportTarget::Module { path, .. } if path == "example.com::svc::internal::store")));
    }

    // ---- CA.3a: normalised method signatures ----

    /// Every recorded method signature as sorted `(qname, signature)` pairs.
    fn sigs_of(parse: &FileParse) -> Vec<(String, String)> {
        let mut got: Vec<(String, String)> = parse
            .nav
            .method_sigs
            .iter()
            .map(|(id, sig)| (parse.nav.qname_by_id[id].clone(), sig.clone()))
            .collect();
        got.sort();
        got
    }

    #[test]
    fn go_signatures_drop_names_and_qualifiers() {
        let source = r#"package app

import (
    "context"
    "encoding/json"

    "example.com/app/balancer"
    "example.com/app/base"
    "example.com/app/credentials"
    "example.com/app/x"
)

type R struct{}

type bb struct{}

type Getter interface {
    Get(string) string
    Put(string, string) error
    Done() <-chan struct{}
}

type PickerBuilder interface {
    Build(PickerBuildInfo) Picker
}

func (r *R) Get(key string) string { return key }

func (r *R) Put(key, value string) error { return nil }

func (r *R) Build(config json.RawMessage) (credentials.Bundle, func(), error) {
    return nil, nil, nil
}

func (r *R) Log(format string, args ...any) {}

func (r *R) Close() {}

func (r *R) Walk(fn func(ctx context.Context) error) map[string]*x.Y { return nil }

func (r R) Any(v interface{}, ch chan<- []byte, arr [4]byte, p (int)) (n int, err error) {
    return 0, nil
}

func (r *R) Done() <-chan struct{} { return nil }

func (r *R) Page(q x.Query[x.User], f func() (err error)) (*Page[User], error) {
    return nil, nil
}

func (r *R) Meta(o interface {
    Name() string
    Apply(*x.Cfg) error
}) struct {
    A, B int
    x.Base
} {
    return struct {
        A, B int
        x.Base
    }{}
}

func (b *bb) Build(info base.PickerBuildInfo) balancer.Picker { return nil }
"#;
        let parse = parse_file(source, "app/app.go", "app", "example.com/app", repo()).unwrap();
        assert_eq!(
            sigs_of(&parse),
            pairs(&[
                ("app::Getter::Done", "()(<-chan(struct{}))"),
                ("app::Getter::Get", "(string)(string)"),
                ("app::Getter::Put", "(string,string)(error)"),
                ("app::PickerBuilder::Build", "(PickerBuildInfo)(Picker)"),
                ("app::R::Any", "(any,chan<-([]byte),[4]byte,int)(int,error)"),
                ("app::R::Build", "(RawMessage)(Bundle,func(),error)"),
                ("app::R::Close", "()()"),
                ("app::R::Done", "()(<-chan(struct{}))"),
                ("app::R::Get", "(string)(string)"),
                ("app::R::Log", "(string,...any)()"),
                (
                    "app::R::Meta",
                    "(interface{Apply(*Cfg)error;Name()string})(struct{A,B(int);Base})",
                ),
                ("app::R::Page", "(Query[User],func()error)(*Page[User],error)"),
                ("app::R::Put", "(string,string)(error)"),
                ("app::R::Walk", "(func(Context)error)(map[string]*Y)"),
                ("app::bb::Build", "(PickerBuildInfo)(Picker)"),
            ])
        );
        // The implementation and the interface element compare equal, names
        // and qualifiers dropped on whichever side wrote them.
        let sig = |q: &str| parse.nav.method_sigs[&method_id(q)].clone();
        for (imp, iface) in [
            ("app::R::Get", "app::Getter::Get"),
            ("app::R::Put", "app::Getter::Put"),
            ("app::R::Done", "app::Getter::Done"),
            ("app::bb::Build", "app::PickerBuilder::Build"),
        ] {
            assert_eq!(sig(imp), sig(iface), "{imp} vs {iface}");
        }
        assert_ne!(sig("app::R::Build"), sig("app::PickerBuilder::Build"));
    }

    #[test]
    fn go_signatures_skip_generics() {
        let source = r#"package app

type Collection[T any] struct{}

type Plain struct{}

type Repo[T any] interface {
    Get(id string) (T, error)
    All() []T
}

type Closer interface {
    Close() error
}

func (c *Collection[T]) InsertOne(doc T) error { return nil }

func (c Collection[T]) Count() int { return 0 }

func (p *Plain) Close() error { return nil }
"#;
        let parse = parse_file(source, "app/app.go", "app", "example.com/app", repo()).unwrap();
        // The generic receiver's and the generic interface's methods are
        // nodes still; they record no signature.
        for q in ["app::Collection::InsertOne", "app::Collection::Count", "app::Repo::Get", "app::Repo::All"] {
            assert!(parse.nav.qname_by_id.contains_key(&method_id(q)), "{q} is a METHOD");
        }
        assert_eq!(
            sigs_of(&parse),
            pairs(&[("app::Closer::Close", "()(error)"), ("app::Plain::Close", "()(error)")])
        );
    }
}
