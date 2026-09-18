//! repo-graph-parser-go — tree-sitter Go → code-domain FileParse.
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

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use repo_graph_code_domain::di_stats::{self, DiShape};
use repo_graph_code_domain::endpoint::{
    ClientEndpoint, HitExtras, canonical_http_path, client_url_split, join_path,
    push_client_endpoint_with, route_path_qname,
};

// ============================================================================
// Public entry point
// ============================================================================

/// Parse one Go source file.
///
/// `package_qname` is the repo-local `::`-separated path for the package
/// (e.g. `svc::users` for `<repo>/svc/users/*.go`).
///
/// `module_import_prefix` is the `module` line from `go.mod` (e.g.
/// `github.com/foo/bar`) — used to map absolute Go import paths onto
/// repo-local qnames. Pass `""` for a packageless / single-file parse.
pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    package_qname: &str,
    module_import_prefix: &str,
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
                collect_imports(child, src, package_qname, module_import_prefix, &mut acc);
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
            "type_declaration" => { /* already collected in first pass */ }
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
    let forms = &acc.route_forms;
    if forms.registrations > 0 {
        eprintln!(
            "[go-routes] registrations={} positioned={} forms(handle={} any={} match={} pattern={}) in {file_rel_path}",
            forms.registrations,
            forms.positioned,
            forms.handle,
            forms.any,
            forms.matched,
            forms.pattern
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
    /// Route NodeId → set of methods already recorded on that node within this
    /// file. Prevents stacking duplicate ROUTE_METHOD cells when a body walks
    /// past the same registration twice (shouldn't happen, defensive).
    route_methods_seen: HashMap<NodeId, HashMap<String, ()>>,
    /// Dedup for client-HTTP ENDPOINT nodes (Pattern A) — one node per
    /// (method, path) even if the same endpoint is called twice in a file.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Dedup for ACCESSES_DATA edges — one edge per (enclosing fn, DATA_ENTITY)
    /// even if the same table is queried repeatedly inside the same function.
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
    /// LA.18d: start byte of every func-literal route handler already expanded
    /// in this file. A Gorilla `.Methods("GET", "POST")` chain re-enters
    /// `emit_route_from_call` once per verb with the same literal; its callee
    /// refs are pushed once. `len()` is the marker's handler count.
    func_literal_handlers: std::collections::HashSet<usize>,
    /// LA.18d: HANDLED_BY refs pushed from func-literal handlers in this file.
    func_literal_refs: usize,
    /// LA.32a: route registrations emitted in this file, the POSITION cells
    /// pushed for them, and the method-bearing forms among them — the
    /// `[go-routes] registrations=` marker's counters.
    route_forms: RouteFormCounts,
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
        });
        type_ids.insert(name, id);
    }
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
    if repo_graph_doc::leading_doc(&spec, src).is_some() {
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
    });

    if let Some(body) = decl.child_by_field_name("body") {
        collect_calls_in(body, src, id, None, repo, file_rel, acc);
        collect_routes_in(body, src, file_rel, module_id, repo, acc);
    }
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
    });

    if let Some(body) = decl.child_by_field_name("body") {
        collect_calls_in(body, src, id, receiver_var.as_deref(), repo, file_rel, acc);
        collect_routes_in(body, src, file_rel, module_id, repo, acc);
    }
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
// Import collection
// ============================================================================

fn collect_imports(
    decl: TsNode,
    src: &[u8],
    package_qname: &str,
    module_import_prefix: &str,
    acc: &mut Acc,
) {
    // import_declaration may wrap an import_spec_list or a single import_spec.
    let mut cursor = decl.walk();
    for child in decl.named_children(&mut cursor) {
        match child.kind() {
            "import_spec" => {
                record_import(child, src, package_qname, module_import_prefix, acc);
            }
            "import_spec_list" => {
                let mut inner = child.walk();
                for spec in child.named_children(&mut inner) {
                    if spec.kind() == "import_spec" {
                        record_import(spec, src, package_qname, module_import_prefix, acc);
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
    module_import_prefix: &str,
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

    // A7.6: remember which local name binds a DI container package. Blank and
    // dot imports bind no selector base, so they are skipped.
    if let Some((container, pkg_name)) = DiContainer::from_import_path(&path_str) {
        let local = alias.as_deref().unwrap_or(pkg_name);
        if local != "_" && local != "." {
            acc.di_containers.insert(local.to_string(), container);
        }
    }

    // LA.18d: remember the local name of every import outside the go.mod
    // module, so a func-literal handler's `pkg.Fn(..)` through it is not
    // mistaken for an in-repo callee. With no module prefix every import is
    // external. Blank and dot imports bind no selector base.
    let in_module = !module_import_prefix.is_empty()
        && (path_str == module_import_prefix
            || path_str
                .strip_prefix(module_import_prefix)
                .is_some_and(|rest| rest.starts_with('/')));
    if !in_module {
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

    // If the import lies within the go.mod module, convert to repo-local qname.
    let qname = if !module_import_prefix.is_empty() && path_str.starts_with(module_import_prefix) {
        let rel = path_str.trim_start_matches(module_import_prefix).trim_start_matches('/');
        if rel.is_empty() {
            // `import "github.com/foo/bar"` with module == "github.com/foo/bar" —
            // degenerate; ignore.
            return;
        }
        rel.replace('/', "::")
    } else {
        // External import (stdlib or third-party). Keep the raw path for now;
        // cross-repo resolution is a v0.4.4 concern.
        path_str.replace('/', "::")
    };

    acc.imports.push(ImportStmt {
        from_module: package_qname.to_string(),
        target: ImportTarget::Module {
            path: qname,
            alias,
        },
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

#[allow(clippy::too_many_arguments)]
fn collect_calls_in(
    node: TsNode,
    src: &[u8],
    from: NodeId,
    receiver_var: Option<&str>,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "call_expression" {
            if let Some(q) = classify_call(child, src, receiver_var) {
                acc.calls.push(CallSite {
                    from,
                    qualifier: q,
                });
            }
            // Pattern A: outbound client HTTP call (`http.Get('http://…/x')`) →
            // ENDPOINT node so HttpStackResolver can pair it with a server ROUTE.
            try_detect_go_endpoint(child, src, from, repo, file_rel, acc);
            // Data access: `db.Query("SELECT … FROM users")` → DATA_ENTITY node +
            // ACCESSES_DATA edge anchored to the *enclosing* fn (`from`), not the
            // module. The module-anchored edge is still emitted by the
            // cross-cutting data-entities extractor; this adds the fine-grained
            // fn→table attribution the DbResolver / call-site queries want.
            try_detect_go_data_access(child, src, from, repo, acc);
            // DI container registration: `wire.Build(NewA, NewB)` → INJECTS
            // from the injector (`from`) to each provider (A7.6).
            try_detect_go_provider_set(child, src, from, acc);
        }
        if child.kind() != "func_literal" {
            collect_calls_in(child, src, from, receiver_var, repo, file_rel, acc);
        }
    }
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
                _ => Some(CallQualifier::ComplexReceiver {
                    receiver: text_of(operand, src).to_string(),
                    name,
                }),
            }
        }
        _ => None,
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
// Data access (ACCESSES_DATA) — raw-SQL queries issued from a function body.
// ============================================================================
//
// The cross-cutting `data_entities` extractor already scans the whole file and
// mints one `DATA_ENTITY` node per table, anchoring an ACCESSES_DATA edge to the
// *module*. That loses which function issued the query. Here we walk each fn/
// method body (where the enclosing node id is known) and, for every string-
// literal call argument that carries a SQL statement, emit an ACCESSES_DATA edge
// from the enclosing fn to the table's DATA_ENTITY node.
//
// The DATA_ENTITY NodeId is built with the exact same qname shape the extractor
// uses (`data_entity:sql:<table>`) so the two collapse onto one node at graph
// build; only the edge anchor differs (fn vs module).
//
// Recognised shape: any call whose arguments contain a string literal with a
// SQL statement signature — `db.Query("SELECT … FROM users")`,
// `tx.ExecContext(ctx, "INSERT INTO orders …")`, `sqlx.Get(&u, `SELECT … `)`.
// Table names come from `FROM` / `JOIN` / `INTO` / `UPDATE` clauses.

/// Detect raw-SQL query calls and emit fn→table ACCESSES_DATA edges. No-op for
/// calls whose arguments hold no SQL-shaped string literal.
fn try_detect_go_data_access(
    call: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(args) = call.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        let Some(sql) = string_literal_text(arg, src) else {
            continue;
        };
        if !sql_has_context(&sql) {
            continue;
        }
        for table in scan_sql_tables(&sql) {
            emit_data_access(&table, from, repo, acc);
        }
    }
}

/// Emit (once per enclosing-fn × table) a DATA_ENTITY node + ACCESSES_DATA edge.
/// The node mirrors the data-entities extractor so ids collapse at graph build.
fn emit_data_access(table: &str, from: NodeId, repo: RepoId, acc: &mut Acc) {
    let qname = format!("data_entity:sql:{table}");
    let entity_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
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
    });
}

/// True when `s` contains an unambiguous SQL statement signature. Mirrors the
/// data-entities extractor's gate so a plain string that merely uses the word
/// `from`/`update` isn't mistaken for SQL.
fn sql_has_context(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    const SIG: &[&str] = &[
        "select ",
        "insert into",
        "delete from",
        "create table",
        "alter table",
        "truncate table",
        "merge into",
    ];
    if SIG.iter().any(|sig| lower.contains(sig)) {
        return true;
    }
    lower.contains("update ") && lower.contains(" set ")
}

/// Pull table names from `FROM`/`JOIN`/`INTO`/`UPDATE` clauses in a SQL string.
/// Case-insensitive on the keyword, identifier-shaped on the name; strips a
/// schema prefix (`public.users` → `users`) and rejects SQL keywords.
fn scan_sql_tables(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    for keyword in ["FROM", "JOIN", "INTO", "UPDATE"] {
        let kw = keyword.as_bytes();
        let mut i = 0;
        while i + kw.len() <= bytes.len() {
            let matches_kw = (0..kw.len()).all(|j| bytes[i + j].eq_ignore_ascii_case(&kw[j]));
            if !matches_kw {
                i += 1;
                continue;
            }
            let prev_ok = i == 0 || !is_sql_word_byte(bytes[i - 1]);
            let after = i + kw.len();
            let next_ok = after < bytes.len()
                && matches!(bytes[after], b' ' | b'\t' | b'\n' | b'\r');
            if !(prev_ok && next_ok) {
                i += 1;
                continue;
            }
            // Skip whitespace to the identifier.
            let mut k = after;
            while k < bytes.len() && matches!(bytes[k], b' ' | b'\t' | b'\n' | b'\r') {
                k += 1;
            }
            let start = k;
            while k < bytes.len() && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_' || bytes[k] == b'.') {
                k += 1;
            }
            if k > start {
                if let Some(name) = canonical_sql_table(&sql[start..k]) {
                    out.push(name);
                }
            }
            i = after;
        }
    }
    out
}

fn is_sql_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
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

    let qname = route_path_qname(&full_path);
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
    // duplicate entries.
    let seen = acc.route_methods_seen.entry(route_id).or_default();
    if seen.is_empty() {
        acc.nav
            .record(route_id, &full_path, &qname, node_kind::ROUTE, None);
    }
    seen.insert(method.to_string(), ());

    if let Some(q) = handler_qualifier {
        acc.refs.push(UnresolvedRef {
            from: route_id,
            from_module: module_id,
            qualifier: q,
            category: edge_category::HANDLED_BY,
        });
    }

    // LA.18d: a func-literal handler (`http.HandleFunc("/ws", func(w, r) {
    // serveWs(hub, w, r) })`) names no single target, so it has no display
    // name; what runs for the route is the literal's own direct in-repo
    // callees. Not the enclosing function (it registers every route), not the
    // module (it carries no CALLS). Expanded once per literal per file.
    if let Some(h) = handler_arg
        && h.kind() == "func_literal"
        && acc.func_literal_handlers.insert(h.start_byte())
    {
        for qualifier in func_literal_callees(h, src, &acc.external_pkgs) {
            acc.refs.push(UnresolvedRef {
                from: route_id,
                from_module: module_id,
                qualifier,
                category: edge_category::HANDLED_BY,
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
) -> Vec<CallQualifier> {
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
    out: &mut Vec<CallQualifier>,
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
            && !out.contains(&q)
        {
            out.push(q);
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
    if let Some(doc) = repo_graph_doc::leading_doc(&node, src) {
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
        payload: CellPayload::Json(repo_graph_doc::position_json(&node, file_rel)),
    }
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
    use repo_graph_core::EdgeCategoryId;

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
    // Data access (ACCESSES_DATA) — fn → DATA_ENTITY attribution
    // ========================================================================

    const DB_QUERY: &str = r#"package main

import "database/sql"

func getUsers(db *sql.DB) error {
    rows, err := db.Query("SELECT id, name FROM users WHERE active = true")
    if err != nil {
        return err
    }
    defer rows.Close()
    return nil
}
"#;

    #[test]
    fn accesses_data_edge_anchored_to_enclosing_function() {
        let parse = parse_file(DB_QUERY, "main.go", "main", "", repo()).unwrap();

        let get_users =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "main::getUsers");
        let users = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            "data_entity:sql:users",
        );

        // DATA_ENTITY node minted with the extractor-compatible qname.
        assert!(parse.nodes.iter().any(|n| n.id == users));

        // ACCESSES_DATA edge is anchored to getUsers, NOT the module.
        assert!(has_edge(&parse, get_users, users, edge_category::ACCESSES_DATA));

        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "main");
        assert!(
            !has_edge(&parse, module_id, users, edge_category::ACCESSES_DATA),
            "parser must not anchor ACCESSES_DATA to the module"
        );
    }

    #[test]
    fn accesses_data_dedupes_repeated_table_in_same_fn() {
        const SRC: &str = r#"package main

func touch(db *DB) {
    db.Query("SELECT * FROM users")
    db.Exec("INSERT INTO users (name) VALUES (?)")
}
"#;
        let parse = parse_file(SRC, "main.go", "main", "", repo()).unwrap();
        let touch = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "main::touch");
        let users = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::DATA_ENTITY,
            "data_entity:sql:users",
        );
        let edges = parse
            .edges
            .iter()
            .filter(|e| {
                e.from == touch && e.to == users && e.category == edge_category::ACCESSES_DATA
            })
            .count();
        assert_eq!(edges, 1, "one edge per (fn, table) despite two queries");
    }

    #[test]
    fn non_sql_string_arg_does_not_emit_data_access() {
        // A string that merely contains the word `from` is not SQL.
        const SRC: &str = r#"package main

func log(l *Logger) {
    l.Info("received request from client")
}
"#;
        let parse = parse_file(SRC, "main.go", "main", "", repo()).unwrap();
        assert!(
            !parse
                .edges
                .iter()
                .any(|e| e.category == edge_category::ACCESSES_DATA),
            "non-SQL string must not mint ACCESSES_DATA"
        );
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

    fn route_id(repo: RepoId, path: &str) -> NodeId {
        NodeId::from_parts(
            GRAPH_TYPE,
            repo,
            node_kind::ROUTE,
            &format!("route:{path}"),
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
    fn emits_route_node_per_path_with_method_cells() {
        let parse = parse_file(
            GIN_SIMPLE,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let health = route_id(repo(), "/health");
        let login = route_id(repo(), "/login");

        // Route nodes exist, one per path.
        assert!(parse.nodes.iter().any(|n| n.id == health));
        assert!(parse.nodes.iter().any(|n| n.id == login));

        // Each route has exactly one ROUTE_METHOD cell in this fixture.
        assert_eq!(route_methods(&parse, health), vec!["GET".to_string()]);
        assert_eq!(route_methods(&parse, login), vec!["POST".to_string()]);
    }

    /// LB.5 — an unprefixed relative literal gets the one canonical leading
    /// `/`: `r.GET("items", h)` and `e.GET("/parts", h)` alike are
    /// `route:/<path>`, the nav name is the canonical path, and the handler
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
        for q in ["route:/items", "route:/parts", "route:/api/users"] {
            assert!(qnames.contains(&q), "missing {q}: {qnames:?}");
        }
        assert!(!qnames.contains(&"route:items"), "{qnames:?}");
        let items = route_id(repo(), "/items");
        assert_eq!(
            parse.nav.name_by_id.get(&items).map(String::as_str),
            Some("/items")
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

        let health = route_id(repo(), "/health");
        let login = route_id(repo(), "/login");
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

        let health = route_id(repo(), "/api/health");
        let login = route_id(repo(), "/api/protected/login");

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

    #[test]
    fn same_path_two_methods_stack_cells_on_one_route_node() {
        let parse = parse_file(
            GIN_SAME_PATH_TWO_METHODS,
            "server/server.go",
            "server",
            "github.com/foo/bar",
            repo(),
        )
        .unwrap();

        let users = route_id(repo(), "/users");
        let occurrences = parse.nodes.iter().filter(|n| n.id == users).count();

        // Parser emits two Node structs with the same id (graph-build merges them).
        // Both should carry exactly one ROUTE_METHOD cell, for GET and POST.
        assert_eq!(occurrences, 2);
        let methods = route_methods(&parse, users);
        assert!(methods.contains(&"GET".to_string()));
        assert!(methods.contains(&"POST".to_string()));
        assert_eq!(methods.len(), 2);
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
        let show = route_id(repo(), "/users/:id");
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
            route_methods(&parse, route_id(repo(), "/health")),
            vec!["GET".to_string()],
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "/login")),
            vec!["POST".to_string()],
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "/users/:id")),
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
            route_methods(&parse, route_id(repo(), "/wildcard")),
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

        let health = route_id(repo(), "/health");
        let users = route_id(repo(), "/users");
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
    // per method, both stacking cells onto the shared path NodeId. The inner
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

        let users = route_id(repo(), "/users");
        let methods = route_methods(&parse, users);

        assert!(methods.contains(&"GET".to_string()));
        assert!(methods.contains(&"POST".to_string()));
        assert_eq!(methods.len(), 2, "expected exactly 2 methods, no ANY leak");
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
    //   cargo test -p repo-graph-parser-go -- --ignored eval --nocapture
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
            route_methods(&parse, route_id(repo(), "/legacy")),
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
        let route = route_id(repo(), "/users/:id");
        assert_eq!(route_qnames(&parse), vec!["route:/users/:id".to_string()]);
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
                "route:/health".to_string(),
                "route:/items/:id".to_string(),
                "route:/items/{id}".to_string(),
            ]
        );
        let del = route_id(repo(), "/items/:id");
        assert_eq!(route_methods(&parse, del), vec!["DELETE".to_string()]);
        assert_eq!(handled_by(&parse, del), vec![bare("deleteItem")]);
        let put = route_id(repo(), "/items/{id}");
        assert_eq!(route_methods(&parse, put), vec!["PUT".to_string()]);
        assert_eq!(handled_by(&parse, put), vec![attr("handlers", "PutItem")]);
        let health = route_id(repo(), "/health");
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
        let ping = route_id(repo(), "/ping");
        assert_eq!(route_qnames(&parse), vec!["route:/ping".to_string()]);
        assert_eq!(route_methods(&parse, ping), vec!["ANY".to_string()]);
        assert_eq!(handled_by(&parse, ping), vec![bare("anyPing")]);
    }

    #[test]
    fn match_emits_one_cell_per_listed_method() {
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
        assert_eq!(route_qnames(&parse), vec!["route:/orders".to_string()]);
        let orders = route_id(repo(), "/orders");
        assert_eq!(
            route_methods(&parse, orders),
            vec!["GET".to_string(), "POST".to_string()]
        );
        assert_eq!(
            handled_by(&parse, orders),
            vec![bare("matchOrders"), bare("matchOrders")]
        );
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
            vec!["route:/items".to_string(), "route:/items/{id}".to_string()]
        );
        let item = route_id(repo(), "/items/{id}");
        assert_eq!(route_methods(&parse, item), vec!["GET".to_string()]);
        assert_eq!(handled_by(&parse, item), vec![bare("getItem")]);
        let items = route_id(repo(), "/items");
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
        let users = route_id(repo(), "/users");
        assert_eq!(
            route_positions(&parse, users),
            vec![
                r#"{"file":"server.go","start_line":3,"end_line":3}"#.to_string(),
                r#"{"file":"server.go","start_line":4,"end_line":4}"#.to_string(),
            ]
        );
        // The Gorilla chain places both of its registrations at the inner
        // `HandleFunc` call.
        let legacy = route_id(repo(), "/legacy");
        assert_eq!(
            route_positions(&parse, legacy),
            vec![r#"{"file":"server.go","start_line":5,"end_line":5}"#.to_string(); 2]
        );
        // Each emitted copy is exactly [POSITION, ROUTE_METHOD]: POSITION
        // first, so a first-POSITION reader sees the registration.
        for n in parse.nodes.iter().filter(|n| n.id == users || n.id == legacy) {
            let kinds: Vec<_> = n.cells.iter().map(|c| c.kind).collect();
            assert_eq!(kinds, vec![cell_type::POSITION, cell_type::ROUTE_METHOD]);
        }
        // The ROUTE_METHOD payload keeps its 1-based line.
        let first = parse.nodes.iter().find(|n| n.id == users).unwrap();
        match &first.cells[1].payload {
            CellPayload::Json(j) => assert!(j.contains(r#""line":4,"#), "{j}"),
            other => panic!("ROUTE_METHOD is not JSON: {other:?}"),
        }
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
            vec!["route:/metrics".to_string(), "route:/static".to_string()]
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "/static")),
            vec!["ANY".to_string()]
        );
        assert_eq!(
            route_methods(&parse, route_id(repo(), "/metrics")),
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
        let ws = route_id(repo(), "/ws");
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
        assert_eq!(handled_by(&parse, route_id(repo(), "/x")), vec![bare("build")]);
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
            handled_by(&parse, route_id(repo(), "/health")),
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
            handled_by(&parse, route_id(repo(), "/items/{id}")),
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
            handled_by(&parse, route_id(repo(), "/users")),
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
        assert_eq!(handled_by(&parse, route_id(repo(), "/n")), vec![bare("serve")]);
    }

    #[test]
    fn func_literal_callees_are_capped_and_expanded_once_per_methods_chain() {
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
        let many = route_id(repo(), "/many");
        assert_eq!(
            route_methods(&parse, many),
            vec!["GET".to_string(), "POST".to_string()]
        );
        let expected: Vec<CallQualifier> = (1..=8).map(|i| bare(&format!("a{i}"))).collect();
        assert_eq!(handled_by(&parse, many), expected);
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
}
