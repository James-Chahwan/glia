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
    CallQualifier, CodeNav, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category, endpoint,
    node_kind,
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
}

const HTTP_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "options", "head", "all"];

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
    repo: RepoId,
) -> RouteNodes {
    let mut by_route: RouteMap = BTreeMap::new();
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
            let Some(end) = rest.find(quote) else { continue };
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
            if endpoint::is_http_client_receiver(endpoint::ident_before(t, idx))
                && !has_inline_handler(after_literal)
            {
                skipped_client_calls += 1;
                continue;
            }
            if looks_like_http_client(t) {
                continue;
            }
            let handler = named_handler(after_literal);
            if let Some(key) = add_method(&mut by_route, route, method, handler.unwrap_or(""), row)
                && let Some(h) = handler
            {
                handled_by.entry((key.0, key.1, h.to_string())).or_insert(row);
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
    // decorators @Get/@Post/...(suffix), located at the decorator.
    for (method, route, row) in nestjs_routes(source) {
        add_method(&mut by_route, &route, method, "", row);
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
        // canonical (`add_method`), so the qname is `<METHOD> <path>`.
        let qname = endpoint::route_qname(method, &route);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname);
        let mut rows = entry.rows;
        rows.sort_unstable();
        rows.dedup();
        let mut cells: Vec<Cell> = rows.iter().map(|&r| anchor::position_cell(path, r)).collect();
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
            let qname = endpoint::route_qname(method, &route);
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

    RouteNodes { nodes, nav, refs, skipped_client_calls, positioned, any }
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
        s.strip_prefix(kw).is_some_and(|r| {
            r.starts_with(|c: char| c.is_whitespace() || c == '(' || c == '*')
        })
    };
    let a = if keyword(arg, "async") { arg["async".len()..].trim_start() } else { arg };
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
            && !matches!(p, "this" | "null" | "undefined" | "true" | "false" | "function" | "async" | "new")
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
    let idx = norm.find("src/routes/").map(|i| i + "src/routes/".len())
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

/// Scan a TS source for NestJS @Controller + @Get/@Post/... method decorators.
/// Returns (method, full_path, row) triples, `row` the decorator's 0-based
/// line.
fn nestjs_routes(source: &str) -> Vec<(&'static str, String, u32)> {
    let mut out = Vec::new();
    let mut controller_prefix: Option<String> = None;
    for (row, line) in source.lines().enumerate() {
        let row = u32::try_from(row).unwrap_or(u32::MAX);
        let t = line.trim();
        if t.starts_with("@Controller(") {
            controller_prefix = Some(extract_decorator_string(t).unwrap_or_default());
            continue;
        }
        for (deco, method) in &[
            ("@Get(", "get"),
            ("@Post(", "post"),
            ("@Put(", "put"),
            ("@Patch(", "patch"),
            ("@Delete(", "delete"),
            ("@Head(", "head"),
            ("@Options(", "options"),
            ("@All(", "all"),
        ] {
            if t.starts_with(deco) {
                let suffix = extract_decorator_string(t).unwrap_or_default();
                let full = combine_nest_paths(controller_prefix.as_deref().unwrap_or(""), &suffix);
                out.push((*method, full, row));
                break;
            }
        }
    }
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
            if let Some(p) = path {
                if looks_like_url_path(&p) {
                    for m in methods {
                        out.push((m, p.clone(), i));
                    }
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
                    let s = tok.trim().trim_matches(|c| c == '\'' || c == '"' || c == '`');
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
            if j < bytes.len() {
                if let Some(m) = canonical_http_method(&after[1..j]) {
                    return vec![m];
                }
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
        debug_assert_eq!(source.get(routes_start..routes_start + routes_obj.len()), Some(routes_obj));
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
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(qnames(&r), vec!["GET /users/:id"]);
        // LB.11b: the name is the qname, like every other server parser.
        let id = r.nodes[0].id;
        assert_eq!(r.nav.name_by_id.get(&id).map(String::as_str), Some("GET /users/:id"));
        let c = cells(&r.nodes[0]);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(c[0], (cell_type::POSITION, r#"{"file":"server.ts","start_line":0,"end_line":0}"#.to_string()));
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
        let r = extract_ts_backend_routes(src, "routes.ts", module_id(), repo());
        assert_eq!(r.nodes.len(), 1);
    }

    #[test]
    fn rejects_fetch_call() {
        let src = "fetch('/api/widgets').then(r => r.json());";
        let r = extract_ts_backend_routes(src, "client.ts", module_id(), repo());
        assert_eq!(r.nodes.len(), 0);
    }

    /// A Pages Router API route's default export is handed EVERY method by
    /// Next.js, so it is `ANY`, located at the `export default`.
    #[test]
    fn detects_nextjs_pages_router() {
        let src = "// users\nexport default function handler(req, res) { res.json({}); }";
        let r = extract_ts_backend_routes(src, "pages/api/users.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "app/api/widgets/route.ts", module_id(), repo());
        assert_eq!(qnames(&r), vec!["GET /api/widgets", "POST /api/widgets"]);
        for (q, row) in [("GET /api/widgets", 0), ("POST /api/widgets", 1)] {
            let n = node(&r, q);
            assert_eq!(position_rows(n), vec![row], "{q}");
            let methods = cells(n).into_iter().filter(|(k, _)| *k == cell_type::ROUTE_METHOD).count();
            assert_eq!(methods, 1, "{q}");
        }
        assert_eq!((r.positioned, r.any), (2, 0));
    }

    #[test]
    fn converts_nextjs_dynamic_segment() {
        let src = "export default function h() {}";
        let r = extract_ts_backend_routes(src, "pages/api/users/[id].ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "src/users.controller.ts", module_id(), repo());
        assert_eq!(
            qnames(&r),
            vec!["DELETE /users/:id", "GET /users", "GET /users/:id", "POST /users", "PUT /users/:id"]
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

    /// Nest `@All(` is method-agnostic: `ANY`, never `ALL`.
    #[test]
    fn nestjs_all_is_any() {
        let src = "@Controller('health')\nexport class H {\n  @All()\n  check() {}\n}\n";
        let r = extract_ts_backend_routes(src, "src/health.controller.ts", module_id(), repo());
        assert_eq!(qnames(&r), vec!["ANY /health"]);
        assert_eq!(position_rows(node(&r, "ANY /health")), vec![2]);
    }

    #[test]
    fn detects_sveltekit_plus_server() {
        let src = "export async function GET() {}\nexport async function POST() {}";
        let r = extract_ts_backend_routes(src, "src/routes/api/widgets/+server.ts", module_id(), repo());
        assert_eq!(qnames(&r), vec!["GET /api/widgets", "POST /api/widgets"]);
        assert_eq!(position_rows(node(&r, "POST /api/widgets")), vec![1]);
    }

    #[test]
    fn sveltekit_dynamic_segment() {
        let src = "export function GET() {}";
        let r = extract_ts_backend_routes(src, "src/routes/api/users/[id]/+server.ts", module_id(), repo());
        assert_eq!(qnames(&r), vec!["GET /api/users/:id"]);
    }

    /// LB.5 — `add_method` keys routes on the canonical path, so a relative
    /// and a slashed spelling of one (method, path) are one entry (and so one
    /// `<METHOD> /…` node); LB.11b — a second method on the path is its own
    /// entry, and the method token is upper-cased (`all` -> `ANY`).
    #[test]
    fn add_method_keys_on_the_canonical_path_and_method() {
        let mut m: RouteMap = BTreeMap::new();
        assert_eq!(add_method(&mut m, "users", "get", "listUsers", 0), Some(("/users".into(), "GET")));
        assert_eq!(add_method(&mut m, "/users", "get", "", 3), Some(("/users".into(), "GET")));
        assert_eq!(add_method(&mut m, "/users", "post", "", 1), Some(("/users".into(), "POST")));
        assert_eq!(add_method(&mut m, "/users", "all", "", 2), Some(("/users".into(), "ANY")));
        assert_eq!(add_method(&mut m, "/${x}", "get", "", 2), None);
        assert_eq!(add_method(&mut m, "/users", "trace", "", 2), None);
        assert_eq!(
            m.keys().cloned().collect::<Vec<_>>(),
            vec![("/users".to_string(), "ANY"), ("/users".to_string(), "GET"), ("/users".to_string(), "POST")]
        );
        let get = &m[&("/users".to_string(), "GET")];
        assert_eq!((get.handler.as_str(), get.rows.clone()), ("listUsers", vec![0, 3]));
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
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(route_methods(&r, "/a"), vec!["GET".to_string()]);
        assert_eq!(route_methods(&r, "/b"), vec!["POST".to_string()]);
    }

    /// Hapi's `method: '*'` (and `ANY` / `ALL`) is method-agnostic: `ANY`.
    #[test]
    fn hapi_wildcard_is_any() {
        let src = "server.route({ method: '*', path: '/x', handler: h });\nserver.route({ method: 'ALL', path: '/y', handler: h });";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "test.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "test.ts", module_id(), repo());
        assert_eq!(route_methods(&r, "/users/{id}"), vec!["GET".to_string()]);
        assert_eq!(route_methods(&r, "/api/v1/users"), vec!["POST".to_string()]);
    }

    #[test]
    fn hapi_skips_non_path_value() {
        // `path: 'cache-key'` (no `/` prefix) must not emit.
        let src = r#"
server.route({ method: 'GET', path: 'cache-key', handler: h });
"#;
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert!(r.nodes.is_empty(), "non-`/` path must not emit a Hapi route");
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
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(route_methods(&r, "/api/x"), vec!["GET".to_string()]);
        // No phantom route for `port`.
        assert_eq!(r.nodes.len(), 1);
    }

    /// Two registrations on one line are two nodes, both on row 0, each with
    /// its own method.
    #[test]
    fn route_cell_carries_method_json() {
        let src = "app.get('/x', h); app.post('/x', h);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "app.ts", module_id(), repo());
        assert_eq!(
            qnames(&r),
            vec!["DELETE /users/:id", "GET /users", "GET /users/:id", "POST /users"]
        );
        assert!(!qnames(&r).iter().any(|q| q.starts_with("route:")));
        let mut refs = handled_by(&r);
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            refs,
            vec![
                ("DELETE /users/:id".to_string(), CallQualifier::Bare("deleteUser".into())),
                ("GET /users".to_string(), CallQualifier::Bare("listUsers".into())),
                ("GET /users/:id".to_string(), CallQualifier::Bare("getUser".into())),
                ("POST /users".to_string(), CallQualifier::Bare("createUser".into())),
            ]
        );
    }

    /// Express `.all(` serves every method: `ANY`, never `ALL`.
    #[test]
    fn express_all_is_any() {
        let src = "app.all('/health', (req, res) => res.send('ok'));";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(qnames(&r), vec!["ANY /health"]);
        assert_eq!(route_methods(&r, "/health"), vec!["ANY"]);
        assert_eq!((r.positioned, r.any), (1, 1));
    }

    /// The ROUTE_METHOD `line` is the 1-based line of the registration (the
    /// Go convention); `col` stays 0 = unknown.
    #[test]
    fn route_method_line_is_the_registration_line() {
        let src = "const app = express();\napp.post('/users', createUser);";
        let r = extract_ts_backend_routes(src, "app.ts", module_id(), repo());
        let n = node(&r, "POST /users");
        assert_eq!(position_rows(n), vec![1]);
        let rm = cells(n).into_iter().find(|(k, _)| *k == cell_type::ROUTE_METHOD).expect("ROUTE_METHOD");
        assert!(rm.1.contains(r#""line":2,"col":0"#), "{}", rm.1);
    }

    /// The same (method, path) registered twice is ONE node with two
    /// ascending POSITION cells and one ROUTE_METHOD at the first line; the
    /// handler the first registration did not name is filled in.
    #[test]
    fn repeated_registration_stacks_positions() {
        let src = "app.get('/x', (req, res) => res.end());\n\n\napp.get('/x', getX);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
            vec![r#"{"method":"GET","handler":"getX","file":"server.ts","line":1,"col":0}"#.to_string()]
        );
        assert_eq!(r.positioned, 2);
    }

    /// A Bun route whose value is a handler function or a `Response` is served
    /// for every method (`ANY`); a verb object keeps its verbs.
    #[test]
    fn bun_single_value_route_is_any() {
        let src = "Bun.serve({\n  routes: {\n    '/api/ping': () => new Response('pong'),\n    '/api/x': { GET: hx },\n  },\n});";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(qnames(&r), vec!["ANY /api/ping", "GET /api/x"]);
        assert_eq!(r.any, 1);
    }

    /// Bun and Hapi rows are source rows, read through absolute offsets of
    /// the route key / config object — not rows within the sliced body. A
    /// multi-byte character ahead of them does not shift a row.
    #[test]
    fn bun_and_hapi_rows_are_source_rows() {
        let src = "// é ünïcode\nimport x from 'y';\n\nBun.serve({\n  routes: {\n    '/a': { GET: ha },\n    '/b': new Response('b'),\n  },\n});\n";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(position_rows(node(&r, "GET /a")), vec![5]);
        assert_eq!(position_rows(node(&r, "ANY /b")), vec![6]);

        let src = "// hapi — server\nconst s = Hapi.server();\ns.route({\n  method: 'GET',\n  path: '/h',\n  handler: h,\n});\ns.route([\n  { method: 'POST', path: '/p', handler: p },\n]);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
        let src = "class S { constructor(private http: any) {} f() { return this.http.get('/users'); } }";
        let r = extract_ts_backend_routes(src, "s.ts", module_id(), repo());
        assert!(r.nodes.is_empty(), "no ROUTE from a client call");
        assert!(r.refs.is_empty());
        assert_eq!(r.skipped_client_calls, 1);

        let src = "this.httpClient.post('/a', b);\napiClient.put('/b', b);\n_client.delete('/c');\n$http.get('/d');\nthis.api.patch('/e', x);\nuserApi.get('/f');";
        let r = extract_ts_backend_routes(src, "s.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "src/api.ts", module_id(), repo());
        assert_eq!(r.skipped_client_calls, 0);
        let posts = route_methods(&r, "/posts");
        assert!(posts.contains(&"GET".to_string()) && posts.contains(&"POST".to_string()), "{posts:?}");
        assert_eq!(route_methods(&r, "/posts/:id"), vec!["DELETE".to_string()]);
        assert_eq!(route_methods(&r, "/legacy"), vec!["GET".to_string()]);

        let src = "this.http.get('/x', { transform: (d) => d });\nthis.http.get('/y').pipe(map((r) => r));\nthis.http.post('/z', body).subscribe(() => done());";
        let r = extract_ts_backend_routes(src, "s.ts", module_id(), repo());
        assert!(r.nodes.is_empty(), "arrows outside the argument list are not handlers");
        assert_eq!(r.skipped_client_calls, 3);
    }

    #[test]
    fn inline_function_shapes() {
        for yes in ["(req, res) => res.end()", "async (c) => {", "c => c.text('x')", "async c => 1",
                    "function (req, res) {", "async function(req, res) {", "function* gen() {",
                    "(req: Request, res: Response): Promise<void> => {"] {
            assert!(is_inline_function(yes), "{yes}");
        }
        for no in ["getUser", "users.list", "{ transform: (d) => d }", "body", "asyncHandler(fn)",
                   "functionsConfig", "a.b => 1", ""] {
            assert!(!is_inline_function(no), "{no}");
        }
    }

    /// The server routers the guard must leave alone — including class-based
    /// Express (`this.app` / `this.router`), which the TS parser's own
    /// `this.<x>.<verb>(` endpoint shape would also claim.
    #[test]
    fn express_router_calls_are_still_routes() {
        let src = "app.get('/x', h);\nrouter.post('/y', h);\nserver.put('/z', h);\nthis.app.delete('/w', h);\nthis.router.patch('/v', h);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
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
        let r = extract_ts_backend_routes(src, "server/app.ts", module_id(), repo());
        let mut refs = handled_by(&r);
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            refs,
            vec![
                ("GET /users/:id".to_string(), CallQualifier::Bare("getUser".into())),
                ("POST /users".to_string(), CallQualifier::Bare("createUser".into())),
                (
                    "PUT /users/:id".to_string(),
                    CallQualifier::Attribute { base: "users".into(), name: "update".into() }
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
        assert!(payloads.iter().any(|p| p.contains("\"handler\":\"getUser\"")), "{payloads:?}");
        assert!(payloads.iter().any(|p| p.contains("\"handler\":\"createUser\"")), "{payloads:?}");
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
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        for p in ["/a", "/b", "/c", "/d", "/e", "/f"] {
            assert_eq!(route_methods(&r, p).len(), 1, "route {p} still emitted");
        }
        assert!(r.refs.is_empty(), "no handler ref: {:?}", handled_by(&r));
    }

    #[test]
    fn named_handler_reads_the_last_argument() {
        assert_eq!(named_handler(", getUser);"), Some("getUser"));
        assert_eq!(named_handler(" , auth, rateLimit({ max: 5, window: '1,2' }), getUser)"), Some("getUser"));
        assert_eq!(named_handler(", h,)"), Some("h"));
        assert_eq!(named_handler(", ctrl.list)"), Some("ctrl.list"));
        assert_eq!(named_handler(", a.b.c)"), None);
        assert_eq!(named_handler(", (req, res) => res.end())"), None);
        assert_eq!(named_handler(", null)"), None);
        assert_eq!(named_handler(")"), None);
        assert_eq!(named_handler(", h"), None);
        assert_eq!(named_handler(", h]"), None);
    }
}
