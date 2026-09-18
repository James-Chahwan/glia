//! Backend HTTP route extraction for JS/TS — complements the endpoint
//! (fetch/axios) extraction in the TypeScript parser. Detects:
//!   - Next.js file-based API routes: `pages/api/...` and `app/api/.../route.ts`
//!   - Express / Koa / Hono / Fastify-style: `app.get('/path', ...)`,
//!     `router.post('/path', ...)`.
//!
//! Runs per-file. Output is route nodes that HttpStackResolver can match
//! against endpoint nodes to produce cross-stack HTTP edges.
//!
//! Emits one Route node per path, with stacked ROUTE_METHOD cells — matches
//! the shape parser-go writes and the shape HttpStackResolver reads.
//!
//! Like the other cross-cutting extractors, this is pattern-based. Only called
//! by the pipeline for JS/TS-family languages.
//!
//! A3.5: the Express scan skips `.get('/x')` on an HTTP-client receiver
//! (`this.http.get` is an outbound ENDPOINT, not a server route), and a route
//! registered with a NAMED handler (`app.get('/users/:id', getUser)`) carries a
//! HANDLED_BY `UnresolvedRef` for the graph builder to bind.

use std::collections::{BTreeMap, BTreeSet};

use repo_graph_code_domain::{
    CallQualifier, CodeNav, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category, endpoint,
    node_kind,
};
use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

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
}

const HTTP_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "options", "head", "all"];

pub fn extract_ts_backend_routes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> RouteNodes {
    // path → (method, handler) pairs (BTreeMap gives stable ordering for
    // deterministic output). `handler` is "" when the shape names none.
    let mut by_path: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    // (route, handler) pairs for the HANDLED_BY refs, deduped and ordered.
    let mut handled_by: BTreeSet<(String, String)> = BTreeSet::new();
    let mut skipped_client_calls = 0usize;

    // Shape 1: Express-style `<x>.<method>('/...', ...)` and Hono/Koa routers.
    for line in source.lines() {
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
            if add_method(&mut by_path, route, method, handler.unwrap_or(""))
                && let Some(h) = handler
            {
                handled_by.insert((route.to_string(), h.to_string()));
            }
        }
    }

    // Shape 2: Next.js file-based routing — path gives us the route, source
    // gives us the method(s).
    if let Some(route) = nextjs_route_from_path(path) {
        for method in nextjs_methods_from_source(source) {
            add_method(&mut by_path, &route, method, "");
        }
    }

    // Shape 3: SvelteKit `+server.ts` — path from file path, methods from
    // named exports (same shape as Next.js App Router).
    if let Some(route) = sveltekit_route_from_path(path) {
        for method in nextjs_methods_from_source(source) {
            add_method(&mut by_path, &route, method, "");
        }
    }

    // Shape 4: NestJS controllers — combine @Controller(prefix) with method
    // decorators @Get/@Post/...(suffix).
    for (method, route) in nestjs_routes(source) {
        add_method(&mut by_path, &route, method, "");
    }

    // Shape 5: Hapi.js — `server.route({ method: 'GET', path: '/x', handler })`
    // and array form `server.route([{ ... }, { ... }])`. Method may be a string
    // ('GET') or array of strings (['GET', 'POST']).
    for (method, route) in hapi_routes(source) {
        add_method(&mut by_path, &route, method, "");
    }

    // Shape 6: Bun.serve `routes:` object —
    // `Bun.serve({ routes: { '/api/users': { GET: h, POST: h2 }, ... } })`.
    // Bun 1.2+ syntax. Single-handler `fetch(req)` style is intentionally
    // skipped (routing is internal to user code).
    for (method, route) in bun_serve_routes(source) {
        add_method(&mut by_path, &route, method, "");
    }

    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for (route, methods) in by_path {
        let qname = format!("route:{route}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &qname);
        let cells = methods
            .into_iter()
            .map(|(m, handler)| Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Json(format!(
                    r#"{{"method":"{}","handler":"{}","file":"{}","line":0,"col":0}}"#,
                    m.to_ascii_uppercase(),
                    escape_json(&handler),
                    escape_json(path),
                )),
            })
            .collect();
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells,
        });
        nav.record(id, &route, &qname, node_kind::ROUTE, Some(module_id));
    }

    let refs = handled_by
        .into_iter()
        .map(|(route, handler)| {
            let qname = format!("route:{route}");
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
            }
        })
        .collect();

    RouteNodes { nodes, nav, refs, skipped_client_calls }
}

/// Record `method` (with its `handler`, "" when none is named) on `route`.
/// Returns false when the route is dropped, so the caller emits no ref for it.
/// A repeated method keeps its first entry, filling in a handler the first
/// registration did not name.
fn add_method(
    by_path: &mut BTreeMap<String, Vec<(String, String)>>,
    route: &str,
    method: &str,
    handler: &str,
) -> bool {
    // Drop template-source expressions captured from framework internals
    // (`/${this.routeConfig.path}`) — not literal routes. (glia-v2 G8)
    if route.contains("${") {
        return false;
    }
    let entry = by_path.entry(route.to_string()).or_default();
    let m = method.to_ascii_lowercase();
    match entry.iter_mut().find(|(existing, _)| existing == &m) {
        Some((_, h)) if h.is_empty() => *h = handler.to_string(),
        Some(_) => {}
        None => entry.push((m, handler.to_string())),
    }
    true
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
/// Returns (method, full_path) pairs.
fn nestjs_routes(source: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let mut controller_prefix: Option<String> = None;
    for line in source.lines() {
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
                out.push((*method, full));
                break;
            }
        }
    }
    out
}

fn extract_decorator_string(line: &str) -> Option<String> {
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

fn combine_nest_paths(prefix: &str, suffix: &str) -> String {
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

fn nextjs_methods_from_source(source: &str) -> Vec<&'static str> {
    let mut methods = Vec::new();
    for method in ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"] {
        let needle_async = format!("export async function {method}");
        let needle_sync = format!("export function {method}");
        let needle_const = format!("export const {method}");
        if source.contains(&needle_async)
            || source.contains(&needle_sync)
            || source.contains(&needle_const)
        {
            methods.push(match method {
                "GET" => "get",
                "POST" => "post",
                "PUT" => "put",
                "DELETE" => "delete",
                "PATCH" => "patch",
                "OPTIONS" => "options",
                "HEAD" => "head",
                _ => unreachable!(),
            });
        }
    }
    if methods.is_empty() && source.contains("export default") {
        methods.push("get");
    }
    methods
}

/// Scan for Hapi.js `server.route(...)` registrations. Returns one entry per
/// (method, path) pair; multiple methods on the same path produce one entry
/// each. Both single-config and array-of-configs are recognised.
fn hapi_routes(source: &str) -> Vec<(&'static str, String)> {
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
        for (method, path) in extract_hapi_configs(body) {
            out.push((method, path));
        }
        search_from = arg_start + close_rel + 1;
    }
    out
}

/// Walk a `server.route(...)` body and pull `{ method, path }` configs. The
/// body may be a single object, or an array of objects. Methods may be a
/// quoted string or an array of quoted strings (`['GET', 'POST']`).
fn extract_hapi_configs(body: &str) -> Vec<(&'static str, String)> {
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
                        out.push((m, p.clone()));
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
/// (`method: 'GET'`) and array (`method: ['GET', 'POST']`) forms; an
/// uppercase wildcard `'*'` becomes `all`.
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
/// (single handler) are both handled. Returns (method, path) pairs.
fn bun_serve_routes(source: &str) -> Vec<(&'static str, String)> {
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
        for (method, path) in parse_bun_routes_object(routes_obj) {
            out.push((method, path));
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
/// key. Path values may be a function reference (single handler → `get`),
/// an object `{ GET: h, POST: h2 }`, or a Response literal — only object
/// form gives explicit methods.
fn parse_bun_routes_object(body: &str) -> Vec<(&'static str, String)> {
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
            // Value: object → enumerate verb keys; otherwise → single handler ⇒ `get`.
            if k < bytes.len() && bytes[k] == b'{' {
                let Some(vclose) = find_balanced_close(&body[k + 1..], b'{', b'}') else {
                    break;
                };
                let val = &body[k + 1..k + 1 + vclose];
                for verb in extract_verb_keys(val) {
                    out.push((verb, key.to_string()));
                }
                i = k + 1 + vclose + 1;
                continue;
            }
            out.push(("get", key.to_string()));
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

    #[test]
    fn detects_express_get() {
        let src = "app.get('/users/:id', handler);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(r.nodes[0].cells.len(), 1);
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

    #[test]
    fn detects_nextjs_pages_router() {
        let src = "export default function handler(req, res) { res.json({}); }";
        let r = extract_ts_backend_routes(src, "pages/api/users.ts", module_id(), repo());
        assert_eq!(r.nodes.len(), 1);
    }

    #[test]
    fn detects_nextjs_app_router_named_exports() {
        let src = "export async function GET() {}\nexport async function POST() {}";
        let r = extract_ts_backend_routes(src, "app/api/widgets/route.ts", module_id(), repo());
        // One Route node per path, with one ROUTE_METHOD cell per method.
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(r.nodes[0].cells.len(), 2);
    }

    #[test]
    fn converts_nextjs_dynamic_segment() {
        let src = "export default function h() {}";
        let r = extract_ts_backend_routes(src, "pages/api/users/[id].ts", module_id(), repo());
        assert_eq!(r.nodes.len(), 1);
        let qname = r.nav.qname_by_id.values().next().unwrap();
        assert!(qname.contains("/api/users/:id"), "qname={qname}");
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
        let qnames: Vec<&str> = r.nav.qname_by_id.values().map(|s| s.as_str()).collect();
        assert!(qnames.iter().any(|q| *q == "route:/users"));
        assert!(qnames.iter().any(|q| *q == "route:/users/:id"));
    }

    #[test]
    fn detects_sveltekit_plus_server() {
        let src = "export async function GET() {}\nexport async function POST() {}";
        let r = extract_ts_backend_routes(src, "src/routes/api/widgets/+server.ts", module_id(), repo());
        let qnames: Vec<&str> = r.nav.qname_by_id.values().map(|s| s.as_str()).collect();
        assert!(qnames.iter().any(|q| *q == "route:/api/widgets"));
        let node = r.nodes.iter().find(|n| n.cells.len() == 2).expect("combined node");
        assert_eq!(node.cells.len(), 2);
    }

    #[test]
    fn sveltekit_dynamic_segment() {
        let src = "export function GET() {}";
        let r = extract_ts_backend_routes(src, "src/routes/api/users/[id]/+server.ts", module_id(), repo());
        let qnames: Vec<&str> = r.nav.qname_by_id.values().map(|s| s.as_str()).collect();
        assert!(qnames.iter().any(|q| q.contains("/api/users/:id")));
    }

    fn route_methods(r: &RouteNodes, path: &str) -> Vec<String> {
        let qname = format!("route:{path}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::ROUTE, &qname);
        r.nodes
            .iter()
            .filter(|n| n.id == id)
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| match &c.payload {
                CellPayload::Json(s) => {
                    let m = s.split("\"method\":\"").nth(1)?;
                    Some(m.split('"').next()?.to_string())
                }
                _ => None,
            })
            .collect()
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
        // '/api/health' value is not an object → defaults to `get`.
        assert_eq!(route_methods(&r, "/api/health"), vec!["GET".to_string()]);
        let users = route_methods(&r, "/api/users");
        assert!(users.contains(&"GET".to_string()));
        assert!(users.contains(&"POST".to_string()));
        let by_id = route_methods(&r, "/api/users/:id");
        assert!(by_id.contains(&"GET".to_string()));
        assert!(by_id.contains(&"DELETE".to_string()));
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

    #[test]
    fn route_cell_carries_method_json() {
        let src = "app.get('/x', h); app.post('/x', h);";
        let r = extract_ts_backend_routes(src, "server.ts", module_id(), repo());
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(r.nodes[0].cells.len(), 2);
        let payloads: Vec<String> = r.nodes[0]
            .cells
            .iter()
            .map(|c| match &c.payload {
                CellPayload::Json(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(payloads.iter().any(|p| p.contains("\"method\":\"GET\"")));
        assert!(payloads.iter().any(|p| p.contains("\"method\":\"POST\"")));
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
                ("route:/users".to_string(), CallQualifier::Bare("createUser".into())),
                ("route:/users/:id".to_string(), CallQualifier::Bare("getUser".into())),
                (
                    "route:/users/:id".to_string(),
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
