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

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_code_domain::data_entity;
use glia_code_domain::di_stats::{self, DiShape};
use glia_code_domain::endpoint::{
    ClientEndpoint, HitExtras, client_url_split, join_path, mount_route_qname,
    push_client_endpoint_with,
};
use glia_code_domain::{Mount, NavFact};

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

    // CB.11: the router groups this file assigns to struct fields, read before
    // any route walk so a registration on `s.v1` finds the group whatever
    // function assigned it.
    acc.field_prefixes = scan_field_prefixes(root, src, package_qname);

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
    if acc.var_init_calls + acc.var_literal_bodies > 0 {
        eprintln!(
            "[go-calls] package-var initialisers calls={} literal bodies={} literal calls={} vars={} in {file_rel_path}",
            acc.var_init_calls, acc.var_literal_bodies, acc.var_literal_calls, acc.var_call_owners
        );
    }
    if acc.receiver_handlers > 0 {
        eprintln!(
            "[go-routes] receiver-method handlers={} in {file_rel_path}",
            acc.receiver_handlers
        );
    }
    let forms = &acc.route_forms;
    let mounts = &acc.mounts;
    if forms.registrations > 0 || mounts.args + mounts.assigns > 0 {
        eprintln!(
            "[go-routes] registrations={} positioned={} forms(handle={} any={} match={} pattern={} field={}) mounts(param={} field={} args={} assigns={}) in {file_rel_path} nodes={} paths={}",
            forms.registrations,
            forms.positioned,
            forms.handle,
            forms.any,
            forms.matched,
            forms.pattern,
            forms.field_calls.len(),
            mounts.param,
            mounts.field,
            mounts.args,
            mounts.assigns,
            acc.route_qnames.len(),
            acc.route_paths.len()
        );
    }
    // CI.5: field mounts whose owner came through an import, and in-file
    // field groups rooted at a group-typed parameter's mount.
    let param_rooted = acc.field_prefixes.param_rooted();
    if mounts.foreign + param_rooted > 0 {
        eprintln!(
            "[go-routes] mount owners foreign={} param_rooted={param_rooted} in {file_rel_path}",
            mounts.foreign
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
    /// CI.5: local name -> repo-local package directory qname of every
    /// import under one of the repo's go.mod modules (`api` -> `api`; the
    /// repo-root package -> `""`), so a struct of another package
    /// (`s := &api.Server{}`) names the owner its own package's files name
    /// ([`RouteScope::owner`]). An alias binds alone; an un-aliased import
    /// binds every [`import_local_names`] candidate (LA.18d's rule for
    /// `external_pkgs`). Filled before any function is visited (Go requires
    /// imports first); only looked up, never iterated.
    import_dirs: BTreeMap<String, String>,
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
    /// CI.1: what the package-var initialisers of this file gave their
    /// STATE_VARs — CallSites made directly in an initialiser, the func
    /// literals it holds that were walked, the CallSites inside those
    /// literals, and the vars that own at least one call or literal — the
    /// `[go-calls] package-var initialisers` marker's counters.
    var_init_calls: usize,
    var_literal_bodies: usize,
    var_literal_calls: usize,
    var_call_owners: usize,
    /// CA.2a: the type parameters of the callable being visited (its own
    /// `[T any]`, or a generic receiver's `Collection[T]`), so a receiver
    /// fact never types a local or result by one. Set and cleared by
    /// `visit_function` / `visit_method`.
    type_params: Vec<String>,
    /// CA.5a: `(receiver var, receiver type)` of the method whose body
    /// `collect_routes_in` is walking (`func (h *TokensHandler) RegisterRoutes`
    /// -> `("h", "TokensHandler")`), so a method-value handler `h.List` names
    /// the receiver's TYPE. Set and cleared by `visit_method` around its route
    /// walk; `None` inside a function.
    route_receiver: Option<(String, String)>,
    /// CB.11: this file's struct-field router groups and each callable's
    /// variable types ([`scan_field_prefixes`]), filled before the second
    /// pass; `collect_routes_in` lends it to the route walk.
    field_prefixes: FieldPrefixes,
    /// CA.5a: HANDLED_BY refs whose base was rewritten from the receiver var
    /// to its type in this file — the `[go-routes] receiver-method` marker.
    receiver_handlers: usize,
    /// LA.32a: route registrations emitted in this file, the POSITION cells
    /// pushed for them, and the method-bearing forms among them — the
    /// `[go-routes] registrations=` marker's counters.
    route_forms: RouteFormCounts,
    /// LB.11a: the distinct `<METHOD> <path>` route qnames and the distinct
    /// paths among them emitted in this file — the marker's `nodes=` /
    /// `paths=`. Only counted, never iterated into output. CB.23: a
    /// provisional mount ROUTE counts by its provisional qname, its path by
    /// the `<mount:..><path>` tail.
    route_qnames: std::collections::BTreeSet<String>,
    route_paths: std::collections::BTreeSet<String>,
    /// CB.23: the router-mount counters of the `[go-routes]` marker's
    /// `mounts(..)`.
    mounts: MountCounts,
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
    /// CB.11: start bytes of the registration calls whose receiver is a
    /// struct field (`s.router.GET(..)`), so a multi-verb call counts once;
    /// the marker's `field=` is its size. Only counted, never iterated.
    field_calls: BTreeSet<usize>,
}

/// CB.23: what one Go file records for the build's mount pass (CB.20).
#[derive(Default)]
struct MountCounts {
    /// Registrations on a parameter-held group: provisional ROUTE qnames
    /// `<METHOD> <mount:param:..><path>`.
    param: usize,
    /// Registrations on a struct-field group the file cannot read:
    /// provisional `<METHOD> <mount:field:..><path>`.
    field: usize,
    /// `NavFact::MountArg` facts recorded (a call passing a router mount).
    args: usize,
    /// `NavFact::FieldMount` facts recorded (a router mount assigned to a
    /// struct field).
    assigns: usize,
    /// CI.5: the `assigns` whose owner is a struct of another package of the
    /// repo, named through this file's import ([`RouteScope::owner`]) — the
    /// `[go-routes] mount owners` marker's `foreign=`.
    foreign: usize,
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
        // CI.2a: the struct's own type parameters (`type Box[T any] struct {
        // T }`) embed nothing a parse can name.
        let type_params = type_param_names(spec.child_by_field_name("type_parameters"), src);
        let mut lc = list.walk();
        for decl in list.named_children(&mut lc) {
            if decl.kind() != "field_declaration" {
                continue;
            }
            for (field, ty) in field_decl_types(decl, src, type_ids, &acc.external_pkgs) {
                acc.nav.record_field_type(struct_id, &field, &ty);
            }
            push_struct_embed(decl, src, struct_id, &type_params, acc);
        }
    }
}

/// CI.2a: an embedded field (a `field_declaration` with no `name`) is an
/// INHERITS_FROM ref out of the struct ([`embedded_struct_qualifier`]), which
/// the graph binds to the in-repo STRUCT or INTERFACE it names, package-scoped
/// (`resolve_go_embeds`), and reads for Go's promoted methods and fields.
///
/// Independent of [`field_decl_types`]' guards: an embedded `*store.Logger`
/// inside `type Logger` records no field type (the bare-name self-bind
/// guard) but still embeds, bound through its import and never by bare name.
/// The predeclared `error` is kept: its ref stays unresolved in the graph.
fn push_struct_embed(
    decl: TsNode,
    src: &[u8],
    struct_id: NodeId,
    type_params: &[String],
    acc: &mut Acc,
) {
    if decl.child_by_field_name("name").is_some() {
        return;
    }
    let Some(type_node) = decl.child_by_field_name("type") else {
        return;
    };
    let Some(module_id) = acc.module_id else {
        return;
    };
    let Some(qualifier) = embedded_struct_qualifier(type_node, src, type_params, &acc.external_pkgs)
    else {
        return;
    };
    acc.refs.push(UnresolvedRef {
        from: struct_id,
        from_module: module_id,
        qualifier,
        category: edge_category::INHERITS_FROM,
        line: line_at(decl),
    });
}

/// How an embedded struct field names its type: `T` / `*T` / `T[..]` ->
/// `Bare("T")`, `pkg.T` / `*pkg.T` / `pkg.T[..]` -> `Attribute { base: "pkg",
/// name: "T" }`. `None` for `any`, one of the struct's own type parameters
/// (`type Box[T any] struct { T }`), a package outside the repo's go.mod
/// modules (`*zap.SugaredLogger`, `sync.Mutex`: LA.18d's `external_pkgs`;
/// with no go.mod every import is external) and every other shape.
fn embedded_struct_qualifier(
    type_node: TsNode,
    src: &[u8],
    type_params: &[String],
    external_pkgs: &std::collections::HashSet<String>,
) -> Option<CallQualifier> {
    let inner = generic_base(unwrap_pointer(type_node))?;
    match inner.kind() {
        "type_identifier" => {
            let name = text_of(inner, src);
            let skip = name.is_empty() || name == "any" || type_params.iter().any(|p| p == name);
            (!skip).then(|| CallQualifier::Bare(name.to_string()))
        }
        "qualified_type" => {
            let base = text_of(inner.child_by_field_name("package")?, src);
            let name = text_of(inner.child_by_field_name("name")?, src);
            (!base.is_empty() && !name.is_empty() && !external_pkgs.contains(base)).then(|| {
                CallQualifier::Attribute { base: base.to_string(), name: name.to_string() }
            })
        }
        _ => None,
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
        // CI.1: the calls made while evaluating this name's initialiser are
        // the STATE_VAR's — the direct ones (`var svc = New()`), then those
        // inside any func literal it holds (`var f = func() { g() }`, nested
        // in a composite too). The walk starts at the VALUE, never the spec:
        // `record_body_locals`' `var_spec` arm would record the var as a
        // local of itself (CA.2a records it on the MODULE scope above). The
        // same walk runs A7.6's detector, so `var ProviderSet =
        // wire.NewSet(NewA, NewB)` registers its providers from the var, which
        // a `wire.Build(ProviderSet)` then names. A constant initialiser may
        // call only builtins, so a `const_spec` is not walked.
        if spec.kind() == "var_spec"
            && let Some(value) = values.get(i)
        {
            let before = acc.calls.len();
            let mut literals = Vec::new();
            collect_calls_at(*value, src, id, None, repo, file_rel, acc, &mut literals);
            let direct = acc.calls.len() - before;
            let (bodies, calls) =
                collect_closure_calls(literals, src, id, None, repo, file_rel, acc);
            acc.var_init_calls += direct;
            acc.var_literal_bodies += bodies;
            acc.var_literal_calls += calls;
            if direct + bodies > 0 {
                acc.var_call_owners += 1;
            }
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
        let callable = RouteFn {
            decl,
            body,
            id,
            qname: &qname,
            package_qname,
        };
        collect_routes_in(&callable, src, file_rel, module_id, repo, acc);
        let (bodies, calls) = collect_closure_calls(closures, src, id, None, repo, file_rel, acc);
        acc.closure_bodies += bodies;
        acc.closure_calls += calls;
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
        // CA.5a: a route registered here with the receiver's method value
        // (`public.GET("/tokens", h.List)`) is handled by that method of the
        // receiver's type.
        acc.route_receiver = receiver_var.map(|v| (v.to_string(), receiver_type.clone()));
        let callable = RouteFn {
            decl,
            body,
            id,
            qname: &qname,
            package_qname,
        };
        collect_routes_in(&callable, src, file_rel, module_id, repo, acc);
        acc.route_receiver = None;
        let (bodies, calls) =
            collect_closure_calls(closures, src, id, receiver_var, repo, file_rel, acc);
        acc.closure_bodies += bodies;
        acc.closure_calls += calls;
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
    /// `Some("")` is the repo-root module's own path: it names the
    /// repository-root package (dir `""`), recorded like any package (CI.3).
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

    // CI.5: the package directory an in-repo import's local name stands for,
    // the repo-root package (`Some("")`) included, so recorded before that
    // arm returns below.
    if let Some(dir) = &local {
        match alias.as_deref() {
            Some("_") | Some(".") => {}
            Some(name) => {
                acc.import_dirs.insert(name.to_string(), dir.clone());
            }
            None => {
                for name in import_local_names(&path_str) {
                    acc.import_dirs.insert(name.to_string(), dir.clone());
                }
            }
        }
    }

    let (qname, alias) = match local {
        // CI.3: the repo-root module's own path (`import
        // "google.golang.org/grpc"` with that module at the repo root) names
        // the repository-root package, dir `""`, recorded like any package.
        // The graph names every other dir import by the dir's last segment,
        // which the root dir does not have, so the binding name is written
        // down here: the explicit alias (`_` / `.` kept as is), else the
        // import path's last element.
        Some(q) if q.is_empty() => {
            let name = alias.unwrap_or_else(|| go_import_local_name(&path_str).to_string());
            (q, Some(name))
        }
        Some(q) => (q, alias),
        // External import (stdlib or third-party). Keep the raw path for now;
        // cross-repo resolution is a v0.4.4 concern.
        None => (path_str.replace('/', "::"), alias),
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

/// CI.3: the one name an un-aliased import of the repository-root package
/// binds: the first of [`import_local_names`], the path's last element with a
/// `/vN` major-version element skipped (`google.golang.org/grpc` -> `grpc`,
/// `example.com/x/y/v3` -> `y`). A root package whose `package` clause
/// differs from it (module `github.com/nats-io/nats.go`, `package nats`)
/// binds under the path's name, which its callers do not spell.
fn go_import_local_name(path: &str) -> &str {
    import_local_names(path).first().copied().unwrap_or(path)
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
        collect_calls_at(child, src, from, receiver_var, repo, file_rel, acc, closures);
    }
}

/// [`collect_calls_in`] for one node: `node` itself is visited as a statement
/// child of a body is — its locals, its CallSite and detectors when it is a
/// call, then its children, unless it is a `func_literal`, which is queued on
/// `closures` instead. A package var's initialiser enters here (CI.1), so its
/// direct calls and the literals it holds go through the same code as a
/// function body's.
#[allow(clippy::too_many_arguments)]
fn collect_calls_at<'t>(
    node: TsNode<'t>,
    src: &[u8],
    from: NodeId,
    receiver_var: Option<&str>,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
    closures: &mut Vec<TsNode<'t>>,
) {
    // CA.2a: the locals this statement binds, on the enclosing fn.
    record_body_locals(node, src, from, receiver_var, acc);
    if node.kind() == "call_expression" {
        if let Some(q) = classify_call(node, src, receiver_var) {
            acc.calls.push(CallSite {
                from,
                qualifier: q,
                line: line_at(node),
            });
        }
        // Pattern A: outbound client HTTP call (`http.Get('http://…/x')`) →
        // ENDPOINT node so HttpStackResolver can pair it with a server ROUTE.
        try_detect_go_endpoint(node, src, from, repo, file_rel, acc);
        // Raw SQL (`db.Query("SELECT … FROM users")`) is not read here:
        // the cross-cutting data-entities extractor reads every SQL
        // literal with LG.3b's rejects and the engine re-homes its edge
        // to this function (LE.4a, `anchor::rehome_to_owner`).
        // GORM: `db.Model(&User{})` → the model-keyed entity,
        // `db.Table("x")` → the table-keyed one, from the same `from` (A13.12).
        try_detect_gorm_access(node, src, from, repo, acc);
        // DI container registration: `wire.Build(NewA, NewB)` → INJECTS
        // from the injector (`from`) to each provider (A7.6); from a
        // package var, `var Set = wire.NewSet(..)` (CI.1).
        try_detect_go_provider_set(node, src, from, acc);
    }
    if node.kind() == "func_literal" {
        closures.push(node);
    } else {
        collect_calls_in(node, src, from, receiver_var, repo, file_rel, acc, closures);
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
///
/// Returns `(bodies, calls)`: the literals walked and the CallSites pushed
/// from them. A function's caller adds them to CA.1's `[go-calls]
/// func-literal` counters, a package var's (CI.1) to the `package-var
/// initialisers` ones.
#[allow(clippy::too_many_arguments)]
fn collect_closure_calls(
    closures: Vec<TsNode>,
    src: &[u8],
    from: NodeId,
    receiver_var: Option<&str>,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> (usize, usize) {
    let calls_before = acc.calls.len();
    let mut bodies = 0usize;
    let mut queue: std::collections::VecDeque<TsNode> = closures.into();
    while let Some(lit) = queue.pop_front() {
        if acc.func_literal_handlers.contains(&lit.start_byte()) {
            acc.route_literals_skipped += 1;
            continue;
        }
        bodies += 1;
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
    (bodies, acc.calls.len() - calls_before)
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
// `x := y.Group("/prefix")`, records `x` → concatenated prefix in the
// `RouteScope`'s groups. For each recognised registration call, builds the
// full path by prepending the receiver's prefix and emits a Route node with one
// ROUTE_METHOD cell plus an `UnresolvedRef` (category=HANDLED_BY) for the
// handler. CB.11: a receiver may be a struct field (`s.v1.GET(..)`), whose
// prefix is the group the same file assigns that field ([`FieldPrefixes`]).
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

// ----------------------------------------------------------------------------
// CB.11: routers held on a struct field
// ----------------------------------------------------------------------------
//
// `type Server struct { router *gin.Engine; v1 *gin.RouterGroup }` registers
// on `s.router.GET(..)` / `s.v1.POST(..)`, and the group a field holds is
// assigned in some other function of the file (`s.v1 = s.router.Group("/v1")`
// in `NewServer`). One scan per file, before any route walk, types each
// callable's variables and records every group the file assigns to a field;
// the route walk then prefixes a field receiver's routes with that group. A
// field assigned in another file is CB.23's provisional mount.

/// CB.11: one router group as a file spells it: the struct field its chain
/// starts from (`None` = the engine itself, a local group, or a receiver the
/// file cannot type, all rooted at `""` as the route walk roots them) and the
/// `Group` literals joined onto it, in order. CI.5: `param` is the
/// `Mount::Param` of the group-typed parameter the chain starts from
/// (`rg.Group("/admin")` in `NewPanel(rg *gin.RouterGroup)`), so the field
/// holds the mount its callers hand `rg`, not the root; `field` is then
/// `None`.
#[derive(Debug, Clone, Default)]
struct GroupPath {
    field: Option<(String, String)>,
    segs: Vec<String>,
    param: Option<Mount>,
}

impl GroupPath {
    /// This group's `.Group(lit)`.
    fn then(mut self, lit: String) -> GroupPath {
        self.segs.push(lit);
        self
    }
}

/// CB.11: the router groups one Go file assigns to struct fields, and the
/// struct type each callable's variables name. Built by
/// [`scan_field_prefixes`]; only looked up, never iterated into output.
#[derive(Debug, Default)]
struct FieldPrefixes {
    /// `(struct type, field)` -> the mount every assignment in the file agrees
    /// on (`("Server", "v1")` -> `Const("/v1")`; CI.5: a group built from a
    /// group-typed parameter -> that parameter's `Param` with the `.Group`
    /// literals as its suffix). A field two assignments disagree on, or whose
    /// chain loops, is absent: unknown beats wrong. The struct type is its
    /// simple name for this package's structs, `pkg.T` for another's
    /// ([`struct_type_name`]).
    known: BTreeMap<(String, String), Mount>,
    /// A callable body's start byte -> its variable -> struct type map
    /// ([`scope_types`]). Empty for a file whose text never says `Group`;
    /// the route walk then types a body only when it meets a field receiver
    /// ([`RouteScope::types`]).
    scopes: HashMap<usize, BTreeMap<String, String>>,
    /// CB.23: `(struct simple name, field)` -> the router type this file
    /// declares the field with (`router *gin.Engine` -> Root, `v1
    /// *gin.RouterGroup` -> Group; [`router_type`]).
    declared: BTreeMap<(String, String), RouterType>,
    /// CB.23: every `(struct, field)` this file assigns a group or a router
    /// root to, agreeing or not (the keys `known` is resolved from).
    assigned: BTreeSet<(String, String)>,
}

impl FieldPrefixes {
    /// The mount field `field` of struct `ty` holds, when the file assigns it
    /// one consistent group.
    fn get(&self, ty: &str, field: &str) -> Option<&Mount> {
        self.known.get(&(ty.to_string(), field.to_string()))
    }

    /// CI.5: how many fields hold a group rooted at a group-typed parameter
    /// (a `Param` mount) — the `[go-routes] mount owners` marker's
    /// `param_rooted=`.
    fn param_rooted(&self) -> usize {
        self.known
            .values()
            .filter(|m| matches!(m, Mount::Param { .. }))
            .count()
    }

    /// CB.23: whether this file shows field `field` of struct `ty` holds a
    /// router: it declares the field router-typed or assigns it a group.
    fn is_router(&self, ty: &str, field: &str) -> bool {
        let key = (ty.to_string(), field.to_string());
        self.declared.contains_key(&key) || self.assigned.contains(&key)
    }

    /// CB.23: whether any struct's field `field` is one [`Self::is_router`]
    /// knows: the cheap test before a body is typed.
    fn names_router(&self, field: &str) -> bool {
        let named = |(_, f): &(String, String)| f == field;
        self.declared.keys().any(named) || self.assigned.iter().any(named)
    }

    /// CB.23: whether this file declares field `field` of struct `ty` with a
    /// router ROOT type, which holds no prefix whatever assigns it.
    fn is_root(&self, ty: &str, field: &str) -> bool {
        self.declared.get(&(ty.to_string(), field.to_string())) == Some(&RouterType::Root)
    }
}

/// CB.11: read one file's struct-field router groups: for every function and
/// method body, with its [`scope_types`], an assignment `x.f = <recv>.Group(
/// "<lit>")` (or `x.f = g`, `g` a local group) records `(type of x, f)`, and
/// a keyed composite literal `&T{f: <recv>.Group("<lit>")}` records `(T, f)`.
/// `<recv>` is a local group var, another struct field (resolved after the
/// scan, so the functions may come in any order), a group-typed parameter
/// (CI.5: its `Mount::Param`, by the route walk's own rule,
/// [`seed_param_mounts`]) or anything else (the engine itself, `""`). A file
/// that never says `Group` holds no field group, and is skipped.
fn scan_field_prefixes(root: TsNode, src: &[u8], package_qname: &str) -> FieldPrefixes {
    let mut out = FieldPrefixes {
        declared: declared_router_fields(root, src),
        ..FieldPrefixes::default()
    };
    if !src.windows(5).any(|w| w == b"Group") {
        return out;
    }
    let mut facts: BTreeMap<(String, String), Vec<GroupPath>> = BTreeMap::new();
    let mut cursor = root.walk();
    for decl in root.named_children(&mut cursor) {
        if !matches!(decl.kind(), "function_declaration" | "method_declaration") {
            continue;
        }
        let Some(body) = decl.child_by_field_name("body") else {
            continue;
        };
        let types = scope_types(decl, body, src);
        // CI.5: a GROUP-typed parameter starts its chains at the mount its
        // callers hand it; a ROOT-typed one is the root, as any unknown
        // receiver already is.
        let mut groups: HashMap<String, GroupPath> = callable_qname(decl, src, package_qname)
            .map(|qname| seed_param_mounts(decl, &qname, src))
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, mount)| matches!(mount, Mount::Param { .. }))
            .map(|(name, mount)| {
                let path = GroupPath {
                    param: Some(mount),
                    ..GroupPath::default()
                };
                (name, path)
            })
            .collect();
        scan_field_groups(body, src, &types, &mut groups, &mut facts);
        out.scopes.insert(body.start_byte(), types);
    }
    let mut memo = BTreeMap::new();
    for key in facts.keys() {
        let mut visiting = BTreeSet::new();
        if let Some(prefix) = resolve_field_prefix(key, &facts, &mut memo, &mut visiting) {
            out.known.insert(key.clone(), prefix);
        }
    }
    out.assigned = facts.into_keys().collect();
    out
}

/// CI.5: the nav qname [`visit_function`] / [`visit_method`] give a
/// top-level callable (`<package>::<name>`, `<package>::<receiver type>::<name>`),
/// the `fn_qname` its `Mount::Param`s name. `None` where they emit no
/// callable (no name, or a method with no receiver type).
fn callable_qname(decl: TsNode, src: &[u8], package_qname: &str) -> Option<String> {
    let name = text_of(decl.child_by_field_name("name")?, src);
    match decl.kind() {
        "function_declaration" => Some(format!("{package_qname}::{name}")),
        "method_declaration" => {
            let (_, receiver_type) = parse_receiver(decl.child_by_field_name("receiver")?, src);
            Some(format!("{package_qname}::{}::{name}", receiver_type?))
        }
        _ => None,
    }
}

/// CB.23: the struct fields this file's top-level struct types declare with a
/// router type ([`router_type`]), keyed `(struct simple name, field)`.
fn declared_router_fields(root: TsNode, src: &[u8]) -> BTreeMap<(String, String), RouterType> {
    let mut out = BTreeMap::new();
    for decl in named_kids(Some(root)) {
        if decl.kind() != "type_declaration" {
            continue;
        }
        for spec in named_kids(Some(decl)) {
            let (Some(name), Some(ty)) = (
                spec.child_by_field_name("name"),
                spec.child_by_field_name("type"),
            ) else {
                continue;
            };
            if spec.kind() != "type_spec" || ty.kind() != "struct_type" {
                continue;
            }
            let owner = text_of(name, src);
            let lists = named_kids(Some(ty));
            let Some(list) = lists.iter().find(|c| c.kind() == "field_declaration_list") else {
                continue;
            };
            for field in named_kids(Some(*list)) {
                if field.kind() != "field_declaration" {
                    continue;
                }
                let router = field
                    .child_by_field_name("type")
                    .and_then(|t| router_type(t, src));
                let Some(router) = router else {
                    continue;
                };
                let mut c = field.walk();
                for f in field.children_by_field_name("name", &mut c) {
                    out.insert((owner.to_string(), text_of(f, src).to_string()), router);
                }
            }
        }
    }
    out
}

/// CB.11: one body's field-group facts, in source order. Local groups
/// (`g := r.Group("/x")`) follow `record_group_assignment`'s rule, so a
/// field assigned `g` holds the prefix the route walk gives `g`. Func
/// literals are skipped, as the route walk skips them.
fn scan_field_groups(
    n: TsNode,
    src: &[u8],
    types: &BTreeMap<String, String>,
    groups: &mut HashMap<String, GroupPath>,
    facts: &mut BTreeMap<(String, String), Vec<GroupPath>>,
) {
    match n.kind() {
        "func_literal" => return,
        "short_var_declaration" => {
            let names = named_kids(n.child_by_field_name("left"));
            let values = named_kids(n.child_by_field_name("right"));
            if let ([name], [value]) = (names.as_slice(), values.as_slice())
                && name.kind() == "identifier"
                && let Some(group) = group_call_path(*value, src, types, groups)
            {
                groups.insert(text_of(*name, src).to_string(), group);
            }
        }
        "assignment_statement"
            if n.child_by_field_name("operator")
                .is_some_and(|op| op.kind() == "=") =>
        {
            let targets = named_kids(n.child_by_field_name("left"));
            let values = named_kids(n.child_by_field_name("right"));
            if targets.len() == values.len() {
                for (target, value) in targets.iter().zip(&values) {
                    if let Some(key) = field_key(*target, src, types)
                        && let Some(group) = group_value(*value, src, types, groups)
                    {
                        facts.entry(key).or_default().push(group);
                    }
                }
            }
        }
        "composite_literal" => {
            let owner = n
                .child_by_field_name("type")
                .and_then(|t| struct_type_name(t, src));
            if let Some(owner) = owner {
                for el in named_kids(n.child_by_field_name("body")) {
                    if el.kind() != "keyed_element" {
                        continue;
                    }
                    let key = el
                        .child_by_field_name("key")
                        .and_then(|k| k.named_child(0))
                        .filter(|k| k.kind() == "identifier");
                    let value = el
                        .child_by_field_name("value")
                        .and_then(|v| v.named_child(0));
                    if let (Some(key), Some(value)) = (key, value)
                        && let Some(group) = group_value(value, src, types, groups)
                    {
                        let key = (owner.clone(), text_of(key, src).to_string());
                        facts.entry(key).or_default().push(group);
                    }
                }
            }
        }
        _ => {}
    }
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        scan_field_groups(child, src, types, groups, facts);
    }
}

/// CB.11: the group a field is assigned: a `.Group("<lit>")` call, or a local
/// group var; CB.23: or a router constructor (`gin.New()`, the root, `""`);
/// CI.5: or a group-typed parameter (its `Mount::Param`). Anything else (the
/// engine var, a root-typed parameter) records nothing.
fn group_value(
    value: TsNode,
    src: &[u8],
    types: &BTreeMap<String, String>,
    groups: &HashMap<String, GroupPath>,
) -> Option<GroupPath> {
    match value.kind() {
        "identifier" => groups.get(text_of(value, src)).cloned(),
        _ if is_router_ctor(value, src) => Some(GroupPath::default()),
        _ => group_call_path(value, src, types, groups),
    }
}

/// CB.11: `<recv>.Group("<lit>")` as a [`GroupPath`]: `<recv>` a local group
/// var (CI.5: or a group-typed parameter) continues that group, `x.f` with a
/// typed `x` starts from field `(type of x, f)`, anything else from the root.
fn group_call_path(
    call: TsNode,
    src: &[u8],
    types: &BTreeMap<String, String>,
    groups: &HashMap<String, GroupPath>,
) -> Option<GroupPath> {
    if call.kind() != "call_expression" {
        return None;
    }
    let func = call
        .child_by_field_name("function")
        .filter(|f| f.kind() == "selector_expression")?;
    if text_of(func.child_by_field_name("field")?, src) != "Group" {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let lit = string_literal_text(args.named_child(0)?, src)?;
    let recv = func.child_by_field_name("operand")?;
    let base = match recv.kind() {
        "identifier" => groups.get(text_of(recv, src)).cloned().unwrap_or_default(),
        _ => GroupPath {
            field: field_key(recv, src, types),
            ..GroupPath::default()
        },
    };
    Some(base.then(lit))
}

/// CB.11: `x.f` with an `x` this scope types -> `(type of x, f)`.
fn field_key(
    sel: TsNode,
    src: &[u8],
    types: &BTreeMap<String, String>,
) -> Option<(String, String)> {
    if sel.kind() != "selector_expression" {
        return None;
    }
    let x = sel
        .child_by_field_name("operand")
        .filter(|o| o.kind() == "identifier")?;
    let ty = types.get(text_of(x, src))?;
    let f = sel.child_by_field_name("field")?;
    Some((ty.clone(), text_of(f, src).to_string()))
}

/// CB.11: the mount of field `key`: its every assignment's group, resolved
/// through the fields they start from, when all agree. A field the file
/// assigns no group is the engine itself (`Const("")`); CI.5: a group rooted
/// at a group-typed parameter is that parameter's `Param`, the `.Group`
/// literals folded into its suffix ([`mount_then`], as the route walk folds
/// them). A disagreement, or a chain that loops back through `visiting`, is
/// `None`.
fn resolve_field_prefix(
    key: &(String, String),
    facts: &BTreeMap<(String, String), Vec<GroupPath>>,
    memo: &mut BTreeMap<(String, String), Option<Mount>>,
    visiting: &mut BTreeSet<(String, String)>,
) -> Option<Mount> {
    if let Some(done) = memo.get(key) {
        return done.clone();
    }
    let Some(paths) = facts.get(key) else {
        return Some(Mount::Const(String::new()));
    };
    if !visiting.insert(key.clone()) {
        return None;
    }
    let mut agreed: Option<Mount> = None;
    let mut consistent = true;
    for path in paths {
        let base = match (&path.param, &path.field) {
            (Some(param), _) => Some(param.clone()),
            (None, None) => Some(Mount::Const(String::new())),
            (None, Some(field)) => resolve_field_prefix(field, facts, memo, visiting),
        };
        let Some(base) = base else {
            consistent = false;
            break;
        };
        let prefix = path.segs.iter().fold(base, |acc, seg| mount_then(acc, seg));
        match &agreed {
            None => agreed = Some(prefix),
            Some(seen) if *seen == prefix => {}
            Some(_) => {
                consistent = false;
                break;
            }
        }
    }
    visiting.remove(key);
    let out = agreed.filter(|_| consistent);
    memo.insert(key.clone(), out.clone());
    out
}

/// CB.11: the struct type each variable of one callable names: the method
/// receiver, parameters typed `T` / `*T` / `pkg.T`, and locals bound by
/// `x := &T{..}`, `x := T{..}`, `x := new(T)`, `var x T` (pointer stripped;
/// [`struct_type_name`]: `T`, or `pkg.T` for another package's struct, CI.5).
/// A name bound twice to different types, or once to
/// something else (`s := NewServer()`, a range variable), is left out:
/// unknown beats wrong. Route-only, read at parse time: not
/// `CodeNav.local_types`, which records call-chain text for the graph's
/// receiver pass (CA.2a).
fn scope_types(decl: TsNode, body: TsNode, src: &[u8]) -> BTreeMap<String, String> {
    let mut scope = ScopeTypes::default();
    if let Some(receiver) = decl.child_by_field_name("receiver")
        && let (Some(var), ty) = parse_receiver(receiver, src)
    {
        scope.bind(&var, ty.filter(|t| !t.is_empty()));
    }
    for param in named_kids(decl.child_by_field_name("parameters")) {
        let ty = match param.kind() {
            "parameter_declaration" => param
                .child_by_field_name("type")
                .and_then(|t| struct_type_name(t, src)),
            "variadic_parameter_declaration" => None,
            _ => continue,
        };
        let mut c = param.walk();
        for name in param.children_by_field_name("name", &mut c) {
            scope.bind(text_of(name, src), ty.clone());
        }
    }
    bind_scope_locals(body, src, &mut scope);
    scope.types
}

/// CB.11: [`scope_types`]' accumulator.
#[derive(Default)]
struct ScopeTypes {
    types: BTreeMap<String, String>,
    /// Names bound to two types, or to something untyped.
    unknown: BTreeSet<String>,
}

impl ScopeTypes {
    fn bind(&mut self, var: &str, ty: Option<String>) {
        if var == "_" || self.unknown.contains(var) {
            return;
        }
        match (self.types.get(var), ty) {
            (None, Some(ty)) => {
                self.types.insert(var.to_string(), ty);
            }
            (Some(seen), Some(ty)) if *seen == ty => {}
            _ => {
                self.types.remove(var);
                self.unknown.insert(var.to_string());
            }
        }
    }
}

/// CB.11: the locals one body declares, in any nested block (func literals
/// skipped, as the route walk skips them).
fn bind_scope_locals(n: TsNode, src: &[u8], scope: &mut ScopeTypes) {
    match n.kind() {
        "func_literal" => return,
        "short_var_declaration" => {
            let names = named_kids(n.child_by_field_name("left"));
            let values = named_kids(n.child_by_field_name("right"));
            for (i, name) in names.iter().enumerate() {
                if name.kind() == "identifier" {
                    let ty =
                        paired(&values, names.len(), i).and_then(|v| value_struct_type(v, src));
                    scope.bind(text_of(*name, src), ty);
                }
            }
        }
        "var_spec" => {
            let mut c = n.walk();
            let names: Vec<TsNode> = n.children_by_field_name("name", &mut c).collect();
            let declared = n
                .child_by_field_name("type")
                .map(|t| struct_type_name(t, src));
            let values = named_kids(n.child_by_field_name("value"));
            for (i, name) in names.iter().enumerate() {
                let ty = match &declared {
                    Some(ty) => ty.clone(),
                    None => paired(&values, names.len(), i).and_then(|v| value_struct_type(v, src)),
                };
                scope.bind(text_of(*name, src), ty);
            }
        }
        "range_clause" | "receive_statement" if declares(n) => {
            for name in named_kids(n.child_by_field_name("left")) {
                if name.kind() == "identifier" {
                    scope.bind(text_of(name, src), None);
                }
            }
        }
        "type_switch_statement" => {
            for name in named_kids(n.child_by_field_name("alias")) {
                if name.kind() == "identifier" {
                    scope.bind(text_of(name, src), None);
                }
            }
        }
        _ => {}
    }
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        bind_scope_locals(child, src, scope);
    }
}

/// CB.11: the struct an initialiser builds: `T{..}`, `&T{..}`, `new(T)`
/// (and `pkg.T` forms, CI.5: as `pkg.T`); anything else names none.
fn value_struct_type(value: TsNode, src: &[u8]) -> Option<String> {
    match value.kind() {
        "composite_literal" => struct_type_name(value.child_by_field_name("type")?, src),
        "unary_expression" => {
            let op = value.child_by_field_name("operator")?;
            let inner = value.child_by_field_name("operand")?;
            if op.kind() == "&" && inner.kind() == "composite_literal" {
                value_struct_type(inner, src)
            } else {
                None
            }
        }
        "call_expression" => {
            let func = value.child_by_field_name("function")?;
            if func.kind() != "identifier" || text_of(func, src) != "new" {
                return None;
            }
            let arg = value.child_by_field_name("arguments")?.named_child(0)?;
            match arg.kind() {
                "identifier" => Some(text_of(arg, src).to_string()),
                "selector_expression" => {
                    let pkg = arg
                        .child_by_field_name("operand")
                        .filter(|p| p.kind() == "identifier")?;
                    let name = arg.child_by_field_name("field")?;
                    qualified_name(text_of(pkg, src), text_of(name, src))
                }
                _ => struct_type_name(arg, src),
            }
        }
        "parenthesized_expression" => value_struct_type(value.named_child(0)?, src),
        _ => None,
    }
}

/// CB.11: a named type's name: `T`, `*T`, `T[U]` -> `T`; CI.5: another
/// package's `pkg.T` / `*pkg.T` keeps its qualifier, `pkg.T`, so it never
/// collides with this package's own `T` and [`RouteScope::owner`] can place
/// it through the file's import. Slices, maps, funcs and anonymous structs
/// name no struct.
fn struct_type_name(ty: TsNode, src: &[u8]) -> Option<String> {
    let name = match ty.kind() {
        "type_identifier" => text_of(ty, src),
        "qualified_type" => {
            let pkg = text_of(ty.child_by_field_name("package")?, src);
            let name = text_of(ty.child_by_field_name("name")?, src);
            return qualified_name(pkg, name);
        }
        "pointer_type" => return struct_type_name(ty.named_child(0)?, src),
        "generic_type" => return struct_type_name(ty.child_by_field_name("type")?, src),
        _ => return None,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// CI.5: `pkg.T` as [`struct_type_name`] spells it; `None` when either side
/// is empty (an error-recovered node).
fn qualified_name(pkg: &str, name: &str) -> Option<String> {
    (!pkg.is_empty() && !name.is_empty()).then(|| format!("{pkg}.{name}"))
}

/// CB.23: the callable a route walk runs in: its declaration and body, its
/// node id (the scope its mount facts are recorded on, the `from` of its
/// CallSites), its nav qname (what a Param mount names) and the file's
/// module qname (a Field mount's owner is its package directory).
struct RouteFn<'a> {
    decl: TsNode<'a>,
    body: TsNode<'a>,
    id: NodeId,
    qname: &'a str,
    package_qname: &'a str,
}

/// CB.23: how a declared type holds a router. A ROOT (`*gin.Engine`,
/// `*echo.Echo`, `*fiber.App`, `*http.ServeMux`, a chi / gorilla /
/// httprouter router) is never what `.Group("/p")` returns, so it holds no
/// prefix the parser reads: the root, `""`. A GROUP (`*gin.RouterGroup`,
/// `gin.IRouter` / `IRoutes`, `*echo.Group`, `fiber.Router`) holds whatever
/// prefix its holder was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouterType {
    Root,
    Group,
}

/// CB.23: the router type a declared type names, pointers stripped: a
/// qualified type of a known router package (`gin`, `echo`, `chi`, `mux`,
/// `fiber`, `httprouter`, `http`) by that package's router type names, or a
/// bare `RouterGroup` / `IRouter` / `IRoutes` (a dot-import or an in-repo
/// wrapper). A handler context (`*gin.Context`, `echo.Context`) and every
/// other type -> `None`.
fn router_type(ty: TsNode, src: &[u8]) -> Option<RouterType> {
    let ty = unwrap_pointer(ty);
    match ty.kind() {
        "qualified_type" => {
            let pkg = text_of(ty.child_by_field_name("package")?, src);
            let name = text_of(ty.child_by_field_name("name")?, src);
            match (pkg, name) {
                ("gin", "Engine")
                | ("echo", "Echo")
                | ("fiber", "App")
                | ("chi", "Mux" | "Router")
                | ("mux", "Router")
                | ("http", "ServeMux")
                | ("httprouter", "Router") => Some(RouterType::Root),
                ("gin", "RouterGroup" | "IRouter" | "IRoutes")
                | ("echo", "Group")
                | ("fiber", "Router" | "Group") => Some(RouterType::Group),
                _ => None,
            }
        }
        "type_identifier" => matches!(text_of(ty, src), "RouterGroup" | "IRouter" | "IRoutes")
            .then_some(RouterType::Group),
        _ => None,
    }
}

/// CB.23: a router constructor call (`gin.Default()`, `gin.New()`,
/// `echo.New()`, `chi.NewRouter()`, `mux.NewRouter()`, `http.NewServeMux()`,
/// `fiber.New(..)`, `httprouter.New()`): the root of a router tree, `""`.
fn is_router_ctor(call: TsNode, src: &[u8]) -> bool {
    if call.kind() != "call_expression" {
        return false;
    }
    let Some(func) = call
        .child_by_field_name("function")
        .filter(|f| f.kind() == "selector_expression")
    else {
        return false;
    };
    let (Some(pkg), Some(name)) = (
        func.child_by_field_name("operand"),
        func.child_by_field_name("field"),
    ) else {
        return false;
    };
    pkg.kind() == "identifier"
        && matches!(
            (text_of(pkg, src), text_of(name, src)),
            ("gin", "Default" | "New")
                | ("echo", "New")
                | ("chi", "NewRouter" | "NewMux")
                | ("mux", "NewRouter")
                | ("http", "NewServeMux")
                | ("fiber", "New")
                | ("httprouter", "New")
        )
}

/// CB.23: `mount` then a `.Group(lit)`: a Const's prefix, a Param's or a
/// Field's suffix grows by `lit` ([`join_path`], as the walk has always
/// joined a group onto its parent).
fn mount_then(mount: Mount, lit: &str) -> Mount {
    match mount {
        Mount::Const(prefix) => Mount::Const(join_path(&prefix, lit)),
        Mount::Param {
            fn_qname,
            index,
            suffix,
        } => Mount::Param {
            fn_qname,
            index,
            suffix: join_path(&suffix, lit),
        },
        Mount::Field {
            owner,
            field,
            suffix,
        } => Mount::Field {
            owner,
            field,
            suffix: join_path(&suffix, lit),
        },
    }
}

/// The route walk's view of one callable body: the router mount each of its
/// locals holds and, for CB.11, the struct type each of its variables names
/// plus the file's struct-field groups.
struct RouteScope<'a> {
    /// Local name -> the router mount it holds (CB.23; a prefix string
    /// before): a local group var (`api := r.Group("/api")`, filled in walk
    /// order), a router-typed parameter ([`seed_param_mounts`]) or a router
    /// root (`r := gin.Default()`). A name absent here registers at the root,
    /// `""`, as it always has. Only looked up, never iterated.
    groups: BTreeMap<String, Mount>,
    /// CB.11: this body's variable -> struct type ([`scope_types`]), from the
    /// file's scan; `None` in a file that never says `Group`, whose body is
    /// typed on first use instead ([`RouteScope::types`]).
    scanned: Option<&'a BTreeMap<String, String>>,
    /// CB.23: the body's types when the scan had none, computed on the first
    /// field receiver the walk meets.
    own_types: std::cell::OnceCell<BTreeMap<String, String>>,
    /// CB.11: the file's struct-field router groups.
    fields: &'a FieldPrefixes,
    /// CI.5: the file's in-repo imports, local name -> package directory
    /// qname ([`Acc::import_dirs`]), which place another package's struct.
    imports: &'a BTreeMap<String, String>,
    /// CB.23: the callable walked.
    callable: &'a RouteFn<'a>,
}

impl RouteScope<'_> {
    /// CB.11: this body's variable -> struct type map.
    fn types(&self, src: &[u8]) -> &BTreeMap<String, String> {
        match self.scanned {
            Some(types) => types,
            None => self
                .own_types
                .get_or_init(|| scope_types(self.callable.decl, self.callable.body, src)),
        }
    }

    /// CB.23: the owner a Field mount names for struct `ty`: `<package dir
    /// qname>::<ty>`. A Go package is its directory (LA.13b), so the file
    /// that assigns a field and the file that registers on it name one owner.
    /// CI.5: another package's `pkg.T` names the directory the file's import
    /// of `pkg` maps to (the repo-root package: `T`), so `cmd/main.go`'s
    /// `&api.Server{}` and `api/server.go`'s `Server` meet; a package outside
    /// the repo, or one no import binds, places no owner: `None`.
    fn owner(&self, ty: &str) -> Option<String> {
        if let Some((pkg, name)) = ty.split_once('.') {
            let dir = self.imports.get(pkg)?;
            return Some(if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}::{name}")
            });
        }
        Some(match self.callable.package_qname.rsplit_once("::") {
            Some((dir, _)) => format!("{dir}::{ty}"),
            None => ty.to_string(),
        })
    }

    /// `x.f` with an identifier `x`: `(type of x, f)`, the type `None` when
    /// the body cannot type `x`. `None` for any other node.
    fn field_key(&self, sel: TsNode, src: &[u8]) -> Option<(Option<String>, String)> {
        if sel.kind() != "selector_expression" {
            return None;
        }
        let x = sel
            .child_by_field_name("operand")
            .filter(|o| o.kind() == "identifier")?;
        let f = sel.child_by_field_name("field")?;
        let ty = self.types(src).get(text_of(x, src)).cloned();
        Some((ty, text_of(f, src).to_string()))
    }

    /// CB.11 / CB.23: the mount field `f` of struct `ty` holds: the file's
    /// group for it (CB.11; CI.5: a `Param` when the group is rooted at a
    /// group-typed parameter) or, for a field the file declares with a router
    /// root type, the root; otherwise a Field mount the build resolves from
    /// the package's `FieldMount` facts (CB.20), or re-keys to the local path
    /// when none reaches it. CI.5: a struct of a package outside the repo
    /// (`srv.Handler` of `&http.Server{}`) has no owner a `FieldMount` could
    /// name, so it is the unprefixed root it would re-key to anyway.
    fn typed_field_mount(&self, ty: &str, f: &str) -> Mount {
        if let Some(mount) = self.fields.get(ty, f) {
            return mount.clone();
        }
        if self.fields.is_root(ty, f) {
            return Mount::Const(String::new());
        }
        match self.owner(ty) {
            Some(owner) => Mount::Field {
                owner,
                field: f.to_string(),
                suffix: String::new(),
            },
            None => Mount::Const(String::new()),
        }
    }

    /// CB.11 / CB.23: the mount a struct-field receiver `x.f` holds
    /// ([`Self::typed_field_mount`]); an `x` the body cannot type is the
    /// HEAD-equivalent unprefixed root. `None` unless `sel` is
    /// `<identifier>.<field>`.
    fn field_mount(&self, sel: TsNode, src: &[u8]) -> Option<Mount> {
        let (ty, f) = self.field_key(sel, src)?;
        Some(match ty {
            Some(ty) => self.typed_field_mount(&ty, &f),
            None => Mount::Const(String::new()),
        })
    }

    /// CB.23: the mount a registration or `.Group` receiver holds: an
    /// identifier's ([`RouteScope::groups`], else the root) or a struct
    /// field's ([`Self::field_mount`]). `None` for any other receiver (a
    /// call, an index, a deeper chain).
    fn receiver_mount(&self, operand: TsNode, src: &[u8]) -> Option<Mount> {
        match operand.kind() {
            "identifier" => Some(
                self.groups
                    .get(text_of(operand, src))
                    .cloned()
                    .unwrap_or_else(|| Mount::Const(String::new())),
            ),
            _ => self.field_mount(operand, src),
        }
    }

    /// CB.23: the mount `<operand>.Group("<lit>")` yields: the operand's
    /// ([`Self::receiver_mount`]; any other operand, a call chain included,
    /// is the root, as the walk has always rooted it) then `<lit>`. `None`
    /// unless `call` is such a call with a string literal.
    fn group_mount(&self, call: TsNode, src: &[u8]) -> Option<Mount> {
        if call.kind() != "call_expression" {
            return None;
        }
        let func = call
            .child_by_field_name("function")
            .filter(|f| f.kind() == "selector_expression")?;
        if text_of(func.child_by_field_name("field")?, src) != "Group" {
            return None;
        }
        let first = call.child_by_field_name("arguments")?.named_child(0)?;
        let lit = string_literal_text(first, src)?;
        let base = func
            .child_by_field_name("operand")
            .and_then(|operand| self.receiver_mount(operand, src))
            .unwrap_or_else(|| Mount::Const(String::new()));
        Some(mount_then(base, &lit))
    }

    /// CB.23: the router mount an argument or an assigned value carries: a
    /// local that holds one, a `.Group("<lit>")` call, a router constructor
    /// (the root) or, with `fields`, a struct field this file declares
    /// router-typed or assigns a group. Anything else (an identifier of
    /// unknown origin included) carries none.
    fn value_mount(&self, value: TsNode, src: &[u8], fields: bool) -> Option<Mount> {
        match value.kind() {
            "identifier" => self.groups.get(text_of(value, src)).cloned(),
            "parenthesized_expression" => self.value_mount(value.named_child(0)?, src, fields),
            "call_expression" if is_router_ctor(value, src) => Some(Mount::Const(String::new())),
            "call_expression" => self.group_mount(value, src),
            "selector_expression" if fields => {
                // The field name first: typing the body is the costly part.
                let f = text_of(value.child_by_field_name("field")?, src);
                if !self.fields.names_router(f) {
                    return None;
                }
                let (ty, f) = self.field_key(value, src)?;
                let ty = ty?;
                self.fields
                    .is_router(&ty, &f)
                    .then(|| self.typed_field_mount(&ty, &f))
            }
            _ => None,
        }
    }
}

/// CB.23: the router-typed parameters of the callable `decl` ([`router_type`];
/// the receiver is no parameter): a ROOT-typed one holds the root, `""`; a
/// GROUP-typed one the mount its callers hand it, `Param { fn_qname, index }`.
/// `index` counts parameter names, so `public, protected *gin.RouterGroup`
/// are 0 and 1, and an unnamed parameter counts one. CI.5: one rule for the
/// route walk ([`collect_routes_in`]) and the field-group scan
/// ([`scan_field_prefixes`]).
fn seed_param_mounts(decl: TsNode, fn_qname: &str, src: &[u8]) -> BTreeMap<String, Mount> {
    let mut out = BTreeMap::new();
    let mut index: u32 = 0;
    for param in named_kids(decl.child_by_field_name("parameters")) {
        let router = match param.kind() {
            "parameter_declaration" => param
                .child_by_field_name("type")
                .and_then(|t| router_type(t, src)),
            "variadic_parameter_declaration" => None,
            _ => continue,
        };
        let mut c = param.walk();
        let names: Vec<TsNode> = param.children_by_field_name("name", &mut c).collect();
        if names.is_empty() {
            index += 1;
            continue;
        }
        for name in names {
            let name = text_of(name, src);
            let mount = match router {
                Some(RouterType::Root) => Some(Mount::Const(String::new())),
                Some(RouterType::Group) => Some(Mount::Param {
                    fn_qname: fn_qname.to_string(),
                    index,
                    suffix: String::new(),
                }),
                None => None,
            };
            if let Some(mount) = mount
                && name != "_"
            {
                out.insert(name.to_string(), mount);
            }
            index += 1;
        }
    }
    out
}

fn collect_routes_in(
    callable: &RouteFn,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    // CB.11: the field groups (CI.5: and the import map) are lent out of
    // `acc` for the walk, which mutates `acc` as it emits.
    let fields = std::mem::take(&mut acc.field_prefixes);
    let imports = std::mem::take(&mut acc.import_dirs);
    let mut scope = RouteScope {
        groups: seed_param_mounts(callable.decl, callable.qname, src),
        scanned: fields.scopes.get(&callable.body.start_byte()),
        own_types: std::cell::OnceCell::new(),
        fields: &fields,
        imports: &imports,
        callable,
    };
    walk_routes(callable.body, src, file_rel, module_id, repo, &mut scope, acc);
    acc.field_prefixes = fields;
    acc.import_dirs = imports;
}

fn walk_routes(
    n: TsNode,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    scope: &mut RouteScope,
    acc: &mut Acc,
) {
    // Closure bodies run as handlers at request time; anything registered inside
    // them is unreachable from the surrounding group map. Skip.
    if matches!(n.kind(), "func_literal") {
        return;
    }
    match n.kind() {
        "short_var_declaration" => record_group_assignment(n, src, scope),
        "assignment_statement" | "composite_literal" => record_field_mounts(n, src, scope, acc),
        _ => {}
    }
    // CB.23: a registration records no mount argument.
    if n.kind() == "call_expression"
        && !try_emit_route(n, src, file_rel, module_id, repo, scope, acc)
    {
        record_mount_args(n, src, scope, acc);
    }
    let mut cursor = n.walk();
    for child in n.named_children(&mut cursor) {
        walk_routes(child, src, file_rel, module_id, repo, scope, acc);
    }
}

/// `y := x.Group("/p")` binds `y` to `x`'s mount then `/p` (CB.11: `x` may
/// be a struct field); CB.23: `r := gin.Default()` binds `r` to the root.
fn record_group_assignment(decl: TsNode, src: &[u8], scope: &mut RouteScope) {
    let names = named_kids(decl.child_by_field_name("left"));
    let values = named_kids(decl.child_by_field_name("right"));
    let ([lhs], [rhs]) = (names.as_slice(), values.as_slice()) else {
        return;
    };
    if lhs.kind() != "identifier" {
        return;
    }
    let mount = if is_router_ctor(*rhs, src) {
        Some(Mount::Const(String::new()))
    } else {
        scope.group_mount(*rhs, src)
    };
    if let Some(mount) = mount {
        scope.groups.insert(text_of(*lhs, src).to_string(), mount);
    }
}

/// CB.23: record `fact` (a `MountArg` or a `FieldMount`) on `scope` and count
/// it for the marker, once per distinct fact ([`CodeNav::record_fact`]
/// dedups per scope). CI.5: whether the fact is new to the scope.
fn record_mount_fact(acc: &mut Acc, scope: NodeId, fact: NavFact) -> bool {
    let seen = acc
        .nav
        .nav_facts
        .get(&scope)
        .is_some_and(|facts| facts.contains(&fact));
    if !seen {
        match &fact {
            NavFact::MountArg { .. } => acc.mounts.args += 1,
            NavFact::FieldMount { .. } => acc.mounts.assigns += 1,
            _ => {}
        }
    }
    acc.nav.record_fact(scope, fact);
    !seen
}

/// CB.23: every router mount this body assigns to a struct field — `x.f =
/// <mount>` with a typed `x`, or a keyed `T{f: <mount>}` — as a
/// `NavFact::FieldMount { owner: <package dir qname>::<T> }` on the callable,
/// for the build's mount pass (CB.20). CI.5: a struct of another package of
/// the repo (`s := &api.Server{}`, `&api.Server{Public: ..}`) names the
/// directory the file's import maps `api` to ([`RouteScope::owner`]), the
/// owner that package's own registrations name; a struct of a package outside
/// the repo (`&http.Server{}`) records nothing.
fn record_field_mounts(n: TsNode, src: &[u8], scope: &RouteScope, acc: &mut Acc) {
    // (struct type as the scope spells it, field, mount)
    let mut found: Vec<(String, String, Mount)> = Vec::new();
    match n.kind() {
        "assignment_statement" => {
            if n
                .child_by_field_name("operator")
                .is_none_or(|op| op.kind() != "=")
            {
                return;
            }
            let targets = named_kids(n.child_by_field_name("left"));
            let values = named_kids(n.child_by_field_name("right"));
            if targets.len() != values.len() {
                return;
            }
            for (target, value) in targets.iter().zip(&values) {
                let Some(mount) = scope.value_mount(*value, src, false) else {
                    continue;
                };
                if let Some((Some(ty), field)) = scope.field_key(*target, src) {
                    found.push((ty, field, mount));
                }
            }
        }
        "composite_literal" => {
            let Some(ty) = n
                .child_by_field_name("type")
                .and_then(|t| struct_type_name(t, src))
            else {
                return;
            };
            for el in named_kids(n.child_by_field_name("body")) {
                if el.kind() != "keyed_element" {
                    continue;
                }
                let key = el
                    .child_by_field_name("key")
                    .and_then(|k| k.named_child(0))
                    .filter(|k| k.kind() == "identifier");
                let value = el
                    .child_by_field_name("value")
                    .and_then(|v| v.named_child(0));
                let (Some(key), Some(value)) = (key, value) else {
                    continue;
                };
                if let Some(mount) = scope.value_mount(value, src, false) {
                    found.push((ty.clone(), text_of(key, src).to_string(), mount));
                }
            }
        }
        _ => {}
    }
    for (ty, field, mount) in found {
        let Some(owner) = scope.owner(&ty) else {
            continue;
        };
        let fact = NavFact::FieldMount {
            owner,
            field,
            mount,
        };
        if record_mount_fact(acc, scope.callable.id, fact) && ty.contains('.') {
            acc.mounts.foreign += 1;
        }
    }
}

/// CB.23: a call that is no route registration records one
/// `NavFact::MountArg` per argument carrying a router mount
/// ([`RouteScope::value_mount`]), on the callable (the scope its CallSite
/// leaves from) at the call's 0-based row (its CALLS edge's EVIDENCE line),
/// `callee` the function identifier or the selector's field
/// (`api.RegisterUsers(v2)` -> `RegisterUsers`, argument 0). A `.Group`
/// call (its mount is its result's), a call into a package outside the
/// module (`http.ListenAndServe(addr, r)`) and a router's own method
/// (`r.Use(mw)`) record nothing: none reaches an in-repo parameter.
fn record_mount_args(call: TsNode, src: &[u8], scope: &RouteScope, acc: &mut Acc) {
    let Some(func) = call.child_by_field_name("function") else {
        return;
    };
    let callee = match func.kind() {
        "identifier" => text_of(func, src),
        "selector_expression" => {
            let (Some(operand), Some(field)) = (
                func.child_by_field_name("operand"),
                func.child_by_field_name("field"),
            ) else {
                return;
            };
            if text_of(field, src) == "Group" {
                return;
            }
            if operand.kind() == "identifier" {
                let base = text_of(operand, src);
                if acc.external_pkgs.contains(base) || scope.groups.contains_key(base) {
                    return;
                }
            }
            text_of(field, src)
        }
        _ => return,
    };
    let line = line_at(call);
    let mut index: u32 = 0;
    for arg in named_kids(call.child_by_field_name("arguments")) {
        if arg.kind() == "comment" {
            continue;
        }
        if let Some(mount) = scope.value_mount(arg, src, true) {
            let fact = NavFact::MountArg {
                line,
                callee: callee.to_string(),
                arg: index,
                mount,
            };
            record_mount_fact(acc, scope.callable.id, fact);
        }
        index += 1;
    }
}

/// Emit the route(s) `call` registers, if it is a registration. Returns
/// whether it is one (a route emitted, or the inner call of a Gorilla
/// `.Methods(..)` chain): CB.23 records no mount argument of a registration.
fn try_emit_route(
    call: TsNode,
    src: &[u8],
    file_rel: &str,
    module_id: NodeId,
    repo: RepoId,
    scope: &RouteScope,
    acc: &mut Acc,
) -> bool {
    let Some(func) = call.child_by_field_name("function") else {
        return false;
    };
    if func.kind() != "selector_expression" {
        return false;
    }
    let Some(field) = func.child_by_field_name("field") else {
        return false;
    };
    let method_name = text_of(field, src);

    // Gorilla Mux: `r.HandleFunc("/u", h).Methods("GET", "POST")` — promote
    // the inner registration to one route per method.
    if method_name == "Methods" {
        return try_emit_gorilla_methods_chain(call, src, file_rel, module_id, repo, scope, acc);
    }

    // A registration wrapped in `.Methods(...)` — the wrapping call took the
    // route already.
    let registration = matches!(
        method_name,
        "HandleFunc" | "Handle" | "Add" | "Method" | "MethodFunc"
    );
    if registration && is_inner_of_methods_chain(call, src) {
        return true;
    }
    let Some(args) = call.child_by_field_name("arguments") else {
        return false;
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
        let emitted = emit_route_from_call(
            call, verb, 1, 2, None, src, file_rel, module_id, repo, scope, acc,
        );
        if emitted {
            acc.route_forms.handle += 1;
        }
        return emitted;
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
                    call, verb, 1, 2, None, src, file_rel, module_id, repo, scope, acc,
                );
            }
            if emitted {
                acc.route_forms.matched += 1;
            }
            return emitted;
        }
        return false;
    }

    // stdlib + Gorilla Mux: bare `HandleFunc` / `Handle`. Require the path to
    // begin with `/` to avoid colliding with stdlib map/method names.
    if method_name == "HandleFunc" || method_name == "Handle" {
        // LA.32a — Go 1.22 ServeMux: `"GET /items/{id}"` is a method plus a
        // path, never a path. A host pattern matches neither arm below.
        if let Some((verb, path)) = args.named_child(0).and_then(|a| method_pattern(a, src)) {
            let emitted = emit_route_from_call(
                call, verb, 0, 1, Some(&path), src, file_rel, module_id, repo, scope, acc,
            );
            if emitted {
                acc.route_forms.pattern += 1;
            }
            return emitted;
        }
        if !first_arg_is_url_path(call, src) {
            return false;
        }
        return emit_route_from_call(
            call, "ANY", 0, 1, None, src, file_rel, module_id, repo, scope, acc,
        );
    }

    // Idiomatic verb form: Gin/Echo (all-caps) and Chi/Fiber (Title-case).
    // Title-case `Get` / `Post` collide with common getters (`Header.Get(...)`,
    // `pool.Get()`); require a URL-shaped path. All-caps `GET` is unambiguous
    // and stays permissive for back-compat with the original Gin scanner.
    let Some(canonical) = normalize_http_method(method_name) else {
        return false;
    };
    // A client HTTP call (`client.Get("/x")`) has the same verb shape but is an
    // outbound ENDPOINT (handled by try_detect_go_endpoint); skip its receiver
    // here so it isn't mis-emitted as a phantom server ROUTE. CB.11: a struct
    // field receiver (`s.client.Get("/x")`) is judged by its field name.
    let receiver_name = func
        .child_by_field_name("operand")
        .and_then(|operand| match operand.kind() {
            "identifier" => Some(operand),
            "selector_expression" => operand.child_by_field_name("field"),
            _ => None,
        });
    if receiver_name.is_some_and(|name| is_http_client_receiver(text_of(name, src))) {
        return false;
    }
    let is_title_case = method_name
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false)
        && method_name.chars().skip(1).any(|c| c.is_ascii_lowercase());
    if is_title_case && !first_arg_is_url_path(call, src) {
        return false;
    }
    let emitted = emit_route_from_call(
        call, canonical, 0, 1, None, src, file_rel, module_id, repo, scope, acc,
    );
    if emitted && method_name == "Any" {
        acc.route_forms.any += 1;
    }
    emitted
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
    scope: &RouteScope,
    acc: &mut Acc,
) -> bool {
    let Some(func) = outer.child_by_field_name("function") else {
        return false;
    };
    let Some(inner_call) = func.child_by_field_name("operand") else {
        return false;
    };
    if inner_call.kind() != "call_expression" {
        return false;
    }
    let Some(inner_func) = inner_call.child_by_field_name("function") else {
        return false;
    };
    if inner_func.kind() != "selector_expression" {
        return false;
    }
    let Some(inner_field) = inner_func.child_by_field_name("field") else {
        return false;
    };
    let inner_method = text_of(inner_field, src);
    if inner_method != "HandleFunc" && inner_method != "Handle" {
        return false;
    }

    let Some(method_args) = outer.child_by_field_name("arguments") else {
        return false;
    };
    let mut emitted = false;
    let mut cursor = method_args.walk();
    for arg in method_args.named_children(&mut cursor) {
        let Some(method_str) = string_literal_text(arg, src) else {
            continue;
        };
        let method_upper = method_str.to_ascii_uppercase();
        emitted |= emit_route_from_call(
            inner_call,
            &method_upper,
            0,
            1,
            None,
            src,
            file_rel,
            module_id,
            repo,
            scope,
            acc,
        );
    }
    emitted
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
    scope: &RouteScope,
    acc: &mut Acc,
) -> bool {
    let Some(func) = call.child_by_field_name("function") else {
        return false;
    };
    let Some(operand) = func.child_by_field_name("operand") else {
        return false;
    };
    // The receiver's router mount: a local group var's, a router
    // parameter's (CB.23), or (CB.11) a struct field's (`s.v1.GET(..)`).
    // Any other receiver (a call, an index, a deeper chain) is no
    // registration.
    let Some(mount) = scope.receiver_mount(operand, src) else {
        return false;
    };
    let on_field = operand.kind() != "identifier";
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

    // LB.11a: one node per (method, path), the `<METHOD> <path>` shape every
    // other server parser emits, so a route is HANDLED_BY only its own
    // handler and a client call pairs only with the route of its method.
    // LB.5: `join_path` keeps an unprefixed relative literal relative; the
    // qname builder adds the one canonical leading `/`. CB.23: a Const mount
    // is that qname exactly; a Param / Field mount (a group the file cannot
    // read) mints the provisional `<METHOD> <mount:..><path>`, which the
    // build's mount pass (CB.20) re-keys to one ROUTE per prefix the group
    // receives, or to the local path when nothing mounts it.
    let qname = mount_route_qname(method, &mount, &path_literal);
    let full_path = qname
        .split_once(' ')
        .map_or_else(String::new, |(_, path)| path.to_string());
    match mount {
        Mount::Const(_) => {}
        Mount::Param { .. } => acc.mounts.param += 1,
        Mount::Field { .. } => acc.mounts.field += 1,
    }
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
                        // The display is the source text (`h.List`), so the
                        // ROUTE_METHOD cell does not change.
                        let display = format!("{base}.{name}");
                        // CA.5a: `h.List` where `h` is the registering
                        // method's receiver names the receiver's TYPE, which
                        // the graph binds to that type's own method; any
                        // other base (a package, a local) stays a name.
                        let base = match &acc.route_receiver {
                            Some((var, ty)) if *var == base => {
                                acc.receiver_handlers += 1;
                                ty.clone()
                            }
                            _ => base,
                        };
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
    if on_field {
        acc.route_forms.field_calls.insert(call.start_byte());
    }
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

    // ---- CA.5a: a receiver method value names the receiver's type ----

    /// The `handler` field of `route`'s ROUTE_METHOD cells, in push order.
    fn route_handlers(parse: &FileParse, route: NodeId) -> Vec<Option<String>> {
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
            .map(|v| v.get("handler").and_then(|h| h.as_str()).map(String::from))
            .collect()
    }

    #[test]
    fn receiver_method_handler_names_the_receiver_type() {
        // Kina's shape: the handler type registers its own method values.
        let source = r#"package handlers

import (
    "github.com/gin-gonic/gin"
    "example.com/kina/health"
)

type TokensHandler struct{}

func (h *TokensHandler) RegisterRoutes(public *gin.RouterGroup) {
    public.GET("/tokens", h.List)
    public.POST("/tokens", h.Create)
    public.GET("/health", health.Check)
}

func (h *TokensHandler) List(c *gin.Context)   {}
func (h *TokensHandler) Create(c *gin.Context) {}
"#;
        let parse = parse_file(
            source,
            "handlers/tokens.go",
            "handlers::tokens",
            "example.com/kina",
            repo(),
        )
        .unwrap();
        // CB.23: `public` is a `*gin.RouterGroup` parameter, so its routes are
        // provisional mount ROUTEs (the build re-keys them, CB.20); the
        // handler rewrite reads the same refs off the provisional ids.
        let register = "handlers::tokens::TokensHandler::RegisterRoutes";
        let list = param_route_id(register, 0, "GET", "/tokens");
        let create = param_route_id(register, 0, "POST", "/tokens");
        assert_eq!(handled_by(&parse, list), vec![attr("TokensHandler", "List")]);
        assert_eq!(handled_by(&parse, create), vec![attr("TokensHandler", "Create")]);
        // The ROUTE_METHOD cell keeps the source text of the handler.
        assert_eq!(route_handlers(&parse, list), vec![Some("h.List".to_string())]);
        assert_eq!(route_handlers(&parse, create), vec![Some("h.Create".to_string())]);
        // A package-qualified handler in the same method is untouched.
        assert_eq!(
            handled_by(&parse, param_route_id(register, 0, "GET", "/health")),
            vec![attr("health", "Check")]
        );
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "handlers::tokens");
        assert!(parse
            .refs
            .iter()
            .filter(|r| r.from == list || r.from == create)
            .all(|r| r.from_module == module_id));
    }

    #[test]
    fn local_var_handler_is_unchanged() {
        // A local `h` in a FUNCTION, a local `h` in a method whose receiver is
        // `s`, and a function after a receiver method (the receiver is cleared).
        let source = r#"package server

import "github.com/gin-gonic/gin"

type Server struct{}

func (s *Server) Routes(r *gin.Engine) {
    h := NewUsers()
    r.GET("/users", h.List)
}

func setup(r *gin.Engine) {
    h := NewItems()
    r.GET("/items", h.List)
}
"#;
        let parse =
            parse_file(source, "server/server.go", "server", "example.com/app", repo()).unwrap();
        assert_eq!(
            handled_by(&parse, route_id(repo(), "GET", "/users")),
            vec![attr("h", "List")]
        );
        assert_eq!(
            handled_by(&parse, route_id(repo(), "GET", "/items")),
            vec![attr("h", "List")]
        );
        assert_eq!(
            route_handlers(&parse, route_id(repo(), "GET", "/items")),
            vec![Some("h.List".to_string())]
        );
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

    /// The INHERITS_FROM refs out of the struct `qname`, in parse order.
    fn embed_refs(parse: &FileParse, qname: &str) -> Vec<CallQualifier> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STRUCT, qname);
        parse
            .refs
            .iter()
            .filter(|r| r.from == id && r.category == edge_category::INHERITS_FROM)
            .map(|r| r.qualifier.clone())
            .collect()
    }

    /// CI.2a: every embedded field of a struct is an INHERITS_FROM ref, in
    /// field order: bare (pointer and generic forms included) and qualified
    /// through an in-module import; a package outside the module
    /// (`*zap.SugaredLogger`) embeds nothing a parse can bind. The embedded
    /// `*store.Logger` inside `type Logger` records no field type but still
    /// embeds. With no go.mod every import is external: only bare embeds.
    #[test]
    fn struct_embeds_emit_inherits_from_refs() {
        let bare = |n: &str| CallQualifier::Bare(n.to_string());
        let attr = |b: &str, n: &str| CallQualifier::Attribute {
            base: b.to_string(),
            name: n.to_string(),
        };
        let parse = parse_file(FIELD_DECLS, "svc.go", "shop", "example.com/shop", repo()).unwrap();
        assert_eq!(
            embed_refs(&parse, "shop::UserService"),
            vec![bare("UserRepo"), bare("Audit"), attr("store", "Tx"), bare("Base")]
        );
        assert_eq!(embed_refs(&parse, "shop::Logger"), vec![attr("store", "Logger")]);
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "shop");
        let lines: Vec<(u32, NodeId)> = parse
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INHERITS_FROM)
            .map(|r| (r.line, r.from_module))
            .collect();
        let rows = [14, 25, 26, 27, 37];
        assert_eq!(lines, rows.map(|row| (row, module)).to_vec());

        let no_mod = parse_file(FIELD_DECLS, "svc.go", "shop", "", repo()).unwrap();
        assert_eq!(
            embed_refs(&no_mod, "shop::UserService"),
            vec![bare("UserRepo"), bare("Audit"), bare("Base")]
        );
        assert!(embed_refs(&no_mod, "shop::Logger").is_empty());

        // A struct's own type parameter and `any` embed nothing; the
        // predeclared `error` is kept (the graph leaves it unresolved).
        let src = "package p\n\ntype Box[T any] struct {\n\tT\n\terror\n\tany\n\tn int\n}\n";
        let parse = parse_file(src, "p.go", "p", "example.com/p", repo()).unwrap();
        assert_eq!(embed_refs(&parse, "p::Box"), vec![bare("error")]);
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

    /// The imports `src` records under module path `prefix` at the repo
    /// root, with the external-package names they bound.
    fn imports_under(prefix: &str, src: &str) -> (Vec<ImportStmt>, Vec<String>) {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
        parser.set_language(&lang).expect("go grammar");
        let tree = parser.parse(src, None).expect("tree");
        let go = GoModules::root_only(prefix);
        let mut acc = Acc::default();
        let root = tree.root_node();
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if child.kind() == "import_declaration" {
                collect_imports(child, src.as_bytes(), "cmd::main", &go, &mut acc);
            }
        }
        let mut external: Vec<String> = acc.external_pkgs.into_iter().collect();
        external.sort();
        (acc.imports, external)
    }

    /// CI.3: an import of the repository-root module's own path is recorded
    /// as path `""` (the root package dir), bound under the import path's
    /// last element (a `/vN` skipped) or the explicit alias, `_` included; it
    /// is in-repo, so no external package name.
    #[test]
    fn root_package_import_is_recorded() {
        let module = |alias: Option<&str>| ImportTarget::Module {
            path: String::new(),
            alias: alias.map(str::to_string),
        };
        let src = "package main\n\nimport (\n\t\"example.com/app\"\n\trp \"example.com/app\"\n\
                   \t_ \"example.com/app\"\n\t\"example.com/app/store\"\n\t\"fmt\"\n)\n";
        let (imports, external) = imports_under("example.com/app", src);
        let targets: Vec<&ImportTarget> = imports.iter().map(|i| &i.target).collect();
        assert_eq!(
            targets,
            [
                &module(Some("app")),
                &module(Some("rp")),
                &module(Some("_")),
                &ImportTarget::Module { path: "store".to_string(), alias: None },
                &ImportTarget::Module { path: "fmt".to_string(), alias: None },
            ]
        );
        assert!(imports.iter().all(|i| i.from_module == "cmd::main"));
        assert_eq!(external, ["fmt"], "the root import is in-repo");

        let (imports, external) =
            imports_under("example.com/app/v3", "package main\n\nimport \"example.com/app/v3\"\n");
        assert_eq!(imports.iter().map(|i| &i.target).collect::<Vec<_>>(), [&module(Some("app"))]);
        assert!(external.is_empty());
        assert_eq!(go_import_local_name("google.golang.org/grpc"), "grpc");
        assert_eq!(go_import_local_name("example.com/x/y/v3"), "y");
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

    // ---- CB.11: routers held on a struct field ----

    /// Every ROUTE qname of a Go source parsed as `api/server.go`.
    fn field_routes(source: &str) -> (FileParse, Vec<String>) {
        let parse = parse_file(
            source,
            "api/server.go",
            "api::server",
            "example.com/shop",
            repo(),
        )
        .unwrap();
        let routes = route_qnames(&parse);
        (parse, routes)
    }

    #[test]
    fn field_router_registers() {
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    router *gin.Engine
}

func (s *Server) routes() {
    s.router.GET("/health", s.health)
}

func (s *Server) health(c *gin.Context) {}
"#;
        let (parse, routes) = field_routes(source);
        assert_eq!(routes, vec!["GET /health".to_string()]);
        let health = route_id(repo(), "GET", "/health");
        // CA.5a's rewrite applies unchanged: `s` is the receiver of `routes`.
        assert_eq!(handled_by(&parse, health), vec![attr("Server", "health")]);
        assert_eq!(
            route_handlers(&parse, health),
            vec![Some("s.health".to_string())]
        );
        assert_eq!(route_positions(&parse, health).len(), 1);
    }

    #[test]
    fn field_group_prefix_from_constructor() {
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    router *gin.Engine
    v1     *gin.RouterGroup
}

func NewServer() *Server {
    s := &Server{router: gin.New()}
    s.v1 = s.router.Group("/v1")
    s.routes()
    return s
}

func (s *Server) routes() {
    s.router.GET("/health", s.health)
    s.v1.GET("/orders", s.listOrders)
    s.v1.POST("/orders", s.createOrder)
}
"#;
        let (parse, routes) = field_routes(source);
        assert_eq!(
            routes,
            vec![
                "GET /health".to_string(),
                "GET /v1/orders".to_string(),
                "POST /v1/orders".to_string(),
            ]
        );
        assert_eq!(
            handled_by(&parse, route_id(repo(), "GET", "/v1/orders")),
            vec![attr("Server", "listOrders")]
        );
    }

    #[test]
    fn composite_literal_field_group() {
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    api *gin.RouterGroup
}

func NewServer(r *gin.Engine) *Server {
    return &Server{api: r.Group("/api")}
}

func (s *Server) routes() {
    s.api.GET("/users", s.listUsers)
}
"#;
        let (_, routes) = field_routes(source);
        assert_eq!(routes, vec!["GET /api/users".to_string()]);
    }

    #[test]
    fn group_from_a_field() {
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    router *gin.Engine
    v1     *gin.RouterGroup
}

func NewServer() *Server {
    s := new(Server)
    s.v1 = s.router.Group("/v1")
    return s
}

func (s *Server) routes() {
    g := s.v1.Group("/x")
    g.GET("/y", s.y)
}
"#;
        let (_, routes) = field_routes(source);
        assert_eq!(routes, vec!["GET /v1/x/y".to_string()]);
    }

    #[test]
    fn client_field_is_not_a_router() {
        let source = r#"package api

type Server struct {
    client     *Client
    httpClient *http.Client
}

func (s *Server) sync() {
    s.client.Get("/x")
    s.httpClient.Post("/y", "application/json", nil)
    s.client.GET("/z")
}
"#;
        let (_, routes) = field_routes(source);
        assert!(
            routes.is_empty(),
            "a client-shaped field is no router: {routes:?}"
        );
    }

    #[test]
    fn unknown_field_is_a_field_mount() {
        // CB.23: a group-typed field the file assigns nothing is a provisional
        // Field mount, which the build re-keys to the local path (`GET /z`)
        // when no FieldMount fact of the package reaches it (CB.20).
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    other *gin.RouterGroup
    v1    *gin.RouterGroup
}

func (s *Server) routes() {
    s.other.GET("/z", s.z)
    // A deeper chain or a call receiver stays no registration.
    s.deps.router.GET("/deep", s.z)
    router().GET("/call", s.z)
}
"#;
        let (_, routes) = field_routes(source);
        assert_eq!(routes, vec!["GET <mount:field:api::Server.other>/z".to_string()]);
    }

    #[test]
    fn conflicting_field_groups_are_field_mounts() {
        // Two assignments that disagree give the field no in-file prefix:
        // unknown beats wrong. CB.23: its routes are a provisional Field
        // mount, and each assignment is a FieldMount fact, so the build mounts
        // the routes at every prefix a constructor assigns (CB.20). A field
        // typed through a parameter still records its one prefix.
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    router *gin.Engine
    v1     *gin.RouterGroup
    admin  *gin.RouterGroup
}

func NewServer() *Server {
    s := &Server{router: gin.New()}
    s.v1 = s.router.Group("/v1")
    return s
}

func NewLegacy() *Server {
    s := &Server{router: gin.New()}
    s.v1 = s.router.Group("/legacy")
    return s
}

func mountAdmin(s *Server) {
    s.admin = s.router.Group("/admin")
}

func (s *Server) routes() {
    s.v1.GET("/orders", s.list)
    s.admin.DELETE("/users/:id", s.remove)
}
"#;
        let (parse, routes) = field_routes(source);
        assert_eq!(
            routes,
            vec![
                "DELETE /admin/users/:id".to_string(),
                "GET <mount:field:api::Server.v1>/orders".to_string()
            ]
        );
        let v1 = |fn_qname: &str| -> Vec<Mount> {
            field_mounts(&parse, func_id(fn_qname))
                .into_iter()
                .filter(|(owner, field, _)| owner == "api::Server" && field == "v1")
                .map(|(_, _, mount)| mount)
                .collect()
        };
        assert_eq!(v1("api::server::NewServer"), vec![Mount::Const("/v1".into())]);
        assert_eq!(v1("api::server::NewLegacy"), vec![Mount::Const("/legacy".into())]);
    }

    #[test]
    fn field_group_chain_resolves_in_any_function_order() {
        // `v2` is assigned from `api` before the file assigns `api`, and a
        // local group handed to a field keeps its prefix. The stdlib mux and
        // a Gorilla chain on a field register too.
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    router *gin.Engine
    api    *gin.RouterGroup
    v2     *gin.RouterGroup
    admin  *gin.RouterGroup
    mux    *http.ServeMux
}

func (s *Server) versions() {
    s.v2 = s.api.Group("/v2")
    grp := s.router.Group("/admin")
    s.admin = grp
}

func NewServer() *Server {
    var s Server
    s.api = s.router.Group("/api")
    return &s
}

func (s *Server) routes() {
    s.v2.GET("/items", s.items)
    s.admin.GET("/stats", s.stats)
    s.mux.HandleFunc("/metrics", s.metrics)
}
"#;
        let (_, routes) = field_routes(source);
        assert_eq!(
            routes,
            vec![
                "ANY /metrics".to_string(),
                "GET /admin/stats".to_string(),
                "GET /api/v2/items".to_string(),
            ]
        );
    }

    #[test]
    fn shadowed_receiver_type_is_unknown() {
        // `s` is the receiver AND a local of another type in one body: the
        // walk cannot tell which `s.v1` a registration means, so it is
        // unprefixed rather than guessed.
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    router *gin.Engine
    v1     *gin.RouterGroup
}

type Other struct {
    v1 *gin.RouterGroup
}

func NewServer() *Server {
    s := &Server{router: gin.New()}
    s.v1 = s.router.Group("/v1")
    return s
}

func (s *Server) routes() {
    if true {
        s := &Other{}
        _ = s
    }
    s.v1.GET("/orders", s.list)
}
"#;
        let (_, routes) = field_routes(source);
        assert_eq!(routes, vec!["GET /orders".to_string()]);
    }

    // ---- CB.23: router mounts through parameters and struct fields ----

    /// The fixture's sources (`go-route-mounts`), so the unit tests and the
    /// graded fixture describe one program.
    const MOUNTS_MAIN: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/go-route-mounts/cmd/main.go");
    const MOUNTS_USERS: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/go-route-mounts/api/users.go");
    const MOUNTS_SERVER: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/go-route-mounts/api/server.go");
    const MOUNTS_ADMIN: &str = include_str!(
        "../../../../bench/substrate-gap/fixtures/go-route-mounts/api/admin_routes.go"
    );

    /// `source` parsed as `file` of module `module` under `example.com/shop`.
    fn parse_as(source: &str, file: &str, module: &str) -> FileParse {
        parse_file(source, file, module, "example.com/shop", repo()).unwrap()
    }

    fn param_mount(fn_qname: &str, index: u32, suffix: &str) -> Mount {
        Mount::Param {
            fn_qname: fn_qname.to_string(),
            index,
            suffix: suffix.to_string(),
        }
    }

    /// The provisional id of a ROUTE registered on parameter `index` of
    /// `fn_qname`: `<METHOD> <mount:param:<fn>#<i>><path>`.
    fn param_route_id(fn_qname: &str, index: u32, method: &str, path: &str) -> NodeId {
        let qname = mount_route_qname(method, &param_mount(fn_qname, index, ""), path);
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, &qname)
    }

    /// `scope`'s MountArg facts as `(line, callee, arg, mount)`, in record
    /// order.
    fn mount_args(parse: &FileParse, scope: NodeId) -> Vec<(u32, String, u32, Mount)> {
        let facts = parse.nav.nav_facts.get(&scope).cloned().unwrap_or_default();
        facts
            .into_iter()
            .filter_map(|f| match f {
                NavFact::MountArg {
                    line,
                    callee,
                    arg,
                    mount,
                } => Some((line, callee, arg, mount)),
                _ => None,
            })
            .collect()
    }

    /// `scope`'s FieldMount facts as `(owner, field, mount)`, in record order.
    fn field_mounts(parse: &FileParse, scope: NodeId) -> Vec<(String, String, Mount)> {
        let facts = parse.nav.nav_facts.get(&scope).cloned().unwrap_or_default();
        facts
            .into_iter()
            .filter_map(|f| match f {
                NavFact::FieldMount {
                    owner,
                    field,
                    mount,
                } => Some((owner, field, mount)),
                _ => None,
            })
            .collect()
    }

    /// Every MountArg / FieldMount fact of the parse, any scope.
    fn mount_fact_count(parse: &FileParse) -> usize {
        parse
            .nav
            .nav_facts
            .values()
            .flatten()
            .filter(|f| matches!(f, NavFact::MountArg { .. } | NavFact::FieldMount { .. }))
            .count()
    }

    #[test]
    fn param_route_is_provisional() {
        let source = r#"package pkg

import "github.com/gin-gonic/gin"

func Register(rg *gin.RouterGroup) {
    rg.GET("/u", h)
}

func h(c *gin.Context) {}
"#;
        let parse = parse_as(source, "pkg/routes.go", "pkg");
        assert_eq!(
            route_qnames(&parse),
            vec!["GET <mount:param:pkg::Register#0>/u".to_string()]
        );
        // The refs and cells ride on the provisional id, as on any ROUTE.
        let u = param_route_id("pkg::Register", 0, "GET", "/u");
        assert_eq!(handled_by(&parse, u), vec![bare("h")]);
        assert_eq!(route_methods(&parse, u), vec!["GET".to_string()]);
        assert_eq!(route_positions(&parse, u).len(), 1);
        // The fixture's register functions: one parameter each.
        let users = parse_as(MOUNTS_USERS, "api/users.go", "api::users");
        assert_eq!(
            route_qnames(&users),
            vec![
                "GET <mount:param:api::users::RegisterHealth#0>/healthz".to_string(),
                "GET <mount:param:api::users::RegisterUsers#0>/me/profile".to_string(),
                "GET <mount:param:api::users::RegisterUsers#0>/users".to_string(),
            ]
        );
    }

    #[test]
    fn group_from_param_carries_suffix() {
        // `me := rg.Group("/me")` keeps the parameter as its base and grows the
        // suffix; a grouped parameter list counts names (Kina's shape).
        let source = r#"package pkg

import "github.com/gin-gonic/gin"

func Register(public, protected *gin.RouterGroup, kyc gin.IRouter) {
    me := protected.Group("/me")
    me.GET("/p", h)
    v := me.Group("v")
    v.POST("/x", h)
    kyc.GET("/k", h)
    public.GET("/pub", h)
}
"#;
        let parse = parse_as(source, "pkg/routes.go", "pkg");
        assert_eq!(
            route_qnames(&parse),
            vec![
                "GET <mount:param:pkg::Register#0>/pub".to_string(),
                "GET <mount:param:pkg::Register#1>/me/p".to_string(),
                "GET <mount:param:pkg::Register#2>/k".to_string(),
                "POST <mount:param:pkg::Register#1>/me/v/x".to_string(),
            ]
        );
    }

    #[test]
    fn mount_arg_recorded() {
        let parse = parse_as(MOUNTS_MAIN, "cmd/main.go", "cmd::main");
        let main = func_id("cmd::main::main");
        let args = mount_args(&parse, main);
        let v2_row = row_of(MOUNTS_MAIN, "api.RegisterUsers(v2)");
        assert!(
            args.contains(&(
                v2_row,
                "RegisterUsers".to_string(),
                0,
                Mount::Const("/api/v2".into())
            )),
            "{args:?}"
        );
        // On the row of the call's CallSite (its CALLS edge's EVIDENCE line),
        // from the same scope: what the build's mount pass joins on (CB.20).
        assert!(
            calls_from(&parse, main).contains(&(attr("api", "RegisterUsers"), v2_row)),
            "{:?}",
            calls_from(&parse, main)
        );
        // The fixture's main records exactly its three mounting calls; the
        // router's own methods (`r.Run()`) record nothing.
        assert_eq!(args.len(), 3, "{args:?}");
    }

    #[test]
    fn inline_group_arg() {
        let parse = parse_as(MOUNTS_MAIN, "cmd/main.go", "cmd::main");
        let args = mount_args(&parse, func_id("cmd::main::main"));
        assert!(
            args.contains(&(
                row_of(MOUNTS_MAIN, "api.RegisterUsers(r.Group(\"/api/v1\"))"),
                "RegisterUsers".to_string(),
                0,
                Mount::Const("/api/v1".into())
            )),
            "{args:?}"
        );
    }

    #[test]
    fn param_forwarded() {
        let source = r#"package pkg

import "github.com/gin-gonic/gin"

func A(ctx context.Context, rg *gin.RouterGroup) {
    B(rg.Group("/x"))
    helpers.Mount(ctx, rg)
}
"#;
        let parse = parse_as(source, "pkg/routes.go", "pkg");
        assert_eq!(
            mount_args(&parse, func_id("pkg::A")),
            vec![
                (
                    row_of(source, "B(rg.Group"),
                    "B".to_string(),
                    0,
                    param_mount("pkg::A", 1, "/x")
                ),
                (
                    row_of(source, "helpers.Mount"),
                    "Mount".to_string(),
                    1,
                    param_mount("pkg::A", 1, "")
                ),
            ]
        );
    }

    #[test]
    fn field_mount_recorded() {
        // The fixture: `NewServer(r *gin.Engine)` — an engine is a router
        // root, so the field's mount is the literal group.
        let server = parse_as(MOUNTS_SERVER, "api/server.go", "api::server");
        assert_eq!(
            field_mounts(&server, func_id("api::server::NewServer")),
            vec![(
                "api::Server".to_string(),
                "admin".to_string(),
                Mount::Const("/admin".into())
            )]
        );
        // A group-typed parameter hands the field whatever mounts the
        // constructor; a keyed literal records too.
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    admin *gin.RouterGroup
    api   *gin.RouterGroup
}

func NewServer(r *gin.RouterGroup) *Server {
    s := &Server{api: r.Group("/api")}
    s.admin = r.Group("/admin")
    return s
}
"#;
        let parse = parse_as(source, "api/server.go", "api::server");
        assert_eq!(
            field_mounts(&parse, func_id("api::server::NewServer")),
            vec![
                (
                    "api::Server".to_string(),
                    "api".to_string(),
                    param_mount("api::server::NewServer", 0, "/api")
                ),
                (
                    "api::Server".to_string(),
                    "admin".to_string(),
                    param_mount("api::server::NewServer", 0, "/admin")
                ),
            ]
        );
        // The route in another file of the package names the same owner: the
        // package directory, not the file's module.
        let admin = parse_as(MOUNTS_ADMIN, "api/admin_routes.go", "api::admin_routes");
        assert_eq!(
            route_qnames(&admin),
            vec!["GET <mount:field:api::Server.admin>/stats".to_string()]
        );
    }

    #[test]
    fn router_root_is_a_const_mount() {
        let parse = parse_as(MOUNTS_MAIN, "cmd/main.go", "cmd::main");
        let args = mount_args(&parse, func_id("cmd::main::main"));
        assert!(
            args.contains(&(
                row_of(MOUNTS_MAIN, "api.NewServer(r)"),
                "NewServer".to_string(),
                0,
                Mount::Const(String::new())
            )),
            "{args:?}"
        );
        // An engine / echo parameter is a root too: it registers at the local
        // path, and hands the root on.
        let source = r#"package pkg

func setup(r *gin.Engine, e *echo.Echo) {
    r.GET("/a", h)
    e.GET("/b", h)
    api.Register(r)
}
"#;
        let parse = parse_as(source, "pkg/routes.go", "pkg");
        assert_eq!(
            route_qnames(&parse),
            vec!["GET /a".to_string(), "GET /b".to_string()]
        );
        assert_eq!(
            mount_args(&parse, func_id("pkg::setup")),
            vec![(
                row_of(source, "api.Register(r)"),
                "Register".to_string(),
                0,
                Mount::Const(String::new())
            )]
        );
    }

    #[test]
    fn body_local_groups_unchanged() {
        // A HEAD-shaped registration (every group read in one body) keeps its
        // exact qname and records no mount fact.
        let source = r#"package main

import "github.com/gin-gonic/gin"

func main() {
    r := gin.Default()
    api := r.Group("/api")
    v1 := api.Group("/v1")
    v1.GET("/users", listUsers)
    r.GET("/health", health)
    r.Run(":8080")
}
"#;
        let parse = parse_as(source, "main.go", "main");
        assert_eq!(
            route_qnames(&parse),
            vec!["GET /api/v1/users".to_string(), "GET /health".to_string()]
        );
        assert_eq!(mount_fact_count(&parse), 0);
    }

    #[test]
    fn non_router_args_record_nothing() {
        // A handler context, an unknown local, a call into a package outside
        // the module and a router's own method carry no mount.
        let source = r#"package pkg

import (
    "net/http"

    "github.com/gin-gonic/gin"
)

func handle(c *gin.Context) {
    svc.Do(c, id)
}

func serve(mux *http.ServeMux, rg *gin.RouterGroup, router *Router) {
    http.ListenAndServe(":8080", mux)
    rg.Use(auth(rg))
    other(router)
}
"#;
        let parse = parse_as(source, "pkg/routes.go", "pkg");
        assert!(mount_args(&parse, func_id("pkg::handle")).is_empty());
        // `auth(rg)` is a call of its own, nested in the router's `Use`.
        assert_eq!(
            mount_args(&parse, func_id("pkg::serve")),
            vec![(
                row_of(source, "rg.Use(auth(rg))"),
                "auth".to_string(),
                0,
                param_mount("pkg::serve", 1, "")
            )]
        );
    }

    // ---- CI.1: package-var initialiser calls belong to the var's STATE_VAR ----

    /// The bench fixture's source (`go-package-var-closures`), so the unit
    /// tests and the graded fixture describe one program.
    const PKG_VAR_HOOKS: &str = include_str!(
        "../../../../bench/substrate-gap/fixtures/go-package-var-closures/hooks/hooks.go"
    );

    fn hooks_parse() -> FileParse {
        parse_file(PKG_VAR_HOOKS, "hooks/hooks.go", "hooks::hooks", "example.com/hooks", repo())
            .unwrap()
    }

    fn state_var_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::STATE_VAR, qname)
    }

    #[test]
    fn package_var_literal_calls_belong_to_the_var() {
        let parse = hooks_parse();
        let var = |name: &str| state_var_id(&format!("hooks::hooks::{name}"));
        let row = |needle: &str| row_of(PKG_VAR_HOOKS, needle);
        assert_eq!(
            calls_from(&parse, var("newStore")),
            vec![(attr("store", "Open"), row("return store.Open()"))]
        );
        assert_eq!(
            calls_from(&parse, var("timeNow")),
            vec![(bare("clock"), row("return clock()"))]
        );
        assert_eq!(
            calls_from(&parse, var("onEvict")),
            vec![
                (attr("store", "Evict"), row("store.Evict(key)")),
                (bare("audit"), row("audit(key)")),
            ]
        );
        assert_eq!(
            calls_from(&parse, var("handlers")),
            vec![(bare("flush"), row("{ flush() }"))]
        );
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "hooks::hooks");
        assert_eq!(calls_from(&parse, module), vec![]);
        // A literal is no declaration: no FUNCTION is minted for it.
        let minted: Vec<&String> = parse
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, q)| {
                parse.nav.kind_by_id.get(*id) == Some(&node_kind::FUNCTION)
                    && (q.ends_with("newStore") || q.ends_with("onEvict"))
            })
            .map(|(_, q)| q)
            .collect();
        assert!(minted.is_empty(), "{minted:?}");
        for name in ["newStore", "onEvict"] {
            let fn_id = func_id(&format!("hooks::hooks::{name}"));
            assert!(!parse.nodes.iter().any(|n| n.id == fn_id), "FUNCTION {name}");
        }
    }

    #[test]
    fn nested_package_var_literals_are_drained() {
        let source = r#"package xds

import "sync"

var once sync.Once

var z string

var getZone = func() string {
    once.Do(func() {
        fetch()
    })
    return z
}

func fetch() {}
"#;
        let parse = parse_file(source, "xds/utils.go", "xds", "", repo()).unwrap();
        // The body first, then the nested literal it queued.
        assert_eq!(
            calls_from(&parse, state_var_id("xds::getZone")),
            vec![
                (attr("once", "Do"), row_of(source, "once.Do(func()")),
                (bare("fetch"), row_of(source, "        fetch()")),
            ]
        );
    }

    #[test]
    fn package_var_literal_params_are_locals_of_the_var() {
        let source = r#"package app

import "example.com/app/store"

var onSave = func(repo *store.Repo) {
    repo.Save()
}
"#;
        let parse = parse_file(source, "app/hooks.go", "app", "example.com/app", repo()).unwrap();
        let on_save = state_var_id("app::onSave");
        assert_eq!(
            locals_of(&parse, on_save),
            Some(pairs(&[("repo", "store.Repo")]))
        );
        assert_eq!(
            calls_from(&parse, on_save),
            vec![(attr("repo", "Save"), row_of(source, "repo.Save()"))]
        );
    }

    #[test]
    fn package_var_initialiser_calls_belong_to_the_var() {
        let parse = hooks_parse();
        let var = |name: &str| state_var_id(&format!("hooks::hooks::{name}"));
        let row = |needle: &str| row_of(PKG_VAR_HOOKS, needle);
        assert_eq!(
            calls_from(&parse, var("defaultStore")),
            vec![(attr("store", "Open"), 20)]
        );
        assert_eq!(row("var defaultStore = store.Open()"), 20);
        let paired = row("limit, ttl = clamp(10), clamp(20)");
        assert_eq!(calls_from(&parse, var("limit")), vec![(bare("clamp"), paired)]);
        assert_eq!(calls_from(&parse, var("ttl")), vec![(bare("clamp"), paired)]);
        // One multi-value initialiser pairs with the first name only.
        assert_eq!(
            calls_from(&parse, var("lo")),
            vec![(bare("bounds"), row("lo, hi     = bounds()"))]
        );
        assert_eq!(calls_from(&parse, var("hi")), vec![]);

        let source = r#"package app

import "github.com/google/wire"

var svc = New()
var n = 3
var ProviderSet = wire.NewSet(NewA)
const c = len("ab")
"#;
        let parse = parse_file(source, "app/set.go", "app", "", repo()).unwrap();
        let (svc, set) = (state_var_id("app::svc"), state_var_id("app::ProviderSet"));
        assert_eq!(
            calls_from(&parse, svc),
            vec![(bare("New"), row_of(source, "var svc = New()"))]
        );
        assert_eq!(
            calls_from(&parse, set),
            vec![(attr("wire", "NewSet"), row_of(source, "var ProviderSet"))]
        );
        // `n` is noise-gated, the const is not walked: nothing else calls.
        let others: Vec<&CallSite> = parse
            .calls
            .iter()
            .filter(|c| c.from != svc && c.from != set)
            .collect();
        assert!(others.is_empty(), "{others:?}");
        assert!(
            parse.nodes.iter().any(|n| n.id == state_var_id("app::c")),
            "the const keeps its STATE_VAR"
        );
        // The provider walk is the call walk: each ref once, not doubled.
        assert_eq!(injects_refs(&parse), vec![(set, bare("NewA"))]);
    }

    #[test]
    fn initialiser_walk_records_no_locals() {
        let source = r#"package app

import "example.com/app/store"

var svc = New()
var onSave = func(repo *store.Repo) {}
"#;
        let parse = parse_file(source, "app/vars.go", "app", "example.com/app", repo()).unwrap();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "app");
        let (svc, on_save) = (state_var_id("app::svc"), state_var_id("app::onSave"));
        let module_locals = locals_of(&parse, module).unwrap_or_default();
        assert!(
            module_locals.contains(&("svc".to_string(), "New()".to_string())),
            "{module_locals:?}"
        );
        assert_eq!(
            locals_of(&parse, on_save),
            Some(pairs(&[("repo", "store.Repo")]))
        );
        assert_eq!(locals_of(&parse, svc), None);
        // The MODULE scope and the literal's var scope, nothing else.
        assert_eq!(parse.nav.local_types.len(), 2);
    }

    // ---- CI.5: mount owners through imports, parameter-rooted field groups ----

    /// The bench fixture's sources (`go-route-mount-owners`), so the unit
    /// tests and the graded fixture describe one program.
    const OWNERS_MAIN: &str =
        include_str!("../../../../bench/substrate-gap/fixtures/go-route-mount-owners/cmd/main.go");
    const OWNERS_PANEL: &str = include_str!(
        "../../../../bench/substrate-gap/fixtures/go-route-mount-owners/admin/panel.go"
    );

    /// `source` parsed as `file` of module `module` under the fixture's
    /// `example.com/owners`.
    fn parse_owners(source: &str, file: &str, module: &str) -> FileParse {
        parse_file(source, file, module, "example.com/owners", repo()).unwrap()
    }

    #[test]
    fn foreign_struct_field_mount_names_the_imported_package() {
        // `s := &api.Server{}; s.Public = r.Group("/public")` in cmd/: the
        // owner is the imported package's directory, the owner api/server.go's
        // `s.Public.GET(..)` names (HEAD: `cmd::Server`, never met).
        let parse = parse_owners(OWNERS_MAIN, "cmd/main.go", "cmd::main");
        assert_eq!(
            field_mounts(&parse, func_id("cmd::main::main")),
            vec![(
                "api::Server".to_string(),
                "Public".to_string(),
                Mount::Const("/public".into())
            )]
        );
        let server = parse_owners(
            include_str!(
                "../../../../bench/substrate-gap/fixtures/go-route-mount-owners/api/server.go"
            ),
            "api/server.go",
            "api::server",
        );
        assert_eq!(
            route_qnames(&server),
            vec!["GET <mount:field:api::Server.Public>/items".to_string()]
        );
    }

    #[test]
    fn foreign_composite_literal_mount() {
        // A keyed literal of another package's struct, under its own name, an
        // alias, and the repo-root package (whose directory is the root).
        let source = r#"package main

import (
    "example.com/owners"
    "example.com/owners/api"
    web "example.com/owners/web/v2"
    "github.com/gin-gonic/gin"
)

func main() {
    r := gin.Default()
    s := &api.Server{Public: r.Group("/p")}
    w := web.Server{Admin: r.Group("/w")}
    a := owners.App{Root: r}
    run(s, w, a)
}
"#;
        let parse = parse_owners(source, "cmd/main.go", "cmd::main");
        assert_eq!(
            field_mounts(&parse, func_id("cmd::main::main")),
            vec![
                (
                    "api::Server".to_string(),
                    "Public".to_string(),
                    Mount::Const("/p".into())
                ),
                (
                    "web::v2::Server".to_string(),
                    "Admin".to_string(),
                    Mount::Const("/w".into())
                ),
                ("App".to_string(), "Root".to_string(), Mount::Const(String::new())),
            ]
        );
    }

    #[test]
    fn foreign_struct_never_collides_with_a_local_one() {
        // This package's `Server.v1` and api's `Server.v1` are two fields:
        // HEAD keyed both `("Server", "v1")`, saw two prefixes and read none.
        let source = r#"package app

import (
    "example.com/owners/api"
    "github.com/gin-gonic/gin"
)

type Server struct {
    v1 *gin.RouterGroup
}

func NewServer(r *gin.Engine) *Server {
    s := &Server{}
    s.v1 = r.Group("/v1")
    o := new(api.Server)
    o.v1 = r.Group("/other")
    s.v1.GET("/a", h)
    o.v1.GET("/b", h)
    return s
}
"#;
        let parse = parse_owners(source, "app/server.go", "app::server");
        assert_eq!(
            route_qnames(&parse),
            vec!["GET /other/b".to_string(), "GET /v1/a".to_string()]
        );
        assert_eq!(
            field_mounts(&parse, func_id("app::server::NewServer")),
            vec![
                (
                    "app::Server".to_string(),
                    "v1".to_string(),
                    Mount::Const("/v1".into())
                ),
                (
                    "api::Server".to_string(),
                    "v1".to_string(),
                    Mount::Const("/other".into())
                ),
            ]
        );
    }

    #[test]
    fn outside_package_struct_records_no_mount() {
        // A struct of a package outside the module places no owner: no
        // FieldMount, and a registration on its field is the unprefixed root
        // route itself, not a provisional no FieldMount could reach.
        let source = r#"package main

import (
    "net/http"

    "github.com/gin-gonic/gin"
)

func main() {
    r := gin.Default()
    srv := &http.Server{}
    srv.Handler = r
    srv.ListenAndServe()
}

func serve(srv *http.Server) {
    srv.Handler.GET("/health", health)
}
"#;
        let parse = parse_owners(source, "cmd/main.go", "cmd::main");
        assert_eq!(mount_fact_count(&parse), 0);
        assert_eq!(route_qnames(&parse), vec!["GET /health".to_string()]);
        let health = route_id(repo(), "GET", "/health");
        assert_eq!(handled_by(&parse, health), vec![bare("health")]);
    }

    #[test]
    fn param_rooted_field_holds_the_param_mount() {
        // The fixture: `p.admin = rg.Group("/admin")` in NewPanel(rg
        // *gin.RouterGroup) holds rg's mount then `/admin` (HEAD: the root,
        // `GET /admin/stats` as a Const route).
        let parse = parse_owners(OWNERS_PANEL, "admin/panel.go", "admin::panel");
        assert_eq!(
            route_qnames(&parse),
            vec!["GET <mount:param:admin::panel::NewPanel#0>/admin/stats".to_string()]
        );
        let new_panel = func_id("admin::panel::NewPanel");
        assert_eq!(
            field_mounts(&parse, new_panel),
            vec![(
                "admin::Panel".to_string(),
                "admin".to_string(),
                param_mount("admin::panel::NewPanel", 0, "/admin")
            )]
        );
        let stats = param_route_id("admin::panel::NewPanel", 0, "GET", "/admin/stats");
        assert_eq!(handled_by(&parse, stats), vec![attr("p", "stats")]);

        // Registered in another callable of the file, through a local group
        // and a second parameter; a field passed on carries the Param too.
        let source = r#"package admin

import "github.com/gin-gonic/gin"

type Panel struct {
    admin *gin.RouterGroup
    raw   *gin.RouterGroup
}

func NewPanel(ctx context.Context, rg *gin.RouterGroup) *Panel {
    p := &Panel{}
    g := rg.Group("/admin")
    p.admin = g.Group("/v1")
    p.raw = rg
    return p
}

func (p *Panel) routes() {
    p.admin.GET("/stats", p.stats)
    p.raw.GET("/ping", p.ping)
    register(p.admin)
}
"#;
        let parse = parse_owners(source, "admin/panel.go", "admin::panel");
        assert_eq!(
            route_qnames(&parse),
            vec![
                "GET <mount:param:admin::panel::NewPanel#1>/admin/v1/stats".to_string(),
                "GET <mount:param:admin::panel::NewPanel#1>/ping".to_string(),
            ]
        );
        assert_eq!(
            mount_args(&parse, method_id("admin::panel::Panel::routes")),
            vec![(
                row_of(source, "register(p.admin)"),
                "register".to_string(),
                0,
                param_mount("admin::panel::NewPanel", 1, "/admin/v1")
            )]
        );
    }

    #[test]
    fn unmounted_param_field_keeps_its_suffix() {
        // panel.go alone: no caller mounts NewPanel, so CB.20 re-keys the
        // provisional to its suffix + local path, HEAD's `GET /admin/stats`
        // byte for byte.
        let parse = parse_owners(OWNERS_PANEL, "admin/panel.go", "admin::panel");
        let routes = route_qnames(&parse);
        let [route] = routes.as_slice() else {
            panic!("one route: {routes:?}");
        };
        let (method, mount, path) =
            glia_code_domain::endpoint::parse_mount_route_qname(route).expect("provisional");
        assert_eq!(mount, param_mount("admin::panel::NewPanel", 0, ""));
        assert_eq!(format!("{method} {path}"), "GET /admin/stats");
    }

    #[test]
    fn root_param_field_stays_known() {
        // A ROOT-typed parameter is the root, as before CI.5: the field holds
        // the literal group, and the route is that Const route (CB.11).
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    admin *gin.RouterGroup
}

func NewServer(r *gin.Engine) *Server {
    s := &Server{}
    s.admin = r.Group("/admin")
    s.admin.GET("/stats", s.stats)
    return s
}
"#;
        let parse = parse_owners(source, "api/server.go", "api::server");
        assert_eq!(route_qnames(&parse), vec!["GET /admin/stats".to_string()]);
    }

    #[test]
    fn param_and_root_assignments_disagree() {
        // A field one callable roots at a group-typed parameter while another
        // assigns it from a router root holds two mounts: unknown in the file,
        // so the route is CB.23's Field mount, which CB.20 resolves from both
        // FieldMount facts (HEAD read both as the root: `GET /m/x`).
        let source = r#"package api

import "github.com/gin-gonic/gin"

type Server struct {
    mixed *gin.RouterGroup
}

func NewServer(r *gin.Engine) *Server {
    s := &Server{}
    s.mixed = r.Group("/m")
    s.mixed.GET("/x", s.x)
    return s
}

func (s *Server) Mount(rg *gin.RouterGroup) {
    s.mixed = rg.Group("/m")
}
"#;
        let parse = parse_owners(source, "api/server.go", "api::server");
        assert_eq!(
            route_qnames(&parse),
            vec!["GET <mount:field:api::Server.mixed>/x".to_string()]
        );
        assert_eq!(
            field_mounts(&parse, func_id("api::server::NewServer")),
            vec![(
                "api::Server".to_string(),
                "mixed".to_string(),
                Mount::Const("/m".into())
            )]
        );
        assert_eq!(
            field_mounts(&parse, method_id("api::server::Server::Mount")),
            vec![(
                "api::Server".to_string(),
                "mixed".to_string(),
                param_mount("api::server::Server::Mount", 0, "/m")
            )]
        );
    }
}
