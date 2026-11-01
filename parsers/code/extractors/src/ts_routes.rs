//! Backend HTTP route extraction for JS/TS — complements the endpoint
//! (fetch/axios) extraction in the TypeScript parser. Detects:
//!   - Next.js file-based API routes: `pages/api/...` and `app/api/.../route.ts`
//!   - Express / Koa / Hono / Fastify-style: `app.get('/path', ...)`,
//!     `router.post('/path', ...)`.
//!   - SvelteKit `+server.ts`, NestJS controllers, Hapi `server.route(...)`
//!     and Bun.serve `routes:` objects.
//!
//! Runs per-file. Output is route nodes that HttpStackResolver can match
//! against endpoint nodes to produce cross-stack HTTP edges.
//!
//! LB.11b: one ROUTE node per (method, path), qname `<METHOD> <path>` built by
//! `endpoint::route_qname` with name = qname — the shape every other server
//! parser writes (Go since LB.11a). Each registration row adds a POSITION cell
//! (ascending, deduped), then one ROUTE_METHOD cell carries the method, the
//! named handler and the 1-based line of the first registration. A
//! method-agnostic registration is `ANY`, the token the resolver's any tier
//! keys on: Express `.all(`, Nest `@All(`, Hapi `*` / `ANY` / `ALL`, a Next.js
//! Pages Router default export (Next hands it every method) and a Bun route
//! whose value is a handler function or a `Response` (Bun serves it for every
//! method).
//!
//! Like the other cross-cutting extractors, this is pattern-based. Only called
//! by the pipeline for JS/TS-family languages.
//!
//! A3.5: the Express scan skips `.get('/x')` on an HTTP-client receiver
//! (`this.http.get` is an outbound ENDPOINT, not a server route), and a route
//! registered with a NAMED handler (`app.get('/users/:id', getUser)`) carries a
//! HANDLED_BY `UnresolvedRef` for the graph builder to bind.

use std::collections::BTreeMap;

use glia_code_domain::{
    CallQualifier, CodeNav, GRAPH_TYPE, IncludeTarget, Mount, NavFact, UnresolvedRef, cell_type,
    edge_category, endpoint, node_kind,
};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

use crate::anchor;

pub struct RouteNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// A3.5: `route --HANDLED_BY--> handler` refs for Express-style routes
    /// registered with a named handler. Parsers extract, the graph resolves:
    /// `resolve_refs` binds the name through the module's imports, then its
    /// own top-level defs, then a repo-unique function.
    pub refs: Vec<UnresolvedRef>,
    /// A3.5: Shape-1 matches dropped because the receiver is an HTTP client
    /// (`this.http.get('/users')`) — each one a phantom server ROUTE that is
    /// no longer minted. Feeds the `[extract] ts-routes client-calls skipped`
    /// build marker.
    pub skipped_client_calls: usize,
    /// LB.11b: POSITION cells pushed, one per distinct registration row of
    /// each route. Feeds the per-file `[ts-routes]` marker.
    pub positioned: usize,
    /// LB.11b: ROUTE nodes whose method is `ANY` (a method-agnostic
    /// registration). Feeds the per-file `[ts-routes]` marker.
    pub any: usize,
    /// GD.42: the file's include-style mount facts
    /// ([`NavFact::IncludeMount`]: router values, `.use(..)` / `.route(..)`
    /// mounts, default exports, path lists, and the NestJS controller,
    /// `@Module`, `RouterModule` and `setGlobalPrefix` facts), for the caller
    /// to record on the file MODULE (`CodeNav::record_fact`), in scan order.
    /// The graph crate's route-mount pass (`graph/src/build/route_mounts.rs`)
    /// reads them.
    pub mount_facts: Vec<NavFact>,
}

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "options", "head", "all",
];

/// The method token of a method-agnostic ROUTE — the one every server parser
/// writes and the HTTP resolver's any tier keys on.
const ANY: &str = "ANY";

/// One ROUTE under construction: the handler its registrations named ("" when
/// none did) and the 0-based source row of every registration.
#[derive(Debug, Default)]
struct RouteEntry {
    handler: String,
    rows: Vec<u32>,
}

/// `(canonical path, METHOD)` -> its route. A BTreeMap, so emission order is a
/// function of the keys alone (the byte-identical store gate).
type RouteMap = BTreeMap<(String, &'static str), RouteEntry>;

pub fn extract_ts_backend_routes(
    source: &str,
    path: &str,
    module_id: NodeId,
    module_qname: &str,
    repo: RepoId,
) -> RouteNodes {
    let mut by_route: RouteMap = BTreeMap::new();
    // GD.42: the file's router values, verb chains and mount facts.
    let scan = MountScan::read(source);
    // (path, METHOD, handler) triples for the HANDLED_BY refs, deduped and
    // ordered; each ref leaves from its own per-method node. The value is the
    // 0-based row of the first registration that named that handler: the
    // ref's site line (LC.3b).
    let mut handled_by: BTreeMap<(String, &'static str, String), u32> = BTreeMap::new();
    let mut skipped_client_calls = 0usize;

    // Shape 1: Express-style `<x>.<method>('/...', ...)` and Hono/Koa routers.
    for (row, line) in source.lines().enumerate() {
        let row = u32::try_from(row).unwrap_or(u32::MAX);
        let t = line.trim();
        for method in HTTP_METHODS {
            let needle = format!(".{method}(");
            let Some(idx) = t.find(&needle) else {
                continue;
            };
            let after = &t[idx + needle.len()..];
            let arg_start = after.trim_start();
            let Some((quote, rest)) = arg_start
                .strip_prefix('"')
                .map(|r| ('"', r))
                .or_else(|| arg_start.strip_prefix('\'').map(|r| ('\'', r)))
                .or_else(|| arg_start.strip_prefix('`').map(|r| ('`', r)))
            else {
                continue;
            };
            let Some(end) = rest.find(quote) else {
                continue;
            };
            let route = &rest[..end];
            if !route.starts_with('/') || route.len() > 256 {
                continue;
            }
            let after_literal = &rest[end + quote.len_utf8()..];
            // A3.5: `this.http.get('/users')` has exactly this shape but is an
            // OUTBOUND call — the receiver names an HTTP client. Rejected here
            // so it is not also minted as a phantom server ROUTE that the
            // service's own ENDPOINT then pairs to. An inline function handler
            // overrides the name: HTTP clients never take one, and Hono's
            // `const api = new Hono(); api.get('/posts', async (c) => …)` is a
            // server router named like a client. `looks_like_http_client`
            // stays as the second line for the receiverless shapes.
            // GD.42: a route on a module-level router value rides that
            // router's mount slot (its own prefix folded into the path); a
            // declared router (`const api = new Router()`) is no client.
            let router = scan
                .receiver_at(t, idx, row as usize)
                .filter(|_| !module_qname.is_empty());
            if router.is_none()
                && endpoint::is_http_client_receiver(endpoint::ident_before(t, idx))
                && !has_inline_handler(after_literal)
            {
                skipped_client_calls += 1;
                continue;
            }
            if router.is_none() && looks_like_http_client(t) {
                continue;
            }
            let handler = named_handler(after_literal);
            let added = match router.and_then(|r| scan.routers.get(r).map(|v| (r, v))) {
                Some((var, rv)) => add_mounted(
                    &mut by_route,
                    &Mount::Module {
                        module: module_qname.to_string(),
                        var: var.to_string(),
                        suffix: rv.own.clone(),
                    },
                    route,
                    method,
                    handler.unwrap_or(""),
                    row,
                ),
                None => add_method(&mut by_route, route, method, handler.unwrap_or(""), row),
            };
            if let Some(key) = added
                && let Some(h) = handler
            {
                handled_by
                    .entry((key.0, key.1, h.to_string()))
                    .or_insert(row);
            }
        }
    }

    // Shape 2: Next.js file-based routing — path gives us the route, source
    // gives us the method(s) and the row of each export.
    if let Some(route) = nextjs_route_from_path(path) {
        for (method, row) in nextjs_methods_from_source(source) {
            add_method(&mut by_route, &route, method, "", row);
        }
    }

    // Shape 3: SvelteKit `+server.ts` — path from file path, methods from
    // named exports (same shape as Next.js App Router).
    if let Some(route) = sveltekit_route_from_path(path) {
        for (method, row) in nextjs_methods_from_source(source) {
            add_method(&mut by_route, &route, method, "", row);
        }
    }

    // Shape 4: NestJS controllers — combine @Controller(prefix) with method
    // decorators @Get/@Post/...(suffix), located at the decorator. FH.19: the
    // method the decorator decorates is the route's handler, a HANDLED_BY ref
    // `<Class>.<method>` the graph binds to that class's own METHOD (the
    // registering file declares the class: `module_type_method`). The ROUTE
    // cell's `handler` stays "" as before: the edge carries the binding.
    let nest = nestjs_routes(source);
    let (mut nest_bound, mut nest_unbound) = (0usize, 0usize);
    for r in &nest {
        let Some(key) = add_method(&mut by_route, &r.path, r.method, "", r.row) else {
            continue;
        };
        match &r.handler {
            Some(h) => {
                nest_bound += 1;
                handled_by.entry((key.0, key.1, h.clone())).or_insert(r.row);
            }
            None => nest_unbound += 1,
        }
    }
    // FH.19 fired_on marker: one per file holding a NestJS verb decorator.
    if nest_bound + nest_unbound > 0 {
        eprintln!(
            "[ts-routes] nest handlers bound={nest_bound} unbound={nest_unbound} file={path}"
        );
    }

    // Shape 5: Hapi.js — `server.route({ method: 'GET', path: '/x', handler })`
    // and array form `server.route([{ ... }, { ... }])`. Method may be a string
    // ('GET') or array of strings (['GET', 'POST']). Located at the config
    // object's `{`.
    for (method, route, row) in hapi_routes(source) {
        add_method(&mut by_route, &route, method, "", row);
    }

    // Shape 6: Bun.serve `routes:` object —
    // `Bun.serve({ routes: { '/api/users': { GET: h, POST: h2 }, ... } })`.
    // Bun 1.2+ syntax, located at the route key. Single-handler `fetch(req)`
    // style is intentionally skipped (routing is internal to user code).
    for (method, route, row) in bun_serve_routes(source) {
        add_method(&mut by_route, &route, method, "", row);
    }

    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let (mut positioned, mut any) = (0usize, 0usize);
    for ((route, method), entry) in by_route {
        // LB.5 / LB.11b: the shared builder. Every key's path is already
        // canonical (`add_method`), so the qname is `<METHOD> <path>`; a
        // GD.42 provisional key is the mount qname's tail.
        let qname = key_qname(method, &route);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname);
        let mut rows = entry.rows;
        rows.sort_unstable();
        rows.dedup();
        let mut cells: Vec<Cell> = rows
            .iter()
            .map(|&r| anchor::position_cell(path, r))
            .collect();
        positioned += cells.len();
        let line = rows.first().map_or(0, |r| u64::from(*r) + 1);
        cells.push(Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Json(format!(
                r#"{{"method":"{method}","handler":"{}","file":"{}","line":{line},"col":0}}"#,
                escape_json(&entry.handler),
                escape_json(path),
            )),
        });
        any += usize::from(method == ANY);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells,
        });
        nav.record(id, &qname, &qname, node_kind::ROUTE, Some(module_id));
    }

    let refs = handled_by
        .into_iter()
        .map(|((route, method, handler), line)| {
            let qname = key_qname(method, &route);
            let qualifier = match handler.split_once('.') {
                Some((base, name)) => CallQualifier::Attribute {
                    base: base.to_string(),
                    name: name.to_string(),
                },
                None => CallQualifier::Bare(handler),
            };
            UnresolvedRef {
                from: NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname),
                from_module: module_id,
                qualifier,
                category: edge_category::HANDLED_BY,
                line,
            }
        })
        .collect();

    let mut mount_facts = scan.facts;
    mount_facts.extend(nest_mount_facts(source, &nest));

    RouteNodes {
        nodes,
        nav,
        refs,
        skipped_client_calls,
        positioned,
        any,
        mount_facts,
    }
}

/// The upper-case ROUTE method token for a lower- or upper-case verb:
/// `get` -> `GET`, and the method-agnostic spellings (`all` — Express
/// `.all(`, Nest `@All(`, Hapi `*` via [`canonical_http_method`] — and `any`)
/// -> [`ANY`]. None for anything else, which drops the registration.
fn method_token(method: &str) -> Option<&'static str> {
    Some(match method.to_ascii_lowercase().as_str() {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "options" => "OPTIONS",
        "head" => "HEAD",
        "all" | "any" | "*" => ANY,
        _ => return None,
    })
}

/// Record a registration of `method` on `route` at 0-based source `row`, with
/// its `handler` ("" when none is named). Returns the `(path, METHOD)` key it
/// landed on, or None when the registration is dropped, so the caller emits no
/// ref for it. A repeated (method, path) keeps one node: the row is appended
/// and a handler the first registration did not name is filled in.
fn add_method(
    by_route: &mut RouteMap,
    route: &str,
    method: &str,
    handler: &str,
    row: u32,
) -> Option<(String, &'static str)> {
    // Drop template-source expressions captured from framework internals
    // (`/${this.routeConfig.path}`) — not literal routes. (glia-v2 G8)
    if route.contains("${") {
        return None;
    }
    let method = method_token(method)?;
    // LB.5: keyed on the canonical path, so a relative and a slashed spelling
    // of one route can never become two nodes with one id. Every shape above
    // hands in a slashed path today; this is the guarantee, not a rewrite.
    let key = (endpoint::canonical_http_path(route).into_owned(), method);
    let entry = by_route.entry(key.clone()).or_default();
    if entry.handler.is_empty() {
        entry.handler = handler.to_string();
    }
    entry.rows.push(row);
    Some(key)
}

/// GD.42: [`add_method`] for a registration on a module router: `mount` is
/// its [`Mount::Module`] slot with the router's own prefix as suffix, and
/// the key's path is the provisional mount qname's tail,
/// `<mount:module:<module>#<var>><own + route>` ([`endpoint::mount_route_qname`]),
/// which [`key_qname`] turns back into the qname.
fn add_mounted(
    by_route: &mut RouteMap,
    mount: &Mount,
    route: &str,
    method: &str,
    handler: &str,
    row: u32,
) -> Option<(String, &'static str)> {
    if route.contains("${") {
        return None;
    }
    let method = method_token(method)?;
    let qname = endpoint::mount_route_qname(method, mount, route);
    let tail = qname.split_once(' ').map(|(_, t)| t)?.to_string();
    let key = (tail, method);
    let entry = by_route.entry(key.clone()).or_default();
    if entry.handler.is_empty() {
        entry.handler = handler.to_string();
    }
    entry.rows.push(row);
    Some(key)
}

/// The ROUTE qname of a [`RouteMap`] key: `<METHOD> <path>` through
/// [`endpoint::route_qname`], or a GD.42 provisional key (its path the mount
/// qname's tail, [`add_mounted`]) joined back as it was built.
fn key_qname(method: &str, path: &str) -> String {
    if path.starts_with(MOUNT_OPEN) {
        format!("{method} {path}")
    } else {
        endpoint::route_qname(method, path)
    }
}

/// The token a provisional mount ROUTE qname's path opens with
/// ([`endpoint::mount_route_qname`]).
const MOUNT_OPEN: &str = "<mount:";

/// The top-level arguments that follow the route literal on this line, and
/// whether the call's closing `)` was seen there. None when the literal is not
/// followed by `,` (the call has no further arguments) or the text is not a
/// well-formed argument list. When the call does not close on this line, the
/// last entry runs to the end of the line.
fn trailing_args(after_literal: &str) -> Option<(Vec<&str>, bool)> {
    let rest = after_literal.trim_start().strip_prefix(',')?;
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let mut seg_start = 0usize;
    let mut args: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => {
                let delim = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != delim {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    if bytes[i] != b')' {
                        return None;
                    }
                    args.push(rest.get(seg_start..i)?);
                    return Some((args, true));
                }
                depth -= 1;
            }
            b',' if depth == 0 => {
                args.push(rest.get(seg_start..i)?);
                seg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    args.push(rest.get(seg_start..).unwrap_or(""));
    Some((args, false))
}

/// The last non-empty argument — a trailing comma (`app.get('/x', h,)`)
/// leaves an empty final segment.
fn last_arg<'a>(args: &[&'a str]) -> Option<&'a str> {
    args.iter().map(|a| a.trim()).rev().find(|a| !a.is_empty())
}

/// The NAMED handler of an Express-style registration, read from the text
/// that follows the route literal's closing quote on the same line: the LAST
/// top-level argument, when it is a plain identifier (`getUser`) or a one-dot
/// member (`users.list`). Middleware ahead of it is skipped, so
/// `app.get('/x', auth, getUser)` names `getUser`.
///
/// None for inline handlers (`(req, res) => …`, `function (…) {…}`), wrapper
/// calls (`asyncHandler(getUser)`), `this.x` members (a route has no enclosing
/// class to bind `this` to), and when the call does not close on this line —
/// a multi-line registration's last argument is not visible here, and a
/// middleware mistaken for the handler would be a wrong edge.
fn named_handler(after_literal: &str) -> Option<&str> {
    let (args, closed) = trailing_args(after_literal)?;
    if !closed {
        return None;
    }
    let last = last_arg(&args)?;
    is_handler_reference(last).then_some(last)
}

/// True when the registration's last argument is an inline function —
/// `(c) => …`, `async (req, res) => {`, `c => …`, `function (req, res) {`.
/// Read even when the call does not close on this line, since an inline
/// handler's body usually spans several. The server-side signal that
/// overrides a client-looking receiver name: Angular's HttpClient, axios, ky
/// and fetch wrappers take a path and data/options, never a handler.
fn has_inline_handler(after_literal: &str) -> bool {
    trailing_args(after_literal)
        .and_then(|(args, _)| last_arg(&args))
        .is_some_and(is_inline_function)
}

fn is_inline_function(arg: &str) -> bool {
    let keyword = |s: &str, kw: &str| -> bool {
        s.strip_prefix(kw)
            .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '(' || c == '*'))
    };
    let a = if keyword(arg, "async") {
        arg["async".len()..].trim_start()
    } else {
        arg
    };
    if keyword(a, "function") {
        return true;
    }
    // An arrow: `=>` after a parameter list (`(req, res)`, typed or not) or
    // after a single bare parameter (`c`). An options object `{ f: x => x }`
    // starts with `{`, so it is not one.
    a.find("=>").is_some_and(|i| {
        let head = a[..i].trim();
        head.starts_with('(') || (is_handler_reference(head) && !head.contains('.'))
    })
}

/// `name` or `base.name`, each a JS identifier, and not a literal keyword or a
/// `this.` member.
fn is_handler_reference(s: &str) -> bool {
    fn ident(p: &str) -> bool {
        let mut cs = p.chars();
        cs.next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
            && !matches!(
                p,
                "this" | "null" | "undefined" | "true" | "false" | "function" | "async" | "new"
            )
    }
    match s.split_once('.') {
        Some((base, name)) => ident(base) && ident(name),
        None => ident(s),
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn looks_like_http_client(line: &str) -> bool {
    line.contains("fetch(")
        || line.contains("axios.")
        || line.contains("axios(")
        || line.contains(".request(")
        || line.contains("got(")
        || line.contains("got.")
        || line.contains("ky.")
        || line.contains(".ajax(")
        || line.contains("expect(")
        || line.contains(".resolves")
        || line.contains(".rejects")
}

/// Extract a Next.js route path from a source path, or None if not a Next.js
/// API route file. Handles both Pages Router (`pages/api/foo.ts` → `/api/foo`)
/// and App Router (`app/api/foo/route.ts` → `/api/foo`). Dynamic segments
/// `[id]` become `:id`.
fn nextjs_route_from_path(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/");
    if let Some(rest) = norm.split("pages/api/").nth(1) {
        let without_ext = strip_js_ext(rest)?;
        let cleaned = without_ext.strip_suffix("/index").unwrap_or(without_ext);
        return Some(format!("/api/{}", nextjs_params_to_colon(cleaned)));
    }
    if let Some(rest) = norm.split("app/api/").nth(1) {
        let without_route = rest
            .strip_suffix("/route.ts")
            .or_else(|| rest.strip_suffix("/route.tsx"))
            .or_else(|| rest.strip_suffix("/route.js"))
            .or_else(|| rest.strip_suffix("/route.jsx"))?;
        return Some(format!("/api/{}", nextjs_params_to_colon(without_route)));
    }
    None
}

fn strip_js_ext(s: &str) -> Option<&str> {
    for ext in [".tsx", ".ts", ".jsx", ".js"] {
        if let Some(stripped) = s.strip_suffix(ext) {
            return Some(stripped);
        }
    }
    None
}

fn nextjs_params_to_colon(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            let mut name = String::new();
            while let Some(&nc) = chars.peek() {
                if nc == ']' {
                    chars.next();
                    break;
                }
                name.push(nc);
                chars.next();
            }
            let cleaned = name.trim_start_matches("...").trim_start_matches("..");
            out.push(':');
            out.push_str(cleaned);
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract a SvelteKit route path from `+server.ts`. Example:
/// `src/routes/api/users/+server.ts` → `/api/users`.
/// `src/routes/api/users/[id]/+server.ts` → `/api/users/:id`.
fn sveltekit_route_from_path(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/");
    let file = ["+server.ts", "+server.js"]
        .iter()
        .find(|ext| norm.ends_with(*ext))?;
    let idx = norm
        .find("src/routes/")
        .map(|i| i + "src/routes/".len())
        .or_else(|| norm.find("routes/").map(|i| i + "routes/".len()))?;
    let tail = &norm[idx..norm.len() - file.len()];
    let tail = tail.trim_end_matches('/');
    let cleaned = sveltekit_params_to_colon(tail);
    if cleaned.is_empty() {
        Some("/".to_string())
    } else {
        Some(format!("/{}", cleaned))
    }
}

fn sveltekit_params_to_colon(path: &str) -> String {
    // SvelteKit uses `[param]` same as Next.js dynamic segments.
    nextjs_params_to_colon(path)
}

/// One NestJS verb decorator (`@Get(..)`, `@Post(..)`, ..): its method, the
/// full path (the enclosing `@Controller` prefix joined to its own), the
/// decorator's 0-based row and, FH.19, the handler it decorates as
/// `<Class>.<method>` when the scan reads both a method declaration after it
/// and an enclosing class declaration before it.
#[derive(Debug, PartialEq)]
struct NestRoute {
    method: &'static str,
    path: String,
    row: u32,
    handler: Option<String>,
    /// GD.42: the class the last `class` declaration before the decorator
    /// names (the controller), whether or not a handler method was read.
    class: Option<String>,
}

/// The verb decorators [`nestjs_routes`] reads, with the method each serves.
const NEST_VERBS: &[(&str, &str)] = &[
    ("@Get(", "get"),
    ("@Post(", "post"),
    ("@Put(", "put"),
    ("@Patch(", "patch"),
    ("@Delete(", "delete"),
    ("@Head(", "head"),
    ("@Options(", "options"),
    ("@All(", "all"),
];

/// Scan a TS source for NestJS @Controller + @Get/@Post/... method decorators.
/// Each route is located at the decorator's 0-based line. FH.19: its handler
/// is the first method declaration after the decorator ([`nest_method_after`]:
/// further decorators, comments and modifiers skipped, so a method carrying
/// several verb decorators is every one's handler) in the class the last
/// `class` declaration line before the decorator names ([`class_decl_name`]).
fn nestjs_routes(source: &str) -> Vec<NestRoute> {
    let mut out = Vec::new();
    let mut controller_prefix: Option<String> = None;
    let mut class: Option<&str> = None;
    let mut line_start = 0usize;
    for (row, raw) in source.split_inclusive('\n').enumerate() {
        let at = line_start + (raw.len() - raw.trim_start().len());
        line_start += raw.len();
        let row = u32::try_from(row).unwrap_or(u32::MAX);
        let t = raw.trim();
        if let Some(name) = class_decl_name(t) {
            class = Some(name);
        }
        if t.starts_with("@Controller(") {
            controller_prefix = Some(nest_path_arg(t).unwrap_or_default());
            continue;
        }
        let Some((deco, method)) = NEST_VERBS.iter().find(|(d, _)| t.starts_with(d)) else {
            continue;
        };
        let suffix = nest_path_arg(t).unwrap_or_default();
        let path = combine_nest_paths(controller_prefix.as_deref().unwrap_or(""), &suffix);
        let args = at + deco.len();
        let handler = source
            .get(args..)
            .and_then(|rest| find_balanced_close(rest, b'(', b')'))
            .and_then(|close| source.get(args + close + 1..))
            .and_then(nest_method_after)
            .zip(class)
            .map(|(m, c)| format!("{c}.{m}"));
        out.push(NestRoute {
            method,
            path,
            row,
            handler,
            class: class.map(str::to_string),
        });
    }
    out
}

/// FH.19: the class a declaration line declares: `class X`, behind any of
/// `export` / `default` / `abstract` / `declare` and leading decorators
/// (`@Controller('users') export class UsersController {`). None for any
/// other line, a comment or an anonymous `export default class {`.
fn class_decl_name(line: &str) -> Option<&str> {
    let mut i = skip_decorators(line, 0)?;
    loop {
        let (word, end) = read_ident(line, i)?;
        let next = skip_trivia(line, end);
        match word {
            "export" | "default" | "abstract" | "declare" => i = next,
            "class" => return read_ident(line, next).map(|(name, _)| name),
            _ => return None,
        }
    }
}

/// FH.19: the method name a NestJS verb decorator decorates, read from the
/// text right after the decorator's closing `)`: further decorators and
/// comments are skipped, then the modifiers (`public`, `async`, `static`,
/// `override`, ..), and the identifier that follows is the method when a
/// `(` (after an optional `?` and type parameters) opens its parameters. A
/// property, a constructor or anything unreadable is None.
fn nest_method_after(s: &str) -> Option<&str> {
    const MODIFIERS: &[&str] = &[
        "public",
        "private",
        "protected",
        "static",
        "async",
        "override",
        "readonly",
        "abstract",
        "declare",
        "get",
        "set",
    ];
    let mut i = skip_decorators(s, 0)?;
    loop {
        let (word, end) = read_ident(s, i)?;
        let mut next = skip_trivia(s, end);
        let follows_ident = |at: usize| {
            s.get(at..)
                .and_then(|r| r.chars().next())
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$' || c == '*')
        };
        if MODIFIERS.contains(&word) && follows_ident(next) {
            i = next;
            if s.get(i..).is_some_and(|r| r.starts_with('*')) {
                i = skip_trivia(s, i + 1);
            }
            continue;
        }
        if s.get(next..).is_some_and(|r| r.starts_with('?')) {
            next = skip_trivia(s, next + 1);
        }
        if s.get(next..).is_some_and(|r| r.starts_with('<')) {
            let close = s
                .get(next + 1..)
                .and_then(|r| find_balanced_close(r, b'<', b'>'))?;
            next = skip_trivia(s, next + 1 + close + 1);
        }
        let opens = s.get(next..).is_some_and(|r| r.starts_with('('));
        return (opens && word != "constructor").then_some(word);
    }
}

/// FH.19: `s` from `i` past whitespace and `//` / `/* */` comments.
fn skip_trivia(s: &str, mut i: usize) -> usize {
    let bytes = s.as_bytes();
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let rest = s.get(i..).unwrap_or("");
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if let Some(body) = rest.strip_prefix("/*") {
            i += 2 + body.find("*/").map_or(body.len(), |e| e + 2);
        } else {
            return i;
        }
    }
}

/// FH.19: `s` from `i` past trivia and every decorator (`@Name`, `@a.b`, each
/// with an optional balanced argument list). None when a decorator's
/// arguments never close.
fn skip_decorators(s: &str, mut i: usize) -> Option<usize> {
    loop {
        i = skip_trivia(s, i);
        if !s.get(i..).is_some_and(|r| r.starts_with('@')) {
            return Some(i);
        }
        let mut end = i + 1;
        while let Some((_, e)) = read_ident(s, end) {
            end = e;
            if !s.get(end..).is_some_and(|r| r.starts_with('.')) {
                break;
            }
            end += 1;
        }
        let next = skip_trivia(s, end);
        i = if s.get(next..).is_some_and(|r| r.starts_with('(')) {
            let close = s
                .get(next + 1..)
                .and_then(|r| find_balanced_close(r, b'(', b')'))?;
            next + 1 + close + 1
        } else {
            end
        };
    }
}

/// FH.19: the identifier starting at `i` (ASCII letters, digits, `_`, `$`;
/// not starting with a digit) and the offset just past it.
fn read_ident(s: &str, i: usize) -> Option<(&str, usize)> {
    let rest = s.get(i..)?;
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(rest.len());
    let word = rest.get(..len)?;
    word.chars()
        .next()
        .is_some_and(|c| !c.is_ascii_digit())
        .then_some((word, i + len))
}

/// FH.3: opens a symbolic path segment: `~const:<symbol>~`, the placeholder
/// a NestJS decorator whose path argument is an identifier or a member
/// expression (`@Controller(RouteKey.User)`) writes into its ROUTE qname,
/// and a template hole over one (`${RouteKey.User}`). The engine's
/// post-cache path fold resolves the symbol through the repo's const table
/// and rewrites the qname; one it cannot resolve stays. `~` is no path
/// parameter in any framework's syntax (the HTTP resolver's normaliser folds
/// `:x`, `{x}`, `<x>`, `[x]`, `*x` and `${..}` to `{}`), so an unresolved
/// placeholder is a literal segment that pairs with nothing.
pub const PATH_SYMBOL_OPEN: &str = "~const:";

/// FH.3: closes a [`PATH_SYMBOL_OPEN`] placeholder.
pub const PATH_SYMBOL_CLOSE: char = '~';

/// FH.3: the placeholder segment for `symbol`.
pub fn path_symbol(symbol: &str) -> String {
    format!("{PATH_SYMBOL_OPEN}{symbol}{PATH_SYMBOL_CLOSE}")
}

/// FH.3: every placeholder `path` holds, in order, as (byte range of the
/// whole placeholder, symbol).
pub fn path_symbols(path: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = path.get(from..).and_then(|r| r.find(PATH_SYMBOL_OPEN)) {
        let start = from + at;
        let body = start + PATH_SYMBOL_OPEN.len();
        let Some(len) = path.get(body..).and_then(|r| r.find(PATH_SYMBOL_CLOSE)) else {
            break;
        };
        let end = body + len + PATH_SYMBOL_CLOSE.len_utf8();
        if let Some(symbol) = path.get(body..body + len).filter(|s| is_path_symbol(s)) {
            out.push((start..end, symbol));
        }
        from = end;
    }
    out
}

/// FH.3: an identifier or a dotted member chain a const table may bind
/// (`PREFIX`, `RouteKey.User`, `Api.v1.users`): never rooted at `this` /
/// `super` / `process`, never a literal keyword, never reading an `env`.
pub fn is_path_symbol(s: &str) -> bool {
    let segs: Vec<&str> = s.split('.').collect();
    let ident = |seg: &&str| {
        let mut c = seg.chars();
        c.next()
            .is_some_and(|f| f.is_ascii_alphabetic() || f == '_' || f == '$')
            && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
    };
    let root = segs.first().copied().unwrap_or("");
    s.len() <= 256
        && segs.iter().all(ident)
        && !matches!(
            root,
            "this" | "super" | "process" | "undefined" | "null" | "true" | "false"
        )
        && !segs.iter().any(|seg| seg.eq_ignore_ascii_case("env"))
}

/// FH.3: the path argument of a NestJS route decorator line. A string
/// literal reads as [`extract_decorator_string`] always has; a backtick
/// template has each `${<symbol>}` hole replaced by its [`path_symbol`]
/// placeholder (any other hole is kept, so the registration is dropped as
/// before); an identifier or member expression ending the argument is one
/// placeholder. Anything else (an options object, no argument) falls back to
/// [`extract_decorator_string`]. FH.3b: `openapi_annot` keys a NestJS
/// `@ApiOperation` through this same rule, so an op and its ROUTE share one
/// placeholder path and the engine's path fold rewrites both.
pub(crate) fn nest_path_arg(line: &str) -> Option<String> {
    let open = line.find('(')?;
    let arg = line.get(open + 1..)?.trim_start();
    match arg.as_bytes().first()? {
        b'`' => extract_decorator_string(line).map(|raw| template_symbols(&raw)),
        c if c.is_ascii_alphabetic() || *c == b'_' || *c == b'$' => {
            let end = arg
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.')))
                .unwrap_or(arg.len());
            let (symbol, rest) = arg.split_at(end);
            let closes = rest.trim_start().starts_with([')', ',']);
            (closes && is_path_symbol(symbol)).then(|| path_symbol(symbol))
        }
        _ => extract_decorator_string(line),
    }
}

/// FH.3: `raw` (a template's body) with every `${<symbol>}` hole replaced by
/// its placeholder; other holes are kept verbatim.
fn template_symbols(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find("${") {
        out.push_str(rest.get(..at).unwrap_or(""));
        let inner = rest.get(at + 2..).unwrap_or("");
        let Some(end) = inner.find('}') else {
            out.push_str(rest.get(at..).unwrap_or(""));
            return out;
        };
        let expr = inner.get(..end).unwrap_or("").trim();
        if is_path_symbol(expr) {
            out.push_str(&path_symbol(expr));
        } else {
            out.push_str(rest.get(at..at + 2 + end + 1).unwrap_or(""));
        }
        rest = inner.get(end + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

pub(crate) fn extract_decorator_string(line: &str) -> Option<String> {
    // Find first quoted literal after the opening paren.
    let open = line.find('(')?;
    let rest = &line[open + 1..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' || c == b'`' {
            let delim = c;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != delim {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                return Some(rest[start..j].to_string());
            }
            return None;
        }
        if c == b')' {
            return None;
        }
        i += 1;
    }
    None
}

pub(crate) fn combine_nest_paths(prefix: &str, suffix: &str) -> String {
    let prefix = prefix.trim_matches('/');
    let suffix = suffix.trim_matches('/');
    let mut out = String::from("/");
    if !prefix.is_empty() {
        out.push_str(prefix);
    }
    if !suffix.is_empty() {
        if !out.ends_with('/') {
            out.push('/');
        }
        out.push_str(suffix);
    }
    // Convert NestJS :param (already :) or express-style. No transform needed;
    // both Nest and Express use :param natively.
    out
}

/// The methods a Next.js route file (App Router `route.ts`, Pages Router
/// `pages/api/*`) or a SvelteKit `+server.ts` serves, each with the 0-based row
/// of its export: a named `export async function M` / `export function M` /
/// `export const M` (the first of the three needles in the file). A file with
/// no named verb export but an `export default` is a Pages Router API route,
/// which Next.js hands EVERY method, so it serves `any` at the default export.
fn nextjs_methods_from_source(source: &str) -> Vec<(&'static str, u32)> {
    let mut methods = Vec::new();
    for method in ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"] {
        let first = [
            format!("export async function {method}"),
            format!("export function {method}"),
            format!("export const {method}"),
        ]
        .iter()
        .filter_map(|needle| source.find(needle.as_str()))
        .min();
        if let Some(at) = first {
            methods.push((method, anchor::line_of(source, at)));
        }
    }
    if methods.is_empty()
        && let Some(at) = source.find("export default")
    {
        methods.push(("any", anchor::line_of(source, at)));
    }
    methods
}

/// Scan for Hapi.js `server.route(...)` registrations. Returns one entry per
/// (method, path) pair, with the 0-based row of its config object's `{`;
/// multiple methods on the same path produce one entry each. Both
/// single-config and array-of-configs are recognised.
fn hapi_routes(source: &str) -> Vec<(&'static str, String, u32)> {
    let mut out = Vec::new();
    let mut search_from = 0;
    // Match both `.route(` (instance) and rare `route: [...]` connection-options
    // shapes. Instance form is dominant.
    while let Some(rel) = source[search_from..].find(".route(") {
        let arg_start = search_from + rel + ".route(".len();
        let Some(close_rel) = find_balanced_close(&source[arg_start..], b'(', b')') else {
            search_from = arg_start;
            continue;
        };
        let body = &source[arg_start..arg_start + close_rel];
        // `body` starts at `arg_start`, so an object offset within it is an
        // absolute offset once `arg_start` is added.
        for (method, path, at) in extract_hapi_configs(body) {
            out.push((method, path, anchor::line_of(source, arg_start + at)));
        }
        search_from = arg_start + close_rel + 1;
    }
    out
}

/// Walk a `server.route(...)` body and pull `{ method, path }` configs, each
/// with the byte offset of its object's `{` within `body`. The body may be a
/// single object, or an array of objects. Methods may be a quoted string or an
/// array of quoted strings (`['GET', 'POST']`).
fn extract_hapi_configs(body: &str) -> Vec<(&'static str, String, usize)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let Some(close) = find_balanced_close(&body[i + 1..], b'{', b'}') else {
                break;
            };
            let obj = &body[i + 1..i + 1 + close];
            let path = obj_string_field(obj, "path");
            let methods = obj_method_field(obj);
            if let Some(p) = path
                && looks_like_url_path(&p)
            {
                for m in methods {
                    out.push((m, p.clone(), i));
                }
            }
            i += 1 + close + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// Validate that an extracted "path" string actually looks like a URL path.
/// The `/`-prefix gate alone isn't enough — Hapi's own source code has
/// `path: relativeTo` references and string literals with embedded code that
/// the substring scanner can grab. URL paths don't contain newlines, parens,
/// braces other than param markers, semicolons, or control characters.
/// Allows `{name}` / `:name` param markers and standard URL chars.
fn looks_like_url_path(p: &str) -> bool {
    if p.is_empty() || p.len() > 256 || !p.starts_with('/') {
        return false;
    }
    p.chars().all(|c| match c {
        // Whitespace + control chars: rejected (real paths URL-encode these).
        '\n' | '\r' | '\t' | ' ' => false,
        c if c.is_ascii_control() => false,
        // Code-shaped chars: rejected.
        '(' | ')' | ';' | '"' | '\'' | '`' | ',' => false,
        _ => true,
    })
}

/// Read a string-valued object field: `path: '/x'` or `path: "/x"` or
/// `"path": '/x'`. Returns the string value (without quotes), or None.
fn obj_string_field(obj: &str, field: &str) -> Option<String> {
    let needles = [
        format!("{field}:"),
        format!("'{field}':"),
        format!("\"{field}\":"),
    ];
    for needle in &needles {
        let Some(idx) = obj.find(needle.as_str()) else {
            continue;
        };
        let after = obj[idx + needle.len()..].trim_start();
        let bytes = after.as_bytes();
        if let Some(&first) = bytes.first()
            && (first == b'\'' || first == b'"' || first == b'`')
        {
            let delim = first;
            let mut j = 1;
            while j < bytes.len() && bytes[j] != delim {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                return Some(after[1..j].to_string());
            }
        }
    }
    None
}

/// Read the `method` field from a Hapi config object. Returns one or more
/// canonical method names (`get`, `post`, ...). Handles both single-string
/// (`method: 'GET'`) and array (`method: ['GET', 'POST']`) forms; the
/// wildcard `'*'` becomes `all`, which `add_method` records as `ANY`.
fn obj_method_field(obj: &str) -> Vec<&'static str> {
    let needles = ["method:", "'method':", "\"method\":"];
    for needle in &needles {
        let Some(idx) = obj.find(*needle) else {
            continue;
        };
        let after = obj[idx + needle.len()..].trim_start();
        let bytes = after.as_bytes();
        if bytes.first() == Some(&b'[') {
            let Some(close) = find_balanced_close(&after[1..], b'[', b']') else {
                return Vec::new();
            };
            let list = &after[1..1 + close];
            return list
                .split(',')
                .filter_map(|tok| {
                    let s = tok
                        .trim()
                        .trim_matches(|c| c == '\'' || c == '"' || c == '`');
                    canonical_http_method(s)
                })
                .collect();
        }
        if let Some(&first) = bytes.first()
            && (first == b'\'' || first == b'"' || first == b'`')
        {
            let delim = first;
            let mut j = 1;
            while j < bytes.len() && bytes[j] != delim {
                j += 1;
            }
            if j < bytes.len()
                && let Some(m) = canonical_http_method(&after[1..j])
            {
                return vec![m];
            }
        }
    }
    Vec::new()
}

fn canonical_http_method(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_uppercase().as_str() {
        "GET" => Some("get"),
        "POST" => Some("post"),
        "PUT" => Some("put"),
        "PATCH" => Some("patch"),
        "DELETE" => Some("delete"),
        "HEAD" => Some("head"),
        "OPTIONS" => Some("options"),
        "*" | "ANY" | "ALL" => Some("all"),
        _ => None,
    }
}

/// Scan for `Bun.serve({ routes: { '/path': { GET: h, POST: h2 }, ... } })`.
/// Each route key is the path; the value object's keys (uppercase HTTP verbs)
/// give the methods. Method-shorthand `GET: handler` and `'/path': handler`
/// (single handler, served for every method) are both handled. Returns
/// (method, path, row) triples, `row` the 0-based line of the route key.
fn bun_serve_routes(source: &str) -> Vec<(&'static str, String, u32)> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("Bun.serve(") {
        let arg_start = search_from + rel + "Bun.serve(".len();
        let Some(close) = find_balanced_close(&source[arg_start..], b'(', b')') else {
            search_from = arg_start;
            continue;
        };
        // Descend into the config-object literal: skip leading whitespace and
        // the opening `{` so `routes:` is at depth 0 within the inner body.
        let arg_body = &source[arg_start..arg_start + close];
        let trimmed = arg_body.trim_start();
        // Absolute offsets are tracked alongside the slices: each trim only
        // drops a prefix, so `len` differences give where a slice starts.
        let config_start = arg_start + (arg_body.len() - trimmed.len()) + 1;
        if !trimmed.starts_with('{') {
            search_from = arg_start + close + 1;
            continue;
        }
        let Some(obj_close) = find_balanced_close(&trimmed[1..], b'{', b'}') else {
            search_from = arg_start + close + 1;
            continue;
        };
        let config_body = &trimmed[1..1 + obj_close];

        let Some(routes_idx) = find_obj_field(config_body, "routes") else {
            search_from = arg_start + close + 1;
            continue;
        };
        let after = config_body[routes_idx..].trim_start();
        let after_start = config_start + (config_body.len() - after.len());
        let bytes = after.as_bytes();
        if bytes.first() != Some(&b'{') {
            search_from = arg_start + close + 1;
            continue;
        }
        let Some(routes_close) = find_balanced_close(&after[1..], b'{', b'}') else {
            search_from = arg_start + close + 1;
            continue;
        };
        let routes_obj = &after[1..1 + routes_close];
        let routes_start = after_start + 1;
        debug_assert_eq!(
            source.get(routes_start..routes_start + routes_obj.len()),
            Some(routes_obj)
        );
        for (method, path, at) in parse_bun_routes_object(routes_obj) {
            out.push((method, path, anchor::line_of(source, routes_start + at)));
        }
        search_from = arg_start + close + 1;
    }
    out
}

/// Find the byte offset just past `field:` (or `'field':` / `"field":`)
/// within an object body, skipping nested braces / strings to avoid false
/// hits inside string values or sub-objects.
fn find_obj_field(obj: &str, field: &str) -> Option<usize> {
    let bytes = obj.as_bytes();
    let needle = format!("{field}:");
    let mut i = 0;
    let mut depth = 0i32;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth -= 1,
            b'\'' | b'"' | b'`' => {
                let delim = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != delim {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => {
                if depth == 0
                    && i + needle.len() <= bytes.len()
                    && &bytes[i..i + needle.len()] == needle.as_bytes()
                {
                    let prev_ok = i == 0 || {
                        let p = bytes[i - 1];
                        !(p.is_ascii_alphanumeric() || p == b'_' || p == b'$')
                    };
                    if prev_ok {
                        return Some(i + needle.len());
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Walk the body of a `routes: { ... }` object, pulling each `'<path>':`
/// key with the byte offset of its opening quote within `body`. Path values
/// may be an object `{ GET: h, POST: h2 }` (its verb keys are the methods), or
/// a function reference / inline handler / `Response` literal, which Bun
/// serves for EVERY method, so the route is `any`.
fn parse_bun_routes_object(body: &str) -> Vec<(&'static str, String, usize)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' || c == b'`' {
            let delim = c;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != delim {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j >= bytes.len() {
                break;
            }
            let key = &body[start..j];
            // Past the closing quote, expect `:`.
            let mut k = j + 1;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                k += 1;
            }
            if k >= bytes.len() || bytes[k] != b':' {
                i = j + 1;
                continue;
            }
            k += 1;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t' || bytes[k] == b'\n') {
                k += 1;
            }
            if !key.starts_with('/') {
                i = k;
                continue;
            }
            // Value: object → enumerate verb keys; otherwise → a single
            // handler or Response, served for every method ⇒ `any`.
            if k < bytes.len() && bytes[k] == b'{' {
                let Some(vclose) = find_balanced_close(&body[k + 1..], b'{', b'}') else {
                    break;
                };
                let val = &body[k + 1..k + 1 + vclose];
                for verb in extract_verb_keys(val) {
                    out.push((verb, key.to_string(), i));
                }
                i = k + 1 + vclose + 1;
                continue;
            }
            out.push(("any", key.to_string(), i));
            i = k;
            continue;
        }
        i += 1;
    }
    out
}

/// Pull verb keys (`GET`, `POST`, ...) from a route-value object body.
fn extract_verb_keys(body: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let verbs = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    let mut i = 0;
    while i < bytes.len() {
        for v in &verbs {
            if i + v.len() < bytes.len()
                && &bytes[i..i + v.len()] == v.as_bytes()
                && bytes[i + v.len()] == b':'
            {
                let prev_ok = i == 0 || {
                    let p = bytes[i - 1];
                    !(p.is_ascii_alphanumeric() || p == b'_' || p == b'$')
                };
                if prev_ok {
                    if let Some(m) = canonical_http_method(v) {
                        out.push(m);
                    }
                    i += v.len();
                    break;
                }
            }
        }
        i += 1;
    }
    out
}

/// Given a slice whose first character matches `open`, find the offset of the
/// matching close character (returned as offset relative to the slice start
/// after `open`, so `&s[..result]` is the inner body). Skips strings.
fn find_balanced_close(s: &str, open: u8, close: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        } else if c == b'\'' || c == b'"' || c == b'`' {
            let delim = c;
            i += 1;
            while i < bytes.len() && bytes[i] != delim {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
        i += 1;
    }
    None
}

// ============================================================================
// GD.42: include-style route mounts, TypeScript / JavaScript
// ============================================================================

/// GD.42: a module-level router value: its framework family (`express`,
/// `koa`, `hono`) and its own prefix (a Koa `new Router({ prefix: '/x' })`,
/// a Hono `new Hono().basePath('/x')`; `""` for any other, or when the
/// prefix is not a string literal).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RouterVar {
    family: &'static str,
    own: String,
}

/// GD.42: what one TS / JS file says about include-style mounts, read by a
/// text scan in the extractor's style.
///
/// - Router values: a declaration at brace depth 0, `[export] const|let|var
///   <x>[: T] = <ctor>`, `<ctor>` one of `express.Router(..)`, `Router(..)`,
///   `express()` (family `express`), `new Router(..)` / `new KoaRouter(..)`
///   (`koa`), `new Hono(..)` / `new Hono<..>(..)` (`hono`). A route
///   registered on one (`<x>.get('/p', ..)`, or a `.get('/p', ..)` line
///   continuing the chain the declaration starts: Hono's `export const files
///   = new Hono().post('/upload', ..).get(..)`) mints the provisional
///   `<VERB> <mount:module:<module>#<x>><own + /p>`. One fact per router,
///   `{ via: "router:<family>", target: Dynamic(<x>), prefix: <own> }`.
/// - Mounts: `<x>.use(['/p',] <ref>..)` and `<x>.route('/p', <ref>)`, each
///   `<ref>` argument a router variable or an imported name (`Import`), a
///   module attribute (`users.router`, `Import`), `require('<spec>')`
///   (`Spec`) or a Koa `<sub>.routes()` (`Import` of `<sub>`); any other
///   argument (a call, an inline function) is skipped, and a prefix holding
///   `*` (a middleware glob, `app.use('/api/v2/*', cors())`) or `${`
///   records nothing. `{ via: "<express|koa|hono>:<x>", prefix: <x's own> +
///   '/p' }`; the graph pass decides whether the target is a router (a
///   middleware's holds no route slot and mounts nothing).
/// - Exports: `module.exports = <x>` / `export default <x>` of a router
///   variable, `{ via: "export_default", target: Dynamic(<x>) }`, so an
///   importer's default or `require` binding names it.
/// - Path lists: a depth-0 `[export] const <X>[: T] = ['/a', '/b']` whose
///   every element is a string literal starting with `/`, one `{ via:
///   "path_list:<X>", target: Spec(<path>) }` per element (a NestJS
///   `exclude` may name one from another file).
#[derive(Debug, Default)]
struct MountScan {
    routers: BTreeMap<String, RouterVar>,
    /// Per 0-based row, the router variable whose declaration's statement
    /// is still open at the row's start (depth 0 again only on a line that
    /// continues the chain with `.`), with the declaration's own row.
    chain_at: Vec<Option<String>>,
    facts: Vec<NavFact>,
}

impl MountScan {
    fn read(source: &str) -> MountScan {
        let mut scan = MountScan::default();
        let depths = line_depths(source);
        let starts = line_starts(source);
        let mut chain: Option<String> = None;
        for (row, line) in source.split('\n').enumerate() {
            let t = line.trim();
            let depth = depths.get(row).copied().unwrap_or(0);
            if depth == 0 {
                let rest = starts
                    .get(row)
                    .and_then(|&at| source.get(at..))
                    .unwrap_or("");
                if let Some((name, rv)) = router_decl(t, rest) {
                    scan.facts.push(NavFact::IncludeMount {
                        line: row_u32(row),
                        target: IncludeTarget::Dynamic(name.to_string()),
                        prefix: rv.own.clone(),
                        via: format!("router:{}", rv.family),
                    });
                    scan.routers.insert(name.to_string(), rv);
                    chain = Some(name.to_string());
                } else if let Some(list) = path_list_decl(t, rest) {
                    chain = None;
                    let (name, paths) = list;
                    for p in paths {
                        scan.facts.push(NavFact::IncludeMount {
                            line: row_u32(row),
                            target: IncludeTarget::Spec(p),
                            prefix: String::new(),
                            via: format!("path_list:{name}"),
                        });
                    }
                } else if !(t.is_empty()
                    || t.starts_with('.')
                    || t.starts_with("//")
                    || t.starts_with("/*")
                    || t.starts_with('*'))
                {
                    chain = None;
                }
            }
            scan.chain_at.push(chain.clone());
        }
        scan.read_mounts(source, &starts);
        scan.read_exports(source, &depths);
        scan
    }

    /// The router variable a verb call at `idx` of trimmed line `t` (0-based
    /// `row`) registers on: its bare receiver when that is one, else, with no
    /// receiver (`.post(` opening the line, or right after a call's `)`),
    /// the chain open at the row.
    fn receiver_at(&self, t: &str, idx: usize, row: usize) -> Option<&str> {
        if let Some(r) = bare_receiver(t, idx) {
            return self.routers.get_key_value(r).map(|(k, _)| k.as_str());
        }
        let before = t.get(..idx).unwrap_or("").trim_end();
        if before.is_empty() || before.ends_with(')') {
            return self.chain_at.get(row).and_then(|c| c.as_deref());
        }
        None
    }

    /// Every `<x>.use(..)` / `<x>.route(..)` mount site (see the type doc).
    fn read_mounts(&mut self, source: &str, starts: &[usize]) {
        for (needle, hono) in [(".use(", false), (".route(", true)] {
            let mut from = 0;
            while let Some(found) = source.get(from..).and_then(|r| r.find(needle)) {
                let dot = from + found;
                from = dot + needle.len();
                let row = starts.partition_point(|&s| s <= dot).saturating_sub(1);
                let line_start = starts.get(row).copied().unwrap_or(0);
                let includer = match bare_receiver(source.get(..dot).unwrap_or(""), dot) {
                    Some(r) => r.to_string(),
                    None => {
                        let before = source.get(line_start..dot).unwrap_or("").trim();
                        match self.chain_at.get(row).and_then(|c| c.clone()) {
                            Some(c) if before.is_empty() || before.ends_with(')') => c,
                            _ => continue,
                        }
                    }
                };
                let Some(close) = source
                    .get(from..)
                    .and_then(|r| find_balanced_close(r, b'(', b')'))
                else {
                    continue;
                };
                let args = split_top_level(source.get(from..from + close).unwrap_or(""));
                let (prefix, refs) = match args.first().and_then(|a| string_literal(a)) {
                    Some(p) => (p, &args[1..]),
                    None if hono => continue,
                    None => (String::new(), &args[..]),
                };
                if prefix.contains('*') || prefix.contains("${") || refs.is_empty() {
                    continue;
                }
                let own = self
                    .routers
                    .get(&includer)
                    .map(|r| r.own.clone())
                    .unwrap_or_default();
                let prefix = endpoint::join_path(&own, &prefix);
                for arg in refs {
                    let Some((target, koa)) = mount_ref(arg) else {
                        continue;
                    };
                    let family = if hono {
                        "hono"
                    } else if koa {
                        "koa"
                    } else {
                        "express"
                    };
                    self.facts.push(NavFact::IncludeMount {
                        line: row_u32(row),
                        target,
                        prefix: prefix.clone(),
                        via: format!("{family}:{includer}"),
                    });
                }
            }
        }
    }

    /// `module.exports = <x>` / `export default <x>` of a router variable,
    /// at depth 0.
    fn read_exports(&mut self, source: &str, depths: &[i32]) {
        for (row, line) in source.split('\n').enumerate() {
            if depths.get(row).copied().unwrap_or(0) != 0 {
                continue;
            }
            let t = line.trim().trim_end_matches(';').trim_end();
            let Some(name) = t
                .strip_prefix("module.exports")
                .and_then(|r| r.trim_start().strip_prefix('='))
                .or_else(|| t.strip_prefix("export default "))
                .map(str::trim)
            else {
                continue;
            };
            if self.routers.contains_key(name) {
                self.facts.push(NavFact::IncludeMount {
                    line: row_u32(row),
                    target: IncludeTarget::Dynamic(name.to_string()),
                    prefix: String::new(),
                    via: "export_default".to_string(),
                });
            }
        }
    }
}

fn row_u32(row: usize) -> u32 {
    u32::try_from(row).unwrap_or(u32::MAX)
}

/// Byte offset of each line's start (`'\n'`-split, so row `i` of
/// `source.split('\n')`).
fn line_starts(source: &str) -> Vec<usize> {
    let mut out = vec![0];
    out.extend(
        source
            .bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i + 1),
    );
    out
}

/// GD.42: the bracket depth (`(`, `[`, `{` together) at the start of each
/// `'\n'`-split line, skipping string literals, comments and template text
/// (a `${..}` hole counts as code). Never below 0. A regex literal holding a
/// bracket can skew it; the scan it serves only needs depth 0 at top-level
/// statements.
fn line_depths(source: &str) -> Vec<i32> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Code,
        Line,
        Block,
        Str(u8),
        Tmpl,
    }
    let b = source.as_bytes();
    let mut out = vec![0];
    let mut depth = 0i32;
    // Each open `${` hole: the depth it was opened at.
    let mut holes: Vec<i32> = Vec::new();
    let mut state = State::Code;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == b'\n' {
            if matches!(state, State::Line | State::Str(_)) {
                state = State::Code;
            }
            out.push(depth.max(0));
            i += 1;
            continue;
        }
        match state {
            State::Line => {}
            State::Block => {
                if c == b'*' && next == Some(b'/') {
                    state = State::Code;
                    i += 1;
                }
            }
            State::Str(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    state = State::Code;
                }
            }
            State::Tmpl => {
                if c == b'\\' {
                    i += 1;
                } else if c == b'`' {
                    state = State::Code;
                } else if c == b'$' && next == Some(b'{') {
                    holes.push(depth);
                    depth += 1;
                    state = State::Code;
                    i += 1;
                }
            }
            State::Code => match c {
                b'/' if next == Some(b'/') => state = State::Line,
                b'/' if next == Some(b'*') => {
                    state = State::Block;
                    i += 1;
                }
                b'\'' | b'"' => state = State::Str(c),
                b'`' => state = State::Tmpl,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' => depth -= 1,
                b'}' => {
                    depth -= 1;
                    if holes.last() == Some(&depth) {
                        holes.pop();
                        state = State::Tmpl;
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    out
}

/// GD.42: the identifier right before the `.` at byte `dot` of `s` when it
/// is a bare variable (not itself a member `a.b.use`, nor `a?.use`).
fn bare_receiver(s: &str, dot: usize) -> Option<&str> {
    let ident = endpoint::ident_before(s, dot);
    if ident.is_empty() || ident.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let start = s.get(..dot)?.trim_end().len() - ident.len();
    // On the receiver's own line: a comment ending the line above in `.`
    // says nothing about it.
    let before = s.get(..start)?;
    let before = before.rsplit('\n').next().unwrap_or(before).trim_end();
    if before.ends_with('.') || before.ends_with('?') {
        return None;
    }
    Some(ident)
}

/// GD.42: a router declaration at a trimmed depth-0 line `t` (see
/// [`MountScan`]): the variable and its [`RouterVar`]. `rest` is the source
/// from the line's start, for a constructor's arguments spanning lines.
fn router_decl<'a>(t: &'a str, rest: &str) -> Option<(&'a str, RouterVar)> {
    let (name, rhs) = const_decl(t)?;
    let (family, ctor_len) = [
        ("express.Router(", "express"),
        ("Router(", "express"),
        ("express(", "express"),
        ("new Router(", "koa"),
        ("new KoaRouter(", "koa"),
        ("new Hono(", "hono"),
        ("new Hono<", "hono"),
    ]
    .iter()
    .find(|(ctor, _)| rhs.starts_with(ctor))
    .map(|(ctor, family)| (*family, ctor.len()))?;
    // The constructor's `(` in `rest` (past a Hono `<..>`).
    let rhs_at = rest.find(rhs)?;
    let mut open = rhs_at + ctor_len - 1;
    if rest.as_bytes().get(open) == Some(&b'<') {
        let close = rest
            .get(open + 1..)
            .and_then(|r| find_balanced_close(r, b'<', b'>'))?;
        open = open + 1 + close + 1;
        if rest.as_bytes().get(open) != Some(&b'(') {
            return None;
        }
    }
    let close = rest
        .get(open + 1..)
        .and_then(|r| find_balanced_close(r, b'(', b')'))?;
    let args = rest.get(open + 1..open + 1 + close).unwrap_or("");
    let after = rest.get(open + 1 + close + 1..).unwrap_or("");
    let own = match family {
        "koa" => obj_string_field(args, "prefix").unwrap_or_default(),
        "hono" => after
            .trim_start()
            .strip_prefix(".basePath(")
            .and_then(|r| {
                let close = find_balanced_close(r, b'(', b')')?;
                string_literal(r.get(..close)?.trim())
            })
            .unwrap_or_default(),
        _ => String::new(),
    };
    let own = if own.contains("${") {
        String::new()
    } else {
        own
    };
    Some((name, RouterVar { family, own }))
}

/// `[export] const|let|var <name>[: T] = <rhs>` -> (name, rhs trimmed).
fn const_decl(t: &str) -> Option<(&str, &str)> {
    let t = t.strip_prefix("export ").map_or(t, str::trim_start);
    let t = ["const ", "let ", "var "]
        .iter()
        .find_map(|kw| t.strip_prefix(kw))?
        .trim_start();
    let (name, end) = read_ident(t, 0)?;
    let rest = t.get(end..)?;
    // The first `=` that is no `==` / `=>` (a type annotation before it).
    let bytes = rest.as_bytes();
    let eq = (0..bytes.len()).find(|&i| {
        bytes[i] == b'='
            && bytes.get(i + 1) != Some(&b'=')
            && bytes.get(i + 1) != Some(&b'>')
            && (i == 0 || !matches!(bytes[i - 1], b'=' | b'!' | b'<' | b'>'))
    })?;
    let annotation = rest.get(..eq)?.trim();
    if !(annotation.is_empty() || annotation.starts_with(':')) {
        return None;
    }
    Some((name, rest.get(eq + 1..)?.trim()))
}

/// GD.42: a depth-0 `[export] const <X>[: T] = [..]` whose every element is
/// a string literal starting with `/` (at least one): `(X, paths)`.
fn path_list_decl<'a>(t: &'a str, rest: &str) -> Option<(&'a str, Vec<String>)> {
    let (name, rhs) = const_decl(t)?;
    if !rhs.starts_with('[') {
        return None;
    }
    let open = rest.find(rhs)?;
    let close = rest
        .get(open + 1..)
        .and_then(|r| find_balanced_close(r, b'[', b']'))?;
    let body = rest.get(open + 1..open + 1 + close)?;
    let mut paths = Vec::new();
    for item in split_top_level(body) {
        let p = string_literal(&item)?;
        if !p.starts_with('/') || p.contains("${") {
            return None;
        }
        paths.push(p);
    }
    (!paths.is_empty()).then_some((name, paths))
}

/// GD.42: `s`'s comma-separated items at depth 0 (brackets and strings
/// skipped), each trimmed, empty ones (a trailing comma) dropped.
fn split_top_level(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut start, mut i) = (0i32, 0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'\'' | b'"' | b'`' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b',' if depth == 0 => {
                out.push(s.get(start..i).unwrap_or("").trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(s.get(start..).unwrap_or("").trim().to_string());
    out.retain(|a| !a.is_empty());
    out
}

/// GD.42: the text of `s` when it is exactly one string literal (`'a'`,
/// `"a"`, or a template with no `${` hole).
fn string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    let q = *s.as_bytes().first()?;
    if !matches!(q, b'\'' | b'"' | b'`') || s.len() < 2 || s.as_bytes()[s.len() - 1] != q {
        return None;
    }
    let inner = s.get(1..s.len() - 1)?;
    if inner.contains(q as char) || (q == b'`' && inner.contains("${")) {
        return None;
    }
    Some(inner.to_string())
}

/// GD.42: what one mount argument names: `require('<spec>')` a Spec; a
/// Koa `<sub>.routes()` an Import of `<sub>` (`true`: the Koa form); an
/// identifier or a one-level member (`users.router`) an Import. None for
/// anything else.
fn mount_ref(arg: &str) -> Option<(IncludeTarget, bool)> {
    let a = arg.trim();
    if let Some(inner) = a.strip_prefix("require(").and_then(|r| r.strip_suffix(')')) {
        return string_literal(inner).map(|spec| (IncludeTarget::Spec(spec), false));
    }
    let (name, koa) = match a.strip_suffix(".routes()") {
        Some(sub) => (sub.trim(), true),
        None => (a, false),
    };
    let is_ident = |s: &str| {
        !s.is_empty()
            && !s.as_bytes()[0].is_ascii_digit()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
    };
    let ok = match name.split_once('.') {
        Some((obj, member)) => is_ident(obj) && is_ident(member),
        None => is_ident(name),
    };
    ok.then(|| {
        (
            IncludeTarget::Import {
                local: name.to_string(),
            },
            koa,
        )
    })
}

/// GD.42: the NestJS mount facts of a file, recorded on its MODULE:
///
/// - per controller route the scan read (`nest`), `{ line: <its row>, via:
///   "nest_route", target: Dynamic(<controller class>) }`, and once per
///   controller class `{ via: "router:nest", target: Dynamic(<class>) }`;
/// - per `@Module({ controllers: [A, B] })`, `{ via: "router:nest_module",
///   target: Dynamic(<module class>) }` and one `{ via:
///   "nest_module:<module class>", target: Import(A) }` per controller;
/// - per `RouterModule.register([{ path, module, children }])` entry, `{
///   via: "nest_router", target: Import(<module>), prefix: <the entry's
///   path under its parents'> }` (a bare module class among `children`
///   takes its parent's path);
/// - per `app.setGlobalPrefix('<p>'[, { exclude: [..] }])`, `{ via:
///   "nest_global", prefix: <p> }`, then per excluded literal path `{ via:
///   "nest_exclude:<METHOD|ANY>", target: Spec(<path>) }` (a `{ path,
///   method: RequestMethod.X }` object, or a string) and per excluded
///   identifier `{ via: "nest_exclude_ref", target: Import(<X>) }` (a
///   [`MountScan`] path list, here or imported).
fn nest_mount_facts(source: &str, nest: &[NestRoute]) -> Vec<NavFact> {
    let mut out = Vec::new();
    let mut classes: Vec<&str> = Vec::new();
    for r in nest {
        if let Some(c) = &r.class
            && !classes.contains(&c.as_str())
        {
            classes.push(c);
        }
    }
    for c in &classes {
        out.push(NavFact::IncludeMount {
            line: nest
                .iter()
                .find(|r| r.class.as_deref() == Some(c))
                .map_or(0, |r| r.row),
            target: IncludeTarget::Dynamic((*c).to_string()),
            prefix: String::new(),
            via: "router:nest".to_string(),
        });
    }
    for r in nest {
        if let Some(c) = &r.class {
            out.push(NavFact::IncludeMount {
                line: r.row,
                target: IncludeTarget::Dynamic(c.clone()),
                prefix: String::new(),
                via: "nest_route".to_string(),
            });
        }
    }
    let starts = line_starts(source);
    let row_of = |at: usize| row_u32(starts.partition_point(|&s| s <= at).saturating_sub(1));
    for (at, args) in call_args(source, "@Module(") {
        // The class the decorator decorates: the first class declaration in
        // the lines after the call's `)` (its own line's rest included).
        let after = source
            .get(at + "@Module(".len() + args.len() + 1..)
            .unwrap_or("");
        let Some(module) = after
            .lines()
            .take(8)
            .find_map(|l| class_decl_name(l.trim()))
        else {
            continue;
        };
        let row = row_of(at);
        out.push(NavFact::IncludeMount {
            line: row,
            target: IncludeTarget::Dynamic(module.to_string()),
            prefix: String::new(),
            via: "router:nest_module".to_string(),
        });
        let obj = args.trim();
        let Some(body) = obj.strip_prefix('{').and_then(|r| r.strip_suffix('}')) else {
            continue;
        };
        let Some(list) = field_value(body, "controllers").and_then(array_items) else {
            continue;
        };
        for c in list {
            if let Some((IncludeTarget::Import { local }, false)) = mount_ref(&c)
                && !local.contains('.')
            {
                out.push(NavFact::IncludeMount {
                    line: row,
                    target: IncludeTarget::Import { local },
                    prefix: String::new(),
                    via: format!("nest_module:{module}"),
                });
            }
        }
    }
    for (at, args) in call_args(source, "RouterModule.register(") {
        if let Some(items) = array_items(args.trim()) {
            route_tree(&items, "", row_of(at), &mut out);
        }
    }
    for (at, args) in call_args(source, ".setGlobalPrefix(") {
        let items = split_top_level(args);
        let Some(prefix) = items.first().and_then(|a| string_literal(a)) else {
            continue;
        };
        let row = row_of(at);
        out.push(NavFact::IncludeMount {
            line: row,
            target: IncludeTarget::Dynamic(String::new()),
            prefix,
            via: "nest_global".to_string(),
        });
        let Some(body) = items
            .get(1)
            .and_then(|o| o.strip_prefix('{'))
            .and_then(|r| r.strip_suffix('}'))
        else {
            continue;
        };
        let Some(exclude) = field_value(body, "exclude") else {
            continue;
        };
        let entries = match array_items(exclude) {
            Some(list) => list,
            None => vec![exclude.to_string()],
        };
        for e in entries {
            let (method, path) = if let Some(p) = string_literal(&e) {
                ("ANY".to_string(), p)
            } else if let Some(inner) = e.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
                let Some(p) = field_value(inner, "path").and_then(string_literal) else {
                    continue;
                };
                let method = field_value(inner, "method")
                    .and_then(|m| m.rsplit('.').next())
                    .and_then(method_token)
                    .unwrap_or(ANY);
                (method.to_string(), p)
            } else {
                if let Some((IncludeTarget::Import { local }, false)) = mount_ref(&e) {
                    out.push(NavFact::IncludeMount {
                        line: row,
                        target: IncludeTarget::Import { local },
                        prefix: String::new(),
                        via: "nest_exclude_ref".to_string(),
                    });
                }
                continue;
            };
            out.push(NavFact::IncludeMount {
                line: row,
                target: IncludeTarget::Spec(path),
                prefix: String::new(),
                via: format!("nest_exclude:{method}"),
            });
        }
    }
    out
}

/// GD.42: each call of `needle` (ending in `(`) in `source`: the needle's
/// byte offset and the argument text up to its balanced `)`.
fn call_args<'s>(source: &'s str, needle: &str) -> Vec<(usize, &'s str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = source.get(from..).and_then(|r| r.find(needle)) {
        let at = from + found;
        let open = at + needle.len();
        from = open;
        if let Some(close) = source
            .get(open..)
            .and_then(|r| find_balanced_close(r, b'(', b')'))
        {
            out.push((at, source.get(open..open + close).unwrap_or("")));
        }
    }
    out
}

/// GD.42: the value text of top-level field `field` of an object body,
/// up to the next depth-0 comma, trimmed.
fn field_value<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let at = find_obj_field(body, field)?;
    let rest = body.get(at..)?;
    let bytes = rest.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'\'' | b'"' | b'`' => {
                let q = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b',' if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    Some(rest.get(..i.min(rest.len()))?.trim())
}

/// GD.42: the items of an array literal `[a, b]`; None when `s` is no
/// array literal.
fn array_items(s: &str) -> Option<Vec<String>> {
    let s = s.trim();
    let inner = s.strip_prefix('[')?;
    let close = find_balanced_close(inner, b'[', b']')?;
    Some(split_top_level(inner.get(..close)?))
}

/// GD.42: the `nest_router` facts of one `RouterModule.register` array
/// level, each entry's path under `parent`.
fn route_tree(items: &[String], parent: &str, row: u32, out: &mut Vec<NavFact>) {
    for item in items {
        let push = |module: &str, path: &str, out: &mut Vec<NavFact>| {
            if let Some((IncludeTarget::Import { local }, false)) = mount_ref(module)
                && !local.contains('.')
            {
                out.push(NavFact::IncludeMount {
                    line: row,
                    target: IncludeTarget::Import { local },
                    prefix: path.to_string(),
                    via: "nest_router".to_string(),
                });
            }
        };
        let Some(body) = item.strip_prefix('{').and_then(|r| r.strip_suffix('}')) else {
            push(item, parent, out);
            continue;
        };
        let path = match field_value(body, "path") {
            Some(v) => match string_literal(v) {
                Some(p) => endpoint::join_path(parent, &p),
                None => continue,
            },
            None => parent.to_string(),
        };
        if let Some(module) = field_value(body, "module") {
            push(module, &path, out);
        }
        if let Some(children) = field_value(body, "children").and_then(array_items) {
            route_tree(&children, &path, row, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    /// Every cell payload of `n`, as `(cell type, text)`.
    fn cells(n: &Node) -> Vec<(glia_core::CellTypeId, String)> {
        n.cells
            .iter()
            .map(|c| match &c.payload {
                CellPayload::Json(s) | CellPayload::Text(s) => (c.kind, s.clone()),
                CellPayload::Bytes(_) => (c.kind, String::new()),
            })
            .collect()
    }

    /// Every ROUTE qname the extraction recorded, sorted.
    fn qnames(r: &RouteNodes) -> Vec<String> {
        let mut q: Vec<String> = r.nav.qname_by_id.values().cloned().collect();
        q.sort_unstable();
        q
    }

    /// The node recorded under `qname`.
    fn node<'a>(r: &'a RouteNodes, qname: &str) -> &'a Node {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, qname);
        r.nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("no ROUTE `{qname}` in {:?}", qnames(r)))
    }

    /// The POSITION `start_line`s on `n`, in cell order.
    fn position_rows(n: &Node) -> Vec<u32> {
        cells(n)
            .into_iter()
            .filter(|(k, _)| *k == cell_type::POSITION)
            .filter_map(|(_, s)| {
                let v: serde_json::Value = serde_json::from_str(&s).ok()?;
                u32::try_from(v["start_line"].as_u64()?).ok()
            })
            .collect()
    }

    #[test]
    fn detects_express_get() {
        let src = "app.get('/users/:id', handler);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(qnames(&r), vec!["GET /users/:id"]);
        // LB.11b: the name is the qname, like every other server parser.
        let id = r.nodes[0].id;
        assert_eq!(
            r.nav.name_by_id.get(&id).map(String::as_str),
            Some("GET /users/:id")
        );
        let c = cells(&r.nodes[0]);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(
            c[0],
            (
                cell_type::POSITION,
                r#"{"file":"server.ts","start_line":0,"end_line":0}"#.to_string()
            )
        );
        assert_eq!(c[1].0, cell_type::ROUTE_METHOD);
        assert_eq!(
            c[1].1,
            r#"{"method":"GET","handler":"handler","file":"server.ts","line":1,"col":0}"#
        );
        assert_eq!((r.positioned, r.any), (1, 0));
    }

    #[test]
    fn detects_router_post() {
        let src = "router.post('/widgets', createWidget);";
        let r = extract_ts_backend_routes(src, "routes.ts", module_id(), "test", repo());
        assert_eq!(r.nodes.len(), 1);
    }

    #[test]
    fn rejects_fetch_call() {
        let src = "fetch('/api/widgets').then(r => r.json());";
        let r = extract_ts_backend_routes(src, "client.ts", module_id(), "test", repo());
        assert_eq!(r.nodes.len(), 0);
    }

    /// A Pages Router API route's default export is handed EVERY method by
    /// Next.js, so it is `ANY`, located at the `export default`.
    #[test]
    fn detects_nextjs_pages_router() {
        let src = "// users\nexport default function handler(req, res) { res.json({}); }";
        let r = extract_ts_backend_routes(src, "pages/api/users.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["ANY /api/users"]);
        assert_eq!(position_rows(node(&r, "ANY /api/users")), vec![1]);
        assert_eq!(route_methods(&r, "/api/users"), vec!["ANY"]);
        assert_eq!(r.any, 1);
    }

    /// App Router named exports: one node per method, one ROUTE_METHOD each,
    /// each located at its own export.
    #[test]
    fn detects_nextjs_app_router_named_exports() {
        let src = "export async function GET() {}\nexport async function POST() {}";
        let r =
            extract_ts_backend_routes(src, "app/api/widgets/route.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["GET /api/widgets", "POST /api/widgets"]);
        for (q, row) in [("GET /api/widgets", 0), ("POST /api/widgets", 1)] {
            let n = node(&r, q);
            assert_eq!(position_rows(n), vec![row], "{q}");
            let methods = cells(n)
                .into_iter()
                .filter(|(k, _)| *k == cell_type::ROUTE_METHOD)
                .count();
            assert_eq!(methods, 1, "{q}");
        }
        assert_eq!((r.positioned, r.any), (2, 0));
    }

    #[test]
    fn converts_nextjs_dynamic_segment() {
        let src = "export default function h() {}";
        let r =
            extract_ts_backend_routes(src, "pages/api/users/[id].ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["ANY /api/users/:id"]);
    }

    #[test]
    fn detects_nestjs_controller_and_methods() {
        let src = r#"
@Controller('users')
export class UsersController {
  @Get()
  list() {}

  @Get(':id')
  getOne() {}

  @Post()
  create() {}

  @Put(':id')
  update() {}

  @Delete(':id')
  destroy() {}
}
"#;
        let r =
            extract_ts_backend_routes(src, "src/users.controller.ts", module_id(), "test", repo());
        assert_eq!(
            qnames(&r),
            vec![
                "DELETE /users/:id",
                "GET /users",
                "GET /users/:id",
                "POST /users",
                "PUT /users/:id"
            ]
        );
        // Each located at its decorator's 0-based row.
        for (q, row) in [
            ("GET /users", 3),
            ("GET /users/:id", 6),
            ("POST /users", 9),
            ("PUT /users/:id", 12),
            ("DELETE /users/:id", 15),
        ] {
            assert_eq!(position_rows(node(&r, q)), vec![row], "{q}");
        }
    }

    /// FH.3: a decorator path held in an enum member, an object const or a
    /// template over them is kept as a placeholder segment, never dropped.
    #[test]
    fn nestjs_symbolic_paths_mint_placeholders() {
        let src = "@Controller(RouteKey.User)\nexport class U {\n  @Put('me')\n  me() {}\n\n  @Get(`${RouteKey.Album}/shared`)\n  shared() {}\n\n  @Get(`${this.base}/x`)\n  dropped() {}\n\n  @Delete(ROUTE_ID, { x: 1 })\n  del() {}\n}\n@Controller({ path: 'opts' })\nexport class O {\n  @Get()\n  list() {}\n}\n";
        let r = extract_ts_backend_routes(src, "src/u.controller.ts", module_id(), "test", repo());
        assert_eq!(
            qnames(&r),
            vec![
                "DELETE /~const:RouteKey.User~/~const:ROUTE_ID~",
                "GET /opts",
                "GET /~const:RouteKey.User~/~const:RouteKey.Album~/shared",
                "PUT /~const:RouteKey.User~/me",
            ]
        );
        assert_eq!(
            path_symbols("/~const:RouteKey.User~/~const:A~/me"),
            vec![(1..22, "RouteKey.User"), (23..32, "A")]
        );
        assert!(path_symbols("/~const:this.x~/me").is_empty());
        assert!(!is_path_symbol("process.env.BASE"));
    }

    /// FH.19: each Nest route is HANDLED_BY the method its verb decorator
    /// decorates, read past further decorators (multi-line arguments
    /// included), comments, modifiers and multi-line parameter decorators.
    #[test]
    fn nestjs_routes_are_handled_by_their_decorated_method() {
        let src = "import { Controller, Get, Put } from '@nestjs/common';\n\n@Controller('users')\nexport class UsersController {\n  @Get(':id')\n  getUser(@Param('id') id: string) {\n    return { id };\n  }\n\n  @Put('me')\n  @UseGuards(AuthGuard)\n  @ApiOperation({\n    summary: 'update (me)',\n  })\n  // the caller's own profile\n  public async updateMe(\n    @Body() body: Dto,\n  ): Promise<Dto> {\n    return body;\n  }\n}\n";
        let r = extract_ts_backend_routes(
            src,
            "src/users/users.controller.ts",
            module_id(),
            "test",
            repo(),
        );
        assert_eq!(qnames(&r), vec!["GET /users/:id", "PUT /users/me"]);
        let attr = |base: &str, name: &str| CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        };
        assert_eq!(
            handled_by(&r),
            vec![
                (
                    "GET /users/:id".to_string(),
                    attr("UsersController", "getUser")
                ),
                (
                    "PUT /users/me".to_string(),
                    attr("UsersController", "updateMe")
                ),
            ]
        );
        // The ref's site line is the decorator's row.
        let lines: Vec<u32> = r.refs.iter().map(|x| x.line).collect();
        assert_eq!(lines, vec![4, 9]);
    }

    /// FH.19: a method carrying several verb decorators is the handler of
    /// every ROUTE they mint; a same-line decorator, `static` / `async *`
    /// modifiers and type parameters still read the method; and the class is
    /// the last one declared before the decorator.
    #[test]
    fn nestjs_handler_shapes() {
        let src = "class Helper {}\n@Controller('a') export class AController {\n  @Get('x')\n  @Post('x')\n  both() {}\n  @Get('one') one() {}\n  @Delete('gen')\n  async generic<T>(t: T) {}\n  @Patch('opt')\n  maybe?(): void {}\n}\n@Controller('b')\nexport default abstract class BController {\n  @Get()\n  static async *stream() {}\n}\n";
        let r = extract_ts_backend_routes(src, "src/a.controller.ts", module_id(), "test", repo());
        // Refs come in (path, METHOD) key order.
        let got: Vec<(String, String)> = handled_by(&r)
            .into_iter()
            .map(|(q, h)| match h {
                CallQualifier::Attribute { base, name } => (q, format!("{base}.{name}")),
                other => (q, format!("{other:?}")),
            })
            .collect();
        let want: Vec<(String, String)> = [
            ("DELETE /a/gen", "AController.generic"),
            ("GET /a/one", "AController.one"),
            ("PATCH /a/opt", "AController.maybe"),
            ("GET /a/x", "AController.both"),
            ("POST /a/x", "AController.both"),
            ("GET /b", "BController.stream"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(got, want);
    }

    /// FH.19: no ref when the decorator decorates no readable method (a
    /// property) or no class declaration precedes it; the route is still
    /// minted.
    #[test]
    fn nestjs_unreadable_handler_emits_route_without_ref() {
        let src = "@Get('prop')\nhandler = () => 1;\n@Controller('c')\nexport class C {\n  @Get('field')\n  field = 1;\n}\n";
        let r = extract_ts_backend_routes(src, "src/c.controller.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["GET /c/field", "GET /prop"]);
        assert!(r.refs.is_empty(), "no handler ref: {:?}", handled_by(&r));
        assert_eq!(nest_method_after("\n  constructor(x: X) {}"), None);
        assert_eq!(class_decl_name("// a class Foo"), None);
        assert_eq!(class_decl_name("export default class {"), None);
    }

    /// Nest `@All(` is method-agnostic: `ANY`, never `ALL`.
    #[test]
    fn nestjs_all_is_any() {
        let src = "@Controller('health')\nexport class H {\n  @All()\n  check() {}\n}\n";
        let r =
            extract_ts_backend_routes(src, "src/health.controller.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["ANY /health"]);
        assert_eq!(position_rows(node(&r, "ANY /health")), vec![2]);
    }

    #[test]
    fn detects_sveltekit_plus_server() {
        let src = "export async function GET() {}\nexport async function POST() {}";
        let r = extract_ts_backend_routes(
            src,
            "src/routes/api/widgets/+server.ts",
            module_id(),
            "test",
            repo(),
        );
        assert_eq!(qnames(&r), vec!["GET /api/widgets", "POST /api/widgets"]);
        assert_eq!(position_rows(node(&r, "POST /api/widgets")), vec![1]);
    }

    #[test]
    fn sveltekit_dynamic_segment() {
        let src = "export function GET() {}";
        let r = extract_ts_backend_routes(
            src,
            "src/routes/api/users/[id]/+server.ts",
            module_id(),
            "test",
            repo(),
        );
        assert_eq!(qnames(&r), vec!["GET /api/users/:id"]);
    }

    /// LB.5 — `add_method` keys routes on the canonical path, so a relative
    /// and a slashed spelling of one (method, path) are one entry (and so one
    /// `<METHOD> /…` node); LB.11b — a second method on the path is its own
    /// entry, and the method token is upper-cased (`all` -> `ANY`).
    #[test]
    fn add_method_keys_on_the_canonical_path_and_method() {
        let mut m: RouteMap = BTreeMap::new();
        assert_eq!(
            add_method(&mut m, "users", "get", "listUsers", 0),
            Some(("/users".into(), "GET"))
        );
        assert_eq!(
            add_method(&mut m, "/users", "get", "", 3),
            Some(("/users".into(), "GET"))
        );
        assert_eq!(
            add_method(&mut m, "/users", "post", "", 1),
            Some(("/users".into(), "POST"))
        );
        assert_eq!(
            add_method(&mut m, "/users", "all", "", 2),
            Some(("/users".into(), "ANY"))
        );
        assert_eq!(add_method(&mut m, "/${x}", "get", "", 2), None);
        assert_eq!(add_method(&mut m, "/users", "trace", "", 2), None);
        assert_eq!(
            m.keys().cloned().collect::<Vec<_>>(),
            vec![
                ("/users".to_string(), "ANY"),
                ("/users".to_string(), "GET"),
                ("/users".to_string(), "POST")
            ]
        );
        let get = &m[&("/users".to_string(), "GET")];
        assert_eq!(
            (get.handler.as_str(), get.rows.clone()),
            ("listUsers", vec![0, 3])
        );
    }

    /// The ROUTE_METHOD method of every node whose qname's path part (after
    /// the first space) is `path`, sorted — a path's method set, spread over
    /// its per-method nodes.
    fn route_methods(r: &RouteNodes, path: &str) -> Vec<String> {
        let mut out: Vec<String> = r
            .nodes
            .iter()
            .filter(|n| {
                r.nav
                    .qname_by_id
                    .get(&n.id)
                    .and_then(|q| q.split_once(' '))
                    .is_some_and(|(_, p)| p == path)
            })
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::ROUTE_METHOD)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => {
                    let m = s.split("\"method\":\"").nth(1)?;
                    Some(m.split('"').next()?.to_string())
                }
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn detects_hapi_single_object_route() {
        let src = r#"
server.route({
    method: 'GET',
    path: '/health',
    handler: (req, h) => 'ok',
});
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(route_methods(&r, "/health"), vec!["GET".to_string()]);
    }

    #[test]
    fn detects_hapi_array_of_methods() {
        let src = r#"
server.route({
    method: ['GET', 'POST'],
    path: '/users',
    handler: usersHandler,
});
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        let methods = route_methods(&r, "/users");
        assert!(methods.contains(&"GET".to_string()));
        assert!(methods.contains(&"POST".to_string()));
    }

    #[test]
    fn detects_hapi_array_of_routes() {
        let src = r#"
server.route([
    { method: 'GET', path: '/a', handler: ha },
    { method: 'POST', path: '/b', handler: hb },
]);
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(route_methods(&r, "/a"), vec!["GET".to_string()]);
        assert_eq!(route_methods(&r, "/b"), vec!["POST".to_string()]);
    }

    /// Hapi's `method: '*'` (and `ANY` / `ALL`) is method-agnostic: `ANY`.
    #[test]
    fn hapi_wildcard_is_any() {
        let src = "server.route({ method: '*', path: '/x', handler: h });\nserver.route({ method: 'ALL', path: '/y', handler: h });";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["ANY /x", "ANY /y"]);
        assert_eq!(r.any, 2);
    }

    #[test]
    fn hapi_rejects_code_shaped_path_strings() {
        // Was the dominant FP class in the 2026-05-05 framework-coverage check
        // (Hapi extractor over-fired 163 ROUTE nodes against hapi/hapi's own
        // source code — multi-line strings, file paths, ./templates/plugin
        // etc. all leaked through the bare `/`-prefix gate).
        let src = r#"
// Path argument variable reference, not a literal — must skip.
function relativeTo(path) {
    server.route({ method: 'GET', path: relativeTo, handler: h });
}

// File-system paths like ./templates start with '.', not '/'; reject.
server.route({ method: 'GET', path: './templates/plugin', handler: h });

// Multi-line garbage simulating a broken string close — has newlines, parens.
server.route({ method: 'GET', path: '/);\n});\n        it(', handler: h });
"#;
        let r = extract_ts_backend_routes(src, "test.ts", module_id(), "test", repo());
        assert!(
            r.nodes.is_empty(),
            "all three paths should be rejected; got {:?}",
            route_methods(&r, "/")
        );
    }

    #[test]
    fn hapi_accepts_param_paths() {
        let src = r#"
server.route({ method: 'GET', path: '/users/{id}', handler: h });
server.route({ method: 'POST', path: '/api/v1/users', handler: h });
"#;
        let r = extract_ts_backend_routes(src, "test.ts", module_id(), "test", repo());
        assert_eq!(route_methods(&r, "/users/{id}"), vec!["GET".to_string()]);
        assert_eq!(route_methods(&r, "/api/v1/users"), vec!["POST".to_string()]);
    }

    #[test]
    fn hapi_skips_non_path_value() {
        // `path: 'cache-key'` (no `/` prefix) must not emit.
        let src = r#"
server.route({ method: 'GET', path: 'cache-key', handler: h });
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert!(
            r.nodes.is_empty(),
            "non-`/` path must not emit a Hapi route"
        );
    }

    #[test]
    fn detects_bun_serve_routes_object() {
        let src = r#"
Bun.serve({
    routes: {
        '/api/health': new Response('ok'),
        '/api/users': {
            GET: () => listUsers(),
            POST: createUser,
        },
        '/api/users/:id': {
            GET: getUser,
            DELETE: deleteUser,
        },
    },
});
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        // '/api/health' value is a Response, not a verb object: Bun serves it
        // for every method, so it is `ANY`.
        assert_eq!(route_methods(&r, "/api/health"), vec!["ANY".to_string()]);
        assert_eq!(route_methods(&r, "/api/users"), vec!["GET", "POST"]);
        assert_eq!(route_methods(&r, "/api/users/:id"), vec!["DELETE", "GET"]);
    }

    #[test]
    fn bun_serve_skips_non_path_keys() {
        // Keys not starting with `/` (e.g. nested config) must be skipped.
        let src = r#"
Bun.serve({
    routes: {
        '/api/x': { GET: hx },
        port: 3000,
    },
});
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(route_methods(&r, "/api/x"), vec!["GET".to_string()]);
        // No phantom route for `port`.
        assert_eq!(r.nodes.len(), 1);
    }

    /// Two registrations on one line are two nodes, both on row 0, each with
    /// its own method.
    #[test]
    fn route_cell_carries_method_json() {
        let src = "app.get('/x', h); app.post('/x', h);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["GET /x", "POST /x"]);
        for (q, m) in [("GET /x", "GET"), ("POST /x", "POST")] {
            let n = node(&r, q);
            assert_eq!(position_rows(n), vec![0], "{q}");
            let rm: Vec<String> = cells(n)
                .into_iter()
                .filter(|(k, _)| *k == cell_type::ROUTE_METHOD)
                .map(|(_, s)| s)
                .collect();
            assert_eq!(rm.len(), 1, "{q}");
            assert!(rm[0].contains(&format!("\"method\":\"{m}\"")), "{rm:?}");
        }
    }

    /// LB.11b: each method on a path is its own node, and a HANDLED_BY ref
    /// leaves from its own method's node only.
    #[test]
    fn express_methods_are_separate_route_nodes() {
        let src = "app.get('/users', listUsers);\napp.post('/users', createUser);\napp.get('/users/:id', getUser);\napp.delete('/users/:id', deleteUser);";
        let r = extract_ts_backend_routes(src, "app.ts", module_id(), "test", repo());
        assert_eq!(
            qnames(&r),
            vec![
                "DELETE /users/:id",
                "GET /users",
                "GET /users/:id",
                "POST /users"
            ]
        );
        assert!(!qnames(&r).iter().any(|q| q.starts_with("route:")));
        let mut refs = handled_by(&r);
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            refs,
            vec![
                (
                    "DELETE /users/:id".to_string(),
                    CallQualifier::Bare("deleteUser".into())
                ),
                (
                    "GET /users".to_string(),
                    CallQualifier::Bare("listUsers".into())
                ),
                (
                    "GET /users/:id".to_string(),
                    CallQualifier::Bare("getUser".into())
                ),
                (
                    "POST /users".to_string(),
                    CallQualifier::Bare("createUser".into())
                ),
            ]
        );
    }

    /// Express `.all(` serves every method: `ANY`, never `ALL`.
    #[test]
    fn express_all_is_any() {
        let src = "app.all('/health', (req, res) => res.send('ok'));";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["ANY /health"]);
        assert_eq!(route_methods(&r, "/health"), vec!["ANY"]);
        assert_eq!((r.positioned, r.any), (1, 1));
    }

    /// The ROUTE_METHOD `line` is the 1-based line of the registration (the
    /// Go convention); `col` stays 0 = unknown.
    #[test]
    fn route_method_line_is_the_registration_line() {
        let src = "const app = express();\napp.post('/users', createUser);";
        let r = extract_ts_backend_routes(src, "app.ts", module_id(), "test", repo());
        // GD.42: `express()` is a router value, so its routes ride its slot.
        let n = node(&r, "POST <mount:module:test#app>/users");
        assert_eq!(position_rows(n), vec![1]);
        let rm = cells(n)
            .into_iter()
            .find(|(k, _)| *k == cell_type::ROUTE_METHOD)
            .expect("ROUTE_METHOD");
        assert!(rm.1.contains(r#""line":2,"col":0"#), "{}", rm.1);
    }

    /// The same (method, path) registered twice is ONE node with two
    /// ascending POSITION cells and one ROUTE_METHOD at the first line; the
    /// handler the first registration did not name is filled in.
    #[test]
    fn repeated_registration_stacks_positions() {
        let src = "app.get('/x', (req, res) => res.end());\n\n\napp.get('/x', getX);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(r.nodes.len(), 1);
        let n = node(&r, "GET /x");
        assert_eq!(position_rows(n), vec![0, 3]);
        let rm: Vec<String> = cells(n)
            .into_iter()
            .filter(|(k, _)| *k == cell_type::ROUTE_METHOD)
            .map(|(_, s)| s)
            .collect();
        assert_eq!(
            rm,
            vec![
                r#"{"method":"GET","handler":"getX","file":"server.ts","line":1,"col":0}"#
                    .to_string()
            ]
        );
        assert_eq!(r.positioned, 2);
    }

    /// A Bun route whose value is a handler function or a `Response` is served
    /// for every method (`ANY`); a verb object keeps its verbs.
    #[test]
    fn bun_single_value_route_is_any() {
        let src = "Bun.serve({\n  routes: {\n    '/api/ping': () => new Response('pong'),\n    '/api/x': { GET: hx },\n  },\n});";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(qnames(&r), vec!["ANY /api/ping", "GET /api/x"]);
        assert_eq!(r.any, 1);
    }

    /// Bun and Hapi rows are source rows, read through absolute offsets of
    /// the route key / config object — not rows within the sliced body. A
    /// multi-byte character ahead of them does not shift a row.
    #[test]
    fn bun_and_hapi_rows_are_source_rows() {
        let src = "// é ünïcode\nimport x from 'y';\n\nBun.serve({\n  routes: {\n    '/a': { GET: ha },\n    '/b': new Response('b'),\n  },\n});\n";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(position_rows(node(&r, "GET /a")), vec![5]);
        assert_eq!(position_rows(node(&r, "ANY /b")), vec![6]);

        let src = "// hapi — server\nconst s = Hapi.server();\ns.route({\n  method: 'GET',\n  path: '/h',\n  handler: h,\n});\ns.route([\n  { method: 'POST', path: '/p', handler: p },\n]);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(position_rows(node(&r, "GET /h")), vec![2]);
        assert_eq!(position_rows(node(&r, "POST /p")), vec![8]);
    }

    fn handled_by(r: &RouteNodes) -> Vec<(String, CallQualifier)> {
        r.refs
            .iter()
            .map(|x| {
                assert_eq!(x.category, edge_category::HANDLED_BY);
                assert_eq!(x.from_module, module_id());
                let q = r.nav.qname_by_id.get(&x.from).cloned().unwrap_or_default();
                (q, x.qualifier.clone())
            })
            .collect()
    }

    /// A3.5: Angular's `this.http.get('/users')` is an OUTBOUND call. Before
    /// A3.5 it minted a phantom server `route:/users` in the client repo.
    #[test]
    fn client_receiver_calls_are_not_routes() {
        let src =
            "class S { constructor(private http: any) {} f() { return this.http.get('/users'); } }";
        let r = extract_ts_backend_routes(src, "s.ts", module_id(), "test", repo());
        assert!(r.nodes.is_empty(), "no ROUTE from a client call");
        assert!(r.refs.is_empty());
        assert_eq!(r.skipped_client_calls, 1);

        let src = "this.httpClient.post('/a', b);\napiClient.put('/b', b);\n_client.delete('/c');\n$http.get('/d');\nthis.api.patch('/e', x);\nuserApi.get('/f');";
        let r = extract_ts_backend_routes(src, "s.ts", module_id(), "test", repo());
        assert!(r.nodes.is_empty(), "every client receiver is rejected");
        assert_eq!(r.skipped_client_calls, 6);
    }

    /// A router NAMED like a client but registering an inline handler is a
    /// server: Hono's `const api = new Hono()` (glia-eval hono/blog lost all
    /// five of its /posts registrations to the bare name test). An options
    /// object that merely contains an arrow is not a handler.
    #[test]
    fn inline_handler_overrides_client_receiver_name() {
        let src = "api.get('/posts', async (c) => {\napi.post('/posts', (c) => c.json({}));\napi.delete('/posts/:id', c => c.body(null));\napiClient.get('/legacy', function (err, res) {";
        let r = extract_ts_backend_routes(src, "src/api.ts", module_id(), "test", repo());
        assert_eq!(r.skipped_client_calls, 0);
        let posts = route_methods(&r, "/posts");
        assert!(
            posts.contains(&"GET".to_string()) && posts.contains(&"POST".to_string()),
            "{posts:?}"
        );
        assert_eq!(route_methods(&r, "/posts/:id"), vec!["DELETE".to_string()]);
        assert_eq!(route_methods(&r, "/legacy"), vec!["GET".to_string()]);

        let src = "this.http.get('/x', { transform: (d) => d });\nthis.http.get('/y').pipe(map((r) => r));\nthis.http.post('/z', body).subscribe(() => done());";
        let r = extract_ts_backend_routes(src, "s.ts", module_id(), "test", repo());
        assert!(
            r.nodes.is_empty(),
            "arrows outside the argument list are not handlers"
        );
        assert_eq!(r.skipped_client_calls, 3);
    }

    #[test]
    fn inline_function_shapes() {
        for yes in [
            "(req, res) => res.end()",
            "async (c) => {",
            "c => c.text('x')",
            "async c => 1",
            "function (req, res) {",
            "async function(req, res) {",
            "function* gen() {",
            "(req: Request, res: Response): Promise<void> => {",
        ] {
            assert!(is_inline_function(yes), "{yes}");
        }
        for no in [
            "getUser",
            "users.list",
            "{ transform: (d) => d }",
            "body",
            "asyncHandler(fn)",
            "functionsConfig",
            "a.b => 1",
            "",
        ] {
            assert!(!is_inline_function(no), "{no}");
        }
    }

    /// The server routers the guard must leave alone — including class-based
    /// Express (`this.app` / `this.router`), which the TS parser's own
    /// `this.<x>.<verb>(` endpoint shape would also claim.
    #[test]
    fn express_router_calls_are_still_routes() {
        let src = "app.get('/x', h);\nrouter.post('/y', h);\nserver.put('/z', h);\nthis.app.delete('/w', h);\nthis.router.patch('/v', h);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        assert_eq!(r.skipped_client_calls, 0);
        assert_eq!(route_methods(&r, "/x"), vec!["GET".to_string()]);
        assert_eq!(route_methods(&r, "/y"), vec!["POST".to_string()]);
        assert_eq!(route_methods(&r, "/z"), vec!["PUT".to_string()]);
        assert_eq!(route_methods(&r, "/w"), vec!["DELETE".to_string()]);
        assert_eq!(route_methods(&r, "/v"), vec!["PATCH".to_string()]);
    }

    /// A3.5 (A15.10's gap): a route registered with a NAMED handler carries a
    /// HANDLED_BY ref for the graph to bind, and the handler on its cell.
    #[test]
    fn named_express_handler_emits_handled_by_ref() {
        let src = "app.get(\"/users/:id\", getUser);\napp.post(\"/users\", auth, createUser);\nrouter.put('/users/:id', users.update);";
        let r = extract_ts_backend_routes(src, "server/app.ts", module_id(), "test", repo());
        let mut refs = handled_by(&r);
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            refs,
            vec![
                (
                    "GET /users/:id".to_string(),
                    CallQualifier::Bare("getUser".into())
                ),
                (
                    "POST /users".to_string(),
                    CallQualifier::Bare("createUser".into())
                ),
                (
                    "PUT /users/:id".to_string(),
                    CallQualifier::Attribute {
                        base: "users".into(),
                        name: "update".into()
                    }
                ),
            ]
        );
        let payloads: Vec<String> = r
            .nodes
            .iter()
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        assert!(
            payloads
                .iter()
                .any(|p| p.contains("\"handler\":\"getUser\"")),
            "{payloads:?}"
        );
        assert!(
            payloads
                .iter()
                .any(|p| p.contains("\"handler\":\"createUser\"")),
            "{payloads:?}"
        );
    }

    /// Inline and wrapped handlers keep their route and name no handler: there
    /// is no identifier to bind, and guessing one would be a wrong edge.
    #[test]
    fn inline_and_unreadable_handlers_emit_route_without_ref() {
        let src = r#"
app.get("/a", (req, res) => { res.json({}); });
app.get("/b", async (req, res) => res.send("ok"));
app.post("/c", function (req, res) {
app.put("/d", asyncHandler(update));
app.delete("/e", this.remove);
app.patch("/f", auth,
  patchIt);
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), "test", repo());
        for p in ["/a", "/b", "/c", "/d", "/e", "/f"] {
            assert_eq!(route_methods(&r, p).len(), 1, "route {p} still emitted");
        }
        assert!(r.refs.is_empty(), "no handler ref: {:?}", handled_by(&r));
    }

    #[test]
    fn named_handler_reads_the_last_argument() {
        assert_eq!(named_handler(", getUser);"), Some("getUser"));
        assert_eq!(
            named_handler(" , auth, rateLimit({ max: 5, window: '1,2' }), getUser)"),
            Some("getUser")
        );
        assert_eq!(named_handler(", h,)"), Some("h"));
        assert_eq!(named_handler(", ctrl.list)"), Some("ctrl.list"));
        assert_eq!(named_handler(", a.b.c)"), None);
        assert_eq!(named_handler(", (req, res) => res.end())"), None);
        assert_eq!(named_handler(", null)"), None);
        assert_eq!(named_handler(")"), None);
        assert_eq!(named_handler(", h"), None);
        assert_eq!(named_handler(", h]"), None);
    }
    // ------------------------------------------------------------------
    // GD.42: include-style mounts
    // ------------------------------------------------------------------

    /// `(via, target, prefix)` of every mount fact, in record order.
    fn mounts(r: &RouteNodes) -> Vec<(String, String, String)> {
        r.mount_facts
            .iter()
            .filter_map(|f| match f {
                NavFact::IncludeMount {
                    target,
                    prefix,
                    via,
                    ..
                } => Some((
                    via.clone(),
                    match target {
                        IncludeTarget::Spec(s) => format!("spec:{s}"),
                        IncludeTarget::Import { local } => format!("import:{local}"),
                        IncludeTarget::Dynamic(d) => format!("dyn:{d}"),
                    },
                    prefix.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    fn m(via: &str, target: &str, prefix: &str) -> (String, String, String) {
        (via.to_string(), target.to_string(), prefix.to_string())
    }

    /// A router file: its routes on the router's slot, the router and its
    /// default export recorded; a route on a non-router receiver stays plain.
    #[test]
    fn express_router_routes_ride_their_slot() {
        let src = "const express = require('express');\nconst router = express.Router();\nrouter.get('/list', (req, res) => res.json([]));\nother.get('/plain', h);\n\nmodule.exports = router;\n";
        let r =
            extract_ts_backend_routes(src, "routes/users.js", module_id(), "routes::users", repo());
        assert_eq!(
            qnames(&r),
            ["GET /plain", "GET <mount:module:routes::users#router>/list"]
        );
        assert_eq!(
            mounts(&r),
            [
                m("router:express", "dyn:router", ""),
                m("export_default", "dyn:router", ""),
            ]
        );
    }

    /// Mount sites: a `require(..)` is a Spec, an imported name an Import;
    /// a glob prefix (middleware), a call and an inline function record
    /// nothing; every candidate argument after the prefix is one fact.
    #[test]
    fn express_use_mounts_are_recorded() {
        let src = "import express from 'express';\nimport items from './routes/items';\n\nconst app = express();\napp.use('/api/users', require('./routes/users'));\napp.use('/api/items', items);\napp.use('/api/v2/*', csrfMiddleware);\napp.use(cors());\napp.use('/x', (req, res, next) => next());\napp.use('/admin', auth, adminRouter);\napp.use(rootRouter);\napp.use('/v1', users.router);\n";
        let r = extract_ts_backend_routes(src, "app.ts", module_id(), "app", repo());
        assert_eq!(
            mounts(&r),
            [
                m("router:express", "dyn:app", ""),
                m("express:app", "spec:./routes/users", "/api/users"),
                m("express:app", "import:items", "/api/items"),
                m("express:app", "import:auth", "/admin"),
                m("express:app", "import:adminRouter", "/admin"),
                m("express:app", "import:rootRouter", ""),
                m("express:app", "import:users.router", "/v1"),
            ]
        );
    }

    /// documenso's Hono shape: a chained `new Hono<E>()` whose `.post(` lines
    /// continue its declaration (inside handler bodies too), and a root app
    /// with a non-literal `basePath` mounting it with `.route(..)`.
    #[test]
    fn hono_chains_and_route_mounts() {
        let files = "import { Hono } from 'hono';\n\nexport const filesRoute = new Hono<HonoEnv>()\n  /**\n   * Uploads.\n   */\n  .post('/upload-pdf', validator, async (c) => {\n    if (x) {\n      return c.json({ a: 1 }, 401);\n    }\n    return c.json({});\n  })\n  .get('/item/:id', async (c) => c.json({}));\n\nconst other = 1;\nfoo.get('/after', h);\n";
        let r = extract_ts_backend_routes(files, "api/files.ts", module_id(), "api::files", repo());
        assert_eq!(
            qnames(&r),
            [
                "GET /after",
                "GET <mount:module:api::files#filesRoute>/item/:id",
                "POST <mount:module:api::files#filesRoute>/upload-pdf",
            ]
        );
        let root = "const app = new Hono<HonoEnv>().basePath(basePath || '/');\napp.route('/api/files', filesRoute);\napp.route('/api/csc', csc);\nconst api = new Hono().basePath('/api');\napi.route('/v1', v1);\napi.get('/health', (c) => c.text('ok'));\n";
        let r = extract_ts_backend_routes(root, "router.ts", module_id(), "router", repo());
        assert_eq!(qnames(&r), ["GET <mount:module:router#api>/api/health"]);
        assert_eq!(
            mounts(&r),
            [
                m("router:hono", "dyn:app", ""),
                m("router:hono", "dyn:api", "/api"),
                m("hono:app", "import:filesRoute", "/api/files"),
                m("hono:app", "import:csc", "/api/csc"),
                m("hono:api", "import:v1", "/api/v1"),
            ]
        );
    }

    /// Koa: a router's own `prefix` joins its routes and its mounts; a
    /// `<sub>.routes()` argument names `<sub>`, `allowedMethods()` nothing.
    #[test]
    fn koa_router_prefix_and_routes_mount() {
        let src = "const Router = require('@koa/router');\nconst api = new Router({ prefix: '/v1' });\napi.get('/a', h);\napi.use('/sub', sub.routes(), sub.allowedMethods());\n";
        let r = extract_ts_backend_routes(src, "api.js", module_id(), "api", repo());
        assert_eq!(qnames(&r), ["GET <mount:module:api#api>/v1/a"]);
        assert_eq!(
            mounts(&r),
            [
                m("router:koa", "dyn:api", "/v1"),
                m("koa:api", "import:sub", "/v1/sub"),
            ]
        );
    }

    /// NestJS: the controller's routes and slot, a module's controllers, a
    /// RouterModule tree (children nested and bare), a global prefix with
    /// literal (string and object) and named excludes, and a path list.
    #[test]
    fn nest_facts() {
        let ctrl = "@Controller('users')\nexport class UsersController {\n  @Get('all')\n  all() {\n    return [];\n  }\n}\n";
        let r = extract_ts_backend_routes(ctrl, "u.controller.ts", module_id(), "u", repo());
        assert_eq!(qnames(&r), ["GET /users/all"]);
        assert_eq!(
            mounts(&r),
            [
                m("router:nest", "dyn:UsersController", ""),
                m("nest_route", "dyn:UsersController", ""),
            ]
        );
        let module = "@Module({\n  imports: [Other],\n  controllers: [UsersController, AuthController],\n  providers: [Svc],\n})\nexport class AdminModule {}\n";
        let r = extract_ts_backend_routes(module, "admin.module.ts", module_id(), "a", repo());
        assert_eq!(
            mounts(&r),
            [
                m("router:nest_module", "dyn:AdminModule", ""),
                m("nest_module:AdminModule", "import:UsersController", ""),
                m("nest_module:AdminModule", "import:AuthController", ""),
            ]
        );
        let app = "@Module({\n  imports: [\n    RouterModule.register([\n      { path: 'backoffice', module: AdminModule, children: [{ path: 'v2', module: V2Module }, BareModule] },\n      { path: 'pub', module: PubModule },\n    ]),\n  ],\n})\nexport class AppModule {}\n";
        let r = extract_ts_backend_routes(app, "app.module.ts", module_id(), "m", repo());
        assert_eq!(
            mounts(&r),
            [
                m("router:nest_module", "dyn:AppModule", ""),
                m("nest_router", "import:AdminModule", "backoffice"),
                m("nest_router", "import:V2Module", "backoffice/v2"),
                m("nest_router", "import:BareModule", "backoffice"),
                m("nest_router", "import:PubModule", "pub"),
            ]
        );
        let main = "export const PATHS = ['/health', '/favicon.ico'];\nasync function boot() {\n  app.setGlobalPrefix('svc', {\n    exclude: [{ path: 'status', method: RequestMethod.GET }, 'metrics', PATHS],\n  });\n  other.setGlobalPrefix('api', { exclude: excludePaths });\n}\n";
        let r = extract_ts_backend_routes(main, "main.ts", module_id(), "main", repo());
        assert_eq!(
            mounts(&r),
            [
                m("path_list:PATHS", "spec:/health", ""),
                m("path_list:PATHS", "spec:/favicon.ico", ""),
                m("nest_global", "dyn:", "svc"),
                m("nest_exclude:GET", "spec:status", ""),
                m("nest_exclude:ANY", "spec:metrics", ""),
                m("nest_exclude_ref", "import:PATHS", ""),
                m("nest_global", "dyn:", "api"),
                m("nest_exclude_ref", "import:excludePaths", ""),
            ]
        );
    }

    /// The depth scan: strings, comments and template text never move it, a
    /// `${..}` hole is code.
    #[test]
    fn line_depths_skip_strings_and_comments() {
        let src = "a('(', \"{\", `x ${f({})} {`, /* ( */ b) // {\nc({\n}\n";
        assert_eq!(line_depths(src), [0, 0, 2, 1]);
    }
}
