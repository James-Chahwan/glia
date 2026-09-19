//! Kotlin parser (A14.2) on tree-sitter-kotlin-ng 1.1.
//!
//! Extracts the declarations of one `.kt` file — its MODULE, every class /
//! interface / enum / object (nested ones included), member and top-level
//! functions, the properties worth a node, and its imports — plus the
//! non-annotation routes ([`routes`]: Ktor DSL routes off the AST, each bound
//! HANDLED_BY to its enclosing function, and the WebFlux / Javalin text scans
//! ported from the Java parser) and the
//! Spring / Micronaut / JAX-RS / JPA annotation needles ([`spring`]: routes
//! with HANDLED_BY, stereotype and `@Inject` constructor / property INJECTS,
//! `@Entity` / `@Document` DATA_ENTITYs, repository ACCESSES_DATA), and the
//! call sites, supertypes and field types of [`calls`] (A14.3: every call in
//! a declared function's body, `: Base()` INHERITS_FROM / `: Iface`
//! IMPLEMENTS refs, primary-constructor and typed properties as field types),
//! and the client ENDPOINTs and Android components of [`android`] (A14.6:
//! Retrofit interface methods and Spring RestTemplate / WebClient calls as
//! ENDPOINT + CALLS, Android framework classes as a ROLE COMPONENT cell).
//! Parsers extract, the graph crate resolves: imports leave as
//! [`ImportStmt`]s for `build_dotted`'s dotted resolver, which the engine runs
//! over the Java and Kotlin parses of a repo as ONE graph (the JVM family), so
//! a Kotlin import binds a Java class and back.
//!
//! # Qnames (`::` separator)
//!
//! - MODULE: the engine's `module_qname` (`path_to_qname` of the file).
//! - Top-level type: the file's DIRECTORY scope plus the type name — the LB.2
//!   recipe the Java parser uses, so `svc.kt`'s `class UserService` is
//!   `UserService`, and `a/b/Foo.kt`'s `class Foo` is `a::b::Foo`, never
//!   `a::b::Foo::Foo`. Nested types hang off their outer type.
//! - Member function (METHOD) / property (ATTRIBUTE): `{type}::{name}`.
//!   `companion object` members are the enclosing type's statics and attach
//!   to it; the companion itself gets no node.
//! - Top-level function (FUNCTION) / property (STATE_VAR):
//!   `{module_qname}::{name}` — file-scoped, as the JVM sees it (a top-level
//!   `fun` compiles into the file's `<Stem>Kt` facade class), and so two
//!   files of one directory that both declare `fun main()` keep distinct ids.
//!
//! A second declaration with an already-emitted id (an overload) is not
//! re-emitted: one node, one DEFINES edge, the first declaration's cells.
//!
//! # Grammar traps (tree-sitter-kotlin-ng 1.1.0, measured)
//!
//! - There is NO `interface_declaration`. `interface Foo {}` (and
//!   `fun interface Foo {}`) is a `class_declaration` with an anonymous
//!   `interface` keyword child; an `enum class` is a `class_declaration` with
//!   an `enum_class_body` child; `object X` is an `object_declaration`.
//! - `class_declaration` / `object_declaration` / `function_declaration`
//!   carry only a `name` field. There is NO `body` field: the body is the
//!   `class_body` / `enum_class_body` / `function_body` named child.
//! - An extension function (`fun String.slugify()`) has its receiver
//!   `user_type` BEFORE the name; `child_by_field_name("name")` is still the
//!   function's own name.
//! - Imports are `import` nodes (not `import_declaration`) holding a
//!   `qualified_identifier`, then an anonymous `.` `*` for a star import, or
//!   `as` + `identifier` for an alias.
//! - `annotation` and `call_expression` have no fields at all.
//! - `true` / `false` / `null` are plain `identifier`s.
//! - `has_error` is set on valid code: a single-line `object X { fun go() {} }`
//!   sets it, the multi-line form does not. Never bail on it; error recovery
//!   keeps the surrounding tree, and an `ERROR` node is walked through as a
//!   transparent container so the declarations inside it still count.

mod android;
mod calls;
mod routes;
mod spring;

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{CallSite, UnresolvedRef};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use repo_graph_code_domain::{
    CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError, cell_type,
    edge_category, node_kind,
};

/// Parse one Kotlin file. Same signature as the Java parser's `parse_file`,
/// which `.kt` files reached until A14.2's routing flip.
pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let parsed = parse_all(source, file_rel_path, module_qname, repo)?;
    // A14.4 / A14.5 / A14.6: what the Spring, Ktor and client passes did, for
    // the `[kotlin/spring]`, `[kotlin/ktor]` and `[kotlin/retrofit]` lines.
    spring::publish(parsed.spring);
    routes::publish(parsed.ktor);
    android::publish(parsed.clients);
    Ok(parsed.fp)
}

/// One file's parse and the per-pass counts its markers publish.
struct Parsed {
    fp: FileParse,
    spring: spring::SpringCounts,
    ktor: routes::KtorCounts,
    clients: android::ClientCounts,
}

/// [`parse_file`] plus the file's [`spring::SpringCounts`] and
/// [`routes::KtorCounts`], unpublished — so tests read one file's counts
/// without the process-global banks.
#[cfg(test)]
fn parse_counting(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<(FileParse, spring::SpringCounts, routes::KtorCounts), ParseError> {
    parse_all(source, file_rel_path, module_qname, repo).map(|p| (p.fp, p.spring, p.ktor))
}

/// The parse behind [`parse_file`], every count unpublished.
fn parse_all(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<Parsed, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
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
        cells: cells_of(&root, src, file_rel_path, false),
    });
    let module_simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    acc.nav
        .record(module_id, module_simple, module_qname, node_kind::MODULE, None);

    let file = File {
        src,
        rel: file_rel_path,
        repo,
        module_qname,
        module_id,
    };
    let no_members = HashSet::new();
    let top = Owner {
        id: module_id,
        qname: module_qname,
        in_type: false,
        is_interface: false,
        route_prefix: "",
        is_bean: false,
        members: &no_members,
    };
    walk_members(root, &file, top, &mut acc);
    // After the walk: `fn_ids` names every declared function a Ktor route can
    // bind, and an annotation route already in `routes_seen` is not re-emitted
    // by a Ktor or text-scan route with the same `METHOD path`.
    routes::scan_ktor(root, source, &file, &mut acc);
    routes::scan_text_routes(source, repo, &mut acc);

    let fp = FileParse {
        nodes: acc.nodes,
        edges: acc.edges,
        imports: acc.imports,
        calls: acc.calls,
        refs: acc.refs,
        nav: acc.nav,
        properties: acc.properties,
    };
    Ok(Parsed {
        fp,
        spring: acc.spring,
        ktor: acc.ktor,
        clients: acc.clients,
    })
}

/// A14.2 fired_on, once per repo that holds Kotlin, counted off the parses so
/// cache-served files count too:
///   `[kotlin] entities: N file(s) types=T fns=F props=P imports=I routes=R repo=<label>`
/// `glia analyze <repo> 2>&1 | grep '\[kotlin\] entities:'`. Later Kotlin
/// packets add their own lines HERE, so the engine's call site never moves.
///
/// A14.4 adds the Spring line, from the detectors that ran in this process
/// since the last repo's line (a cache-served file ran none — see [`spring`]):
///   `[kotlin/spring] stereotypes=S routes=R composed=C injects=J entities=E repos=P repo=<label>`
/// `glia analyze <repo> 2>&1 | grep '\[kotlin/spring\]'`.
///
/// A14.3 adds the refs line, counted off the parses like the entities line
/// (so cache-served files count too):
///   `[kotlin] refs: calls=C self=S inherits=A implements=B field_types=F repo=<label>`
/// `glia analyze <repo> 2>&1 | grep '\[kotlin\] refs:'` — `C` CallSites, `S` of
/// them receiver-less member calls (`SelfMethod`), `A` / `B` INHERITS_FROM /
/// IMPLEMENTS refs, `F` recorded property types (see [`calls`]).
///
/// A14.5 adds the Ktor line, from the same kind of process-global bank:
///   `[kotlin/ktor] ast routes=R handled_by=H (text_scan_would_find=N) repo=<label>`
/// `glia analyze <repo> 2>&1 | grep '\[kotlin/ktor\]'` — `N` is the retired
/// Ktor text scan's count over the same files (see [`routes`]).
///
/// A14.6 adds the client line, from the same kind of process-global bank:
///   `[kotlin/retrofit] endpoints=E components=C spring_clients=K repo=<label>`
/// `glia analyze <repo> 2>&1 | grep '\[kotlin/retrofit\]'` — `E` Retrofit
/// ENDPOINTs, `C` Android component classes, `K` Spring RestTemplate /
/// WebClient ENDPOINTs (see [`android`]).
pub fn trace(parses: &[FileParse], repo_label: &str) {
    let (mut types, mut fns, mut props, mut routes) = (0usize, 0usize, 0usize, 0usize);
    for fp in parses {
        for kind in fp.nav.kind_by_id.values() {
            match *kind {
                k if k == node_kind::CLASS || k == node_kind::INTERFACE || k == node_kind::ENUM => {
                    types += 1;
                }
                k if k == node_kind::METHOD || k == node_kind::FUNCTION => fns += 1,
                k if k == node_kind::ATTRIBUTE || k == node_kind::STATE_VAR => props += 1,
                k if k == node_kind::ROUTE => routes += 1,
                _ => {}
            }
        }
    }
    let imports: usize = parses.iter().map(|fp| fp.imports.len()).sum();
    eprintln!(
        "[kotlin] entities: {} file(s) types={types} fns={fns} props={props} imports={imports} routes={routes} repo={repo_label}",
        parses.len()
    );
    eprintln!("{}", calls::marker(parses, repo_label));
    eprintln!("{}", spring::marker(spring::take(), repo_label));
    eprintln!("{}", routes::marker(routes::take(), repo_label));
    eprintln!("{}", android::marker(android::take(), repo_label));
}

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    nav: CodeNav,
    /// Property-style reads (`obj.x`, never `obj.x()`): properties with a getter.
    properties: HashSet<NodeId>,
    /// Declarations emitted so far: an overload reuses its first node.
    declared: HashSet<NodeId>,
    /// `METHOD path` keys of the ROUTEs emitted so far — the annotation
    /// routes ([`spring`]) first, then the Ktor AST scan, then the text scans.
    routes_seen: HashSet<String>,
    /// Every declared FUNCTION / METHOD, keyed on its `function_declaration`'s
    /// tree-sitter node id: the handler a Ktor route inside it binds ([`routes`]).
    fn_ids: HashMap<usize, NodeId>,
    /// INJECTS refs ([`spring`]) and INHERITS_FROM / IMPLEMENTS refs
    /// ([`calls`]); `resolve_refs` binds each `Bare(TypeName)`.
    refs: Vec<UnresolvedRef>,
    /// The call sites of every declared function's body ([`calls`]).
    calls: Vec<CallSite>,
    /// What the [`spring`] detectors did in this file.
    spring: spring::SpringCounts,
    /// What the Ktor pass ([`routes`]) did in this file.
    ktor: routes::KtorCounts,
    /// Dedups client ENDPOINT nodes across the file ([`android`]).
    seen_endpoints: HashSet<NodeId>,
    /// What the client / component detectors ([`android`]) did in this file.
    clients: android::ClientCounts,
}

/// Per-file constants every visitor needs.
struct File<'a> {
    src: &'a [u8],
    rel: &'a str,
    repo: RepoId,
    module_qname: &'a str,
    /// The file's MODULE node: an `UnresolvedRef`'s `from_module`.
    module_id: NodeId,
}

/// The declaration a member hangs off: the file MODULE at the top level, a
/// type inside a class / object / enum body (a companion's members included).
#[derive(Clone, Copy)]
struct Owner<'a> {
    id: NodeId,
    qname: &'a str,
    in_type: bool,
    /// The type is an INTERFACE: its body-less methods may be Retrofit
    /// mappings (A14.6).
    is_interface: bool,
    /// The type's route prefix its action methods compose onto (A14.4).
    route_prefix: &'a str,
    /// The type is a Spring stereotype: its constructors inject (A14.4).
    is_bean: bool,
    /// The methods the type's body declares (companion members included):
    /// a receiver-less call to one is a self call (A14.3). Empty at the top
    /// level.
    members: &'a HashSet<String>,
}

/// Visit every declaration directly inside `container` (the `source_file`, a
/// `class_body` / `enum_class_body`, or an `ERROR` node recovered in either).
/// Function bodies are never entered: local declarations are not members.
fn walk_members(container: TsNode, file: &File, owner: Owner, acc: &mut Acc) {
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        match child.kind() {
            "import" if !owner.in_type => collect_import(child, file, acc),
            "class_declaration" | "object_declaration" => visit_type(child, file, owner, acc),
            // The companion's members are the enclosing type's statics.
            "companion_object" => {
                if let Some(body) = named_child_of_kind(child, &["class_body"]) {
                    walk_members(body, file, owner, acc);
                }
            }
            "function_declaration" => visit_function(child, file, owner, acc),
            "property_declaration" => visit_property(child, file, owner, acc),
            "secondary_constructor" if owner.in_type => {
                spring::on_secondary_ctor(child, owner.id, owner.is_bean, file, acc);
            }
            "ERROR" => walk_members(child, file, owner, acc),
            _ => {}
        }
    }
}

fn visit_type(node: TsNode, file: &File, owner: Owner, acc: &mut Acc) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, file.src);
    let body = named_child_of_kind(node, &["class_body", "enum_class_body"]);
    let kind = if has_keyword(node, "interface") {
        node_kind::INTERFACE
    } else if body.is_some_and(|b| b.kind() == "enum_class_body") {
        node_kind::ENUM
    } else {
        // `class`, `data` / `sealed` / `annotation` classes, and `object`:
        // a singleton IS a class.
        node_kind::CLASS
    };
    let qname = if owner.in_type {
        format!("{}::{name}", owner.qname)
    } else {
        scoped(type_scope(file.module_qname), name)
    };
    let id = declare(node, name, &qname, kind, owner.id, file, acc);
    android::on_type(node, kind, id, file, acc);
    let spring = spring::on_type(node, name, id, file, acc);
    calls::heritage(node, id, kind == node_kind::INTERFACE, file, acc);
    calls::record_ctor_field_types(node, id, file, acc);
    if let Some(body) = body {
        let members = calls::member_fn_names(body, file.src);
        let inner = Owner {
            id,
            qname: &qname,
            in_type: true,
            is_interface: kind == node_kind::INTERFACE,
            route_prefix: &spring.prefix,
            is_bean: spring.is_bean,
            members: &members,
        };
        walk_members(body, file, inner, acc);
    }
}

fn visit_function(node: TsNode, file: &File, owner: Owner, acc: &mut Acc) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_of(name_node, file.src);
    let kind = if owner.in_type {
        node_kind::METHOD
    } else {
        node_kind::FUNCTION
    };
    let qname = format!("{}::{name}", owner.qname);
    let id = declare(node, name, &qname, kind, owner.id, file, acc);
    acc.fn_ids.insert(node.id(), id);
    // A Retrofit interface method maps a request it SENDS: ENDPOINTs, and
    // never the ROUTE its `@GET` would read as under JAX-RS.
    let client_mapping = owner.is_interface && android::on_interface_fn(node, id, file, acc);
    if owner.in_type && !client_mapping {
        spring::on_method(node, id, owner.route_prefix, file, acc);
    }
    let scope = calls::CallScope {
        members: owner.members,
        type_name: owner
            .in_type
            .then(|| owner.qname.rsplit("::").next().unwrap_or(owner.qname)),
    };
    calls::collect(node, id, scope, file, acc);
}

/// A property gets a node when it carries meaning beyond a literal: `const`,
/// documented, or initialised by anything but a plain literal (a call, a
/// delegate, a getter — or nothing, for `lateinit` / abstract properties).
/// `var plain = 3` stays quiet, like the Java parser's literal-constant gate.
fn visit_property(node: TsNode, file: &File, owner: Owner, acc: &mut Acc) {
    // Field injection hangs off the type whether or not the property earns a
    // node below (`@Resource var m: Mailer? = null` does not).
    if owner.in_type {
        spring::on_property(node, owner.id, file, acc);
        calls::record_property_type(node, owner.id, file, acc);
    }
    // `val (a, b) = pair` destructures into locals-to-be; no single name.
    let Some(var) = named_child_of_kind(node, &["variable_declaration"]) else {
        return;
    };
    let Some(name_node) = named_child_of_kind(var, &["identifier"]) else {
        return;
    };
    let name = text_of(name_node, file.src);
    let is_const = named_child_of_kind(node, &["modifiers"]).is_some_and(|m| {
        let mut c = m.walk();
        m.named_children(&mut c)
            .any(|k| k.kind() == "property_modifier" && text_of(k, file.src) == "const")
    });
    let getter = named_child_of_kind(node, &["getter"]).is_some();
    let literal_init = !getter
        && named_child_of_kind(node, &["property_delegate"]).is_none()
        && initializer(node).is_some_and(|e| is_plain_literal(e, file.src));
    if literal_init && !is_const && repo_graph_doc::leading_doc(&node, file.src).is_none() {
        return;
    }
    let kind = if owner.in_type {
        node_kind::ATTRIBUTE
    } else {
        node_kind::STATE_VAR
    };
    let qname = format!("{}::{name}", owner.qname);
    let id = declare(node, name, &qname, kind, owner.id, file, acc);
    if getter {
        acc.properties.insert(id);
    }
}

/// Emit one declaration: its node (CODE / POSITION / DOC cells), the DEFINES
/// edge from `parent`, and its nav entry. An id already declared in this file
/// (an overload) is returned as-is and not re-emitted.
fn declare(
    node: TsNode,
    name: &str,
    qname: &str,
    kind: repo_graph_core::NodeKindId,
    parent: NodeId,
    file: &File,
    acc: &mut Acc,
) -> NodeId {
    let id = NodeId::from_parts(GRAPH_TYPE, file.repo, kind, qname);
    if acc.declared.insert(id) {
        acc.nodes.push(Node {
            id,
            repo: file.repo,
            confidence: Confidence::Strong,
            cells: cells_of(&node, file.src, file.rel, true),
        });
        acc.edges.push(Edge {
            from: parent,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
        acc.nav.record(id, name, qname, kind, Some(parent));
    }
    id
}

/// `import a.b.C` -> `Symbol { a::b, C }`, `import a.b.C as D` -> the same
/// with alias `D`, `import a.b.*` -> `Module { a::b }` — the Java parser's
/// shape, so `build_dotted`'s resolver binds both languages alike. A Kotlin
/// import always names a declaration (a class, function, property or object
/// member), never a bare package, so every dotted non-star import is a Symbol.
fn collect_import(node: TsNode, file: &File, acc: &mut Acc) {
    let mut cursor = node.walk();
    let children: Vec<TsNode> = node.children(&mut cursor).collect();
    let Some(path_node) = children.iter().find(|c| c.is_named()) else {
        return;
    };
    let path: String = text_of(*path_node, file.src)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if path.is_empty() {
        return;
    }
    let star = children.iter().any(|c| !c.is_named() && c.kind() == "*");
    let alias = children
        .iter()
        .position(|c| !c.is_named() && c.kind() == "as")
        .and_then(|i| children[i + 1..].iter().find(|c| c.is_named()))
        .map(|c| text_of(*c, file.src).to_string());
    let target = if star {
        ImportTarget::Module {
            path: path.replace('.', "::"),
            alias: None,
        }
    } else if let Some((module, name)) = path.rsplit_once('.') {
        ImportTarget::Symbol {
            module: module.replace('.', "::"),
            name: name.to_string(),
            alias,
            level: 0,
        }
    } else {
        ImportTarget::Module { path, alias }
    };
    acc.imports.push(ImportStmt {
        from_module: file.module_qname.to_string(),
        target,
    });
}

/// The initializer expression of a `property_declaration`: the named child
/// that is none of its declaration parts.
fn initializer(node: TsNode) -> Option<TsNode> {
    const PARTS: &[&str] = &[
        "modifiers",
        "variable_declaration",
        "multi_variable_declaration",
        "getter",
        "setter",
        "property_delegate",
        "type_parameters",
        "type_constraints",
        "type_modifiers",
        "user_type",
        "nullable_type",
        "parenthesized_type",
        "line_comment",
        "block_comment",
    ];
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|c| !PARTS.contains(&c.kind()))
}

/// A literal with nothing to resolve: a number, a character, a string with no
/// `$` template, `true` / `false` / `null` (plain identifiers in this
/// grammar), or a negated number.
fn is_plain_literal(expr: TsNode, src: &[u8]) -> bool {
    match expr.kind() {
        "number_literal" | "float_literal" | "character_literal" => true,
        "string_literal" | "multiline_string_literal" => !text_of(expr, src).contains('$'),
        "identifier" => matches!(text_of(expr, src), "true" | "false" | "null"),
        "unary_expression" => {
            let mut cursor = expr.walk();
            let operands: Vec<TsNode> = expr.named_children(&mut cursor).collect();
            matches!(operands.as_slice(), [n] if matches!(n.kind(), "number_literal" | "float_literal"))
        }
        _ => false,
    }
}

/// Whether `node` has the anonymous keyword child `kw` (`interface`).
fn has_keyword(node: TsNode, kw: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|c| !c.is_named() && c.kind() == kw)
}

fn named_child_of_kind<'a>(node: TsNode<'a>, kinds: &[&str]) -> Option<TsNode<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|c| kinds.contains(&c.kind()))
}

/// LB.2: a top-level type belongs to its package (the directory), not its
/// file: the module qname minus its file-stem segment, `""` at the repo root.
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

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// CODE + POSITION (canonical 0-indexed JSON via `repo_graph_doc`), plus DOC
/// for a declaration that carries a leading doc comment.
fn cells_of(node: &TsNode, src: &[u8], file_rel: &str, with_doc: bool) -> Vec<Cell> {
    let mut cells = vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(repo_graph_doc::position_json(node, file_rel)),
        },
    ];
    if with_doc && let Some(doc) = repo_graph_doc::leading_doc(node, src) {
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

    /// Every qname the parse recorded for `kind`, sorted.
    fn qnames_of(fp: &FileParse, kind: repo_graph_core::NodeKindId) -> Vec<&str> {
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

    fn parent_qname<'a>(fp: &'a FileParse, qname: &str) -> Option<&'a str> {
        let (id, _) = fp.nav.qname_by_id.iter().find(|(_, q)| *q == qname)?;
        let parent = fp.nav.parent_of.get(id)?;
        fp.nav.qname_by_id.get(parent).map(String::as_str)
    }

    #[test]
    fn classes_objects_and_interfaces() {
        let source = r#"
package com.acme.api

interface Auditable {
    fun audit(): String
}

fun interface Mapper {
    fun map(x: Int): Int
}

enum class Status {
    ACTIVE,
    GONE;

    fun label(): String = name
}

data class UserDto(val id: Long)

sealed class Shape {
    class Circle : Shape()
}

object Registry {
    fun all(): List<String> = listOf()
}
"#;
        let fp = parse_file(source, "src/api/Types.kt", "src::api::Types", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::INTERFACE),
            vec!["src::api::Auditable", "src::api::Mapper"]
        );
        assert_eq!(qnames_of(&fp, node_kind::ENUM), vec!["src::api::Status"]);
        assert_eq!(
            qnames_of(&fp, node_kind::CLASS),
            vec![
                "src::api::Registry",
                "src::api::Shape",
                "src::api::Shape::Circle",
                "src::api::UserDto",
            ]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec![
                "src::api::Auditable::audit",
                "src::api::Mapper::map",
                "src::api::Registry::all",
                "src::api::Status::label",
            ]
        );
        // Each type is DEFINED by its parent: the module, or its outer type.
        assert_eq!(parent_qname(&fp, "src::api::Auditable"), Some("src::api::Types"));
        assert_eq!(parent_qname(&fp, "src::api::Shape::Circle"), Some("src::api::Shape"));
        let defines = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::DEFINES)
            .count();
        // 7 types + 4 methods, one DEFINES each.
        assert_eq!(defines, 11);
    }

    #[test]
    fn top_level_types_hang_off_the_directory_scope() {
        // LB.2: a top-level type never doubles its file stem; a repo-root file
        // puts its types at a bare qname. Top-level funs stay file-scoped.
        let source = "class Foo {\n    fun run() {}\n}\n\nfun helper() = 1\n";
        let fp = parse_file(source, "a/b/Foo.kt", "a::b::Foo", repo()).unwrap();
        assert_eq!(qnames_of(&fp, node_kind::CLASS), vec!["a::b::Foo"]);
        assert_eq!(qnames_of(&fp, node_kind::METHOD), vec!["a::b::Foo::run"]);
        assert_eq!(qnames_of(&fp, node_kind::FUNCTION), vec!["a::b::Foo::helper"]);
        let root = parse_file(source, "svc.kt", "svc", repo()).unwrap();
        assert_eq!(qnames_of(&root, node_kind::CLASS), vec!["Foo"]);
        assert_eq!(qnames_of(&root, node_kind::FUNCTION), vec!["svc::helper"]);
    }

    #[test]
    fn top_level_fun_is_function_member_fun_is_method() {
        let source = r#"
fun topLevelHelper(n: Int): Int = n * 2

fun String.slugify(): String = this.lowercase()

class UserService {
    fun findById(id: Long): String {
        return "x"
    }

    fun save(dto: String): String = dto

    fun save(dto: Int): String = dto.toString()
}
"#;
        let fp = parse_file(source, "svc.kt", "svc", repo()).unwrap();
        assert_eq!(
            qnames_of(&fp, node_kind::FUNCTION),
            vec!["svc::slugify", "svc::topLevelHelper"]
        );
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec!["UserService::findById", "UserService::save"]
        );
        // The overload is one node with one DEFINES edge.
        let save = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "UserService::save");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == save).count(), 1);
        assert_eq!(fp.edges.iter().filter(|e| e.to == save).count(), 1);
        assert_eq!(parent_qname(&fp, "svc::topLevelHelper"), Some("svc"));
        assert_eq!(parent_qname(&fp, "UserService::findById"), Some("UserService"));
    }

    #[test]
    fn companion_members_attach_to_outer_class() {
        let source = r#"
class Foo(val x: Int) {
    companion object Factory {
        const val MAX = 3

        fun create(): Foo = Foo(1)
    }

    fun own() {}
}
"#;
        let fp = parse_file(source, "Foo.kt", "Foo", repo()).unwrap();
        assert_eq!(qnames_of(&fp, node_kind::CLASS), vec!["Foo"], "no companion node");
        assert_eq!(qnames_of(&fp, node_kind::METHOD), vec!["Foo::create", "Foo::own"]);
        assert_eq!(qnames_of(&fp, node_kind::ATTRIBUTE), vec!["Foo::MAX"]);
        assert_eq!(parent_qname(&fp, "Foo::create"), Some("Foo"));
        assert_eq!(parent_qname(&fp, "Foo::MAX"), Some("Foo"));
    }

    #[test]
    fn properties_are_gated_and_getters_are_property_reads() {
        let source = r#"
const val TOP = "x"
val plainTop = 3
val derived = listOf(1)

class Box(val size: Int) {
    var plain = 3
    val flag = true
    val negative = -1

    /** The doubled size. */
    val documented = 4

    val computed: Int
        get() = size * 2

    val lazyValue by lazy { size }

    lateinit var service: String

    private val cache = mutableMapOf<Long, String>()

    val template = "$size/x"
}
"#;
        let fp = parse_file(source, "box.kt", "box", repo()).unwrap();
        assert_eq!(qnames_of(&fp, node_kind::STATE_VAR), vec!["box::TOP", "box::derived"]);
        assert_eq!(
            qnames_of(&fp, node_kind::ATTRIBUTE),
            vec![
                "Box::cache",
                "Box::computed",
                "Box::documented",
                "Box::lazyValue",
                "Box::service",
                "Box::template",
            ]
        );
        let computed = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "Box::computed");
        assert_eq!(fp.properties, HashSet::from([computed]));
        let documented =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ATTRIBUTE, "Box::documented");
        let doc = fp
            .nodes
            .iter()
            .find(|n| n.id == documented)
            .and_then(|n| n.cells.iter().find(|c| c.kind == cell_type::DOC));
        assert!(
            matches!(doc, Some(Cell { payload: CellPayload::Text(t), .. }) if t.contains("doubled size")),
            "{doc:?}"
        );
    }

    #[test]
    fn imports_symbol_and_star_and_alias() {
        let source = r#"
package com.acme.api

import com.acme.service.UserService
import com.acme.util.*
import com.acme.model.User as U
import com.acme.util.slugify
"#;
        let fp = parse_file(source, "api.kt", "api", repo()).unwrap();
        let targets: Vec<&ImportTarget> = fp.imports.iter().map(|i| &i.target).collect();
        assert_eq!(
            targets,
            vec![
                &ImportTarget::Symbol {
                    module: "com::acme::service".into(),
                    name: "UserService".into(),
                    alias: None,
                    level: 0,
                },
                &ImportTarget::Module {
                    path: "com::acme::util".into(),
                    alias: None,
                },
                &ImportTarget::Symbol {
                    module: "com::acme::model".into(),
                    name: "User".into(),
                    alias: Some("U".into()),
                    level: 0,
                },
                &ImportTarget::Symbol {
                    module: "com::acme::util".into(),
                    name: "slugify".into(),
                    alias: None,
                    level: 0,
                },
            ]
        );
        assert!(fp.imports.iter().all(|i| i.from_module == "api"));
    }

    fn route_names(fp: &FileParse) -> Vec<&str> {
        let mut routes: Vec<&str> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .filter_map(|(id, _)| fp.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        routes.sort_unstable();
        routes
    }

    #[test]
    fn javalin_and_webflux_routes_ported() {
        let source = r#"
fun main() {
    val app = Javalin.create()
    app.get("/users", UserHandler::list)
    app.post("/users", UserHandler::create)
    val cached = cache.get("/key")
}

fun routes(h: OrderHandler) = RouterFunctions.route()
    .GET("/orders", h::list)
    .POST("/orders", h::create)
    .build()

fun forget() {
    forget("/nope") { }
}
"#;
        let fp = parse_file(source, "routes.kt", "routes", repo()).unwrap();
        assert_eq!(
            route_names(&fp),
            vec!["GET /orders", "GET /users", "POST /orders", "POST /users"]
        );
    }

    #[test]
    fn has_error_source_still_yields_declarations() {
        // An unclosed primary constructor and a single-line object both set
        // has_error; the declarations around them still come through.
        let source = r#"
class Broken(val x: Int {
    fun a() {}
}

object Single { fun go() {} }

class Fine {
    fun b() = 1
}

fun Application.itemRoutes() {
    routing {
        get("/api/items") { call.respond(store.all()) }
    }
}
"#;
        let fp = parse_file(source, "broken.kt", "broken", repo()).unwrap();
        assert_eq!(qnames_of(&fp, node_kind::CLASS), vec!["Broken", "Fine", "Single"]);
        assert_eq!(
            qnames_of(&fp, node_kind::METHOD),
            vec!["Broken::a", "Fine::b", "Single::go"]
        );
        assert_eq!(qnames_of(&fp, node_kind::FUNCTION), vec!["broken::itemRoutes"]);
        assert_eq!(route_names(&fp), vec!["GET /api/items"]);
        // The AST route scan survives the recovered tree: bound to its fun.
        let route = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "GET /api/items");
        let handler =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "broken::itemRoutes");
        assert!(fp.edges.iter().any(|e| e.category == edge_category::HANDLED_BY
            && e.from == route
            && e.to == handler));
    }

    #[test]
    fn module_node_and_positions() {
        let source = "package x\n\nclass A {\n    fun a() {}\n}\n";
        let fp = parse_file(source, "src/A.kt", "src::A", repo()).unwrap();
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "src::A");
        assert_eq!(fp.nodes[0].id, module);
        assert_eq!(fp.nav.name_by_id.get(&module).map(String::as_str), Some("A"));
        let method = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "src::A::a");
        let pos = fp
            .nodes
            .iter()
            .find(|n| n.id == method)
            .and_then(|n| n.cells.iter().find(|c| c.kind == cell_type::POSITION))
            .map(|c| c.payload.clone());
        let Some(CellPayload::Json(pos)) = pos else {
            panic!("no POSITION: {pos:?}");
        };
        let v: serde_json::Value = serde_json::from_str(&pos).unwrap();
        assert_eq!(v["file"], "src/A.kt");
        assert_eq!(v["start_line"], 3, "0-indexed");
    }
}
