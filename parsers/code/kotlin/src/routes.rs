//! Kotlin HTTP routes that are not annotations (those are [`crate::spring`]).
//!
//! - **Ktor routing DSL (A14.5), off the AST.** `get("/items") { … }` is a
//!   ROUTE bound HANDLED_BY to the declared function it sits in, carrying a
//!   POSITION cell at the call, with nested `route("/api") { … }` blocks
//!   composed onto its path.
//! - **WebFlux functional** (`.GET("/path", h::list)`) and **Javalin**
//!   (`app.get("/path", handler)`): pure-text scans ported from the Java
//!   parser by A14.2 — same needles, same filters, same cell-less ROUTE with
//!   no handler, so their NodeIds did not move with the `.kt` routing flip.
//!
//! One `METHOD path` key set (`Acc::routes_seen`) is shared with the Spring
//! annotation routes and across all three scans, so a path two of them match
//! is one node.
//!
//! # Ktor: the AST shape (tree-sitter-kotlin-ng 1.1.0, measured)
//!
//! ```text
//! get("/api/items") { call.respond(…) }      get { … }            (no path)
//!
//! call_expression                            call_expression
//!   call_expression                            identifier "get"
//!     identifier "get"                         annotated_lambda
//!     value_arguments
//!       value_argument
//!         string_literal ─ '"' string_content '"'
//!   annotated_lambda
//!     lambda_literal                <- the handler body
//! ```
//!
//! `route("/admin") { … }` has the first shape with `identifier "route"`;
//! `routing { … }` / `authenticate("jwt") { … }` are calls with a lambda and
//! no route meaning — the walk passes through them. A receiver call
//! (`map.get("/key")`, `this.get("/x") { }`) has a `navigation_expression`
//! where the `identifier` sits, so it is structurally not a route; neither is
//! a call with no trailing lambda, a named first argument
//! (`get(path = "/x") { }`) or a triple-quoted path — none of which the text
//! scan accepted either.
//!
//! # Names (the `code_domain::endpoint` recipe)
//!
//! `endpoint::route_qname(METHOD, path)`. At the top level of a routing tree
//! the path is the literal as written and must start with `/` (the text
//! scan's guard, kept: `get("count") { }` is no route), so every top-level
//! route keeps its byte-identical 0.4.18 name and NodeId. Inside
//! `route("p") { … }` blocks the path is `endpoint::compose_route_path` of
//! the joined prefixes and the verb's own template, which may be relative
//! (`route("/users") { get("{id}") { } }` is `GET /users/{id}`), and a verb
//! with no path (`get { }`) maps the block's own path. That composition is
//! the 0.5.0 break: nested routes were flat before (`DELETE /users/{id}`
//! under `route("/admin")`, now `DELETE /admin/users/{id}`), and a flat name
//! could never pair with a client. Prefixes do not cross a function
//! boundary: a `fun Route.users()` installed under `route("/api")` elsewhere
//! is named from its own blocks only.
//!
//! # Ktor: the handler
//!
//! A Ktor handler is an anonymous lambda, so the ROUTE is bound (HANDLED_BY,
//! Confidence::Medium — structural, not a declared handler reference) to the
//! nearest ENCLOSING DECLARED function: the FUNCTION / METHOD node whose body
//! holds the call (`Acc::fn_ids`, filled by `visit_function`). A local `fun`
//! inside it has no node and so inherits it. A route outside every declared
//! function (a top-level `val module = { routing { … } }`) binds the file's
//! MODULE, so no route is orphaned. One HANDLED_BY per distinct
//! (route, handler); a route declared twice is one node, first POSITION wins.
//!
//! Calls: [`ktor_call`] is the one classifier of a Ktor DSL call, so the
//! Kotlin call collector (A14.3) can skip exactly the call_expressions that
//! became ROUTEs (and their `route` / verb heads) instead of minting a
//! CallSite to a phantom `get`.
//!
//! # fired_on
//!
//! `[kotlin/ktor] ast routes=R handled_by=H (text_scan_would_find=N) repo=<label>`,
//! once per repo holding Kotlin, from [`crate::trace`]:
//! `glia analyze <repo> 2>&1 | grep '\[kotlin/ktor\]'`. `N` is how many
//! distinct `METHOD path` keys the retired Ktor text scan finds in the same
//! files — it emits nothing any more and runs only for this count, so an AST
//! miss against the old scanner stays greppable (`N > R`; nested composition
//! and relative paths under a prefix make `R > N` legitimately). Like the
//! `[kotlin/spring]` line, the counts are process-global and cover the files
//! THIS process parsed: a file served from the parse cache is not counted.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use glia_code_domain::endpoint;
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::Node as TsNode;

use crate::{
    Acc, File, GRAPH_TYPE, cell_type, edge_category, named_child_of_kind, node_kind, text_of,
};

// ---------------------------------------------------------------------------
// Ktor (AST)
// ---------------------------------------------------------------------------

/// What a Ktor DSL call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KtorKind {
    /// `get` / `post` / … — mints a ROUTE.
    Verb(&'static str),
    /// `route("p") { … }` — a prefix for the routes inside its lambda.
    Prefix,
}

/// One recognised Ktor DSL call ([`ktor_call`]).
pub(crate) struct KtorCall<'t> {
    pub(crate) kind: KtorKind,
    /// The first argument's string, exactly as written between its quotes;
    /// `None` for a path-less verb (`get { }`).
    pub(crate) path: Option<&'t str>,
    /// The `value_arguments` of the head call, walked like any other code.
    pub(crate) args: Option<TsNode<'t>>,
    /// The trailing `annotated_lambda`: the handler / block body.
    pub(crate) lambda: TsNode<'t>,
}

fn ktor_verb(name: &str) -> Option<&'static str> {
    Some(match name {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "patch" => "PATCH",
        "delete" => "DELETE",
        "head" => "HEAD",
        "options" => "OPTIONS",
        _ => return None,
    })
}

/// Classify `outer` as a Ktor DSL call — a verb or `route`, called with no
/// receiver, a plain string first argument (a verb may have none) and a
/// trailing lambda — or `None`. The shared seam for every Kotlin pass that
/// must agree on what a route call is (the route scan here, A14.3's calls).
pub(crate) fn ktor_call<'t>(outer: TsNode<'t>, src: &'t [u8]) -> Option<KtorCall<'t>> {
    if outer.kind() != "call_expression" {
        return None;
    }
    let lambda = named_child_of_kind(outer, &["annotated_lambda"])?;
    let head = outer.named_child(0)?;
    match head.kind() {
        // `get { … }`: a verb with no path. (A Resources-typed
        // `get<Articles> { … }` never reaches here: the grammar reads it as
        // `get < Articles > { … }`, two binary comparisons.)
        "identifier" => {
            let verb = ktor_verb(text_of(head, src))?;
            Some(KtorCall {
                kind: KtorKind::Verb(verb),
                path: None,
                args: None,
                lambda,
            })
        }
        // `get("/p") { … }` / `route("/p") { … }`.
        "call_expression" => {
            let name = head.named_child(0).filter(|n| n.kind() == "identifier")?;
            let name = text_of(name, src);
            let kind = if name == "route" {
                KtorKind::Prefix
            } else {
                KtorKind::Verb(ktor_verb(name)?)
            };
            let args = named_child_of_kind(head, &["value_arguments"])?;
            let first = named_child_of_kind(args, &["value_argument"])?;
            let lit = first
                .named_child(0)
                .filter(|n| n.kind() == "string_literal")?;
            Some(KtorCall {
                kind,
                path: Some(literal_body(lit, src)?),
                args: Some(args),
                lambda,
            })
        }
        _ => None,
    }
}

/// The text between a `string_literal`'s opening and closing quote tokens,
/// escapes and `$` templates as written — the same bytes the text scan read.
fn literal_body<'t>(lit: TsNode<'t>, src: &'t [u8]) -> Option<&'t str> {
    let open = lit.child(0)?;
    let last = u32::try_from(lit.child_count().checked_sub(1)?).ok()?;
    let close = lit.child(last)?;
    if open.kind() != "\"" || close.kind() != "\"" || open.id() == close.id() {
        return None;
    }
    std::str::from_utf8(src.get(open.end_byte()..close.start_byte())?).ok()
}

/// What the Ktor pass did, for the `[kotlin/ktor]` marker.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KtorCounts {
    /// Distinct `METHOD path` keys the AST scan found in the file.
    pub(crate) routes: usize,
    /// HANDLED_BY edges emitted (one per distinct route and handler).
    pub(crate) handled_by: usize,
    /// Distinct keys the retired Ktor text scan finds in the same source.
    pub(crate) text_would_find: usize,
}

/// The process-global bank the marker reads: routes, handled_by,
/// text_would_find. Diagnostics only: nothing here reaches the graph.
static COUNTS: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// Add one file's counts to the bank (end of `parse_file`).
pub(crate) fn publish(c: KtorCounts) {
    for (slot, n) in COUNTS
        .iter()
        .zip([c.routes, c.handled_by, c.text_would_find])
    {
        if n > 0 {
            slot.fetch_add(n, Ordering::Relaxed);
        }
    }
}

/// Read and zero the bank, so the next repo's line starts clean.
pub(crate) fn take() -> KtorCounts {
    let [routes, handled_by, text_would_find] =
        COUNTS.each_ref().map(|c| c.swap(0, Ordering::Relaxed));
    KtorCounts {
        routes,
        handled_by,
        text_would_find,
    }
}

/// The `[kotlin/ktor]` marker line.
pub(crate) fn marker(c: KtorCounts, repo_label: &str) -> String {
    format!(
        "[kotlin/ktor] ast routes={} handled_by={} (text_scan_would_find={}) repo={repo_label}",
        c.routes, c.handled_by, c.text_would_find
    )
}

/// Walk the whole file for Ktor DSL routes. Runs after `walk_members`, so
/// `Acc::fn_ids` names every declared function and the Spring annotation
/// routes are already in `Acc::routes_seen`. Iterative (an explicit stack),
/// because it descends every expression and a generated file's expression
/// nesting is unbounded.
pub(crate) fn scan_ktor(root: TsNode, source: &str, file: &File, acc: &mut Acc) {
    // Prefix arena: a stack entry holds an index; entry 0 is "no prefix".
    let mut prefixes: Vec<String> = vec![String::new()];
    let mut found: HashSet<String> = HashSet::new();
    let mut bound: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut stack: Vec<(TsNode, NodeId, usize)> = vec![(root, file.module_id, 0)];
    while let Some((node, outer_handler, prefix_ix)) = stack.pop() {
        let handler = if node.kind() == "function_declaration" {
            acc.fn_ids.get(&node.id()).copied().unwrap_or(outer_handler)
        } else {
            outer_handler
        };
        if let Some(call) = ktor_call(node, file.src) {
            let mut inner_ix = prefix_ix;
            match call.kind {
                KtorKind::Prefix => {
                    let joined =
                        endpoint::join_path(&prefixes[prefix_ix], call.path.unwrap_or_default());
                    inner_ix = prefixes.len();
                    prefixes.push(joined);
                }
                KtorKind::Verb(method) => {
                    if let Some(path) = route_path(&prefixes[prefix_ix], call.path) {
                        let name = endpoint::route_qname(method, &path);
                        emit_ktor_route(method, &name, node, handler, file, acc, &mut bound);
                        found.insert(name);
                    }
                }
            }
            // The lambda pushed first pops LAST: source order.
            stack.push((call.lambda, handler, inner_ix));
            if let Some(args) = call.args {
                stack.push((args, handler, prefix_ix));
            }
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<TsNode> = node.named_children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push((child, handler, prefix_ix));
        }
    }
    acc.ktor.routes += found.len();
    acc.ktor.text_would_find += retired_text_scan_count(source);
}

/// The route path a verb call names under `prefix`, or `None`.
///
/// No prefix: the literal as written, when it starts with `/` (the kept text
/// scan guard; no path at all names nothing). Under a prefix:
/// `compose_route_path`, relative templates and a path-less verb included.
fn route_path(prefix: &str, tmpl: Option<&str>) -> Option<String> {
    if prefix.is_empty() {
        return tmpl.filter(|p| p.starts_with('/')).map(str::to_string);
    }
    Some(endpoint::compose_route_path(
        prefix,
        tmpl.unwrap_or_default(),
    ))
}

/// One Ktor ROUTE — Confidence::Medium, a ROUTE_METHOD cell and the call's
/// POSITION — on first sight of its name, and a Medium HANDLED_BY to
/// `handler` once per (route, handler).
fn emit_ktor_route(
    method: &str,
    name: &str,
    call: TsNode,
    handler: NodeId,
    file: &File,
    acc: &mut Acc,
    bound: &mut HashSet<(NodeId, NodeId)>,
) {
    let route_id = NodeId::from_parts(GRAPH_TYPE, file.repo, node_kind::ROUTE, name);
    if acc.routes_seen.insert(name.to_string()) {
        acc.nodes.push(Node {
            id: route_id,
            repo: file.repo,
            confidence: Confidence::Medium,
            cells: vec![
                Cell {
                    kind: cell_type::ROUTE_METHOD,
                    payload: CellPayload::Text(method.to_string()),
                },
                Cell {
                    kind: cell_type::POSITION,
                    payload: CellPayload::Json(glia_doc::position_json(&call, file.rel)),
                },
            ],
        });
        acc.nav.record(route_id, name, name, node_kind::ROUTE, None);
    }
    if bound.insert((route_id, handler)) {
        acc.edges.push(Edge {
            from: route_id,
            to: handler,
            category: edge_category::HANDLED_BY,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        acc.ktor.handled_by += 1;
    }
}

// ---------------------------------------------------------------------------
// Text scans: WebFlux, Javalin, and the retired Ktor scan's count
// ---------------------------------------------------------------------------

/// What must follow a matched `"/path"` for the call to count as a route.
#[derive(Clone, Copy)]
enum After {
    /// Ktor DSL: `get("/path") {` — the route always opens a block.
    Block,
    /// Javalin: `app.get("/path", handler)` — a handler is the second arg,
    /// which rules out a single-arg `cache.get("/key")`.
    Comma,
    /// WebFlux functional: `.GET("/path", h::list)` — the upper-case verb is
    /// discriminator enough.
    Anything,
}

struct Scan {
    needles: [(&'static str, &'static str); 7],
    /// Ktor's needles have no receiver, so they must start a word
    /// (`forget("/x")` is not a route).
    word_start: bool,
    after: After,
}

/// The A14.2 port of the Java parser's `scan_ktor_routes`. Retired by A14.5:
/// it emits nothing, and runs only for the marker's `text_scan_would_find`.
const KTOR_TEXT: Scan = Scan {
    needles: [
        ("get(\"", "GET"),
        ("post(\"", "POST"),
        ("put(\"", "PUT"),
        ("patch(\"", "PATCH"),
        ("delete(\"", "DELETE"),
        ("head(\"", "HEAD"),
        ("options(\"", "OPTIONS"),
    ],
    word_start: true,
    after: After::Block,
};

const WEBFLUX: Scan = Scan {
    needles: [
        (".GET(\"", "GET"),
        (".POST(\"", "POST"),
        (".PUT(\"", "PUT"),
        (".PATCH(\"", "PATCH"),
        (".DELETE(\"", "DELETE"),
        (".HEAD(\"", "HEAD"),
        (".OPTIONS(\"", "OPTIONS"),
    ],
    word_start: false,
    after: After::Anything,
};

const JAVALIN: Scan = Scan {
    needles: [
        (".get(\"", "GET"),
        (".post(\"", "POST"),
        (".put(\"", "PUT"),
        (".patch(\"", "PATCH"),
        (".delete(\"", "DELETE"),
        (".head(\"", "HEAD"),
        (".options(\"", "OPTIONS"),
    ],
    word_start: false,
    after: After::Comma,
};

/// Run the WebFlux and Javalin scans over `source`, emitting one ROUTE per
/// new `METHOD path`.
pub(crate) fn scan_text_routes(source: &str, repo: RepoId, acc: &mut Acc) {
    for scan in [&WEBFLUX, &JAVALIN] {
        for (method, path) in hits(source, scan) {
            if acc.routes_seen.insert(format!("{method} {path}")) {
                emit_text_route(method, path, repo, acc);
            }
        }
    }
}

/// Distinct `METHOD path` keys the retired Ktor text scan finds in `source`.
fn retired_text_scan_count(source: &str) -> usize {
    hits(source, &KTOR_TEXT)
        .into_iter()
        .map(|(method, path)| format!("{method} {path}"))
        .collect::<HashSet<_>>()
        .len()
}

/// Every `(method, path)` one scan matches, in needle then source order.
fn hits<'s>(source: &'s str, scan: &Scan) -> Vec<(&'static str, &'s str)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    for (needle, method) in scan.needles {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let start = pos + needle.len();
            if scan.word_start
                && pos > 0
                && (bytes[pos - 1].is_ascii_alphanumeric()
                    || bytes[pos - 1] == b'_'
                    || bytes[pos - 1] == b'.')
            {
                search_from = start;
                continue;
            }
            // The closing quote, stepping over escapes. Every byte stepped to
            // is either ASCII or a continuation byte, never a `"`, so `j`
            // stops on a char boundary.
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    2
                } else {
                    1
                };
            }
            if j >= bytes.len() {
                break;
            }
            let path = &source[start..j];
            search_from = j + 1;
            if !path.starts_with('/') {
                continue;
            }
            let rest = &source[j + 1..];
            let accepted = match scan.after {
                After::Block => rest
                    .find(|c: char| !c.is_whitespace() && c != ')')
                    .is_some_and(|o| rest.as_bytes()[o] == b'{'),
                After::Comma => rest.trim_start().starts_with(','),
                After::Anything => true,
            };
            if accepted {
                out.push((method, path));
            }
        }
    }
    out
}

fn emit_text_route(method: &str, path: &str, repo: RepoId, acc: &mut Acc) {
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(method.to_string()),
        }],
    });
    acc.nav
        .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);
}

#[cfg(test)]
mod tests {
    use glia_code_domain::FileParse;

    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn parse(source: &str, rel: &str, module: &str) -> (FileParse, KtorCounts) {
        let (fp, _, ktor) = crate::parse_counting(source, rel, module, repo()).unwrap();
        (fp, ktor)
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

    fn route_id(name: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, name)
    }

    /// Every HANDLED_BY as (route name, handler qname), sorted.
    fn handled_by(fp: &FileParse) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .map(|e| {
                assert_eq!(
                    e.confidence,
                    Confidence::Medium,
                    "structural binding is Medium"
                );
                (
                    fp.nav.name_by_id.get(&e.from).cloned().unwrap_or_default(),
                    fp.nav.qname_by_id.get(&e.to).cloned().unwrap_or_default(),
                )
            })
            .collect();
        out.sort();
        out
    }

    fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    /// The kotlin-ktor fixture's app.kt, verbatim.
    const FIXTURE_APP: &str = r#"package com.acme.ktor

import com.acme.store.ItemStore

fun Application.itemRoutes(store: ItemStore) {
    routing {
        get("/api/items") {
            call.respond(store.all())
        }
        post("/api/items") {
            call.respond(store.add("x"))
        }
    }
}
"#;

    #[test]
    fn ktor_ast_routes_match_text_scanner() {
        // The Java parser's `ktor_routes` source. The top-level routes keep the
        // text scan's names and NodeIds byte-identically; the nested one now
        // composes its `route("/admin")` prefix (the 0.5.0 break).
        let source = r#"
fun Application.module() {
    routing {
        get("/users") {
            call.respond(listOf<String>())
        }
        post("/users") {
            call.respond("ok")
        }
        route("/admin") {
            delete("/users/{id}") { call.respond("ok") }
        }
    }
}
"#;
        let (fp, counts) = parse(source, "Application.kt", "com::example::Application");
        assert_eq!(
            route_names(&fp),
            vec!["DELETE /admin/users/{id}", "GET /users", "POST /users"]
        );
        for name in ["GET /users", "POST /users", "DELETE /admin/users/{id}"] {
            assert!(fp.nodes.iter().any(|n| n.id == route_id(name)), "{name}");
        }
        assert!(
            !fp.nodes
                .iter()
                .any(|n| n.id == route_id("DELETE /users/{id}"))
        );
        let module_fn = "com::example::Application::module";
        assert_eq!(
            handled_by(&fp),
            pairs(&[
                ("DELETE /admin/users/{id}", module_fn),
                ("GET /users", module_fn),
                ("POST /users", module_fn),
            ])
        );
        assert_eq!(
            counts,
            KtorCounts {
                routes: 3,
                handled_by: 3,
                text_would_find: 3,
            }
        );
        let node = fp
            .nodes
            .iter()
            .find(|n| n.id == route_id("GET /users"))
            .unwrap();
        assert_eq!(node.confidence, Confidence::Medium);
        assert!(matches!(
            &node.cells[0],
            Cell { kind, payload: CellPayload::Text(m) } if *kind == cell_type::ROUTE_METHOD && m == "GET"
        ));
    }

    #[test]
    fn ktor_route_id_is_stable() {
        // The fixture's route keeps the exact id the 0.4.18 text scan minted.
        let (fp, _) = parse(FIXTURE_APP, "app.kt", "app");
        let expected = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, "GET /api/items");
        let emitted: Vec<NodeId> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(id, k)| {
                **k == node_kind::ROUTE
                    && fp.nav.name_by_id.get(*id).map(String::as_str) == Some("GET /api/items")
            })
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(emitted, vec![expected]);
        assert_eq!(fp.nodes.iter().filter(|n| n.id == expected).count(), 1);
    }

    #[test]
    fn ktor_route_handled_by_enclosing_function() {
        let (fp, counts) = parse(FIXTURE_APP, "app.kt", "app");
        assert_eq!(
            handled_by(&fp),
            pairs(&[
                ("GET /api/items", "app::itemRoutes"),
                ("POST /api/items", "app::itemRoutes"),
            ])
        );
        assert_eq!((counts.routes, counts.handled_by), (2, 2));

        // A member fun binds its METHOD; a local fun has no node and inherits
        // the declared fun around it; an expression-body fun binds too.
        let source = r#"
class Api {
    fun Route.install() {
        get("/a") { }
    }
}

fun outer() {
    fun local() {
        routing { get("/b") { } }
    }
}

fun Application.expr() = routing { put("/c") { } }
"#;
        let (fp, _) = parse(source, "api.kt", "api");
        assert_eq!(
            handled_by(&fp),
            pairs(&[
                ("GET /a", "Api::install"),
                ("GET /b", "api::outer"),
                ("PUT /c", "api::expr"),
            ])
        );
    }

    #[test]
    fn map_get_string_key_is_not_a_route() {
        let source = r#"
fun f(map: Map<String, String>, cache: Cache) {
    val v = map.get("/key")
    cache.get("/key") { it }
    this.get("/q") { }
    get("count") { }
    get("/no-lambda")
    get(path = "/named") { }
    get("""/raw""") { }
    forget("/nope") { }
    routing { get { } }
}
"#;
        let (fp, counts) = parse(source, "f.kt", "f");
        assert_eq!(route_names(&fp), Vec::<&str>::new());
        assert_eq!(counts.routes, 0);
        assert!(
            !fp.edges
                .iter()
                .any(|e| e.category == edge_category::HANDLED_BY)
        );
    }

    #[test]
    fn ktor_route_carries_position_cell() {
        let (fp, _) = parse(FIXTURE_APP, "app.kt", "app");
        let node = fp
            .nodes
            .iter()
            .find(|n| n.id == route_id("GET /api/items"))
            .unwrap();
        let pos = node.cells.iter().find(|c| c.kind == cell_type::POSITION);
        let Some(Cell {
            payload: CellPayload::Json(pos),
            ..
        }) = pos
        else {
            panic!("no POSITION: {:?}", node.cells);
        };
        let v: serde_json::Value = serde_json::from_str(pos).unwrap();
        assert_eq!(v["file"], "app.kt");
        // `get("/api/items") {` is line 7 (0-indexed 6); its block closes on 9.
        assert_eq!(
            (v["start_line"].as_u64(), v["end_line"].as_u64()),
            (Some(6), Some(8))
        );
    }

    #[test]
    fn route_outside_any_function_falls_back_to_module() {
        let source = r#"
val module: Application.() -> Unit = {
    routing {
        post("/top") { }
    }
}
"#;
        let (fp, counts) = parse(source, "srv/main.kt", "srv::main");
        assert_eq!(handled_by(&fp), pairs(&[("POST /top", "srv::main")]));
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "srv::main");
        assert!(
            fp.edges
                .iter()
                .any(|e| e.from == route_id("POST /top") && e.to == module)
        );
        assert_eq!(counts.handled_by, 1);
    }

    #[test]
    fn nested_route_blocks_compose_prefixes() {
        let source = r#"
fun Application.api() {
    routing {
        route("/api") {
            route("v1") {
                get("{id}") { }
                get { }
                post("/") { }
            }
            route("/typed") {
                // Resources: the path is on the type, so no `GET /typed`.
                get<Articles> { }
            }
            get("/health") { }
        }
        authenticate("jwt") {
            get("/me") { }
        }
    }
}
"#;
        let (fp, counts) = parse(source, "api.kt", "api");
        assert_eq!(
            route_names(&fp),
            vec![
                "GET /api/health",
                "GET /api/v1",
                "GET /api/v1/{id}",
                "GET /me",
                "POST /api/v1",
            ]
        );
        // The retired text scan saw three flat keys (`POST /`, `GET /health`,
        // `GET /me`) — the mismatch the marker exists to show.
        assert_eq!(counts.routes, 5);
        assert_eq!(counts.text_would_find, 3);
    }

    #[test]
    fn a_route_declared_twice_is_one_node() {
        let source = r#"
fun Application.a() {
    routing {
        get("/x") { }
        get("/x") { }
    }
}

fun Application.b() {
    routing { get("/x") { } }
}
"#;
        let (fp, counts) = parse(source, "dup.kt", "dup");
        assert_eq!(
            fp.nodes
                .iter()
                .filter(|n| n.id == route_id("GET /x"))
                .count(),
            1
        );
        assert_eq!(
            handled_by(&fp),
            pairs(&[("GET /x", "dup::a"), ("GET /x", "dup::b")])
        );
        assert_eq!((counts.routes, counts.handled_by), (1, 2));
    }

    #[test]
    fn ktor_call_classifies_the_dsl_heads() {
        let source = "fun f() {\n    route(\"/a\") { get(\"/b\") { } }\n    map.get(\"/c\")\n}\n";
        let mut parser = tree_sitter::Parser::new();
        let lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut kinds = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if let Some(call) = ktor_call(n, source.as_bytes()) {
                kinds.push((call.kind, call.path));
            }
            let mut c = n.walk();
            stack.extend(n.named_children(&mut c));
        }
        kinds.sort_by_key(|(_, p)| *p);
        assert_eq!(
            kinds,
            vec![
                (KtorKind::Prefix, Some("/a")),
                (KtorKind::Verb("GET"), Some("/b")),
            ]
        );
    }

    #[test]
    fn marker_line_shape() {
        let c = KtorCounts {
            routes: 2,
            handled_by: 2,
            text_would_find: 2,
        };
        assert_eq!(
            marker(c, "r"),
            "[kotlin/ktor] ast routes=2 handled_by=2 (text_scan_would_find=2) repo=r"
        );
    }
}
